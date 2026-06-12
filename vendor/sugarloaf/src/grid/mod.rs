// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

pub mod atlas;
pub mod cell;
pub mod cpu;
#[cfg(feature = "wgpu")]
pub mod webgpu;

use crate::context::{Context, ContextType};

pub use atlas::{AtlasSlot, GlyphKey, RasterizedGlyph};
pub use cell::{CellBg, CellText, GridUniforms};

#[allow(clippy::large_enum_variant)]
pub enum GridRenderer {
    #[cfg(feature = "wgpu")]
    Wgpu(webgpu::WgpuGridRenderer),
    Cpu(cpu::CpuGridRenderer),
}

impl GridRenderer {
    pub fn new(context: &Context<'_>, cols: u32, rows: u32) -> Self {
        match &context.inner {
            #[cfg(not(feature = "wgpu"))]
            ContextType::_Phantom(_) => unreachable!(),
            #[cfg(feature = "wgpu")]
            ContextType::Wgpu(ctx) => {
                GridRenderer::Wgpu(webgpu::WgpuGridRenderer::new(ctx, cols, rows))
            }
            ContextType::Cpu(_) => {
                GridRenderer::Cpu(cpu::CpuGridRenderer::new(cols, rows))
            }
        }
    }

    pub fn resize(&mut self, cols: u32, rows: u32) {
        match self {
            #[cfg(feature = "wgpu")]
            GridRenderer::Wgpu(r) => r.resize(cols, rows),
            GridRenderer::Cpu(r) => r.resize(cols, rows),
        }
    }

    pub fn write_row(&mut self, row: u32, bg: &[CellBg], fg: &[CellText]) {
        match self {
            #[cfg(feature = "wgpu")]
            GridRenderer::Wgpu(r) => r.write_row(row, bg, fg),
            GridRenderer::Cpu(r) => r.write_row(row, bg, fg),
        }
    }

    pub fn clear_row(&mut self, row: u32) {
        match self {
            #[cfg(feature = "wgpu")]
            GridRenderer::Wgpu(r) => r.clear_row(row),
            GridRenderer::Cpu(r) => r.clear_row(row),
        }
    }

    pub fn set_block_cursor(&mut self, cells: &[CellText]) {
        match self {
            #[cfg(feature = "wgpu")]
            GridRenderer::Wgpu(r) => r.set_block_cursor(cells),
            GridRenderer::Cpu(r) => r.set_block_cursor(cells),
        }
    }

    pub fn set_non_block_cursor(&mut self, cells: &[CellText]) {
        match self {
            #[cfg(feature = "wgpu")]
            GridRenderer::Wgpu(r) => r.set_non_block_cursor(cells),
            GridRenderer::Cpu(r) => r.set_non_block_cursor(cells),
        }
    }

    pub fn clear_cursor(&mut self) {
        match self {
            #[cfg(feature = "wgpu")]
            GridRenderer::Wgpu(r) => r.clear_cursor(),
            GridRenderer::Cpu(r) => r.clear_cursor(),
        }
    }

    #[cfg(feature = "wgpu")]
    pub fn render_bg_wgpu(
        &mut self,
        render_pass: &mut wgpu::RenderPass<'_>,
        uniforms: &GridUniforms,
    ) {
        if let GridRenderer::Wgpu(r) = self {
            r.render_bg(render_pass, uniforms);
        }
    }

    #[cfg(feature = "wgpu")]
    pub fn render_text_wgpu(
        &mut self,
        render_pass: &mut wgpu::RenderPass<'_>,
        uniforms: &GridUniforms,
    ) {
        if let GridRenderer::Wgpu(r) = self {
            r.render_text(render_pass, uniforms);
        }
    }

    pub fn render_bg_cpu(
        &self,
        buf: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        uniforms: &GridUniforms,
    ) {
        if let GridRenderer::Cpu(r) = self {
            r.render_bg(buf, buf_w, buf_h, uniforms);
        }
    }

    pub fn render_text_cpu(
        &self,
        buf: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        uniforms: &GridUniforms,
    ) {
        if let GridRenderer::Cpu(r) = self {
            r.render_text(buf, buf_w, buf_h, uniforms);
        }
    }

    pub fn is_active(&self) -> bool {
        true
    }

    pub fn lookup_glyph(&self, key: atlas::GlyphKey) -> Option<atlas::AtlasSlot> {
        match self {
            #[cfg(feature = "wgpu")]
            GridRenderer::Wgpu(r) => r.lookup_glyph(key),
            GridRenderer::Cpu(r) => r.lookup_glyph(key),
        }
    }

    pub fn lookup_glyph_color(&self, key: atlas::GlyphKey) -> Option<atlas::AtlasSlot> {
        match self {
            #[cfg(feature = "wgpu")]
            GridRenderer::Wgpu(r) => r.lookup_glyph_color(key),
            GridRenderer::Cpu(r) => r.lookup_glyph_color(key),
        }
    }

    pub fn insert_glyph(
        &mut self,
        key: atlas::GlyphKey,
        glyph: atlas::RasterizedGlyph<'_>,
    ) -> Option<atlas::AtlasSlot> {
        match self {
            #[cfg(feature = "wgpu")]
            GridRenderer::Wgpu(r) => r.insert_glyph(key, glyph),
            GridRenderer::Cpu(r) => r.insert_glyph(key, glyph),
        }
    }

    pub fn insert_glyph_color(
        &mut self,
        key: atlas::GlyphKey,
        glyph: atlas::RasterizedGlyph<'_>,
    ) -> Option<atlas::AtlasSlot> {
        match self {
            #[cfg(feature = "wgpu")]
            GridRenderer::Wgpu(r) => r.insert_glyph_color(key, glyph),
            GridRenderer::Cpu(r) => r.insert_glyph_color(key, glyph),
        }
    }

    pub fn needs_full_rebuild(&self) -> bool {
        match self {
            #[cfg(feature = "wgpu")]
            GridRenderer::Wgpu(r) => r.needs_full_rebuild(),
            GridRenderer::Cpu(r) => r.needs_full_rebuild(),
        }
    }

    pub fn mark_full_rebuild_done(&mut self) {
        match self {
            #[cfg(feature = "wgpu")]
            GridRenderer::Wgpu(r) => r.mark_full_rebuild_done(),
            GridRenderer::Cpu(r) => r.mark_full_rebuild_done(),
        }
    }
}
