use rio_backend::config::colors::term::List;
use rio_backend::config::colors::{ColorBuilder, ColorComposition, Colors, Format};
use rio_backend::crosswords::pos::Line;
use sugarloaf::components::core::orthographic_projection;
use sugarloaf::font::{fonts::SugarloafFonts, FontLibrary};
use sugarloaf::grid::{CellBg, CellText, GridRenderer, GridUniforms};
use sugarloaf::layout::RootStyle;
use sugarloaf::{Sugarloaf, SugarloafRenderer};

use crate::grid_emit::{build_row_bg, build_row_fg, GlyphRasterizer};
use crate::terminal::WasmTerminal;

fn hex_arr(hex: &str, fallback: &str) -> rio_backend::config::colors::ColorArray {
    ColorBuilder::from_hex(hex.to_string(), Format::SRGB0_1)
        .or_else(|_| ColorBuilder::from_hex(fallback.to_string(), Format::SRGB0_1))
        .unwrap()
        .to_arr()
}

fn hex_composition(hex: &str, fallback: &str) -> ColorComposition {
    let arr = hex_arr(hex, fallback);
    let wgpu = sugarloaf::Color {
        r: arr[0] as f64,
        g: arr[1] as f64,
        b: arr[2] as f64,
        a: arr[3] as f64,
    };
    (arr, wgpu)
}

pub fn build_palette(fg: &str, bg: &str, cursor: &str) -> List {
    let mut colors = Colors::default();
    colors.foreground = hex_arr(fg, "#D4D4D4");
    colors.background = hex_composition(bg, "#1E1E1E");
    colors.cursor = hex_arr(cursor, "#AEAFAD");
    List::from(&colors)
}

pub struct WasmRenderer {
    pub sugarloaf: Sugarloaf<'static>,
    pub grid: GridRenderer,
    pub font_library: FontLibrary,
    pub rasterizer: GlyphRasterizer,
    pub palette: List,
    pub cell_w: f32,
    pub cell_h: f32,
    pub font_size: f32,
    pub width: f32,
    pub height: f32,
}

impl WasmRenderer {
    pub async fn new(
        canvas: web_sys::HtmlCanvasElement,
        cols: u32,
        rows: u32,
        font_size: f32,
        palette: List,
    ) -> Self {
        let width = canvas.width() as f32;
        let height = canvas.height() as f32;

        let fonts = SugarloafFonts::default();
        let (font_library, _errors) = FontLibrary::new(fonts);

        let layout = RootStyle::new(1.0, font_size, 1.0);
        let renderer_cfg = SugarloafRenderer::default();
        let sugarloaf =
            Sugarloaf::new_wasm_async(canvas, renderer_cfg, &font_library, layout)
                .await
                .unwrap_or_else(|_| panic!("sugarloaf init failed"));

        let cell_w = width / cols as f32;
        let cell_h = height / rows as f32;

        let context = sugarloaf.get_context();
        let grid = GridRenderer::new(context, cols, rows);

        WasmRenderer {
            sugarloaf,
            grid,
            font_library,
            rasterizer: GlyphRasterizer::new(),
            palette,
            cell_w,
            cell_h,
            font_size,
            width,
            height,
        }
    }

    pub fn resize(
        &mut self,
        cols: u32,
        rows: u32,
        width: f32,
        height: f32,
        font_size: f32,
    ) {
        self.width = width;
        self.height = height;
        self.font_size = font_size;
        self.cell_w = width / cols as f32;
        self.cell_h = height / rows as f32;
        self.grid.resize(cols, rows);
        self.sugarloaf.resize(width as u32, height as u32);
    }

    pub fn render_frame(
        &mut self,
        term: &WasmTerminal,
        cursor_col: u32,
        cursor_row: u32,
        show_cursor: bool,
        cursor_shape: u32,
    ) {
        let rows = term.rows();
        let cols = term.cols();
        let font_id: u32 = 0;
        let offset = term.display_offset() as i32;

        let mut bg_scratch: Vec<CellBg> = Vec::with_capacity(cols);
        let mut fg_scratch: Vec<CellText> = Vec::with_capacity(cols * 2);

        let grid = &term.crosswords.grid;
        let style_set = &grid.style_set;

        for y in 0..rows {
            let squares = &grid[Line(y as i32 - offset)].inner;

            build_row_bg(squares, style_set, &self.palette, &mut bg_scratch);

            build_row_fg(
                squares,
                style_set,
                y as u16,
                &self.font_library,
                font_id,
                self.font_size,
                self.cell_h,
                &self.palette,
                &mut self.rasterizer,
                &mut self.grid,
                &mut fg_scratch,
            );

            self.grid.write_row(y as u32, &bg_scratch, &fg_scratch);
        }

        let (cursor_pos, cursor_color, cursor_bg) = if show_cursor {
            (
                [cursor_col, cursor_row],
                [255u8, 255, 255, 255],
                [0.2f32, 0.5, 1.0, 1.0],
            )
        } else {
            ([u32::MAX, u32::MAX], [0u8; 4], [0.0f32; 4])
        };

        let uniforms = GridUniforms {
            projection: orthographic_projection(self.width, self.height),
            grid_padding: [0.0; 4],
            cursor_color: [
                cursor_color[0] as f32 / 255.0,
                cursor_color[1] as f32 / 255.0,
                cursor_color[2] as f32 / 255.0,
                cursor_color[3] as f32 / 255.0,
            ],
            cursor_bg_color: cursor_bg,
            cell_size: [self.cell_w, self.cell_h],
            grid_size: [cols as u32, rows as u32],
            cursor_pos,
            _pad_cursor: [0; 2],
            min_contrast: 0.0,
            flags: cursor_shape & 0x3,
            padding_extend: 0,
            input_colorspace: self.sugarloaf.input_colorspace(),
        };

        self.sugarloaf
            .render_with_grids(&mut [(&mut self.grid, uniforms)]);
    }
}
