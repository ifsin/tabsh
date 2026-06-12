use crate::event_listener::WasmListener;
use rio_backend::ansi::CursorShape;
use rio_backend::crosswords::pos::Line;
use rio_backend::crosswords::square::Wide;
use rio_backend::crosswords::{grid::Dimensions, grid::Scroll, Crosswords, Mode};
use rio_backend::performer::handler::Processor;

pub const SCROLLBACK_LINES: usize = 10_000;

struct TermSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct WasmTerminal {
    pub crosswords: Crosswords<WasmListener>,
    processor: Processor,
    listener: WasmListener,
    last_title: String,
    last_cwd: Option<String>,
    last_cmd: Option<String>,
}

impl WasmTerminal {
    pub fn new(cols: usize, rows: usize, cursor_shape: CursorShape) -> Self {
        let listener = WasmListener::default();
        let crosswords = Crosswords::new(
            TermSize { cols, rows },
            cursor_shape,
            listener.clone(),
            0u64,
            0,
            SCROLLBACK_LINES,
        );
        Self {
            crosswords,
            processor: Processor::default(),
            listener,
            last_title: String::new(),
            last_cwd: None,
            last_cmd: None,
        }
    }

    pub fn feed(&mut self, data: &[u8]) {
        self.processor.advance(&mut self.crosswords, data);
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.crosswords.resize(TermSize { cols, rows });
    }

    pub fn cols(&self) -> usize {
        self.crosswords.columns()
    }

    pub fn rows(&self) -> usize {
        self.crosswords.screen_lines()
    }

    pub fn cursor_col(&self) -> usize {
        self.crosswords.grid.cursor.pos.col.0
    }

    pub fn cursor_row(&self) -> usize {
        self.crosswords.grid.cursor.pos.row.0.max(0) as usize
    }

    pub fn show_cursor(&self) -> bool {
        self.crosswords.mode().contains(Mode::SHOW_CURSOR)
    }

    pub fn pop_title(&mut self) -> Option<String> {
        let current = &self.crosswords.title;
        if *current != self.last_title {
            self.last_title = current.clone();
            Some(self.last_title.clone())
        } else {
            None
        }
    }

    pub fn pop_cwd(&mut self) -> Option<String> {
        let current = self
            .crosswords
            .current_directory
            .as_deref()
            .and_then(|p| p.to_str())
            .map(|s| s.to_owned());
        if current != self.last_cwd {
            self.last_cwd = current.clone();
            current
        } else {
            None
        }
    }

    pub fn pop_bell(&mut self) -> bool {
        self.listener.take_bell()
    }

    /// Returns Some(cmd) when current_command changed since last call.
    /// Returns Some("") when the command was cleared (prompt returned).
    /// Returns None when nothing changed.
    pub fn pop_cmd(&mut self) -> Option<String> {
        let current = self.crosswords.current_command.clone();
        if current != self.last_cmd {
            self.last_cmd = current.clone();
            Some(current.unwrap_or_default())
        } else {
            None
        }
    }

    pub fn display_offset(&self) -> usize {
        self.crosswords.display_offset()
    }

    pub fn history_size(&self) -> usize {
        self.crosswords.history_size()
    }

    pub fn total_lines(&self) -> usize {
        self.history_size() + self.rows()
    }

    pub fn set_display_offset(&mut self, target: usize) {
        let target = target.min(self.history_size());
        let current = self.display_offset() as i32;
        let delta = target as i32 - current;
        if delta != 0 {
            self.crosswords.scroll_display(Scroll::Delta(delta));
        }
    }

    pub fn mode_bits(&self) -> u32 {
        self.crosswords.mode().bits()
    }

    pub fn cursor_shape_bits(&self) -> u32 {
        use rio_backend::ansi::CursorShape;
        match self.crosswords.cursor_shape {
            CursorShape::Block => 0,
            CursorShape::Beam => 1,
            CursorShape::Underline => 2,
            _ => 0,
        }
    }

    pub fn blinking_cursor(&self) -> bool {
        self.crosswords.blinking_cursor
    }

    pub fn line_text(&self, doc_index: usize) -> String {
        let line = doc_index as i32 - self.history_size() as i32;
        let row = &self.crosswords.grid[Line(line)];
        let mut s = String::with_capacity(row.inner.len());
        for sq in row.inner.iter() {
            match sq.wide() {
                Wide::Spacer | Wide::LeadingSpacer => continue,
                _ => s.push(sq.c()),
            }
        }
        while s.ends_with(' ') {
            s.pop();
        }
        s
    }
}
