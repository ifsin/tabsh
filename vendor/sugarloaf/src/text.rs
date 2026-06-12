use rustc_hash::FxHashMap;

use crate::font::FontLibrary;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TextInstance {
    pub pos: [f32; 2],
    pub glyph_pos: [u32; 2],
    pub glyph_size: [u32; 2],
    pub bearings: [i16; 2],
    pub color: [u8; 4],
    pub atlas: u8,
    pub page: u8,
    pub _pad: [u8; 2],
}

const _: () = assert!(std::mem::size_of::<TextInstance>() == 36);

#[derive(Clone, Copy, Debug)]
pub struct DrawOpts {
    pub font_size: f32,
    pub color: [u8; 4],
    pub bold: bool,
    pub italic: bool,
    pub font_id: Option<usize>,
}

impl Default for DrawOpts {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            color: [255, 255, 255, 255],
            bold: false,
            italic: false,
            font_id: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
struct ShapedGlyph {
    id: u16,
    x: f32,
    y: f32,
    advance: f32,
    cluster: u32,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct ShapedRun {
    font_id: u32,
    size_u16: u16,
    size_bucket: u16,
    synthetic_bold: bool,
    synthetic_italic: bool,
    wght_variation: Option<f32>,
    ascent_px: i16,
    glyphs: Vec<ShapedGlyph>,
}

#[inline]
fn shape_hash(font_id: u32, size_bucket: u16, style_flags: u8, text: &str) -> u64 {
    use core::hash::Hasher;
    use rustc_hash::FxHasher;
    let mut h = FxHasher::default();
    h.write_u32(font_id);
    h.write_u16(size_bucket);
    h.write_u8(style_flags);
    h.write(text.as_bytes());
    h.finish()
}

#[cfg(all(feature = "wgpu"))]
struct TextWgpuState {
    device: wgpu::Device,
    queue: wgpu::Queue,
    atlas_grayscale: crate::grid::webgpu::WgpuGlyphAtlas,
    atlas_color: crate::grid::webgpu::WgpuGlyphAtlas,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    atlas_bind_group: wgpu::BindGroup,
    #[allow(dead_code)]
    atlas_bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
}

struct TextCpuState {
    atlas_grayscale: crate::grid::cpu::CpuGridAtlas,
    atlas_color: crate::grid::cpu::CpuGridAtlas,
}

pub struct Text {
    instances: Vec<TextInstance>,
    scale_factor: f32,
    font_library: FontLibrary,
    font_resolve: FxHashMap<(char, u8), (u32, bool)>,
    synthesis_cache: FxHashMap<u32, (bool, bool)>,
    wght_variation_cache: FxHashMap<u32, Option<f32>>,
    ascent_cache: FxHashMap<(u32, u16), i16>,
    shape_cache: FxHashMap<u64, ShapedRun>,
    shape_ctx: swash::shape::ShapeContext,
    scale_ctx: swash::scale::ScaleContext,
    font_data_cache: FxHashMap<u32, (crate::font::SharedData, u32, swash::CacheKey)>,
    #[cfg(feature = "wgpu")]
    wgpu: Option<TextWgpuState>,
    cpu: Option<TextCpuState>,
}

impl Text {
    pub fn new(font_library: &FontLibrary) -> Self {
        Self {
            instances: Vec::new(),
            scale_factor: 1.0,
            font_library: font_library.clone(),
            font_resolve: FxHashMap::default(),
            synthesis_cache: FxHashMap::default(),
            wght_variation_cache: FxHashMap::default(),
            ascent_cache: FxHashMap::default(),
            shape_cache: FxHashMap::default(),
            shape_ctx: swash::shape::ShapeContext::new(),
            scale_ctx: swash::scale::ScaleContext::new(),
            font_data_cache: FxHashMap::default(),
            #[cfg(feature = "wgpu")]
            wgpu: None,
            cpu: None,
        }
    }

