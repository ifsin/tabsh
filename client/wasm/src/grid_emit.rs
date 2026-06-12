use rio_backend::config::colors::term::List;
use rio_backend::config::colors::{AnsiColor, ColorArray, ColorRgb, NamedColor};
use rio_backend::crosswords::square::{ContentTag, Square, Wide};
use rio_backend::crosswords::style::{StyleFlags, StyleSet};
use rustc_hash::FxHashMap;
use sugarloaf::font::FontLibrary;
use sugarloaf::grid::atlas::{AtlasSlot, GlyphKey, RasterizedGlyph};
use sugarloaf::grid::{CellBg, CellText, GridRenderer};
use sugarloaf::swash;
use sugarloaf::swash::FontRef;

fn arr_to_rgb(arr: ColorArray) -> [u8; 3] {
    [
        (arr[0] * 255.0) as u8,
        (arr[1] * 255.0) as u8,
        (arr[2] * 255.0) as u8,
    ]
}

fn ansi_color_rgb(c: AnsiColor, palette: &List) -> [u8; 3] {
    match c {
        AnsiColor::Named(nc) => arr_to_rgb(palette[nc]),
        AnsiColor::Indexed(idx) => arr_to_rgb(palette[idx as usize]),
        AnsiColor::Spec(ColorRgb { r, g, b }) => [r, g, b],
    }
}

fn resolve_fg(sq: Square, style_set: &StyleSet, palette: &List) -> [u8; 4] {
    match sq.content_tag() {
        ContentTag::BgPalette | ContentTag::BgRgb => {
            let [r, g, b] = arr_to_rgb(palette[NamedColor::Foreground]);
            return [r, g, b, 255];
        }
        ContentTag::Codepoint => {}
    }

    let style = style_set.get(sq.style_id());
    let color = if style.flags.contains(StyleFlags::INVERSE) {
        style.bg
    } else {
        style.fg
    };
    let [r, g, b] = ansi_color_rgb(color, palette);
    if style.flags.contains(StyleFlags::DIM) && !style.flags.contains(StyleFlags::BOLD) {
        [
            (r as f32 * 0.66) as u8,
            (g as f32 * 0.66) as u8,
            (b as f32 * 0.66) as u8,
            255,
        ]
    } else {
        [r, g, b, 255]
    }
}

fn resolve_bg(sq: Square, style_set: &StyleSet, palette: &List) -> [u8; 4] {
    match sq.content_tag() {
        ContentTag::BgPalette => {
            let [r, g, b] = arr_to_rgb(palette[sq.bg_palette_index() as usize]);
            return [r, g, b, 255];
        }
        ContentTag::BgRgb => {
            let (r, g, b) = sq.bg_rgb();
            return [r, g, b, 255];
        }
        ContentTag::Codepoint => {}
    }

    let style = style_set.get(sq.style_id());
    let bg = if style.flags.contains(StyleFlags::INVERSE) {
        style.fg
    } else {
        style.bg
    };

    if !style.flags.contains(StyleFlags::INVERSE)
        && matches!(bg, AnsiColor::Named(NamedColor::Background))
    {
        return [0, 0, 0, 0];
    }

    let [r, g, b] = ansi_color_rgb(bg, palette);
    [r, g, b, 255]
}

pub struct GlyphRasterizer {
    scale_ctx: swash::scale::ScaleContext,
    font_data: FxHashMap<u32, (sugarloaf::font::SharedData, u32, swash::CacheKey)>,
    ascent_cache: FxHashMap<(u32, u16), i16>,
    glyph_id_cache: FxHashMap<(u32, u32), u16>,
}

impl GlyphRasterizer {
    pub fn new() -> Self {
        Self {
            scale_ctx: swash::scale::ScaleContext::new(),
            font_data: FxHashMap::default(),
            ascent_cache: FxHashMap::default(),
            glyph_id_cache: FxHashMap::default(),
        }
    }

    pub fn ensure_font_data(&mut self, font_id: u32, font_library: &FontLibrary) -> bool {
        if self.font_data.contains_key(&font_id) {
            return true;
        }
        let lib = font_library.inner.read();
        if let Some(data) = lib.get_data(&(font_id as usize)) {
            self.font_data.insert(font_id, data);
            true
        } else {
            false
        }
    }

    fn size_bucket(font_size: f32) -> u16 {
        (font_size * 4.0).round() as u16
    }

    fn glyph_id_for_char(&mut self, font_id: u32, ch: char) -> Option<u16> {
        let key = (font_id, ch as u32);
        if let Some(&id) = self.glyph_id_cache.get(&key) {
            return if id == u16::MAX { None } else { Some(id) };
        }
        let entry = self.font_data.get(&font_id)?;
        let font_ref = FontRef {
            data: entry.0.as_ref(),
            offset: entry.1,
            key: entry.2,
        };
        let id = font_ref.charmap().map(ch);
        let result = if id == 0 { u16::MAX } else { id as u16 };
        self.glyph_id_cache.insert(key, result);
        if result == u16::MAX {
            None
        } else {
            Some(result)
        }
    }

    fn ascent_px(&mut self, font_id: u32, font_size: f32) -> i16 {
        let bucket = Self::size_bucket(font_size);
        *self
            .ascent_cache
            .entry((font_id, bucket))
            .or_insert_with(|| {
                let entry = match self.font_data.get(&font_id) {
                    Some(e) => e.clone(),
                    None => return 0,
                };
                let font_ref = FontRef {
                    data: entry.0.as_ref(),
                    offset: entry.1,
                    key: entry.2,
                };
                let m = font_ref.metrics(&[]).scale(font_size);
                m.ascent.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16
            })
    }

