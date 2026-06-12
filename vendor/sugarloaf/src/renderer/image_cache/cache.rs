#![allow(dead_code)]

use crate::context::{Context, ContextType};
use tracing::debug;

use super::atlas::*;
use super::ContentType;
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum AtlasKind {
    #[default]
    Mask,
    Color,
}

#[derive(Default)]
pub struct Entry {
    allocated: bool,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    atlas_kind: AtlasKind,
    color_atlas_index: usize,
}

pub struct Atlas {
    alloc: AtlasAllocator,
    buffer: Vec<u8>,
    fresh: bool,
    dirty: bool,
    channels: usize,
}

impl Atlas {
    fn new(kind: AtlasKind, size: u16) -> Self {
        let channels = match kind {
            AtlasKind::Mask => 1,
            AtlasKind::Color => 4,
        };

        Self {
            alloc: AtlasAllocator::new(size, size),
            buffer: vec![0; size as usize * size as usize * channels],
            fresh: true,
            dirty: false,
            channels,
        }
    }
}

pub const SIZE: u16 = 4096;

pub struct ImageCache {
    pub entries: Vec<Entry>,
    mask_atlas: Atlas,
    color_atlases: Vec<ColorAtlasWithTexture>,
    max_texture_size: u16,
    device_queue: DeviceQueue,
}

struct ColorAtlasWithTexture {
    atlas: Atlas,
    texture: ColorAtlasTexture,
}

enum ColorAtlasTexture {
    #[cfg(feature = "wgpu")]
    Wgpu(wgpu::Texture, wgpu::TextureView),
    Cpu,
}

enum DeviceQueue {
    #[cfg(feature = "wgpu")]
    Wgpu {
        device: std::sync::Arc<wgpu::Device>,
        queue: std::sync::Arc<wgpu::Queue>,
        mask_texture: wgpu::Texture,
        mask_texture_view: wgpu::TextureView,
    },
    Cpu,
}

#[inline]
pub fn buffer_size(width: u32, height: u32) -> Option<usize> {
    (width as usize)
        .checked_add(height as usize)?
        .checked_add(4)
}