    pub fn init_cpu(&mut self) {
        if self.cpu.is_some() {
            return;
        }
        self.cpu = Some(TextCpuState {
            atlas_grayscale: crate::grid::cpu::CpuGridAtlas::new_grayscale(),
            atlas_color: crate::grid::cpu::CpuGridAtlas::new_color(),
        });
    }

    #[inline]
    pub fn set_scale_factor(&mut self, scale: f32) {
        self.scale_factor = scale.max(1.0);
    }

    #[inline]
    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    #[inline]
    pub fn clear(&mut self) {
        self.instances.clear();
    }

    #[inline]
    pub fn instances(&self) -> &[TextInstance] {
        &self.instances
    }

    pub fn draw(&mut self, x: f32, y: f32, text: &str, opts: &DrawOpts) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let Some(shaped) = self.shape_for(text, opts) else {
            return 0.0;
        };
        let width_px = shaped_width(&shaped);
        self.emit_instances(x, y, &shaped, opts);
        width_px / self.scale_factor
    }

    pub fn measure(&mut self, text: &str, opts: &DrawOpts) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        self.shape_for(text, opts)
            .map(|r| shaped_width(&r) / self.scale_factor)
            .unwrap_or(0.0)
    }

    fn shape_for(&mut self, text: &str, opts: &DrawOpts) -> Option<ShapedRun> {
        use crate::{Attributes, SpanStyle, Stretch, Style as FontStyle, Weight};

        let scaled = opts.font_size * self.scale_factor;
        let size_bucket = (scaled * 4.0).round().clamp(0.0, u16::MAX as f32) as u16;
        let size_u16 = scaled.round().clamp(1.0, u16::MAX as f32) as u16;
        let style_flags =
            (if opts.bold { 1u8 } else { 0 }) | (if opts.italic { 2u8 } else { 0 });

        let first_ch = text.chars().next()?;
        let (font_id, _is_emoji) = match self.font_resolve.entry((first_ch, style_flags))
        {
            std::collections::hash_map::Entry::Occupied(e) => *e.get(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let mut ss = SpanStyle::default();
                let weight = if opts.bold {
                    Weight::BOLD
                } else {
                    Weight::NORMAL
                };
                let fstyle = if opts.italic {
                    FontStyle::Italic
                } else {
                    FontStyle::Normal
                };
                ss.font_attrs = Attributes::new(Stretch::NORMAL, weight, fstyle);
                let resolved = {
                    let lib = self.font_library.inner.read();
                    lib.find_best_font_match(first_ch, &ss, None)
                        .unwrap_or((0, false))
                };
                let v = (resolved.0 as u32, resolved.1);
                e.insert(v);
                v
            }
        };
        let font_id = opts.font_id.map(|id| id as u32).unwrap_or(font_id);

        let hash = shape_hash(font_id, size_bucket, style_flags, text);
        if let Some(entry) = self.shape_cache.get(&hash) {
            return Some(entry.clone());
        }

        let (synthetic_bold, synthetic_italic) = match self.synthesis_cache.entry(font_id)
        {
            std::collections::hash_map::Entry::Occupied(e) => *e.get(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let lib = self.font_library.inner.read();
                let fd = lib.get(&(font_id as usize));
                *e.insert((fd.should_embolden, fd.should_italicize))
            }
        };

        let (glyphs, ascent_px, wght_variation) = {
            use swash::FontRef;
            use swash::Setting;

            let font_entry = self.font_data_cache.entry(font_id).or_insert_with(|| {
                let lib = self.font_library.inner.read();
                lib.get_data(&(font_id as usize)).expect(
                    "font id resolved but get_data returned None — cache invariant",
                )
            });
            let font_ref = FontRef {
                data: font_entry.0.as_ref(),
                offset: font_entry.1,
                key: font_entry.2,
            };

            let wght = match self.wght_variation_cache.entry(font_id) {
                std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                std::collections::hash_map::Entry::Vacant(e) => {
                    let lib = self.font_library.inner.read();
                    let v = lib.get(&(font_id as usize)).wght_variation;
                    *e.insert(v)
                }
            };
            const WGHT_TAG: swash::Tag = u32::from_be_bytes(*b"wght");
            let wght_var = wght.map(|v| Setting {
                tag: WGHT_TAG,
                value: v,
            });
            let var_slice: &[Setting<f32>] = match wght_var {
                Some(ref s) => std::slice::from_ref(s),
                None => &[],
            };

            let ascent_px = *self
                .ascent_cache
                .entry((font_id, size_bucket))
                .or_insert_with(|| {
                    let m = font_ref.metrics(&[]).scale(size_u16 as f32);
                    m.ascent.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16
                });

            let mut shaper = self
                .shape_ctx
                .builder(font_ref)
                .size(size_u16 as f32)
                .variations(var_slice.iter().copied())
                .build();
            shaper.add_str(text);
            let mut glyphs: Vec<ShapedGlyph> = Vec::new();
            shaper.shape_with(|cluster| {
                let byte_offset = cluster.source.start;
                for g in cluster.glyphs {
                    glyphs.push(ShapedGlyph {
                        id: g.id,
                        x: g.x,
                        y: g.y,
                        advance: g.advance,
                        cluster: byte_offset,
                    });
                }
            });
            (glyphs, ascent_px, wght)
        };

        let run = ShapedRun {
            font_id,
            size_u16,
            size_bucket,
            synthetic_bold,
            synthetic_italic,
            wght_variation,
            ascent_px,
            glyphs,
        };
        self.shape_cache.insert(hash, run.clone());
        Some(run)
    }

