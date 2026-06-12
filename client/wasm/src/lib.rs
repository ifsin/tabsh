mod event_listener;
mod grid_emit;
mod input;
mod mouse;
mod renderer;
mod terminal;

use js_sys::Uint8Array;
use renderer::{build_palette, WasmRenderer};
use rio_backend::ansi::CursorShape;
use terminal::WasmTerminal;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

#[wasm_bindgen]
pub struct TabshTerminal {
    term: WasmTerminal,
    renderer: WasmRenderer,
}

#[wasm_bindgen]
impl TabshTerminal {
    pub fn on_pty_data(&mut self, data: &[u8]) {
        self.term.feed(data);
        self.redraw();
    }

    pub fn redraw(&mut self) {
        self.redraw_ex(true);
    }

    pub fn redraw_ex(&mut self, cursor_phase: bool) {
        let col = self.term.cursor_col() as u32;
        let row = self.term.cursor_row() + self.term.display_offset();
        let on_screen = row < self.term.rows();
        self.renderer.render_frame(
            &self.term,
            col,
            row as u32,
            self.term.show_cursor() && on_screen && cursor_phase,
            self.term.cursor_shape_bits(),
        );
    }

    pub fn cursor_shape_bits(&self) -> u32 {
        self.term.cursor_shape_bits()
    }

    pub fn blinking_cursor(&self) -> bool {
        self.term.blinking_cursor()
    }

    pub fn on_key(
        &self,
        key: &str,
        code: &str,
        shift: bool,
        ctrl: bool,
        alt: bool,
        meta: bool,
    ) -> Uint8Array {
        let bytes = input::encode_key(key, code, shift, ctrl, alt, meta);
        if bytes.is_empty() {
            return Uint8Array::new_with_length(0);
        }
        let mut msg = Vec::with_capacity(1 + bytes.len());
        msg.push(0x00u8);
        msg.extend_from_slice(&bytes);
        let arr = Uint8Array::new_with_length(msg.len() as u32);
        arr.copy_from(&msg);
        arr
    }

    pub fn resize(
        &mut self,
        cols: u32,
        rows: u32,
        width: f32,
        height: f32,
        font_size: f32,
    ) {
        self.term.resize(cols as usize, rows as usize);
        self.renderer.resize(cols, rows, width, height, font_size);
    }

    pub fn pop_title(&mut self) -> Option<String> {
        self.term.pop_title()
    }

    pub fn pop_cwd(&mut self) -> Option<String> {
        self.term.pop_cwd()
    }

    pub fn pop_cmd(&mut self) -> Option<String> {
        self.term.pop_cmd()
    }

    pub fn pop_bell(&mut self) -> bool {
        self.term.pop_bell()
    }

    pub fn display_offset(&self) -> usize {
        self.term.display_offset()
    }

    pub fn history_size(&self) -> usize {
        self.term.history_size()
    }

    pub fn total_lines(&self) -> usize {
        self.term.total_lines()
    }

    pub fn set_display_offset(&mut self, target: usize) {
        self.term.set_display_offset(target);
    }

    pub fn mode_bits(&self) -> u32 {
        self.term.mode_bits()
    }

    pub fn line_text(&self, doc_index: usize) -> String {
        self.term.line_text(doc_index)
    }

    pub fn encode_mouse(
        &self,
        kind: u8,
        button: u8,
        col: u16,
        row: u16,
        shift: bool,
        alt: bool,
        ctrl: bool,
    ) -> Uint8Array {
        let sgr = self.term.mode_bits() & mouse::MODE_SGR != 0;
        let bytes = mouse::encode_mouse(kind, button, col, row, shift, alt, ctrl, sgr);
        let mut msg = Vec::with_capacity(1 + bytes.len());
        msg.push(0x00u8);
        msg.extend_from_slice(&bytes);
        let arr = Uint8Array::new_with_length(msg.len() as u32);
        arr.copy_from(&msg);
        arr
    }

    pub fn resize_message(cols: u32, rows: u32) -> Uint8Array {
        let json = format!(r#"{{"cols":{cols},"rows":{rows}}}"#);
        let payload = json.as_bytes();
        let arr = Uint8Array::new_with_length(1 + payload.len() as u32);
        arr.set_index(0, 0x01);
        for (i, &b) in payload.iter().enumerate() {
            arr.set_index(1 + i as u32, b);
        }
        arr
    }

    pub fn init_message(
        session_id: &str,
        cols: u32,
        rows: u32,
        cwd: &str,
        app_id: &str,
    ) -> Uint8Array {
        let json = format!(
            r#"{{"sessionId":"{session_id}","cols":{cols},"rows":{rows},"cwd":"{cwd}","appId":"{app_id}","cmd":""}}"#
        );
        let payload = json.as_bytes();
        let arr = Uint8Array::new_with_length(1 + payload.len() as u32);
        arr.set_index(0, 0x02);
        for (i, &b) in payload.iter().enumerate() {
            arr.set_index(1 + i as u32, b);
        }
        arr
    }
}

#[wasm_bindgen]
pub async fn init_terminal(
    canvas: web_sys::HtmlCanvasElement,
    cols: u32,
    rows: u32,
    font_size: f32,
    cursor_shape: u32,
    fg: &str,
    bg: &str,
    cursor_color: &str,
) -> TabshTerminal {
    let shape = match cursor_shape {
        1 => CursorShape::Beam,
        2 => CursorShape::Underline,
        _ => CursorShape::Block,
    };
    let palette = build_palette(fg, bg, cursor_color);
    let renderer = WasmRenderer::new(canvas, cols, rows, font_size, palette).await;
    let term = WasmTerminal::new(cols as usize, rows as usize, shape);
    TabshTerminal { term, renderer }
}