impl ImageCache {
    pub fn new(context: &Context) -> Self {
        match &context.inner {
            #[cfg(not(feature = "wgpu"))]
            ContextType::_Phantom(_) => unreachable!(),
            #[cfg(feature = "wgpu")]
            ContextType::Wgpu(wgpu_context) => {
                let max_size = wgpu_context.max_texture_dimension_2d();
                let max_texture_size = std::cmp::min(4096, max_size) as u16;

                let device = std::sync::Arc::new(wgpu_context.device.clone());
                let queue = std::sync::Arc::new(wgpu_context.queue.clone());

                let mask_texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("rich_text mask atlas"),
                    size: wgpu::Extent3d {
                        width: SIZE as u32,
                        height: SIZE as u32,
                        depth_or_array_layers: 1,
                    },
                    view_formats: &[],
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Unorm,
                    usage: wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    mip_level_count: 1,
                    sample_count: 1,
                });
                let mask_texture_view =
                    mask_texture.create_view(&wgpu::TextureViewDescriptor::default());

                let color_texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("rich_text color atlas 0"),
                    size: wgpu::Extent3d {
                        width: SIZE as u32,
                        height: SIZE as u32,
                        depth_or_array_layers: 1,
                    },
                    view_formats: &[],
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    mip_level_count: 1,
                    sample_count: 1,
                });
                let color_texture_view =
                    color_texture.create_view(&wgpu::TextureViewDescriptor::default());

                let color_atlases = vec![ColorAtlasWithTexture {
                    atlas: Atlas::new(AtlasKind::Color, max_texture_size),
                    texture: ColorAtlasTexture::Wgpu(color_texture, color_texture_view),
                }];

                Self {
                    entries: Vec::new(),
                    mask_atlas: Atlas::new(AtlasKind::Mask, max_texture_size),
                    color_atlases,
                    max_texture_size,
                    device_queue: DeviceQueue::Wgpu {
                        device,
                        queue,
                        mask_texture,
                        mask_texture_view,
                    },
                }
            }
            ContextType::Cpu(_) => {
                let max_texture_size: u16 = 2048;
                let color_atlases = vec![ColorAtlasWithTexture {
                    atlas: Atlas::new(AtlasKind::Color, max_texture_size),
                    texture: ColorAtlasTexture::Cpu,
                }];
                Self {
                    entries: Vec::new(),
                    mask_atlas: Atlas::new(AtlasKind::Mask, max_texture_size),
                    color_atlases,
                    max_texture_size,
                    device_queue: DeviceQueue::Cpu,
                }
            }
        }
    }

    #[inline]
    pub fn cpu_max_texture_size(&self) -> u16 {
        self.max_texture_size
    }
    #[inline]
    pub fn cpu_mask_atlas_buffer(&self) -> &[u8] {
        &self.mask_atlas.buffer
    }

    pub fn allocate(&mut self, request: AddImage) -> Option<ImageId> {
        let width = request.width;
        let height = request.height;

        if width == 0 || height == 0 {
            return None;
        }

        buffer_size(width as u32, height as u32)?;

        if !(width <= self.max_texture_size && height <= self.max_texture_size) {
            return None;
        }

        let entry_index = self.entries.len();
        let atlas_kind = match request.content_type {
            ContentType::Mask => AtlasKind::Mask,
            ContentType::Color => AtlasKind::Color,
        };

        if atlas_kind == AtlasKind::Mask {
            let atlas_data = self.mask_atlas.alloc.allocate(width, height);
            if atlas_data.is_none() {
                debug!("Mask atlas full for {}x{}", width, height);
                return None;
            }

            let (x, y) = atlas_data?;
            self.entries.push(Entry {
                allocated: true,
                x,
                y,
                width,
                height,
                atlas_kind,
                color_atlas_index: 0,
            });

            if let Some(data) = request.data() {
                fill(
                    FillParams {
                        x,
                        y,
                        width,
                        _height: height,
                        target_width: self.max_texture_size,
                        channels: self.mask_atlas.channels,
                    },
                    data,
                    &mut self.mask_atlas.buffer,
                );
                self.mask_atlas.dirty = true;
            }

            return ImageId::new(entry_index as u32, request.has_alpha);
        }

        for (atlas_index, atlas_with_texture) in self.color_atlases.iter_mut().enumerate()
        {
            if let Some((x, y)) = atlas_with_texture.atlas.alloc.allocate(width, height) {
                self.entries.push(Entry {
                    allocated: true,
                    x,
                    y,
                    width,
                    height,
                    atlas_kind,
                    color_atlas_index: atlas_index,
                });

                if let Some(data) = request.data() {
                    fill(
                        FillParams {
                            x,
                            y,
                            width,
                            _height: height,
                            target_width: self.max_texture_size,
                            channels: atlas_with_texture.atlas.channels,
                        },
                        data,
                        &mut atlas_with_texture.atlas.buffer,
                    );
                    atlas_with_texture.atlas.dirty = true;
                }

                debug!(
                    "Allocated {}x{} in existing color atlas {}",
                    width, height, atlas_index
                );
                return ImageId::new(entry_index as u32, request.has_alpha);
            }
        }

        debug!(
            "All color atlases full, creating new atlas for {}x{}",
            width, height
        );
        let new_atlas_index = self.color_atlases.len();

        if !self.create_new_color_atlas() {
            debug!("Failed to create new color atlas");
            return None;
        }

        let atlas_with_texture = self.color_atlases.last_mut()?;
        let (x, y) = atlas_with_texture.atlas.alloc.allocate(width, height)?;

        self.entries.push(Entry {
            allocated: true,
            x,
            y,
            width,
            height,
            atlas_kind,
            color_atlas_index: new_atlas_index,
        });

        if let Some(data) = request.data() {
            fill(
                FillParams {
                    x,
                    y,
                    width,
                    _height: height,
                    target_width: self.max_texture_size,
                    channels: atlas_with_texture.atlas.channels,
                },
                data,
                &mut atlas_with_texture.atlas.buffer,
            );
            atlas_with_texture.atlas.dirty = true;
        }

        debug!(
            "Allocated {}x{} in new color atlas {}",
            width, height, new_atlas_index
        );
        ImageId::new(entry_index as u32, request.has_alpha)
    }

    fn create_new_color_atlas(&mut self) -> bool {
        let atlas_index = self.color_atlases.len();
        debug!("Creating color atlas {}", atlas_index);

        match &self.device_queue {
            #[cfg(feature = "wgpu")]
            DeviceQueue::Wgpu {
                device, queue: _, ..
            } => {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(&format!("rich_text color atlas {}", atlas_index)),
                    size: wgpu::Extent3d {
                        width: self.max_texture_size as u32,
                        height: self.max_texture_size as u32,
                        depth_or_array_layers: 1,
                    },
                    view_formats: &[],
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    mip_level_count: 1,
                    sample_count: 1,
                });
                let texture_view =
                    texture.create_view(&wgpu::TextureViewDescriptor::default());

                self.color_atlases.push(ColorAtlasWithTexture {
                    atlas: Atlas::new(AtlasKind::Color, self.max_texture_size),
                    texture: ColorAtlasTexture::Wgpu(texture, texture_view),
                });
                true
            }
            DeviceQueue::Cpu => {
                self.color_atlases.push(ColorAtlasWithTexture {
                    atlas: Atlas::new(AtlasKind::Color, self.max_texture_size),
                    texture: ColorAtlasTexture::Cpu,
                });
                true
            }
        }
    }

    #[allow(unused)]
    pub fn deallocate(&mut self, image: ImageId) -> Option<()> {
        let entry = self.entries.get_mut(image.index())?;
        if !entry.allocated {
            return None;
        }

        match entry.atlas_kind {
            AtlasKind::Mask => {
                self.mask_atlas
                    .alloc
                    .deallocate(entry.x, entry.y, entry.width);
            }
            AtlasKind::Color => {
                if let Some(atlas_with_texture) =
                    self.color_atlases.get_mut(entry.color_atlas_index)
                {
                    atlas_with_texture.atlas.alloc.deallocate(
                        entry.x,
                        entry.y,
                        entry.width,
                    );
                }
            }
        }

        entry.allocated = false;
        Some(())
    }

    pub fn get(&self, handle: &ImageId) -> Option<ImageLocation> {
        if handle.is_empty() {
            return None;
        }

        let entry = self.entries.get(handle.index())?;
        if !entry.allocated {
            return None;
        }

        let s = 1. / self.max_texture_size as f32;
        Some(ImageLocation {
            min: (entry.x as f32 * s, entry.y as f32 * s),
            max: (
                (entry.x + entry.width) as f32 * s,
                (entry.y + entry.height) as f32 * s,
            ),
        })
    }

    pub fn clear_atlas(&mut self) {
        self.entries.clear();

        self.mask_atlas = Atlas::new(AtlasKind::Mask, self.max_texture_size);

        if let Some(first) = self.color_atlases.first_mut() {
            first.atlas = Atlas::new(AtlasKind::Color, self.max_texture_size);
        }
        self.color_atlases.truncate(1);

        tracing::info!(
            "Atlases cleared, {} color atlas(es) remaining",
            self.color_atlases.len()
        );
    }

    pub fn is_valid(&self, image: ImageId) -> bool {
        if image.is_empty() {
            return true;
        }

        if let Some(entry) = self.entries.get(image.index()) {
            entry.allocated
        } else {
            false
        }
    }

    #[inline]
    pub fn process_atlases(&mut self, context: &mut Context) {
        match &context.inner {
            #[cfg(not(feature = "wgpu"))]
            ContextType::_Phantom(_) => unreachable!(),
            #[cfg(feature = "wgpu")]
            ContextType::Wgpu(wgpu_context) => {
                if self.mask_atlas.dirty {
                    if let DeviceQueue::Wgpu {
                        mask_texture,
                        queue,
                        ..
                    } = &self.device_queue
                    {
                        let texture_size = wgpu::Extent3d {
                            width: self.max_texture_size as u32,
                            height: self.max_texture_size as u32,
                            depth_or_array_layers: 1,
                        };

                        queue.write_texture(
                            wgpu::TexelCopyTextureInfo {
                                texture: mask_texture,
                                mip_level: 0,
                                origin: wgpu::Origin3d::ZERO,
                                aspect: wgpu::TextureAspect::All,
                            },
                            &self.mask_atlas.buffer,
                            wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(
                                    self.max_texture_size as u32
                                        * self.mask_atlas.channels as u32,
                                ),
                                rows_per_image: Some(self.max_texture_size as u32),
                            },
                            texture_size,
                        );

                        self.mask_atlas.fresh = false;
                        self.mask_atlas.dirty = false;
                    }
                }

                for atlas_with_texture in &mut self.color_atlases {
                    if atlas_with_texture.atlas.dirty {
                        if let ColorAtlasTexture::Wgpu(texture, _) =
                            &atlas_with_texture.texture
                        {
                            let texture_size = wgpu::Extent3d {
                                width: self.max_texture_size as u32,
                                height: self.max_texture_size as u32,
                                depth_or_array_layers: 1,
                            };

                            wgpu_context.queue.write_texture(
                                wgpu::TexelCopyTextureInfo {
                                    texture,
                                    mip_level: 0,
                                    origin: wgpu::Origin3d::ZERO,
                                    aspect: wgpu::TextureAspect::All,
                                },
                                &atlas_with_texture.atlas.buffer,
                                wgpu::TexelCopyBufferLayout {
                                    offset: 0,
                                    bytes_per_row: Some(
                                        self.max_texture_size as u32
                                            * atlas_with_texture.atlas.channels as u32,
                                    ),
                                    rows_per_image: Some(self.max_texture_size as u32),
                                },
                                texture_size,
                            );

                            atlas_with_texture.atlas.fresh = false;
                            atlas_with_texture.atlas.dirty = false;
                        }
                    }
                }
            }
            ContextType::Cpu(_) => {
                self.mask_atlas.fresh = false;
                self.mask_atlas.dirty = false;
                for atlas_with_texture in &mut self.color_atlases {
                    atlas_with_texture.atlas.fresh = false;
                    atlas_with_texture.atlas.dirty = false;
                }
            }
        }
    }

    #[cfg(feature = "wgpu")]
    pub fn get_texture_views(&self) -> Vec<&wgpu::TextureView> {
        self.color_atlases
            .iter()
            .filter_map(|atlas_with_texture| {
                if let ColorAtlasTexture::Wgpu(_, view) = &atlas_with_texture.texture {
                    Some(view)
                } else {
                    None
                }
            })
            .collect()
    }

    #[cfg(feature = "wgpu")]
    pub fn get_mask_texture_view(&self) -> Option<&wgpu::TextureView> {
        match &self.device_queue {
            DeviceQueue::Wgpu {
                mask_texture_view, ..
            } => Some(mask_texture_view),
            _ => None,
        }
    }

    pub fn get_atlas_index(&self, image: ImageId) -> Option<usize> {
        let entry = self.entries.get(image.index())?;
        if !entry.allocated {
            return None;
        }
        if entry.atlas_kind == AtlasKind::Color {
            Some(entry.color_atlas_index)
        } else {
            None
        }
    }
}