    fn emit_instances(&mut self, x: f32, y: f32, run: &ShapedRun, opts: &DrawOpts) {
        let scale = self.scale_factor;
        let mut pen_x = x * scale;
        let py = y * scale;
        let color = opts.color;

        for glyph in &run.glyphs {
            let Some((slot, is_color)) = self.rasterize_slot(run, glyph.id) else {
                continue;
            };
            if slot.w == 0 || slot.h == 0 {
                pen_x += glyph.advance;
                continue;
            }

            let atlas_tag = if is_color { 1u8 } else { 0u8 };
            let instance_color = if is_color {
                [255u8, 255, 255, 255]
            } else {
                color
            };

            self.instances.push(TextInstance {
                pos: [pen_x + glyph.x, py + glyph.y.max(0.0)],
                glyph_pos: [slot.x as u32, slot.y as u32],
                glyph_size: [slot.w as u32, slot.h as u32],
                bearings: [slot.bearing_x, slot.bearing_y],
                color: instance_color,
                atlas: atlas_tag,
                page: slot.page,
                _pad: [0; 2],
            });

            pen_x += glyph.advance;
        }
    }

    #[allow(clippy::type_complexity)]
    fn rasterize_slot(
        &mut self,
        run: &ShapedRun,
        glyph_id: u16,
    ) -> Option<(crate::grid::atlas::AtlasSlot, bool)> {
        let key = crate::grid::GlyphKey {
            font_id: run.font_id,
            glyph_id: glyph_id as u32,
            size_bucket: run.size_bucket,
        };

        if self.cpu.is_some() {
            return self.rasterize_slot_cpu(run, glyph_id, key);
        }

        #[cfg(feature = "wgpu")]
        {
            let state = self.wgpu.as_mut()?;

            if let Some(s) = state.atlas_grayscale.lookup(key) {
                return Some((s, false));
            }
            if let Some(s) = state.atlas_color.lookup(key) {
                return Some((s, true));
            }

            let font_entry = self.font_data_cache.get(&run.font_id)?.clone();
            let raw = rasterize_swash_glyph(
                &mut self.scale_ctx,
                &font_entry,
                glyph_id,
                run.size_u16 as f32,
                run.synthetic_bold,
                run.synthetic_italic,
                self.font_library.inner.read().hinting,
                run.wght_variation,
            )?;
            let is_color = raw.is_color;

            let raster = crate::grid::RasterizedGlyph {
                width: raw.width.min(u16::MAX as u32) as u16,
                height: raw.height.min(u16::MAX as u32) as u16,
                bearing_x: raw.left.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                bearing_y: {
                    let top_i16 = raw.top.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                    run.ascent_px.saturating_sub(top_i16)
                },
                bytes: &raw.bytes,
            };
            let slot = if is_color {
                state.atlas_color.insert(key, raster)?
            } else {
                state.atlas_grayscale.insert(key, raster)?
            };
            Some((slot, is_color))
        }
        #[cfg(not(feature = "wgpu"))]
        {
            let _ = (run, glyph_id);
            None
        }
    }

