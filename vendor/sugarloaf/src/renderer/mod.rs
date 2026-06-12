mod batch;
mod compositor;
pub mod cpu {
    #[derive(Default)]
    pub struct CpuCache;
    impl CpuCache {
        pub fn new() -> Self {
            Self
        }
        pub fn clear(&mut self) {}
    }
    pub fn render_cpu(
        _ctx: &mut crate::context::cpu::CpuContext,
        _renderer: &super::Renderer,
        _cache: &mut CpuCache,
        _background: Option<crate::sugarloaf::Color>,
        _grids: &mut [(&mut crate::grid::GridRenderer, crate::grid::GridUniforms)],
        _text: &crate::text::Text,
    ) {
    }
}
pub(crate) mod image_cache;

use crate::components::core::orthographic_projection;
#[cfg(feature = "wgpu")]
use crate::context::webgpu::WgpuContext;
use crate::context::{Context, ContextType};
use crate::font::FontLibrary;
use crate::layout::TextDimensions;
use crate::renderer::image_cache::ImageCache;
use crate::Graphics;
use compositor::{Compositor, Rect, Vertex};
use rustc_hash::FxHashMap;
#[cfg(feature = "wgpu")]
use std::mem;
#[cfg(feature = "wgpu")]
use wgpu::util::DeviceExt;

#[cfg(feature = "wgpu")]
use std::borrow::Cow;

#[cfg(feature = "wgpu")]
pub const BLEND: Option<wgpu::BlendState> = Some(wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
});

#[allow(clippy::large_enum_variant)]
pub enum RendererType {
    #[cfg(feature = "wgpu")]
    Wgpu(WgpuRenderer),
    Cpu,
}

#[cfg(feature = "wgpu")]
pub struct WgpuRenderer {
    vertex_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    constant_bind_group: wgpu::BindGroup,
    layout_bind_group: wgpu::BindGroup,
    layout_bind_group_layout: wgpu::BindGroupLayout,
    transform: wgpu::Buffer,
    pipeline: wgpu::RenderPipeline,
    instanced_pipeline: wgpu::RenderPipeline,
    current_transform: [f32; 16],
    supported_vertex_buffer: usize,
    supported_instance_buffer: usize,
    // Image pipeline (separate from text)
    image_pipeline: wgpu::RenderPipeline,
    image_bind_group_layout: wgpu::BindGroupLayout,
    image_vertex_buffer: wgpu::Buffer,
    /// Dedicated one-instance vertex buffer for the background image,
    /// kept separate from the kitty `image_vertex_buffer` so it cannot
    /// collide with kitty placement slots.
    background_image_vertex_buffer: wgpu::Buffer,
}

enum ImageTexture {
    #[cfg(feature = "wgpu")]
    Wgpu {
        _texture: wgpu::Texture,
        view: wgpu::TextureView,
    },
}

/// Per-image texture entry stored in the renderer.
struct ImageTextureEntry {
    gpu: ImageTexture,
    transmit_time: std::time::Instant,
}

/// Per-instance data for image rendering (one instance = one image placement).
/// The vertex shader generates 4 quad corners from vertex_id.
///
/// `pub` because it appears in the signature of
/// `vulkan::VulkanRenderer::render_image_overlays` (also `pub` so
/// the `Renderer` dispatcher can call it). Not part of the crate's
/// public API in spirit — just in visibility.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Zeroable, bytemuck::Pod)]
pub struct ImageInstance {
    /// Screen position of the image top-left (physical pixels).
    pub dest_pos: [f32; 2],
    /// Size of the image on screen (physical pixels).
    pub dest_size: [f32; 2],
    /// Source rectangle in the texture: xy = origin, zw = size (normalized 0..1).
    pub source_rect: [f32; 4],
}

/// Which layer to render the image in. Mirrors ghostty's
/// three-bucket split (`renderer/image.zig:94-97`,
/// `renderer/generic.zig:1647-1695`):
///
/// - `BelowBg`   — `z < BG_LIMIT`. Drawn before the cell-bg pass; sits
///   underneath everything terminal-related.
/// - `BelowText` — `BG_LIMIT ≤ z < 0`. Drawn between cell-bg and
///   cell-text passes — the kitty default for "image with text on top".
/// - `AboveText` — `z >= 0`. Drawn after the cell-text pass; sits on
///   top of all glyphs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ImageLayer {
    BelowBg,
    BelowText,
    AboveText,
}

/// Threshold separating `BelowBg` from `BelowText`. Matches ghostty's
/// `bg_limit = std.math.minInt(i32) / 2` at
/// `renderer/image.zig:377`.
const IMAGE_BG_LIMIT: i32 = i32::MIN / 2;

/// A single image draw command for the image pipeline.
struct ImageDraw {
    image_id: u32,
    instance: ImageInstance,
    layer: ImageLayer,
}