struct FillParams {
    x: u16,
    y: u16,
    width: u16,
    _height: u16,
    target_width: u16,
    channels: usize,
}

fn fill(params: FillParams, image: &[u8], target: &mut [u8]) -> Option<()> {
    let image_pitch = params.width as usize * params.channels;
    let buffer_pitch = params.target_width as usize * params.channels;
    let mut offset =
        params.y as usize * buffer_pitch + params.x as usize * params.channels;
    for row in image.chunks(image_pitch) {
        let dest = target.get_mut(offset..offset + image_pitch)?;
        dest.copy_from_slice(row);
        offset += buffer_pitch;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_growth_preserves_data() {
        let old_size = 4u16;
        let new_size = 8u16;

        let mut old_buffer = vec![0u8; old_size as usize * old_size as usize];
        for y in 0..old_size as usize {
            for x in 0..old_size as usize {
                old_buffer[y * old_size as usize + x] =
                    ((y * old_size as usize + x) % 256) as u8;
            }
        }

        let mut new_buffer = vec![0u8; new_size as usize * new_size as usize];
        for y in 0..old_size as usize {
            let old_offset = y * old_size as usize;
            let new_offset = y * new_size as usize;
            let row_len = old_size as usize;
            new_buffer[new_offset..new_offset + row_len]
                .copy_from_slice(&old_buffer[old_offset..old_offset + row_len]);
        }

        for y in 0..old_size as usize {
            for x in 0..old_size as usize {
                let old_value = old_buffer[y * old_size as usize + x];
                let new_value = new_buffer[y * new_size as usize + x];
                assert_eq!(
                    old_value, new_value,
                    "Pixel at ({}, {}) should be preserved: expected {}, got {}",
                    x, y, old_value, new_value
                );
            }
        }
    }

    #[test]
    fn test_rgba_buffer_growth_preserves_data() {
        let old_size = 4u16;
        let new_size = 8u16;
        let channels = 4;

        let mut old_buffer = vec![0u8; old_size as usize * old_size as usize * channels];
        for y in 0..old_size as usize {
            for x in 0..old_size as usize {
                let base = (y * old_size as usize + x) * channels;
                old_buffer[base] = (x * 16) as u8;
                old_buffer[base + 1] = (y * 16) as u8;
                old_buffer[base + 2] = 128;
                old_buffer[base + 3] = 255;
            }
        }

        let mut new_buffer = vec![0u8; new_size as usize * new_size as usize * channels];
        for y in 0..old_size as usize {
            let old_offset = y * old_size as usize * channels;
            let new_offset = y * new_size as usize * channels;
            let row_len = old_size as usize * channels;
            new_buffer[new_offset..new_offset + row_len]
                .copy_from_slice(&old_buffer[old_offset..old_offset + row_len]);
        }

        for y in 0..old_size as usize {
            for x in 0..old_size as usize {
                let old_base = (y * old_size as usize + x) * channels;
                let new_base = (y * new_size as usize + x) * channels;

                for c in 0..channels {
                    assert_eq!(
                        old_buffer[old_base + c],
                        new_buffer[new_base + c],
                        "RGBA channel {} at ({}, {}) should be preserved",
                        c,
                        x,
                        y
                    );
                }
            }
        }
    }

    #[test]
    fn test_allocator_clone_preserves_state() {
        let mut original = AtlasAllocator::new(512, 512);

        let alloc1 = original.allocate(64, 64);
        let alloc2 = original.allocate(128, 32);
        let alloc3 = original.allocate(32, 128);

        assert!(alloc1.is_some());
        assert!(alloc2.is_some());
        assert!(alloc3.is_some());

        let cloned = original.clone();

        let mut original_next = original.clone();
        let mut cloned_next = cloned.clone();

        let orig_alloc = original_next.allocate(50, 50);
        let clone_alloc = cloned_next.allocate(50, 50);

        assert_eq!(orig_alloc, clone_alloc);
    }

    #[test]
    fn test_texture_size_growth_calculation() {
        let test_cases = vec![
            (1024, 1500, 2048),
            (1024, 2000, 2048),
            (1024, 2048, 2048),
            (1024, 2049, 4096),
            (1024, 4000, 4096),
            (1024, 4096, 4096),
            (1024, 5000, 4096),
            (2048, 3000, 4096),
            (2048, 4096, 4096),
            (4096, 4096, 4096),
            (4096, 5000, 4096),
        ];

        for (current_size, required_size, expected) in test_cases {
            let mut new_size = current_size;
            while new_size < required_size && new_size < 4096 {
                new_size *= 2;
            }
            new_size = new_size.min(4096);
            assert_eq!(new_size, expected);
        }
    }
}