    #[allow(clippy::type_complexity)]
    fn rasterize_slot_cpu(
        &mut self,
        run: &ShapedRun,
        glyph_id: u16,
        key: crate::grid::GlyphKey,
    ) -> Option<(crate::grid::atlas::AtlasSlot, bool)> {
        {
            let state = self.cpu.as_ref()?;
            if let Some(s) = state.atlas_grayscale.lookup(key) {
                return Some((s, false));
            }
            if let Some(s) = state.atlas_color.lookup(key) {
                return Some((s, true));
            }
        }

        let (raw_w, raw_h, raw_left, raw_top, raw_is_color, raw_bytes) = {
            let font_entry = self.font_data_cache.get(&run.font_id)?.clone();
            let raw = rasterize_swash_glyph(
                &mut self.scale_ctx,
                &font_entry,
                glyph_id,
                run.size_u16 as f32,
                run.synthetic_bold,
                run.synthetic_italic,
                self.font_library.inner.read().hinting,
                run.wght_variation,
            )?;
            (
                raw.width,
                raw.height,
                raw.left,
                raw.top,
                raw.is_color,
                raw.bytes,
            )
        };

        let raster = crate::grid::RasterizedGlyph {
            width: raw_w.min(u16::MAX as u32) as u16,
            height: raw_h.min(u16::MAX as u32) as u16,
            bearing_x: raw_left.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            bearing_y: {
                let top_i16 = raw_top.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                run.ascent_px.saturating_sub(top_i16)
            },
            bytes: &raw_bytes,
        };

        let state = self.cpu.as_mut()?;
        let slot = if raw_is_color {
            state.atlas_color.insert(key, raster).or_else(|| {
                if state.atlas_color.grow() {
                    state.atlas_color.insert(key, raster)
                } else {
                    None
                }
            })?
        } else {
            state.atlas_grayscale.insert(key, raster).or_else(|| {
                if state.atlas_grayscale.grow() {
                    state.atlas_grayscale.insert(key, raster)
                } else {
                    None
                }
            })?
        };
        Some((slot, raw_is_color))
    }

    pub fn render_cpu(&self, buf: &mut [u32], buf_w: u32, buf_h: u32) {
        if self.instances.is_empty() {
            return;
        }
        let Some(state) = self.cpu.as_ref() else {
            return;
        };
        let buf_w_i = buf_w as i32;
        let buf_h_i = buf_h as i32;
        let mask = state.atlas_grayscale.pixels();
        let mask_side = state.atlas_grayscale.side() as usize;
        let color_atlas = state.atlas_color.pixels();
        let color_side = state.atlas_color.side() as usize;

        for inst in &self.instances {
            let gw = inst.glyph_size[0] as i32;
            let gh = inst.glyph_size[1] as i32;
            if gw <= 0 || gh <= 0 {
                continue;
            }
            let glyph_x = (inst.pos[0] + inst.bearings[0] as f32) as i32;
            let glyph_y = (inst.pos[1] + inst.bearings[1] as f32) as i32;
            let ax = inst.glyph_pos[0] as usize;
            let ay = inst.glyph_pos[1] as usize;

            if inst.atlas == 1 {
                blit_text_color(
                    buf,
                    buf_w_i,
                    buf_h_i,
                    glyph_x,
                    glyph_y,
                    gw,
                    gh,
                    color_atlas,
                    color_side,
                    ax,
                    ay,
                );
            } else {
                blit_text_mask(
                    buf, buf_w_i, buf_h_i, glyph_x, glyph_y, gw, gh, mask, mask_side, ax,
                    ay, inst.color,
                );
            }
        }
    }