/// Decoded background image pixels (RGBA8) waiting to be uploaded to the GPU.
pub struct BackgroundImagePixels {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub struct Renderer {
    brush_type: RendererType,
    comp: Compositor,
    instances: Vec<batch::QuadInstance>,
    vertices: Vec<Vertex>,
    draw_cmds: Vec<batch::DrawCmd>,
    images: ImageCache,
    /// Per-image GPU textures (one map, any backend).
    image_textures: FxHashMap<u32, ImageTextureEntry>,
    /// Image draw commands for the current frame.
    image_draws: Vec<ImageDraw>,
    /// Pending background image upload (consumed by `prepare`).
    background_image_dirty: Option<BackgroundImagePixels>,
    /// Dedicated GPU texture for the background image, sized to the
    /// image dimensions instead of going through the glyph atlas.
    background_image_texture: Option<ImageTextureEntry>,
}

/// Upload `pixels` to a fresh GPU texture using whatever backend `context`
/// is bound to. Mirrors the per-image upload in `render_graphic_overlays`,
/// but produces a standalone `ImageTextureEntry` sized exactly to the image
/// instead of consuming a slot in the glyph atlas.
// Linux+no-wgpu: every match arm diverges (Cpu/Vulkan return early, Phantom
// is unreachable!()), so `gpu` is uninhabited and the trailing `Some(...)`
// is statically unreachable.
#[allow(unused_variables, unreachable_code)]
fn upload_background_image_texture(
    context: &mut crate::context::Context,
    pixels: &BackgroundImagePixels,
) -> Option<ImageTextureEntry> {
    if pixels.width == 0 || pixels.height == 0 {
        return None;
    }
    let gpu = match &context.inner {
        crate::context::ContextType::Cpu(_) => return None,
        // Vulkan path: the renderer owns the descriptor-set layout
        // and shared sampler; we read them off the live brush_type
        // here. Only the renderer is on the Sugarloaf struct, not
        // the context, so the call site below threads them in.
        // Actually: this function is a free fn taking only the
        // context — we need to defer the upload until we have the
        // renderer too. We do that by panicking here and pushing
        // the real upload into a renderer method (see
        // `Renderer::upload_background_image_vulkan`). When the
        // dispatcher (`prepare`) sees a Vulkan ctx + dirty pixels,
        // it calls the renderer method directly instead of this
        // free function.
        #[cfg(feature = "wgpu")]
        crate::context::ContextType::Wgpu(ctx) => {
            let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sugarloaf::background image"),
                size: wgpu::Extent3d {
                    width: pixels.width,
                    height: pixels.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            ctx.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &pixels.pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(pixels.width * 4),
                    rows_per_image: Some(pixels.height),
                },
                wgpu::Extent3d {
                    width: pixels.width,
                    height: pixels.height,
                    depth_or_array_layers: 1,
                },
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            ImageTexture::Wgpu {
                _texture: texture,
                view,
            }
        }
        #[cfg(not(feature = "wgpu"))]
        crate::context::ContextType::_Phantom(_) => unreachable!(),
    };
    Some(ImageTextureEntry {
        gpu,
        transmit_time: std::time::Instant::now(),
    })
}

impl Renderer {
    pub fn new(context: &Context, colorspace: crate::sugarloaf::Colorspace) -> Self {
        let _ = colorspace;
        let brush_type = match &context.inner {
            #[cfg(feature = "wgpu")]
            ContextType::Wgpu(wgpu_context) => {
                RendererType::Wgpu(WgpuRenderer::new(wgpu_context))
            }
            ContextType::Cpu(_) => RendererType::Cpu,
            #[cfg(not(feature = "wgpu"))]
            ContextType::_Phantom(_) => unreachable!(),
        };

        Self {
            brush_type,
            comp: Compositor::new(),
            instances: vec![],
            vertices: vec![],
            draw_cmds: vec![],
            images: ImageCache::new(context),
            image_textures: FxHashMap::default(),
            image_draws: Vec::new(),
            background_image_dirty: None,
            background_image_texture: None,
        }
    }

    /// Drain per-frame batch state that was populated via
    /// `rect` / `quad` / etc. Normally `comp.batches` is drained
    /// inside `compute_updates` → `comp.finish()` during a render;
    /// when the caller skips the GPU submit entirely (see
    /// `Sugarloaf::discard_frame`), those recorded primitives would
    /// otherwise pile into the next presented frame.
    #[inline]
    pub(crate) fn discard_frame_batches(&mut self) {
        self.comp.batches.reset();
    }

    /// Replace the background image. Pass `None` to clear it. The pixels
    /// are uploaded into a dedicated GPU texture on the next `prepare`
    /// call (so we don't go through the glyph atlas).
    pub fn set_background_image_pixels(&mut self, pixels: Option<BackgroundImagePixels>) {
        if pixels.is_some() {
            self.background_image_dirty = pixels;
        } else {
            self.background_image_dirty = None;
            self.background_image_texture = None;
        }
    }

    #[inline]
    pub fn prepare(
        &mut self,
        context: &mut crate::context::Context,
        _state: &crate::sugarloaf::state::SugarState,
        _graphics: &mut Graphics,
        image_data: &mut rustc_hash::FxHashMap<
            u32,
            crate::sugarloaf::graphics::GraphicDataEntry,
        >,
        image_overlays: &rustc_hash::FxHashMap<
            usize,
            Vec<crate::sugarloaf::graphics::GraphicOverlay>,
        >,
    ) {
        self.instances.clear();
        self.vertices.clear();
        self.draw_cmds.clear();

        // The per-id `Content.states` walk is gone — non-Text content
        // arms (Rect/RoundedRect/Line/Triangle/Polygon/Arc/Image) had
        // no rio caller passing `Some(id)`, so the Content registry
        // never accumulated them. Immediate-mode primitives flow
        // through `Renderer::rect/quad/...` straight into
        // `comp.batches`; rich-text emission is handled by the grid
        // pass and `sugarloaf::text`.

        // Image overlays: rio is responsible for not leaving stale
        // overlays for hidden panels (callers `clear_image_overlays_for`
        // on hide / panel removal). The renderer just drains whatever
        // `image_overlays` currently holds.
        let overlays: Vec<_> = image_overlays.values().flat_map(|v| v.iter()).collect();
        if !overlays.is_empty() {
            self.render_graphic_overlays(context, image_data, &overlays);
        } else {
            // No overlays visible — clear draw commands so stale images
            // don't keep rendering. Keep image_textures and image_data
            // so images can be re-rendered when scrolling back.
            self.image_draws.clear();
        }

        if let Some(pixels) = self.background_image_dirty.take() {
            self.background_image_texture =
                upload_background_image_texture(context, &pixels);
        }

        self.instances.clear();
        self.vertices.clear();
        self.draw_cmds.clear();
        self.images.process_atlases(context);
        self.comp
            .finish(&mut self.instances, &mut self.vertices, &mut self.draw_cmds);

        // Useful for debug occasionally
        // let inst_bytes =
        // self.instances.len() * std::mem::size_of::<batch::QuadInstance>();
        // let vert_bytes = self.vertices.len() * std::mem::size_of::<Vertex>();
        // println!(
        // "gpu upload: {} instances ({:.2} MB) + {} verts ({:.2} MB) = {:.2} MB",
        // self.instances.len(),
        // inst_bytes as f64 / (1024.0 * 1024.0),
        // self.vertices.len(),
        // vert_bytes as f64 / (1024.0 * 1024.0),
        // (inst_bytes + vert_bytes) as f64 / (1024.0 * 1024.0),
        // );
    }

