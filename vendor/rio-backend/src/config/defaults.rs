use crate::{ansi::CursorShape, config::Shell};

#[inline]
pub fn default_bool_true() -> bool {
    true
}

#[inline]
pub fn default_line_height() -> f32 {
    1.0
}

#[inline]
pub fn default_cursor_interval() -> u64 {
    800
}

#[inline]
pub fn default_scrollback_history_limit() -> usize {
    10_000
}

#[inline]
pub fn default_title_placeholder() -> Option<String> {
    Some(String::from("▲"))
}

#[inline]
pub fn default_title_content() -> String {
    String::from("{{ TITLE || PROGRAM }}")
}

#[inline]
pub fn default_margin() -> crate::config::layout::Margin {
    crate::config::layout::Margin::all(2.0)
}

#[inline]
pub fn default_shell() -> crate::config::Shell {
    crate::config::Shell {
        program: String::from(""),
        args: vec![String::from("--login")],
    }
}

#[inline]
pub fn default_use_fork() -> bool {
    true
}

#[inline]
pub fn default_working_dir() -> Option<String> {
    None
}

#[inline]
pub fn default_opacity() -> f32 {
    1.0
}

#[inline]
pub fn default_option_as_alt() -> String {
    String::from("none")
}

#[inline]
pub fn default_log_level() -> String {
    String::from("OFF")
}

#[inline]
pub fn default_cursor() -> CursorShape {
    CursorShape::default()
}

#[inline]
pub fn default_theme() -> String {
    String::from("")
}

#[inline]
pub fn default_editor() -> Shell {
    Shell {
        program: String::from("vi"),
        args: vec![],
    }
}

#[inline]
pub fn default_window_width() -> i32 {
    800
}

#[inline]
pub fn default_window_height() -> i32 {
    490
}

#[inline]
pub fn default_disable_ctlseqs_alt() -> bool {
    false
}

#[inline]
pub fn default_ime_cursor_positioning() -> bool {
    true
}

pub fn default_config_file_content() -> String {
    String::from(
        "# See the full configuration reference: https://rioterm.com/docs/config\n",
    )
}