    #[cfg(feature = "wgpu")]
    pub fn init_wgpu(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) {
        if self.wgpu.is_some() {
            return;
        }
        let atlas_grayscale =
            crate::grid::webgpu::WgpuGlyphAtlas::new_grayscale(device, queue.clone());
        let atlas_color =
            crate::grid::webgpu::WgpuGlyphAtlas::new_color(device, queue.clone());

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sugarloaf.text.uniforms"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("sugarloaf.text.uniform_bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(16),
                    },
                    count: None,
                }],
            });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sugarloaf.text.uniform_bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let atlas_bgl = create_text_atlas_bgl_wgpu(device);
        let atlas_bind_group = create_text_atlas_bg_wgpu(
            device,
            &atlas_bgl,
            atlas_grayscale.view(),
            atlas_color.view(),
        );

        let pipeline = build_text_pipeline_wgpu(
            device,
            format,
            &[Some(&uniform_bgl), Some(&atlas_bgl)],
        );
        let instance_capacity: usize = 256;
        let instance_buffer = alloc_instance_buffer_wgpu(device, instance_capacity);

        self.wgpu = Some(TextWgpuState {
            device: device.to_owned(),
            queue: queue.to_owned(),
            atlas_grayscale,
            atlas_color,
            uniform_buffer,
            uniform_bind_group,
            atlas_bind_group,
            atlas_bind_group_layout: atlas_bgl,
            pipeline,
            instance_buffer,
            instance_capacity,
        });
    }

    #[cfg(feature = "wgpu")]
    pub fn render_wgpu<'pass>(
        &'pass mut self,
        render_pass: &mut wgpu::RenderPass<'pass>,
        viewport: [f32; 2],
    ) {
        let instance_count = self.instances.len();
        if instance_count == 0 {
            return;
        }
        let Some(state) = self.wgpu.as_mut() else {
            return;
        };

        let uniforms: [f32; 4] = [viewport[0], viewport[1], 0.0, 0.0];
        state.queue.write_buffer(
            &state.uniform_buffer,
            0,
            bytemuck::cast_slice(&uniforms),
        );

        if instance_count > state.instance_capacity {
            let new_cap = instance_count.next_power_of_two().max(256);
            state.instance_buffer = alloc_instance_buffer_wgpu(&state.device, new_cap);
            state.instance_capacity = new_cap;
        }

        state.queue.write_buffer(
            &state.instance_buffer,
            0,
            bytemuck_instances(&self.instances),
        );

        render_pass.set_pipeline(&state.pipeline);
        render_pass.set_bind_group(0, &state.uniform_bind_group, &[]);
        render_pass.set_bind_group(1, &state.atlas_bind_group, &[]);
        render_pass.set_vertex_buffer(0, state.instance_buffer.slice(..));
        render_pass.draw(0..4, 0..instance_count as u32);
    }
}

#[inline]
fn shaped_width(run: &ShapedRun) -> f32 {
    run.glyphs.iter().map(|g| g.advance).sum()
}

#[cfg(feature = "wgpu")]
fn bytemuck_instances(insts: &[TextInstance]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            insts.as_ptr() as *const u8,
            std::mem::size_of_val(insts),
        )
    }
}

struct SwashRawGlyph {
    width: u32,
    height: u32,
    left: i32,
    top: i32,
    is_color: bool,
    bytes: Vec<u8>,
}