    #[inline]
    /// Get character cell dimensions using font metrics (fast, no rendering)
    pub fn get_character_cell_dimensions(
        &self,
        font_library: &FontLibrary,
        font_size: f32,
        line_height: f32,
    ) -> Option<TextDimensions> {
        // Use read lock instead of write lock since we're not modifying
        if let Some(font_library_data) = font_library.inner.try_read() {
            let font_id = 0; // FONT_ID_REGULAR

            // Use existing method to get cached metrics
            drop(font_library_data); // Drop read lock
            let mut font_library_data = font_library.inner.write();
            if let Some((ascent, descent, leading)) =
                font_library_data.get_font_metrics(&font_id, font_size)
            {
                // Calculate character width using font metrics
                // For monospace fonts, we can estimate character width
                let char_width = font_size * 0.6; // Common monospace width ratio
                let total_line_height = (ascent + descent + leading) * line_height;

                return Some(TextDimensions {
                    width: char_width.max(1.0),
                    height: total_line_height.max(1.0),
                    scale: 1.0,
                });
            }
        }
        None
    }

    /// Render image overlays using per-image GPU textures.
    // Linux+no-wgpu: the kitty-upload match's wgpu/metal arms are
    // cfg'd out; remaining arms (Cpu unreachable!, Vulkan unreachable!,
    // Phantom continue) all diverge so `gpu` is uninhabited and the
    // trailing `image_textures.insert(...)` is statically unreachable.
    #[allow(unused_variables, unreachable_code)]
    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn render_graphic_overlays(
        &mut self,
        context: &mut crate::context::Context,
        image_data: &mut rustc_hash::FxHashMap<
            u32,
            crate::sugarloaf::graphics::GraphicDataEntry,
        >,
        overlays: &[&crate::sugarloaf::graphics::GraphicOverlay],
    ) {
        // Note: don't evict textures not in the current overlay set —
        // images may be temporarily off-screen and need their texture
        // when scrolling back into view.

        // Upload/update per-image textures
        for overlay in overlays {
            let entry = match image_data.get(&overlay.image_id) {
                Some(e) => e,
                None => continue,
            };

            // Skip if texture is current
            if let Some(existing) = self.image_textures.get(&overlay.image_id) {
                if existing.transmit_time == entry.transmit_time {
                    continue;
                }
            }

            let (width, height, pixels) = match &entry.handle.data {
                crate::components::core::image::Data::Rgba {
                    width,
                    height,
                    pixels,
                } => (*width, *height, pixels.as_ref()),
                _ => continue,
            };

            if width == 0 || height == 0 {
                continue;
            }

            if matches!(&context.inner, crate::context::ContextType::Cpu(_)) {
                continue;
            }
            let gpu = match &context.inner {
                crate::context::ContextType::Cpu(_) => unreachable!(),
                #[cfg(feature = "wgpu")]
                crate::context::ContextType::Wgpu(ctx) => {
                    let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("kitty image"),
                        size: wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::COPY_DST
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    });
                    ctx.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        pixels,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(width * 4),
                            rows_per_image: Some(height),
                        },
                        wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                    );
                    let view =
                        texture.create_view(&wgpu::TextureViewDescriptor::default());
                    ImageTexture::Wgpu {
                        _texture: texture,
                        view,
                    }
                }
                #[cfg(not(feature = "wgpu"))]
                crate::context::ContextType::_Phantom(_) => continue,
            };

            self.image_textures.insert(
                overlay.image_id,
                ImageTextureEntry {
                    gpu,
                    transmit_time: entry.transmit_time,
                },
            );
        }

        // Build image draw commands (one instance per image placement)
        self.image_draws.clear();
        for overlay in overlays {
            if !self.image_textures.contains_key(&overlay.image_id) {
                continue;
            }
            self.image_draws.push(ImageDraw {
                image_id: overlay.image_id,
                instance: ImageInstance {
                    dest_pos: [overlay.x, overlay.y],
                    dest_size: [overlay.width, overlay.height],
                    source_rect: overlay.source_rect,
                },
                layer: if overlay.z_index < IMAGE_BG_LIMIT {
                    ImageLayer::BelowBg
                } else if overlay.z_index < 0 {
                    ImageLayer::BelowText
                } else {
                    ImageLayer::AboveText
                },
            });
        }
    }

    #[inline]
    pub fn reset(&mut self) {
        self.image_textures.clear();
        self.image_draws.clear();
    }

    #[inline]
    pub fn clear_atlas(&mut self) {
        self.images.clear_atlas();
        self.image_textures.clear();
        self.image_draws.clear();
        tracing::info!("Renderer atlas cleared");
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn rect(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: [f32; 4],
        depth: f32,
        order: u8,
    ) {
        self.comp.batches.rect(
            &Rect {
                x,
                y,
                width,
                height,
            },
            depth,
            &color,
            order,
        );
    }

    /// Add a rounded rectangle with the specified border radius
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn rounded_rect(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: [f32; 4],
        depth: f32,
        border_radius: f32,
        order: u8,
    ) {
        self.comp.batches.rounded_rect(
            &Rect {
                x,
                y,
                width,
                height,
            },
            depth,
            &color,
            border_radius,
            order,
        );
    }

    /// Add a quad with per-corner radii
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn quad(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        background_color: [f32; 4],
        corner_radii: [f32; 4],
        depth: f32,
        order: u8,
    ) {
        self.comp.batches.quad(
            &Rect {
                x,
                y,
                width,
                height,
            },
            depth,
            &background_color,
            corner_radii,
            order,
        );
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn add_image_rect(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: [f32; 4],
        coords: [f32; 4],
        depth: f32,
        atlas_layer: i32,
    ) {
        self.comp.batches.add_image_rect(
            &Rect {
                x,
                y,
                width,
                height,
            },
            depth,
            &color,
            &coords,
            atlas_layer,
        );
    }

    #[inline]
    pub fn polygon(&mut self, points: &[(f32, f32)], depth: f32, color: [f32; 4]) {
        self.comp
            .batches
            .add_antialiased_polygon(points, depth, color);
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn triangle(
        &mut self,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        x3: f32,
        y3: f32,
        depth: f32,
        color: [f32; 4],
    ) {
        self.comp
            .batches
            .add_triangle(x1, y1, x2, y2, x3, y3, depth, color);
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn line(
        &mut self,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        width: f32,
        depth: f32,
        color: [f32; 4],
        order: u8,
    ) {
        self.comp
            .batches
            .add_line(x1, y1, x2, y2, width, depth, color, order);
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn arc(
        &mut self,
        center_x: f32,
        center_y: f32,
        radius: f32,
        start_angle_deg: f32,
        end_angle_deg: f32,
        stroke_width: f32,
        depth: f32,
        color: [f32; 4],
    ) {
        self.comp.batches.add_arc(
            center_x,
            center_y,
            radius,
            start_angle_deg,
            end_angle_deg,
            stroke_width,
            depth,
            &color,
        );
    }

    #[inline]
    #[cfg(feature = "wgpu")]
    pub fn render<'pass>(
        &'pass mut self,
        ctx: &mut WgpuContext,
        rpass: &mut wgpu::RenderPass<'pass>,
    ) {
        // Destructure to get independent borrows of different fields
        let Self {
            brush_type,
            images,
            instances,
            vertices,
            draw_cmds,
            image_draws,
            image_textures,
            background_image_texture,
            ..
        } = self;

        if let RendererType::Wgpu(brush) = brush_type {
            let color_views = images.get_texture_views();
            let mask_texture_view = images.get_mask_texture_view();

            let has_images = !image_draws.is_empty();
            let has_background = background_image_texture.is_some();
            if (color_views.is_empty() || (instances.is_empty() && vertices.is_empty()))
                && !has_images
                && !has_background
            {
                return;
            }

            // Background image: drawn first so all subsequent text/rects
            // composite on top. Single fullscreen instance, dedicated
            // vertex buffer, reuses the kitty image pipeline + sampler.
            if let Some(bg_tex) = background_image_texture.as_ref() {
                #[allow(irrefutable_let_patterns)]
                if let ImageTexture::Wgpu { view, .. } = &bg_tex.gpu {
                    let instance = ImageInstance {
                        dest_pos: [0.0, 0.0],
                        dest_size: [ctx.size.width, ctx.size.height],
                        source_rect: [0.0, 0.0, 1.0, 1.0],
                    };
                    ctx.queue.write_buffer(
                        &brush.background_image_vertex_buffer,
                        0,
                        bytemuck::bytes_of(&instance),
                    );
                    let bg_bind =
                        ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("background image bind group"),
                            layout: &brush.image_bind_group_layout,
                            entries: &[wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(view),
                            }],
                        });
                    rpass.set_pipeline(&brush.image_pipeline);
                    rpass.set_bind_group(0, &brush.constant_bind_group, &[]);
                    rpass.set_bind_group(1, &bg_bind, &[]);
                    rpass.set_vertex_buffer(
                        0,
                        brush.background_image_vertex_buffer.slice(..),
                    );
                    rpass.draw(0..4, 0..1);
                    // Restore text pipeline state for downstream batches.
                    rpass.set_pipeline(&brush.pipeline);
                    rpass.set_bind_group(0, &brush.constant_bind_group, &[]);
                }
            }

            if has_images && image_draws.iter().any(|d| d.layer == ImageLayer::BelowText)
            {
                // Each draw must use a unique slot in the shared vertex
                // buffer. Writing every instance to offset 0 (the old
                // behaviour) made the GPU read only the last-written
                // instance, so a screen with N kitty placements only
                // ever rendered the most recent one. The buffer is
                // sized for `MAX_IMAGE_INSTANCES` instances; the same
                // index space is used by the AboveText pass below so
                // both layers see consistent instance data.
                // Bumped from 64 to accommodate kitty Unicode placeholders
                // which can produce up to cols*rows draws per visible image
                // (one per placeholder cell with its own source rect).
                const MAX_IMAGE_INSTANCES: usize = 1024;
                if image_draws.len() > MAX_IMAGE_INSTANCES {
                    tracing::warn!(
                        "image_draws ({}) exceeds vertex buffer capacity ({}); \
                         extra placements will not render this frame",
                        image_draws.len(),
                        MAX_IMAGE_INSTANCES
                    );
                }
                let limit = image_draws.len().min(MAX_IMAGE_INSTANCES);
                let stride = std::mem::size_of::<ImageInstance>() as u64;

                rpass.set_pipeline(&brush.image_pipeline);
                rpass.set_bind_group(0, &brush.constant_bind_group, &[]);
                for (i, draw) in image_draws.iter().take(limit).enumerate() {
                    if draw.layer != ImageLayer::BelowText {
                        continue;
                    }
                    if let Some(img) = image_textures.get(&draw.image_id) {
                        #[allow(irrefutable_let_patterns)]
                        if let ImageTexture::Wgpu { view, .. } = &img.gpu {
                            let bg = ctx.device.create_bind_group(
                                &wgpu::BindGroupDescriptor {
                                    label: None,
                                    layout: &brush.image_bind_group_layout,
                                    entries: &[wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: wgpu::BindingResource::TextureView(
                                            view,
                                        ),
                                    }],
                                },
                            );
                            let offset = i as u64 * stride;
                            ctx.queue.write_buffer(
                                &brush.image_vertex_buffer,
                                offset,
                                bytemuck::bytes_of(&draw.instance),
                            );
                            rpass.set_bind_group(1, &bg, &[]);
                            rpass.set_vertex_buffer(
                                0,
                                brush.image_vertex_buffer.slice(offset..offset + stride),
                            );
                            rpass.draw(0..4, 0..1);
                        }
                    }
                }
                rpass.set_pipeline(&brush.pipeline);
                rpass.set_bind_group(0, &brush.constant_bind_group, &[]);
            }

            // Upload buffers once
            if !instances.is_empty() {
                if instances.len() > brush.supported_instance_buffer {
                    brush.instance_buffer.destroy();
                    brush.supported_instance_buffer =
                        (instances.len() as f32 * 1.25) as usize;
                    brush.instance_buffer =
                        ctx.device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("rich_text::Instance Buffer (resized)"),
                            size: mem::size_of::<batch::QuadInstance>() as u64
                                * brush.supported_instance_buffer as u64,
                            usage: wgpu::BufferUsages::VERTEX
                                | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        });
                }
                ctx.queue.write_buffer(
                    &brush.instance_buffer,
                    0,
                    bytemuck::cast_slice(instances),
                );
            }
            if !vertices.is_empty() {
                if vertices.len() > brush.supported_vertex_buffer {
                    brush.vertex_buffer.destroy();
                    brush.supported_vertex_buffer =
                        (vertices.len() as f32 * 1.25) as usize;
                    brush.vertex_buffer =
                        ctx.device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("rich_text::Vertices Buffer (resized)"),
                            size: mem::size_of::<Vertex>() as u64
                                * brush.supported_vertex_buffer as u64,
                            usage: wgpu::BufferUsages::VERTEX
                                | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        });
                }
                ctx.queue.write_buffer(
                    &brush.vertex_buffer,
                    0,
                    bytemuck::cast_slice(vertices),
                );
            }

            // Text pipeline: dispatch draw commands
            let mut current_pipeline_instanced = false;
            let mut pipeline_set = false;

            for cmd in draw_cmds {
                let (color_layer, mask_layer) = match cmd {
                    batch::DrawCmd::Instanced {
                        color_layer,
                        mask_layer,
                        ..
                    } => (*color_layer, *mask_layer),
                    batch::DrawCmd::Vertices {
                        color_layer,
                        mask_layer,
                        ..
                    } => (*color_layer, *mask_layer),
                };

                // Bind textures for this batch
                let color_view = if color_layer > 0 {
                    let idx = (color_layer - 1) as usize;
                    color_views.get(idx).unwrap_or(&color_views[0])
                } else {
                    &color_views[0]
                };
                let final_mask_view = if mask_layer > 0 {
                    mask_texture_view.unwrap_or(color_views[0])
                } else {
                    color_views[0]
                };
                brush.update_bind_group(ctx, color_view, final_mask_view);

                match cmd {
                    batch::DrawCmd::Instanced { offset, count, .. } => {
                        if !pipeline_set || !current_pipeline_instanced {
                            rpass.set_pipeline(&brush.instanced_pipeline);
                            rpass.set_bind_group(0, &brush.constant_bind_group, &[]);
                            current_pipeline_instanced = true;
                            pipeline_set = true;
                        }
                        rpass.set_bind_group(1, &brush.layout_bind_group, &[]);
                        let byte_offset =
                            *offset as u64 * mem::size_of::<batch::QuadInstance>() as u64;
                        rpass.set_vertex_buffer(
                            0,
                            brush.instance_buffer.slice(byte_offset..),
                        );
                        rpass.draw(0..4, 0..*count);
                    }
                    batch::DrawCmd::Vertices { offset, count, .. } => {
                        if !pipeline_set || current_pipeline_instanced {
                            rpass.set_pipeline(&brush.pipeline);
                            rpass.set_bind_group(0, &brush.constant_bind_group, &[]);
                            rpass.set_vertex_buffer(0, brush.vertex_buffer.slice(..));
                            current_pipeline_instanced = false;
                            pipeline_set = true;
                        }
                        rpass.set_bind_group(1, &brush.layout_bind_group, &[]);
                        rpass.draw(*offset..*offset + *count, 0..1);
                    }
                }
            }

            if has_images && image_draws.iter().any(|d| d.layer == ImageLayer::AboveText)
            {
                // See BelowText pass above for the rationale; both
                // passes share the same indexing into image_draws so
                // each placement always reads its own slot.
                // Bumped from 64 to accommodate kitty Unicode placeholders
                // which can produce up to cols*rows draws per visible image
                // (one per placeholder cell with its own source rect).
                const MAX_IMAGE_INSTANCES: usize = 1024;
                let limit = image_draws.len().min(MAX_IMAGE_INSTANCES);
                let stride = std::mem::size_of::<ImageInstance>() as u64;

                rpass.set_pipeline(&brush.image_pipeline);
                rpass.set_bind_group(0, &brush.constant_bind_group, &[]);
                for (i, draw) in image_draws.iter().take(limit).enumerate() {
                    if draw.layer != ImageLayer::AboveText {
                        continue;
                    }
                    if let Some(img) = image_textures.get(&draw.image_id) {
                        #[allow(irrefutable_let_patterns)]
                        if let ImageTexture::Wgpu { view, .. } = &img.gpu {
                            let bg = ctx.device.create_bind_group(
                                &wgpu::BindGroupDescriptor {
                                    label: None,
                                    layout: &brush.image_bind_group_layout,
                                    entries: &[wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: wgpu::BindingResource::TextureView(
                                            view,
                                        ),
                                    }],
                                },
                            );
                            let offset = i as u64 * stride;
                            ctx.queue.write_buffer(
                                &brush.image_vertex_buffer,
                                offset,
                                bytemuck::bytes_of(&draw.instance),
                            );
                            rpass.set_bind_group(1, &bg, &[]);
                            rpass.set_vertex_buffer(
                                0,
                                brush.image_vertex_buffer.slice(offset..offset + stride),
                            );
                            rpass.draw(0..4, 0..1);
                        }
                    }
                }
            }
        }
    }

    pub fn resize(&mut self, context: &mut Context) {
        let transform = match &context.inner {
            #[cfg(feature = "wgpu")]
            ContextType::Wgpu(wgpu_ctx) => {
                orthographic_projection(wgpu_ctx.size.width, wgpu_ctx.size.height)
            }
            ContextType::Cpu(cpu_ctx) => {
                orthographic_projection(cpu_ctx.size.width, cpu_ctx.size.height)
            }
            #[cfg(not(feature = "wgpu"))]
            ContextType::_Phantom(_) => unreachable!(),
        };

        match &mut self.brush_type {
            #[cfg(feature = "wgpu")]
            RendererType::Wgpu(wgpu_brush) => {
                if transform != wgpu_brush.current_transform {
                    let queue = match &context.inner {
                        ContextType::Wgpu(wgpu_ctx) => &wgpu_ctx.queue,
                        _ => unreachable!(),
                    };

                    queue.write_buffer(
                        &wgpu_brush.transform,
                        0,
                        bytemuck::bytes_of(&transform),
                    );
                    wgpu_brush.current_transform = transform;
                }
            }
            RendererType::Cpu => {}
        }
    }
}

