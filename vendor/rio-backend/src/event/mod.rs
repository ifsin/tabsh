pub mod sync;

use crate::ansi::graphics::UpdateQueues;
use crate::clipboard::ClipboardType;
use crate::config::colors::ColorRgb;
use crate::crosswords::grid::Scroll;
use crate::crosswords::pos::{Direction, Pos};
use crate::crosswords::search::{Match, RegexSearch};
use crate::error::RioError;
use std::borrow::Cow;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::sync::Arc;

pub type WindowId = u64;

/// Terminal size — replaces teletypewriter::WinsizeBuilder so this
/// module compiles on WASM without the teletypewriter crate.
#[derive(Debug, Clone, Copy, Default)]
pub struct WinsizeBuilder {
    pub rows: u16,
    pub cols: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone)]
pub enum RioEventType {
    Rio(RioEvent),
    Frame,
}

#[derive(Debug)]
pub enum Msg {
    Input(Cow<'static, [u8]>),
    #[allow(dead_code)]
    Shutdown,
    Resize(WinsizeBuilder),
}

#[derive(Debug, Eq, PartialEq)]
pub enum ClickState {
    None,
    Click,
    DoubleClick,
    TripleClick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalDamage {
    #[default]
    Noop,
    Full,
    Partial,
    CursorOnly,
}

#[derive(Clone)]
pub enum RioEvent {
    PrepareRender(u64),
    PrepareRenderOnRoute(u64, usize),
    PrepareUpdateConfig,
    Render,
    RenderRoute(usize),
    TerminalDamaged(usize),
    UpdateGraphics {
        route_id: usize,
        queues: UpdateQueues,
    },
    GlyphProtocolInstalled {
        route_id: usize,
        registry: sugarloaf::font::glyph_registry::GlyphRegistry,
    },
    GlyphProtocolQuery {
        route_id: usize,
        cp: u32,
    },
    Paste,
    Copy(String),
    UpdateFontSize(u8),
    Scroll(Scroll),
    ToggleFullScreen,
    ToggleAppearanceTheme,
    Minimize(bool),
    Hide,
    HideOtherApplications,
    UpdateConfig,
    CreateWindow,
    CloseWindow,
    CreateNativeTab(Option<String>),
    CreateConfigEditor,
    SelectNativeTabByIndex(usize),
    SelectNativeTabLast,
    SelectNativeTabNext,
    SelectNativeTabPrev,
    ReportToAssistant(RioError),
    MouseCursorDirty,
    Title(String),
    TitleWithSubtitle(String, String),
    ResetTitle,
    ClipboardStore(ClipboardType, String),
    ClipboardLoad(
        usize,
        ClipboardType,
        Arc<dyn Fn(&str) -> String + Sync + Send + 'static>,
    ),
    ColorRequest(
        usize,
        usize,
        Arc<dyn Fn(ColorRgb) -> String + Sync + Send + 'static>,
    ),
    PtyWrite(usize, String),
    TextAreaSizeRequest(
        usize,
        Arc<dyn Fn(WinsizeBuilder) -> String + Sync + Send + 'static>,
    ),
    CursorBlinkingChange,
    CursorBlinkingChangeOnRoute(usize),
    ProgressReport(ProgressReport),
    Bell,
    DesktopNotification {
        title: String,
        body: String,
    },
    Exit,
    Quit,
    CloseTerminal(usize),
    BlinkCursor(u64, usize),
    SelectionScrollTick,
    UpdateTitles,
    ColorChange(usize, usize, Option<ColorRgb>),
    Noop,
}

impl Debug for RioEvent {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RioEvent::ClipboardStore(ty, text) => {
                write!(f, "ClipboardStore({ty:?}, {text})")
            }
            RioEvent::ClipboardLoad(route_id, ty, _) => {
                write!(f, "ClipboardLoad(route={route_id}, {ty:?})")
            }
            RioEvent::TextAreaSizeRequest(route_id, _) => {
                write!(f, "TextAreaSizeRequest(route={route_id})")
            }
            RioEvent::ColorRequest(route_id, index, _) => {
                write!(f, "ColorRequest(route={route_id}, idx={index})")
            }
            RioEvent::PtyWrite(route_id, text) => {
                write!(f, "PtyWrite(route={route_id}, {text})")
            }
            RioEvent::Title(title) => write!(f, "Title({title})"),
            RioEvent::TitleWithSubtitle(title, subtitle) => {
                write!(f, "TitleWithSubtitle({title}, {subtitle})")
            }
            RioEvent::Minimize(cond) => write!(f, "Minimize({cond})"),
            RioEvent::Hide => write!(f, "Hide)"),
            RioEvent::HideOtherApplications => write!(f, "HideOtherApplications)"),
            RioEvent::CursorBlinkingChange => write!(f, "CursorBlinkingChange"),
            RioEvent::CursorBlinkingChangeOnRoute(route_id) => {
                write!(f, "CursorBlinkingChangeOnRoute {route_id}")
            }
            RioEvent::ProgressReport(report) => write!(f, "ProgressReport({:?})", report),
            RioEvent::MouseCursorDirty => write!(f, "MouseCursorDirty"),
            RioEvent::ResetTitle => write!(f, "ResetTitle"),
            RioEvent::PrepareUpdateConfig => write!(f, "PrepareUpdateConfig"),
            RioEvent::PrepareRender(millis) => write!(f, "PrepareRender({millis})"),
            RioEvent::PrepareRenderOnRoute(millis, route) => {
                write!(f, "PrepareRender({millis} on route {route})")
            }
            RioEvent::Render => write!(f, "Render"),
            RioEvent::RenderRoute(route) => write!(f, "Render route {route}"),
            RioEvent::TerminalDamaged(route_id) => {
                write!(f, "TerminalDamaged route {route_id}")
            }
            RioEvent::GlyphProtocolInstalled { route_id, .. } => {
                write!(f, "GlyphProtocolInstalled route {route_id}")
            }
            RioEvent::GlyphProtocolQuery { route_id, cp } => {
                write!(f, "GlyphProtocolQuery route {route_id} cp {cp:#x}")
            }
            RioEvent::Scroll(scroll) => write!(f, "Scroll {scroll:?}"),
            RioEvent::Bell => write!(f, "Bell"),
            RioEvent::DesktopNotification { title, body } => {
                write!(f, "DesktopNotification({title}, {body})")
            }
            RioEvent::Exit => write!(f, "Exit"),
            RioEvent::Quit => write!(f, "Quit"),
            RioEvent::CloseTerminal(route) => write!(f, "CloseTerminal {route}"),
            RioEvent::CreateWindow => write!(f, "CreateWindow"),
            RioEvent::CloseWindow => write!(f, "CloseWindow"),
            RioEvent::CreateNativeTab(_) => write!(f, "CreateNativeTab"),
            RioEvent::SelectNativeTabByIndex(tab_index) => {
                write!(f, "SelectNativeTabByIndex({tab_index})")
            }
            RioEvent::SelectNativeTabLast => write!(f, "SelectNativeTabLast"),
            RioEvent::SelectNativeTabNext => write!(f, "SelectNativeTabNext"),
            RioEvent::SelectNativeTabPrev => write!(f, "SelectNativeTabPrev"),
            RioEvent::CreateConfigEditor => write!(f, "CreateConfigEditor"),
            RioEvent::UpdateConfig => write!(f, "ReloadConfiguration"),
            RioEvent::ReportToAssistant(error_report) => {
                write!(f, "ReportToAssistant({})", error_report.report)
            }
            RioEvent::ToggleFullScreen => write!(f, "FullScreen"),
            RioEvent::ToggleAppearanceTheme => write!(f, "ToggleAppearanceTheme"),
            RioEvent::BlinkCursor(timeout, route_id) => {
                write!(f, "BlinkCursor {timeout} {route_id}")
            }
            RioEvent::SelectionScrollTick => write!(f, "SelectionScrollTick"),
            RioEvent::UpdateTitles => write!(f, "UpdateTitles"),
            RioEvent::Noop => write!(f, "Noop"),
            RioEvent::Copy(_) => write!(f, "Copy"),
            RioEvent::Paste => write!(f, "Paste"),
            RioEvent::UpdateFontSize(action) => write!(f, "UpdateFontSize({action:?})"),
            RioEvent::UpdateGraphics { route_id, .. } => {
                write!(f, "UpdateGraphics({route_id})")
            }
            RioEvent::ColorChange(route_id, color, rgb) => {
                write!(f, "ColorChange({route_id}, {color:?}, {rgb:?})")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct EventPayload {
    pub payload: RioEventType,
    pub window_id: WindowId,
}

impl EventPayload {
    pub fn new(payload: RioEventType, window_id: WindowId) -> Self {
        Self { payload, window_id }
    }
}

pub trait OnResize {
    fn on_resize(&mut self, window_size: WinsizeBuilder);
}

pub trait EventListener {
    fn event(&self) -> (Option<RioEvent>, bool);
    fn send_event(&self, _event: RioEvent, _id: WindowId) {}
    fn send_event_with_high_priority(&self, _event: RioEvent, _id: WindowId) {}
    fn send_redraw(&self, _id: WindowId) {}
    fn send_global_event(&self, _event: RioEvent) {}
}

#[derive(Clone)]
pub struct VoidListener;

impl From<RioEvent> for RioEventType {
    fn from(rio_event: RioEvent) -> Self {
        Self::Rio(rio_event)
    }
}

impl EventListener for VoidListener {
    fn event(&self) -> (Option<RioEvent>, bool) {
        (None, false)
    }
}

pub struct SearchState {
    pub direction: Direction,
    pub history_index: Option<usize>,
    pub display_offset_delta: i32,
    pub origin: Pos,
    pub focused_match: Option<Match>,
    pub history: VecDeque<String>,
    pub dfas: Option<RegexSearch>,
}

impl SearchState {
    pub fn regex(&self) -> Option<&String> {
        self.history_index.and_then(|index| self.history.get(index))
    }

    pub fn direction(&self) -> Direction {
        self.direction
    }

    pub fn focused_match(&self) -> Option<&Match> {
        self.focused_match.as_ref()
    }

    pub fn clear_focused_match(&mut self) {
        self.focused_match = None;
    }

    pub fn dfas_mut(&mut self) -> Option<&mut RegexSearch> {
        self.dfas.as_mut()
    }

    pub fn dfas(&self) -> Option<&RegexSearch> {
        self.dfas.as_ref()
    }

    pub fn regex_mut(&mut self) -> Option<&mut String> {
        self.history_index
            .and_then(move |index| self.history.get_mut(index))
    }
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            direction: Direction::Right,
            display_offset_delta: Default::default(),
            focused_match: Default::default(),
            history_index: Default::default(),
            history: Default::default(),
            origin: Default::default(),
            dfas: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressState {
    Remove,
    Set,
    Error,
    Indeterminate,
    Pause,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgressReport {
    pub state: ProgressState,
    pub progress: Option<u8>,
}
