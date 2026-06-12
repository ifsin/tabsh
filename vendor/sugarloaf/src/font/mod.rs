pub mod constants;
pub mod fonts;
pub mod glyf_decode;
pub mod glyph_registry;
pub mod loader;
pub mod metrics;
pub mod nerd_font_attributes;
pub mod text_run_cache;

#[cfg(test)]
mod cjk_metrics_tests;

pub const FONT_ID_REGULAR: usize = 0;

use crate::font::constants::*;
use crate::font::metrics::{FaceMetrics, Metrics};
use crate::layout::SpanStyle;
use crate::SugarloafErrors;
use dashmap::DashMap;
use parking_lot::RwLock;
use rustc_hash::FxHashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use swash::text::cluster::Parser;
use swash::text::cluster::Token;
use swash::text::cluster::{CharCluster, Status};
use swash::text::Codepoint;
use swash::text::Script;
use swash::{tag_from_bytes, CacheKey, FontRef, Synthesis};

pub use swash::{Style, Weight};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl Slot {
    #[inline]
    pub fn is_bold(self) -> bool {
        matches!(self, Slot::Bold | Slot::BoldItalic)
    }
    #[inline]
    pub fn is_italic(self) -> bool {
        matches!(self, Slot::Italic | Slot::BoldItalic)
    }
}

type FontDataCache = Arc<DashMap<PathBuf, SharedData>>;

static FONT_DATA_CACHE: OnceLock<FontDataCache> = OnceLock::new();