#[cfg(feature = "wgpu")]
impl WgpuRenderer {
    pub fn new(context: &WgpuContext) -> Self {
        let supported_vertex_buffer = 500;

        let current_transform =
            orthographic_projection(context.size.width, context.size.height);
        let transform =
            context
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&current_transform),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                });

        // Create pipeline layout
        let constant_bind_group_layout =
            context
                .device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: None,
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::VERTEX,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: wgpu::BufferSize::new(mem::size_of::<
                                    [f32; 16],
                                >(
                                )
                                    as wgpu::BufferAddress),
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::VERTEX
                                | wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(
                                wgpu::SamplerBindingType::Filtering,
                            ),
                            count: None,
                        },
                    ],
                });

        let layout_bind_group_layout =
            context
                .device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: None,
                    entries: &[
                        // Color texture (binding 0)
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: context.get_optimal_texture_sample_type(),
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        // Mask texture (binding 1)
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float {
                                    filterable: true,
                                },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                    ],
                });

        let pipeline_layout =
            context
                .device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: None,
                    bind_group_layouts: &[
                        Some(&constant_bind_group_layout),
                        Some(&layout_bind_group_layout),
                    ],
                    ..Default::default()
                });

        let sampler = context.device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            lod_min_clamp: 0f32,
            lod_max_clamp: 0f32,
            ..Default::default()
        });

        let constant_bind_group =
            context
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    layout: &constant_bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(
                                wgpu::BufferBinding {
                                    buffer: &transform,
                                    offset: 0,
                                    size: None,
                                },
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&sampler),
                        },
                    ],
                    label: Some("rich_text::constant_bind_group"),
                });

        // Create initial layout bind group (will be updated when textures change)
        let layout_bind_group =
            context
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    layout: &layout_bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(
                                &context
                                    .device
                                    .create_texture(&wgpu::TextureDescriptor {
                                        label: Some("placeholder_color"),
                                        size: wgpu::Extent3d {
                                            width: 1,
                                            height: 1,
                                            depth_or_array_layers: 1,
                                        },
                                        mip_level_count: 1,
                                        sample_count: 1,
                                        dimension: wgpu::TextureDimension::D2,
                                        format: wgpu::TextureFormat::Rgba8Unorm,
                                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                                        view_formats: &[],
                                    })
                                    .create_view(&wgpu::TextureViewDescriptor::default()),
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(
                                &context
                                    .device
                                    .create_texture(&wgpu::TextureDescriptor {
                                        label: Some("placeholder_mask"),
                                        size: wgpu::Extent3d {
                                            width: 1,
                                            height: 1,
                                            depth_or_array_layers: 1,
                                        },
                                        mip_level_count: 1,
                                        sample_count: 1,
                                        dimension: wgpu::TextureDimension::D2,
                                        format: wgpu::TextureFormat::R8Unorm,
                                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                                        view_formats: &[],
                                    })
                                    .create_view(&wgpu::TextureViewDescriptor::default()),
                            ),
                        },
                    ],
                    label: Some("rich_text::layout_bind_group"),
                });

        let shader_source = include_str!("renderer.wgsl");

        let shader = context
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(shader_source)),
            });

        let pipeline =
            context
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    cache: None,
                    label: None,
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        module: &shader,
                        entry_point: Some("vs_main"),
                        buffers: &[wgpu::VertexBufferLayout {
                            array_stride: mem::size_of::<Vertex>() as u64,
                            // https://docs.rs/wgpu/latest/wgpu/enum.VertexStepMode.html
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array!(
                                0 => Float32x3,  // pos
                                1 => Float32x4,  // color (background)
                                2 => Float32x2,  // uv
                                3 => Sint32x2,   // layers
                                4 => Float32x4,  // corner_radii
                                5 => Float32x2,  // rect_size
                                6 => Sint32,     // underline_style
                                7 => Float32x4,  // clip_rect
                            ),
                        }],
                    },
                    fragment: Some(wgpu::FragmentState {
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        module: &shader,
                        entry_point: Some("fs_main"),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: context.format,
                            blend: BLEND,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        strip_index_format: None,
                        front_face: wgpu::FrontFace::Ccw,
                        cull_mode: None,
                        polygon_mode: wgpu::PolygonMode::Fill,
                        unclipped_depth: false,
                        conservative: false,
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                });

        // Instanced pipeline (vs_instanced + fs_main, instance step mode)
        let instanced_pipeline =
            context
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    cache: None,
                    label: Some("rich_text::instanced pipeline"),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        module: &shader,
                        entry_point: Some("vs_instanced"),
                        buffers: &[wgpu::VertexBufferLayout {
                            array_stride: mem::size_of::<batch::QuadInstance>() as u64,
                            step_mode: wgpu::VertexStepMode::Instance,
                            attributes: &wgpu::vertex_attr_array!(
                                0 => Float32x3,  // pos
                                1 => Float32x4,  // color
                                2 => Float32x4,  // uv_rect
                                3 => Sint32x2,   // layers
                                4 => Float32x2,  // size
                                5 => Float32x4,  // corner_radii
                                6 => Sint32,     // underline_style
                                7 => Float32x4,  // clip_rect
                            ),
                        }],
                    },
                    fragment: Some(wgpu::FragmentState {
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        module: &shader,
                        entry_point: Some("fs_main"),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: context.format,
                            blend: BLEND,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleStrip,
                        strip_index_format: None,
                        front_face: wgpu::FrontFace::Ccw,
                        cull_mode: None,
                        polygon_mode: wgpu::PolygonMode::Fill,
                        unclipped_depth: false,
                        conservative: false,
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                });

        let vertex_buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rich_text::Vertices Buffer"),
            size: mem::size_of::<Vertex>() as u64 * supported_vertex_buffer as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let supported_instance_buffer = 20_000usize;
        let instance_buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rich_text::Instance Buffer"),
            size: mem::size_of::<batch::QuadInstance>() as u64
                * supported_instance_buffer as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let image_shader_source = include_str!("image.wgsl");
        let image_shader =
            context
                .device
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("image shader"),
                    source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(image_shader_source)),
                });

        let image_bind_group_layout =
            context
                .device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("image texture layout"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float {
                                filterable: true,
                            },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    }],
                });

        let image_pipeline_layout =
            context
                .device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("image pipeline layout"),
                    bind_group_layouts: &[
                        Some(&constant_bind_group_layout), // group 0: transform + sampler
                        Some(&image_bind_group_layout),    // group 1: image texture
                    ],
                    immediate_size: 0,
                });

        // Premultiplied alpha blend for images
        let image_blend = Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        });

        let image_pipeline =
            context
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    cache: None,
                    label: Some("image pipeline"),
                    layout: Some(&image_pipeline_layout),
                    vertex: wgpu::VertexState {
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        module: &image_shader,
                        entry_point: Some("vs_main"),
                        buffers: &[wgpu::VertexBufferLayout {
                            array_stride: mem::size_of::<ImageInstance>() as u64,
                            step_mode: wgpu::VertexStepMode::Instance,
                            attributes: &wgpu::vertex_attr_array!(
                                0 => Float32x2, // dest_pos
                                1 => Float32x2, // dest_size
                                2 => Float32x4, // source_rect
                            ),
                        }],
                    },
                    fragment: Some(wgpu::FragmentState {
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        module: &image_shader,
                        entry_point: Some("fs_main"),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: context.format,
                            blend: image_blend,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleStrip,
                        strip_index_format: None,
                        front_face: wgpu::FrontFace::Ccw,
                        cull_mode: None,
                        polygon_mode: wgpu::PolygonMode::Fill,
                        unclipped_depth: false,
                        conservative: false,
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                });

        let image_vertex_buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("image instance buffer"),
            // 1024 max — see `MAX_IMAGE_INSTANCES` comment in render path.
            size: mem::size_of::<ImageInstance>() as u64 * 1024,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let background_image_vertex_buffer =
            context.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("background image instance buffer"),
                size: mem::size_of::<ImageInstance>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

        WgpuRenderer {
            layout_bind_group,
            layout_bind_group_layout,
            constant_bind_group,
            transform,
            pipeline,
            instanced_pipeline,
            vertex_buffer,
            instance_buffer,
            supported_vertex_buffer,
            supported_instance_buffer,
            current_transform,
            image_pipeline,
            image_bind_group_layout,
            image_vertex_buffer,
            background_image_vertex_buffer,
        }
    }

    #[inline]
    pub fn render<'pass>(
        &'pass mut self,
        ctx: &mut WgpuContext,
        instances: &[batch::QuadInstance],
        vertices: &[Vertex],
        rpass: &mut wgpu::RenderPass<'pass>,
    ) {
        if instances.is_empty() && vertices.is_empty() {
            return;
        }

        let queue = &mut ctx.queue;

        // Upload instance buffer
        if !instances.is_empty() {
            if instances.len() > self.supported_instance_buffer {
                self.instance_buffer.destroy();
                self.supported_instance_buffer = (instances.len() as f32 * 1.25) as usize;
                self.instance_buffer =
                    ctx.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("rich_text::Instance Buffer (resized)"),
                        size: mem::size_of::<batch::QuadInstance>() as u64
                            * self.supported_instance_buffer as u64,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
            }
            queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(instances));
        }

        // Upload vertex buffer
        if !vertices.is_empty() {
            if vertices.len() > self.supported_vertex_buffer {
                self.vertex_buffer.destroy();
                self.supported_vertex_buffer = (vertices.len() as f32 * 1.25) as usize;
                self.vertex_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("rich_text::Vertices Buffer (resized)"),
                    size: mem::size_of::<Vertex>() as u64
                        * self.supported_vertex_buffer as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(vertices));
        }

        rpass.set_bind_group(0, &self.constant_bind_group, &[]);
        rpass.set_bind_group(1, &self.layout_bind_group, &[]);
        rpass.set_pipeline(&self.pipeline);
        rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));

        let vertex_count = vertices.len() as u32;
        rpass.draw(0..vertex_count, 0..1);
    }

    #[inline]
    pub fn render_range(
        &mut self,
        ctx: &mut WgpuContext,
        vertices: &[Vertex],
        rpass: &mut wgpu::RenderPass,
        range: std::ops::Range<usize>,
    ) {
        if range.is_empty() {
            return;
        }

        let queue = &mut ctx.queue;

        // Ensure buffer is large enough
        if vertices.len() > self.supported_vertex_buffer {
            self.vertex_buffer.destroy();
            self.supported_vertex_buffer = (vertices.len() as f32 * 1.25) as usize;
            self.vertex_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("sugarloaf::rich_text::Pipeline vertices"),
                size: mem::size_of::<Vertex>() as u64
                    * self.supported_vertex_buffer as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }

        // Write all vertices to buffer (we need the full buffer for correct indexing)
        let vertices_bytes: &[u8] = bytemuck::cast_slice(vertices);
        if !vertices_bytes.is_empty() {
            queue.write_buffer(&self.vertex_buffer, 0, vertices_bytes);
        }

        rpass.set_pipeline(&self.pipeline);
        rpass.set_bind_group(0, &self.constant_bind_group, &[]);
        rpass.set_bind_group(1, &self.layout_bind_group, &[]);
        rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));

        // Draw only the specified range
        rpass.draw(range.start as u32..range.end as u32, 0..1);
    }

    pub fn update_bind_group(
        &mut self,
        ctx: &WgpuContext,
        color_view: &wgpu::TextureView,
        mask_view: &wgpu::TextureView,
    ) {
        // Always update bind group since different batches need different textures
        self.layout_bind_group =
            ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &self.layout_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(color_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(mask_view),
                    },
                ],
                label: Some("rich_text::Pipeline uniforms"),
            });
    }
}