#[allow(clippy::too_many_arguments)]
fn rasterize_swash_glyph(
    scale_ctx: &mut swash::scale::ScaleContext,
    font_entry: &(crate::font::SharedData, u32, swash::CacheKey),
    glyph_id: u16,
    size_px: f32,
    synthetic_bold: bool,
    synthetic_italic: bool,
    hint: bool,
    wght_variation: Option<f32>,
) -> Option<SwashRawGlyph> {
    use swash::scale::{
        image::{Content, Image as GlyphImage},
        Render, Source, StrikeWith,
    };
    use swash::zeno::{Angle, Format, Transform};
    use swash::{FontRef, Setting};

    let font_ref = FontRef {
        data: font_entry.0.as_ref(),
        offset: font_entry.1,
        key: font_entry.2,
    };

    const WGHT_TAG: swash::Tag = u32::from_be_bytes(*b"wght");
    let wght_var = wght_variation.map(|v| Setting {
        tag: WGHT_TAG,
        value: v,
    });
    let var_slice: &[Setting<f32>] = match wght_var {
        Some(ref s) => std::slice::from_ref(s),
        None => &[],
    };

    let mut scaler = scale_ctx
        .builder(font_ref)
        .hint(hint)
        .size(size_px)
        .variations(var_slice.iter().copied())
        .build();

    let mut image = GlyphImage::new();
    let sources: &[Source] = &[
        Source::ColorOutline(0),
        Source::ColorBitmap(StrikeWith::BestFit),
        Source::Outline,
    ];
    let embolden_amount = if synthetic_bold {
        (size_px / 14.0).max(1.0)
    } else {
        0.0
    };
    let rendered = Render::new(sources)
        .format(Format::Alpha)
        .embolden(embolden_amount)
        .transform(if synthetic_italic {
            Some(Transform::skew(
                Angle::from_degrees(14.0),
                Angle::from_degrees(0.0),
            ))
        } else {
            None
        })
        .render_into(&mut scaler, glyph_id, &mut image);

    if !rendered {
        return None;
    }

    let is_color = image.content == Content::Color;
    Some(SwashRawGlyph {
        width: image.placement.width,
        height: image.placement.height,
        left: image.placement.left,
        top: image.placement.top,
        is_color,
        bytes: image.data,
    })
}

#[cfg(feature = "wgpu")]
fn create_text_atlas_bgl_wgpu(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sugarloaf.text.atlas_bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ],
    })
}

#[cfg(feature = "wgpu")]
fn create_text_atlas_bg_wgpu(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    grayscale: &wgpu::TextureView,
    color: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sugarloaf.text.atlas_bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(grayscale),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(color),
            },
        ],
    })
}

#[cfg(feature = "wgpu")]
fn alloc_instance_buffer_wgpu(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    let size = (capacity.max(1) * std::mem::size_of::<TextInstance>()) as u64;
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sugarloaf.text.instances"),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

#[cfg(feature = "wgpu")]
fn build_text_pipeline_wgpu(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    bind_group_layouts: &[Option<&wgpu::BindGroupLayout>],
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sugarloaf.text.wgsl"),
        source: wgpu::ShaderSource::Wgsl(include_str!("text_shader.wgsl").into()),
    });

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sugarloaf.text.pipeline_layout"),
        bind_group_layouts,
        immediate_size: 0,
    });

    let stride = std::mem::size_of::<TextInstance>() as u64;
    let attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint32x2,
            offset: 8,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint32x2,
            offset: 16,
            shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Sint16x2,
            offset: 24,
            shader_location: 3,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Unorm8x4,
            offset: 28,
            shader_location: 4,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint8x4,
            offset: 32,
            shader_location: 5,
        },
    ];
    let vbuf = wgpu::VertexBufferLayout {
        array_stride: stride,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &attrs,
    };

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sugarloaf.text.pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("text_vertex"),
            buffers: &[vbuf],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("text_fragment"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(premul_blend_wgpu()),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(feature = "wgpu")]
fn premul_blend_wgpu() -> wgpu::BlendState {
    wgpu::BlendState {
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
    }
}