pub fn clear_font_data_cache() {
    if let Some(cache) = FONT_DATA_CACHE.get() {
        cache.clear();
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LookupAttrs {
    pub italic: bool,
    pub bold: bool,
}

pub fn lookup_for_font_match(
    cluster: &mut CharCluster,
    synth: &mut Synthesis,
    library: &FontLibraryData,
    spec: Option<LookupAttrs>,
) -> Option<(usize, bool)> {
    let mut search_result = None;

    let fonts_len: usize = library.inner.len();
    for font_id in 0..fonts_len {
        let font = match library.inner.get(&font_id) {
            Some(FontEntry::Owned(d)) => d,
            Some(FontEntry::Alias(_)) | None => continue,
        };
        let is_emoji = font.is_emoji;
        let font_synth = font.synth;

        if let Some(spec) = spec {
            if spec.italic && !font.is_italic() && !font.should_italicize {
                continue;
            }
            if spec.bold && !font.is_bold() && !font.should_embolden {
                continue;
            }
        }

        let matched = {
            if let Some((shared_data, offset, key)) = library.get_data(&font_id) {
                let font_ref = FontRef {
                    data: shared_data.as_ref(),
                    offset,
                    key,
                };
                let charmap = font_ref.charmap();
                let status = cluster.map(|ch| charmap.map(ch));
                status != Status::Discard
            } else {
                false
            }
        };

        if matched {
            *synth = font_synth;
            search_result = Some((font_id, is_emoji));
            break;
        }
    }

    if search_result.is_none() && spec.is_some() {
        return lookup_for_font_match(cluster, synth, library, None);
    }

    search_result
}

#[derive(Clone)]
pub struct FontLibrary {
    pub inner: Arc<RwLock<FontLibraryData>>,
}

impl FontLibrary {
    pub fn new(spec: SugarloafFonts) -> (Self, Option<SugarloafErrors>) {
        let mut font_library = FontLibraryData::default();

        let mut sugarloaf_errors = None;

        let fonts_not_found = font_library.load(spec);
        if !fonts_not_found.is_empty() {
            sugarloaf_errors = Some(SugarloafErrors { fonts_not_found });
        }

        (
            Self {
                inner: Arc::new(RwLock::new(font_library)),
            },
            sugarloaf_errors,
        )
    }

    pub fn font_id_for_postscript_name(&self, name: &str) -> Option<usize> {
        self.inner.read().font_id_for_postscript_name(name)
    }

    pub fn resolve_font_for_char(
        &self,
        ch: char,
        fragment_style: &SpanStyle,
        route_id: Option<usize>,
    ) -> (usize, bool) {
        if let Some(found) =
            self.inner
                .read()
                .find_best_font_match_strict(ch, fragment_style, route_id)
        {
            return found;
        }

        self.cascade_discover(ch, fragment_style)
            .unwrap_or((0, false))
    }

    fn cascade_discover(
        &self,
        _ch: char,
        _fragment_style: &SpanStyle,
    ) -> Option<(usize, bool)> {
        None
    }

    pub fn family_names(&self) -> Vec<String> {
        Vec::new()
    }

    pub fn install_glyph_registry(
        &self,
        route_id: usize,
        registry: glyph_registry::GlyphRegistry,
    ) {
        self.inner
            .write()
            .glyph_registries
            .insert(route_id, registry);
    }

    pub fn remove_glyph_registry(&self, route_id: usize) {
        self.inner.write().glyph_registries.remove(&route_id);
    }

    #[inline]
    pub fn glyph_registry_for(
        &self,
        route_id: usize,
    ) -> Option<glyph_registry::GlyphRegistry> {
        self.inner.read().glyph_registries.get(&route_id).cloned()
    }

    pub fn covers_codepoint(&self, cp: u32) -> bool {
        let Some(ch) = char::from_u32(cp) else {
            return false;
        };
        self.inner
            .read()
            .find_best_font_match_strict(ch, &SpanStyle::default(), None)
            .is_some_and(|(font_id, _)| font_id != glyph_registry::CUSTOM_GLYPH_FONT_ID)
    }
}

impl Default for FontLibrary {
    fn default() -> Self {
        let mut font_library = FontLibraryData::default();
        let _fonts_not_found = font_library.load(SugarloafFonts::default());

        Self {
            inner: Arc::new(RwLock::new(font_library)),
        }
    }
}

pub struct SymbolMap {
    pub font_index: usize,
    pub range: Range<char>,
}

#[derive(Clone)]
pub enum FontEntry {
    Owned(FontData),
    Alias(usize),
}

impl FontEntry {
    #[inline]
    pub fn as_owned(&self) -> Option<&FontData> {
        match self {
            FontEntry::Owned(d) => Some(d),
            FontEntry::Alias(_) => None,
        }
    }
}

pub struct FontLibraryData {
    pub inner: FxHashMap<usize, FontEntry>,
    pub symbol_maps: Option<Vec<SymbolMap>>,
    pub hinting: bool,
    primary_metrics_cache: FxHashMap<u32, Metrics>,
    postscript_to_id: FxHashMap<String, usize>,
    glyph_registries: FxHashMap<usize, glyph_registry::GlyphRegistry>,
}

impl Default for FontLibraryData {
    fn default() -> Self {
        Self {
            inner: FxHashMap::default(),
            hinting: true,
            symbol_maps: None,
            primary_metrics_cache: FxHashMap::default(),
            postscript_to_id: FxHashMap::default(),
            glyph_registries: FxHashMap::default(),
        }
    }
}

impl FontLibraryData {
    #[inline]
    pub fn find_best_font_match(
        &self,
        ch: char,
        fragment_style: &SpanStyle,
        route_id: Option<usize>,
    ) -> Option<(usize, bool)> {
        if let Some(route_id) = route_id {
            if let Some(registry) = self.glyph_registries.get(&route_id) {
                if registry.contains(ch as u32) {
                    return Some((glyph_registry::CUSTOM_GLYPH_FONT_ID, false));
                }
            }
        }

        let mut synth = Synthesis::default();
        let mut char_cluster = CharCluster::new();
        let mut parser = Parser::new(
            Script::Latin,
            std::iter::once(Token {
                ch,
                offset: 0,
                len: ch.len_utf8() as u8,
                info: ch.properties().into(),
                data: 0,
            }),
        );
        if !parser.next(&mut char_cluster) {
            return Some((0, false));
        }

        if let Some(symbol_maps) = &self.symbol_maps {
            for symbol_map in symbol_maps {
                if symbol_map.range.contains(&ch) {
                    return Some((symbol_map.font_index, false));
                }
            }
        }

        let italic = fragment_style.font_attrs.style() == Style::Italic;
        let bold = fragment_style.font_attrs.weight() == Weight::BOLD;
        let spec = (italic || bold).then_some(LookupAttrs { italic, bold });

        if let Some(result) =
            lookup_for_font_match(&mut char_cluster, &mut synth, self, spec)
        {
            return Some(result);
        }

        Some((0, false))
    }

    #[inline]
    pub fn find_best_font_match_strict(
        &self,
        ch: char,
        fragment_style: &SpanStyle,
        route_id: Option<usize>,
    ) -> Option<(usize, bool)> {
        if let Some(route_id) = route_id {
            if let Some(registry) = self.glyph_registries.get(&route_id) {
                if registry.contains(ch as u32) {
                    return Some((glyph_registry::CUSTOM_GLYPH_FONT_ID, false));
                }
            }
        }

        let mut synth = Synthesis::default();
        let mut char_cluster = CharCluster::new();
        let mut parser = Parser::new(
            Script::Latin,
            std::iter::once(Token {
                ch,
                offset: 0,
                len: ch.len_utf8() as u8,
                info: ch.properties().into(),
                data: 0,
            }),
        );
        if !parser.next(&mut char_cluster) {
            return None;
        }

        if let Some(symbol_maps) = &self.symbol_maps {
            for symbol_map in symbol_maps {
                if symbol_map.range.contains(&ch) {
                    return Some((symbol_map.font_index, false));
                }
            }
        }

        let italic = fragment_style.font_attrs.style() == Style::Italic;
        let bold = fragment_style.font_attrs.weight() == Weight::BOLD;
        let spec = (italic || bold).then_some(LookupAttrs { italic, bold });

        lookup_for_font_match(&mut char_cluster, &mut synth, self, spec)
    }

    #[inline]
    pub fn insert(&mut self, font_data: FontData) {
        let id = self.inner.len();
        if let Some(ps_name) = font_data.postscript_name() {
            self.postscript_to_id
                .entry(ps_name.to_string())
                .or_insert(id);
        }
        self.inner.insert(id, FontEntry::Owned(font_data));
    }

    #[inline]
    pub fn insert_alias(&mut self, target: usize) {
        let id = self.inner.len();
        let target = self.resolve_id(target);
        self.inner.insert(id, FontEntry::Alias(target));
    }

    #[inline]
    pub fn resolve_id(&self, font_id: usize) -> usize {
        match self.inner.get(&font_id) {
            Some(FontEntry::Alias(target)) => *target,
            _ => font_id,
        }
    }

    pub fn font_id_for_postscript_name(&self, name: &str) -> Option<usize> {
        self.postscript_to_id.get(name).copied()
    }

    #[inline]
    pub fn get(&self, font_id: &usize) -> &FontData {
        let id = self.resolve_id(*font_id);
        match &self.inner[&id] {
            FontEntry::Owned(d) => d,
            FontEntry::Alias(_) => {
                unreachable!("alias must resolve to Owned in single hop")
            }
        }
    }

    #[inline]
    pub fn try_get(&self, font_id: &usize) -> Option<&FontData> {
        let id = self.resolve_id(*font_id);
        self.inner.get(&id).and_then(FontEntry::as_owned)
    }

    pub fn get_data(&self, font_id: &usize) -> Option<(SharedData, u32, CacheKey)> {
        if let Some(font) = self.try_get(font_id) {
            if let Some(data) = &font.data {
                return Some((data.clone(), font.offset, font.key));
            } else if let Some(path) = &font.path {
                if let Some(raw_data) = load_from_font_source(path) {
                    return Some((raw_data, font.offset, font.key));
                }
            }
        }

        None
    }

    #[inline]
    pub fn get_mut(&mut self, font_id: &usize) -> Option<&mut FontData> {
        let id = self.resolve_id(*font_id);
        match self.inner.get_mut(&id)? {
            FontEntry::Owned(d) => Some(d),
            FontEntry::Alias(_) => None,
        }
    }

    pub fn get_font_metrics(
        &mut self,
        font_id: &usize,
        font_size: f32,
    ) -> Option<(f32, f32, f32)> {
        let size_key = (font_size * 100.0) as u32;

        let primary_metrics =
            if let Some(cached) = self.primary_metrics_cache.get(&size_key) {
                *cached
            } else {
                let primary_font = self.get_mut(&FONT_ID_REGULAR)?;
                let primary_metrics = primary_font.get_metrics(font_size, None)?;
                self.primary_metrics_cache.insert(size_key, primary_metrics);
                primary_metrics
            };

        let resolved = self.resolve_id(*font_id);
        match resolved {
            FONT_ID_REGULAR => Some(primary_metrics.for_rich_text()),
            _ => {
                let font = self.get_mut(&resolved)?;
                font.get_rich_text_metrics(font_size, Some(&primary_metrics))
            }
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn load(&mut self, _font_spec: SugarloafFonts) -> Vec<SugarloafFont> {
        self.insert(FontData::from_slice(FONT_CASCADIA_CODE_NF).unwrap());

        vec![]
    }
}

#[derive(Clone, Debug)]
pub enum SharedData {
    Heap(Arc<[u8]>),
    Static(&'static [u8]),
}

impl SharedData {
    pub fn new(data: Vec<u8>) -> Self {
        Self::Heap(Arc::from(data))
    }

    pub const fn from_static(data: &'static [u8]) -> Self {
        Self::Static(data)
    }

    pub const fn is_static(&self) -> bool {
        matches!(self, Self::Static(_))
    }
}

impl std::ops::Deref for SharedData {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Heap(a) => a,
            Self::Static(s) => s,
        }
    }
}

impl AsRef<[u8]> for SharedData {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Heap(a) => a,
            Self::Static(s) => s,
        }
    }
}

#[derive(Clone)]
pub struct FontData {
    data: Option<SharedData>,
    path: Option<PathBuf>,
    offset: u32,
    pub key: CacheKey,
    pub weight: swash::Weight,
    pub style: swash::Style,
    pub stretch: swash::Stretch,
    pub synth: Synthesis,
    pub should_embolden: bool,
    pub should_italicize: bool,
    pub wght_variation: Option<f32>,
    pub is_emoji: bool,
    metrics_cache: FxHashMap<u32, Metrics>,
    postscript_name: Option<String>,
}

impl PartialEq for FontData {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl FontData {
    #[inline]
    pub fn is_bold(&self) -> bool {
        self.weight >= Weight(700)
    }

    #[inline]
    pub fn is_italic(&self) -> bool {
        self.style == Style::Italic
    }

    pub fn data(&self) -> &Option<SharedData> {
        &self.data
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    pub fn postscript_name(&self) -> Option<&str> {
        self.postscript_name.as_deref()
    }

    pub fn offset(&self) -> u32 {
        self.offset
    }

    pub fn get_metrics(
        &mut self,
        font_size: f32,
        primary_metrics: Option<&Metrics>,
    ) -> Option<Metrics> {
        let size_key = (font_size * 100.0) as u32;

        if let Some(cached) = self.metrics_cache.get(&size_key) {
            return Some(*cached);
        }

        if let Some(ref data) = self.data {
            let font_ref = swash::FontRef {
                data: data.as_ref(),
                offset: self.offset,
                key: self.key,
            };

            let scaled_metrics = font_ref.metrics(&[]).scale(font_size);

            let face_metrics = FaceMetrics::from_font(&font_ref, &scaled_metrics);

            let metrics = if let Some(primary) = primary_metrics {
                Metrics::calc_with_primary_cell_dimensions(face_metrics, primary)
            } else {
                Metrics::calc(face_metrics)
            };

            self.metrics_cache.insert(size_key, metrics);
            Some(metrics)
        } else {
            None
        }
    }

    pub fn get_rich_text_metrics(
        &mut self,
        font_size: f32,
        primary_metrics: Option<&Metrics>,
    ) -> Option<(f32, f32, f32)> {
        self.get_metrics(font_size, primary_metrics)
            .map(|m| m.for_rich_text())
    }

    #[inline]
    pub fn from_static_slice(
        data: &'static [u8],
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::from_static_slice_with_wght(data, None)
    }

    pub fn from_static_slice_with_wght(
        data: &'static [u8],
        wght: Option<f32>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let font = FontRef::from_index(data, 0).unwrap();
        let (offset, key) = (font.offset, font.key);
        let attributes = font.attributes();
        let style = attributes.style();
        let weight = match wght {
            Some(v) => swash::Weight(v.round().clamp(0.0, u16::MAX as f32) as u16),
            None => attributes.weight(),
        };
        let stretch = attributes.stretch();
        let synth = attributes.synthesize(attributes);
        let is_emoji = has_color_tables(&font);
        let postscript_name = parse_postscript_name(data);

        Ok(Self {
            data: Some(SharedData::from_static(data)),
            offset,
            key,
            synth,
            style,
            should_embolden: false,
            should_italicize: false,
            wght_variation: wght,
            weight,
            stretch,
            path: None,
            is_emoji,
            metrics_cache: FxHashMap::default(),
            postscript_name,
        })
    }

    #[inline]
    pub fn from_slice(data: &[u8]) -> Result<Self, Box<dyn std::error::Error>> {
        let font = FontRef::from_index(data, 0).unwrap();
        let (offset, key) = (font.offset, font.key);
        let attributes = font.attributes();
        let style = attributes.style();
        let weight = attributes.weight();
        let stretch = attributes.stretch();
        let synth = attributes.synthesize(attributes);
        let is_emoji = has_color_tables(&font);

        let postscript_name = parse_postscript_name(data);
        Ok(Self {
            data: Some(SharedData::new(data.to_vec())),
            offset,
            key,
            synth,
            style,
            should_embolden: false,
            should_italicize: false,
            wght_variation: None,
            weight,
            stretch,
            path: None,
            is_emoji,
            metrics_cache: FxHashMap::default(),
            postscript_name,
        })
    }
}

fn parse_postscript_name(data: &[u8]) -> Option<String> {
    let face = ttf_parser::Face::parse(data, 0).ok()?;
    face.names()
        .into_iter()
        .find(|n| n.name_id == ttf_parser::name_id::POST_SCRIPT_NAME && n.is_unicode())
        .and_then(|n| n.to_string())
        .or_else(|| {
            face.names()
                .into_iter()
                .find(|n| n.name_id == ttf_parser::name_id::FAMILY && n.is_unicode())
                .and_then(|n| n.to_string())
        })
}

fn has_color_tables(font: &FontRef<'_>) -> bool {
    font.table(tag_from_bytes(b"COLR")).is_some()
        || font.table(tag_from_bytes(b"CBDT")).is_some()
        || font.table(tag_from_bytes(b"CBLC")).is_some()
        || font.table(tag_from_bytes(b"sbix")).is_some()
}

pub type SugarloafFont = fonts::SugarloafFont;
pub type SugarloafFonts = fonts::SugarloafFonts;

fn load_from_font_source(_path: &PathBuf) -> Option<SharedData> {
    None
}

#[cfg(test)]
mod alias_tests {
    use super::*;

    #[test]
    fn insert_alias_resolves_to_target() {
        let mut lib = FontLibraryData::default();
        lib.insert(
            FontData::from_static_slice(constants::FONT_CASCADIA_CODE_NF)
                .expect("load regular"),
        );
        lib.insert_alias(0);

        assert_eq!(lib.len(), 2, "alias takes a slot");
        assert_eq!(lib.resolve_id(1), 0, "alias resolves to its target");
        let owned_key = lib.get(&0).key;
        let aliased_key = lib.get(&1).key;
        assert_eq!(
            owned_key, aliased_key,
            "aliased slot must surface the target FontData"
        );
    }

    #[test]
    fn alias_of_alias_collapses_to_root() {
        let mut lib = FontLibraryData::default();
        lib.insert(
            FontData::from_static_slice(constants::FONT_CASCADIA_CODE_NF)
                .expect("load regular"),
        );
        lib.insert_alias(0);
        lib.insert_alias(1);
        assert_eq!(
            lib.resolve_id(2),
            0,
            "alias pointing at an alias must collapse to the owning id"
        );
        assert!(matches!(lib.inner.get(&2), Some(FontEntry::Alias(0))));
    }

    #[test]
    fn fallback_bold_slot_reports_is_bold() {
        let regular = load_fallback_from_memory(Slot::Regular);
        let bold = load_fallback_from_memory(Slot::Bold);
        let italic = load_fallback_from_memory(Slot::Italic);
        let bold_italic = load_fallback_from_memory(Slot::BoldItalic);

        assert!(!regular.is_bold(), "regular slot must not be bold");
        assert!(bold.is_bold(), "bold slot must report is_bold");
        assert!(!italic.is_bold(), "italic slot must not be bold");
        assert!(
            bold_italic.is_bold(),
            "bold-italic slot must report is_bold"
        );

        assert!(!regular.is_italic(), "regular slot must not be italic");
        assert!(!bold.is_italic(), "bold slot must not be italic");
        assert!(italic.is_italic(), "italic slot must report is_italic");
        assert!(
            bold_italic.is_italic(),
            "bold-italic slot must report is_italic"
        );

        assert_eq!(bold.wght_variation, Some(constants::WGHT_BOLD));
        assert_eq!(bold_italic.wght_variation, Some(constants::WGHT_BOLD));
        assert_eq!(regular.wght_variation, None);
        assert_eq!(italic.wght_variation, None);
    }

    #[test]
    fn alias_shares_metrics_with_target() {
        let mut lib = FontLibraryData::default();
        lib.insert(
            FontData::from_static_slice(constants::FONT_CASCADIA_CODE_NF)
                .expect("load regular"),
        );
        lib.insert_alias(0);

        let from_regular = lib.get_font_metrics(&0, 14.0).expect("regular metrics");
        let from_alias = lib.get_font_metrics(&1, 14.0).expect("alias metrics");
        assert_eq!(from_regular, from_alias);
    }
}

#[cfg(test)]
mod glyph_registry_install_tests {
    use super::*;
    use crate::font::glyph_registry::GlyphRegistry;

    #[test]
    fn install_then_lookup_returns_same_arc() {
        let library = FontLibrary::default();
        let registry = GlyphRegistry::new();
        library.install_glyph_registry(42, registry.clone());

        let fetched = library
            .glyph_registry_for(42)
            .expect("entry installed at 42");
        assert!(fetched.ptr_eq(&registry));
    }

    #[test]
    fn lookup_returns_none_for_unknown_route() {
        let library = FontLibrary::default();
        assert!(library.glyph_registry_for(999).is_none());
    }

    #[test]
    fn install_overwrites_same_route() {
        let library = FontLibrary::default();
        let first = GlyphRegistry::new();
        let second = GlyphRegistry::new();
        assert!(!first.ptr_eq(&second));

        library.install_glyph_registry(7, first.clone());
        library.install_glyph_registry(7, second.clone());

        let fetched = library.glyph_registry_for(7).expect("entry at 7");
        assert!(fetched.ptr_eq(&second));
        assert!(!fetched.ptr_eq(&first));
    }

    #[test]
    fn remove_drops_the_entry() {
        let library = FontLibrary::default();
        let registry = GlyphRegistry::new();
        library.install_glyph_registry(3, registry);
        assert!(library.glyph_registry_for(3).is_some());

        library.remove_glyph_registry(3);
        assert!(library.glyph_registry_for(3).is_none());
    }

    #[test]
    fn distinct_routes_hold_distinct_registries() {
        let library = FontLibrary::default();
        let a = GlyphRegistry::new();
        let b = GlyphRegistry::new();
        library.install_glyph_registry(1, a.clone());
        library.install_glyph_registry(2, b.clone());

        assert!(library.glyph_registry_for(1).unwrap().ptr_eq(&a));
        assert!(library.glyph_registry_for(2).unwrap().ptr_eq(&b));
    }
}