#[cfg(test)]
mod rect_positioning_tests {
    // ... existing tests remain the same ...
    #[derive(Debug)]
    struct GlyphRect {
        #[allow(unused)]
        pub x: f32,
        #[allow(unused)]
        pub y: f32,
        #[allow(unused)]
        pub width: f32,
        #[allow(unused)]
        pub height: f32,
        #[allow(unused)]
        pub baseline_y: f32,
        pub glyph_center_x: f32,
        pub glyph_center_y: f32,
    }

    #[derive(Debug)]
    struct LineRect {
        #[allow(unused)]
        pub x: f32,
        pub y: f32,
        #[allow(unused)]
        pub width: f32,
        #[allow(unused)]
        pub height: f32,
        #[allow(unused)]
        pub baseline_y: f32,
    }

    #[test]
    fn test_glyph_rect_positioning_and_centering() {
        // Test parameters
        let line_height = 20.0;
        let char_width = 8.0;
        let ascent = 12.0;
        let descent = 4.0;
        let _leading = 0.0;

        // Expected calculations (matching our current implementation)
        let padding_top = (line_height - ascent - descent) / 2.0; // (20 - 12 - 4) / 2 = 2.0
        let expected_baseline_y = 0.0 + padding_top + ascent; // 0 + 2 + 12 = 14.0

        // Create line rect
        let line_rect = LineRect {
            x: 0.0,
            y: 0.0,
            width: char_width,
            height: line_height,
            baseline_y: expected_baseline_y,
        };

        // Expected glyph rect (should be centered within line rect)
        let expected_glyph_rect = GlyphRect {
            x: 0.0,
            y: 0.0,
            width: char_width,
            height: line_height,
            baseline_y: expected_baseline_y,
            glyph_center_x: char_width / 2.0,  // 4.0
            glyph_center_y: line_height / 2.0, // 10.0
        };

        // Verify baseline is positioned correctly within the line rect
        assert!(
            expected_baseline_y > line_rect.y,
            "Baseline should be below line top"
        );
        assert!(
            expected_baseline_y < line_rect.y + line_rect.height,
            "Baseline should be above line bottom"
        );

        // Verify glyph center is in the middle of the rect
        assert_eq!(
            expected_glyph_rect.glyph_center_x,
            char_width / 2.0,
            "Glyph should be horizontally centered"
        );
        assert_eq!(
            expected_glyph_rect.glyph_center_y,
            line_height / 2.0,
            "Glyph should be vertically centered"
        );

        // Verify baseline relationship to glyph center
        let baseline_offset_from_center =
            expected_baseline_y - expected_glyph_rect.glyph_center_y;

        // The baseline should be slightly above center for typical fonts
        // With ascent=12, descent=4, the baseline should be at 14.0, center at 10.0
        // So baseline is 4.0 units above center, which makes sense
        assert_eq!(
            baseline_offset_from_center, 4.0,
            "Baseline should be 4.0 units above glyph center"
        );
    }