    pub fn rasterize(
        &mut self,
        font_id: u32,
        glyph_id: u16,
        font_size: f32,
        cell_h: f32,
        font_library: &FontLibrary,
        grid: &mut GridRenderer,
    ) -> Option<(AtlasSlot, bool)> {
        let bucket = Self::size_bucket(font_size);
        let key = GlyphKey {
            font_id,
            glyph_id: glyph_id as u32,
            size_bucket: bucket,
        };

        if let Some(slot) = grid.lookup_glyph(key) {
            return Some((slot, false));
        }
        if let Some(slot) = grid.lookup_glyph_color(key) {
            return Some((slot, true));
        }

        if !self.ensure_font_data(font_id, font_library) {
            return None;
        }

        let entry = self.font_data.get(&font_id)?.clone();
        let font_ref = FontRef {
            data: entry.0.as_ref(),
            offset: entry.1,
            key: entry.2,
        };

        let ascent = self.ascent_px(font_id, font_size);

        use swash::scale::{
            image::{Content, Image as GlyphImage},
            Render, Source, StrikeWith,
        };

        let mut scaler = self.scale_ctx.builder(font_ref).size(font_size).build();

        let sources: &[Source] = &[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ];

        let mut image = GlyphImage::new();
        let ok = Render::new(sources)
            .format(swash::zeno::Format::Alpha)
            .render_into(&mut scaler, glyph_id, &mut image);

        if !ok || image.placement.width == 0 || image.placement.height == 0 {
            return None;
        }

        let is_color = image.content == Content::Color;
        let cell_h_i16 = cell_h.round().clamp(0.0, i16::MAX as f32) as i16;
        let top_i16 = image.placement.top.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let bearing_y = cell_h_i16.saturating_sub(ascent).saturating_add(top_i16);

        let raster = RasterizedGlyph {
            width: image.placement.width.min(u16::MAX as u32) as u16,
            height: image.placement.height.min(u16::MAX as u32) as u16,
            bearing_x: image.placement.left.clamp(i16::MIN as i32, i16::MAX as i32)
                as i16,
            bearing_y,
            bytes: &image.data,
        };

        let slot = if is_color {
            grid.insert_glyph_color(key, raster)?
        } else {
            grid.insert_glyph(key, raster)?
        };

        Some((slot, is_color))
    }
}

pub fn build_row_bg(
    squares: &[Square],
    style_set: &StyleSet,
    palette: &List,
    bg_scratch: &mut Vec<CellBg>,
) {
    bg_scratch.clear();
    bg_scratch.reserve(squares.len());
    for &sq in squares {
        bg_scratch.push(CellBg {
            rgba: resolve_bg(sq, style_set, palette),
        });
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_row_fg(
    squares: &[Square],
    style_set: &StyleSet,
    row_idx: u16,
    font_library: &FontLibrary,
    font_id: u32,
    font_size: f32,
    cell_h: f32,
    palette: &List,
    rasterizer: &mut GlyphRasterizer,
    grid: &mut GridRenderer,
    fg_scratch: &mut Vec<CellText>,
) {
    fg_scratch.clear();

    if !rasterizer.ensure_font_data(font_id, font_library) {
        return;
    }

    for (x, &sq) in squares.iter().enumerate() {
        match sq.wide() {
            Wide::Spacer | Wide::LeadingSpacer => continue,
            _ => {}
        }

        if !matches!(sq.content_tag(), ContentTag::Codepoint) {
            continue;
        }

        let style = style_set.get(sq.style_id());
        if style.flags.contains(StyleFlags::HIDDEN) {
            continue;
        }

        let ch = sq.c();
        if ch == ' ' || ch == '\0' {
            continue;
        }

        let color = resolve_fg(sq, style_set, palette);

        let Some(glyph_id) = rasterizer.glyph_id_for_char(font_id, ch) else {
            continue;
        };

        let Some((slot, is_color)) = rasterizer.rasterize(
            font_id,
            glyph_id,
            font_size,
            cell_h,
            font_library,
            grid,
        ) else {
            continue;
        };

        let atlas = if is_color {
            CellText::ATLAS_COLOR
        } else {
            CellText::ATLAS_GRAYSCALE
        };
        let cell_color = if is_color {
            [255, 255, 255, 255]
        } else {
            color
        };

        fg_scratch.push(CellText {
            glyph_pos: [slot.x as u32, slot.y as u32],
            glyph_size: [slot.w as u32, slot.h as u32],
            bearings: [slot.bearing_x, slot.bearing_y],
            grid_pos: [x as u16, row_idx],
            color: cell_color,
            atlas,
            bools: 0,
            page: slot.page,
            _pad: 0,
        });

        if style.flags.contains(StyleFlags::UNDERLINE)
            || style.flags.contains(StyleFlags::DOUBLE_UNDERLINE)
        {
            fg_scratch.push(CellText {
                glyph_pos: [0, 0],
                glyph_size: [0, 0],
                bearings: [0, 0],
                grid_pos: [x as u16, row_idx],
                color,
                atlas: CellText::ATLAS_GRAYSCALE,
                bools: 0,
                page: 0,
                _pad: 0,
            });
        }

        if style.flags.contains(StyleFlags::STRIKEOUT) {
            fg_scratch.push(CellText {
                glyph_pos: [0, 0],
                glyph_size: [0, 0],
                bearings: [0, (cell_h / 2.0) as i16],
                grid_pos: [x as u16, row_idx],
                color,
                atlas: CellText::ATLAS_GRAYSCALE,
                bools: 0,
                page: 0,
                _pad: 0,
            });
        }
    }
}
