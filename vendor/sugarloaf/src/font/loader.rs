use std::path::PathBuf;

use crate::font::SharedData;
pub use ttf_parser::Language;

#[derive(Clone, Debug)]
pub struct ID {
    _dummy: u32,
}

#[derive(Clone, Debug)]
pub enum Source {
    File(PathBuf),
    Binary(SharedData),
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Query<'a> {
    pub families: &'a [Family<'a>],
    pub weight: Weight,
    pub stretch: Stretch,
    pub style: Style,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family<'a> {
    Name(&'a str),
    Serif,
    SansSerif,
    Cursive,
    Fantasy,
    Monospace,
}

#[derive(Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Debug, Hash)]
pub struct Weight(pub u16);

impl Default for Weight {
    fn default() -> Weight {
        Weight::NORMAL
    }
}

impl Weight {
    pub const THIN: Weight = Weight(100);
    pub const EXTRA_LIGHT: Weight = Weight(200);
    pub const LIGHT: Weight = Weight(300);
    pub const NORMAL: Weight = Weight(400);
    pub const MEDIUM: Weight = Weight(500);
    pub const SEMIBOLD: Weight = Weight(600);
    pub const BOLD: Weight = Weight(700);
    pub const EXTRA_BOLD: Weight = Weight(800);
    pub const BLACK: Weight = Weight(900);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default, PartialOrd, Ord)]
pub enum Stretch {
    UltraCondensed,
    ExtraCondensed,
    Condensed,
    SemiCondensed,
    #[default]
    Normal,
    SemiExpanded,
    Expanded,
    ExtraExpanded,
    UltraExpanded,
}

#[derive(Clone, Default, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Style {
    #[default]
    Normal,
    Italic,
    Oblique,
}

pub struct Database {}

impl Database {
    pub fn new() -> Self {
        Self {}
    }

    pub fn load_fonts_dir<P: AsRef<std::path::Path>>(&mut self, _path: P) {}

    pub fn query(&self, _query: &Query) -> Option<ID> {
        None
    }

    pub fn face_source(&self, _id: ID) -> Option<(Source, u32)> {
        None
    }
}

impl Default for Database {
    fn default() -> Self {
        Self::new()
    }
}