#[inline]
fn pack_opaque(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

#[inline]
fn blend_premul_over(src: [u8; 4], dst: u32) -> u32 {
    let sa = src[3] as u32;
    if sa == 0 {
        return dst;
    }
    if sa == 255 {
        return pack_opaque(src[0], src[1], src[2]);
    }
    let inv = 255 - sa;
    let dr = (dst >> 16) & 0xff;
    let dg = (dst >> 8) & 0xff;
    let db = dst & 0xff;
    let or = src[0] as u32 + (dr * inv + 127) / 255;
    let og = src[1] as u32 + (dg * inv + 127) / 255;
    let ob = src[2] as u32 + (db * inv + 127) / 255;
    pack_opaque(or.min(255) as u8, og.min(255) as u8, ob.min(255) as u8)
}

#[allow(clippy::too_many_arguments)]
fn blit_text_mask(
    buf: &mut [u32],
    buf_w: i32,
    buf_h: i32,
    glyph_x: i32,
    glyph_y: i32,
    gw: i32,
    gh: i32,
    atlas: &[u8],
    atlas_side: usize,
    ax: usize,
    ay: usize,
    color: [u8; 4],
) {
    if color[3] == 0 {
        return;
    }
    let stride = buf_w as usize;
    let x_start = glyph_x.max(0);
    let y_start = glyph_y.max(0);
    let x_end = (glyph_x + gw).min(buf_w);
    let y_end = (glyph_y + gh).min(buf_h);
    if x_end <= x_start || y_end <= y_start {
        return;
    }
    let r = color[0] as u32;
    let g = color[1] as u32;
    let b = color[2] as u32;
    let ca = color[3] as u32;

    for dst_y in y_start..y_end {
        let src_y = (dst_y - glyph_y) as usize + ay;
        if src_y >= atlas_side {
            continue;
        }
        let atlas_row = src_y * atlas_side;
        let buf_row = (dst_y as usize) * stride;
        for dst_x in x_start..x_end {
            let src_x = (dst_x - glyph_x) as usize + ax;
            if src_x >= atlas_side {
                continue;
            }
            let m = atlas[atlas_row + src_x] as u32;
            if m == 0 {
                continue;
            }
            let a = (m * ca + 127) / 255;
            if a == 0 {
                continue;
            }
            let pr = (r * a + 127) / 255;
            let pg = (g * a + 127) / 255;
            let pb = (b * a + 127) / 255;
            let src = [pr as u8, pg as u8, pb as u8, a as u8];
            let idx = buf_row + (dst_x as usize);
            buf[idx] = blend_premul_over(src, buf[idx]);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn blit_text_color(
    buf: &mut [u32],
    buf_w: i32,
    buf_h: i32,
    glyph_x: i32,
    glyph_y: i32,
    gw: i32,
    gh: i32,
    atlas: &[u8],
    atlas_side: usize,
    ax: usize,
    ay: usize,
) {
    let stride = buf_w as usize;
    let x_start = glyph_x.max(0);
    let y_start = glyph_y.max(0);
    let x_end = (glyph_x + gw).min(buf_w);
    let y_end = (glyph_y + gh).min(buf_h);
    if x_end <= x_start || y_end <= y_start {
        return;
    }
    for dst_y in y_start..y_end {
        let src_y = (dst_y - glyph_y) as usize + ay;
        if src_y >= atlas_side {
            continue;
        }
        let atlas_row = src_y * atlas_side * 4;
        let buf_row = (dst_y as usize) * stride;
        for dst_x in x_start..x_end {
            let src_x = (dst_x - glyph_x) as usize + ax;
            if src_x >= atlas_side {
                continue;
            }
            let off = atlas_row + src_x * 4;
            let r = atlas[off];
            let g = atlas[off + 1];
            let b = atlas[off + 2];
            let a = atlas[off + 3];
            if a == 0 {
                continue;
            }
            let src = [r, g, b, a];
            let idx = buf_row + (dst_x as usize);
            buf[idx] = blend_premul_over(src, buf[idx]);
        }
    }
}