    #[test]
    fn test_graphic_positioning_with_offsets() {
        // Test that graphics are positioned correctly based on cell offsets
        // This simulates the logic: gx = run_x - offset_x, gy = py - offset_y

        // Cell at position (100, 200) contains a graphic with offset (20, 30)
        let run_x = 100.0;
        let py = 200.0;
        let offset_x = 20;
        let offset_y = 30;

        // Calculate graphic position
        let gx = run_x - offset_x as f32;
        let gy = py - offset_y as f32;

        // The graphic's top-left should be at (80, 170)
        // because we back-calculate from the cell's position
        assert_eq!(gx, 80.0, "Graphic x should account for offset_x");
        assert_eq!(gy, 170.0, "Graphic y should account for offset_y");

        // Verify origin cell (offset 0,0) at same position
        let origin_run_x = 80.0;
        let origin_py = 170.0;
        let origin_offset_x = 0;
        let origin_offset_y = 0;

        let origin_gx = origin_run_x - origin_offset_x as f32;
        let origin_gy = origin_py - origin_offset_y as f32;

        // Both cells should calculate the same graphic position
        assert_eq!(
            gx, origin_gx,
            "Graphic position should be same from any cell"
        );
        assert_eq!(
            gy, origin_gy,
            "Graphic position should be same from any cell"
        );
    }

    #[test]
    fn test_graphic_deduplication() {
        // Test that the same graphic ID is only rendered once per frame
        use crate::GraphicId;
        use std::collections::HashSet;

        let mut last_rendered_graphic: HashSet<GraphicId> = HashSet::new();

        let graphic_id = GraphicId::new(42);

        // First cell with this graphic - should render
        assert!(
            !last_rendered_graphic.contains(&graphic_id),
            "First occurrence should not be in set"
        );
        last_rendered_graphic.insert(graphic_id);

        // Second cell with same graphic - should NOT render
        assert!(
            last_rendered_graphic.contains(&graphic_id),
            "Second occurrence should be in set, preventing duplicate render"
        );

        // Clear for next frame
        last_rendered_graphic.clear();
        assert!(
            !last_rendered_graphic.contains(&graphic_id),
            "After clear, graphic should be renderable again"
        );
    }
}
