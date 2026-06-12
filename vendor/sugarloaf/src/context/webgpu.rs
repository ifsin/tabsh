use crate::sugarloaf::{Colorspace, SugarloafWindow, SugarloafWindowSize};
use crate::SugarloafRenderer;

pub struct WgpuContext<'a> {
    pub device: wgpu::Device,
    pub surface: wgpu::Surface<'a>,
    pub queue: wgpu::Queue,
    pub format: wgpu::TextureFormat,
    alpha_mode: wgpu::CompositeAlphaMode,
    pub adapter_info: wgpu::AdapterInfo,
    surface_caps: wgpu::SurfaceCapabilities,
    pub size: SugarloafWindowSize,
    pub scale: f32,
    pub supports_f16: bool,
    pub colorspace: Colorspace,
    pub max_texture_dimension_2d: u32,
}

impl<'a> WgpuContext<'a> {
    /// Async init for WASM — takes an `HtmlCanvasElement` directly and
    /// awaits adapter/device requests (can't block in the browser's
    /// single-threaded JS runtime).
    #[cfg(target_arch = "wasm32")]
    pub async fn new_async(
        canvas: web_sys::HtmlCanvasElement,
        renderer_config: SugarloafRenderer,
        wgpu_backend: wgpu::Backends,
    ) -> WgpuContext<'static> {
        let width = canvas.width() as f32;
        let height = canvas.height() as f32;

        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = wgpu_backend;
        let instance = wgpu::Instance::new(instance_desc);

        let surface: wgpu::Surface<'static> = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .expect("create surface from canvas");

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("request adapter");

        let adapter_info = adapter.get_info();
        let surface_caps = surface.get_capabilities(&adapter);

        let format = find_best_texture_format(
            surface_caps.formats.as_slice(),
            renderer_config.colorspace,
        );

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .unwrap_or_else(|_| {
                futures::executor::block_on(adapter.request_device(
                    &wgpu::DeviceDescriptor {
                        memory_hints: wgpu::MemoryHints::Performance,
                        label: None,
                        required_features: wgpu::Features::empty(),
                        required_limits: wgpu::Limits::downlevel_webgl2_defaults(),
                        ..Default::default()
                    },
                ))
                .expect("request device")
            });

        let alpha_mode = if surface_caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PostMultiplied)
        {
            wgpu::CompositeAlphaMode::PostMultiplied
        } else if surface_caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            wgpu::CompositeAlphaMode::Auto
        };

        surface.configure(
            &device,
            &wgpu::SurfaceConfiguration {
                usage: Self::get_texture_usage(&surface_caps),
                format,
                width: width as u32,
                height: height as u32,
                view_formats: vec![],
                alpha_mode,
                present_mode: wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: 2,
            },
        );

        let max_texture_dimension_2d = device.limits().max_texture_dimension_2d;

        WgpuContext {
            device,
            queue,
            surface,
            format,
            alpha_mode,
            size: SugarloafWindowSize { width, height },
            scale: 1.0,
            adapter_info,
            surface_caps,
            supports_f16: false,
            colorspace: renderer_config.colorspace,
            max_texture_dimension_2d,
        }
    }

    pub fn new(
        sugarloaf_window: SugarloafWindow,
        renderer_config: SugarloafRenderer,
        wgpu_backend: wgpu::Backends,
    ) -> WgpuContext<'a> {
        let size = sugarloaf_window.size;
        let scale = sugarloaf_window.scale;

        // The backend can be configured using the `WGPU_BACKEND`
        // environment variable. If the variable is not set, the primary backend
        // will be used. The following values are allowed:
        // - `vulkan`
        // - `metal`
        // - `dx12`
        // - `dx11`
        // - `gl`
        // - `webgpu`
        // - `primary`
        let backend = wgpu::Backends::from_env().unwrap_or(wgpu_backend);
        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = backend;
        let instance = wgpu::Instance::new(instance_desc);

        tracing::info!("selected instance: {instance:?}");

        tracing::info!("initializing the surface");

        let surface: wgpu::Surface<'a> =
            instance.create_surface(sugarloaf_window).unwrap();
        let adapter = futures::executor::block_on(instance.request_adapter(
            &wgpu::RequestAdapterOptions {
                // Hard-coded — sugarloaf used to expose a
                // `power_preference` knob, but in practice every Rio
                // user picks `HighPerformance` (the alternative gives
                // visibly worse text on hybrid laptops). Removed from
                // the public API.
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            },
        ))
        .expect("Request adapter");

        let adapter_info = adapter.get_info();
        tracing::info!("Selected adapter: {:?}", adapter_info);

        let surface_caps = surface.get_capabilities(&adapter);

        let format = find_best_texture_format(
            surface_caps.formats.as_slice(),
            renderer_config.colorspace,
        );

        let (device, queue) = {
            {
                if let Ok(result) = futures::executor::block_on(
                    adapter.request_device(&wgpu::DeviceDescriptor::default()),
                ) {
                    (result.0, result.1)
                } else {
                    // These downlevel limits will allow the code to run on all possible hardware
                    let result = futures::executor::block_on(adapter.request_device(
                        &wgpu::DeviceDescriptor {
                            memory_hints: wgpu::MemoryHints::Performance,
                            label: None,
                            required_features: wgpu::Features::empty(),
                            required_limits: wgpu::Limits::downlevel_webgl2_defaults(),
                            ..Default::default()
                        },
                    ))
                    .expect("Request device");
                    (result.0, result.1)
                }
            }
        };

        let alpha_mode = if surface_caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PostMultiplied)
        {
            wgpu::CompositeAlphaMode::PostMultiplied
        } else if surface_caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            wgpu::CompositeAlphaMode::Auto
        };

        // Configure view formats for wide color gamut support
        let view_formats = match renderer_config.colorspace {
            Colorspace::DisplayP3 | Colorspace::Rec2020 => {
                // For wide color gamut, we may want to support additional view formats
                // This allows the surface to be viewed in different formats
                vec![format]
            }
            Colorspace::Srgb => {
                vec![]
            }
        };

        surface.configure(
            &device,
            &wgpu::SurfaceConfiguration {
                usage: Self::get_texture_usage(&surface_caps),
                format,
                width: size.width as u32,
                height: size.height as u32,
                view_formats,
                alpha_mode,
                present_mode: wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: 2,
            },
        );

        let max_texture_dimension_2d = device.limits().max_texture_dimension_2d;

        tracing::info!("Configured colorspace: {:?}", renderer_config.colorspace);
        tracing::info!("Surface format: {:?}", format);

        WgpuContext {
            device,
            queue,
            surface,
            format,
            alpha_mode,
            size: SugarloafWindowSize {
                width: size.width,
                height: size.height,
            },
            scale,
            adapter_info,
            surface_caps,
            // Always disabled on webgpu
            supports_f16: false,
            colorspace: renderer_config.colorspace,
            max_texture_dimension_2d,
        }
    }

    fn get_texture_usage(caps: &wgpu::SurfaceCapabilities) -> wgpu::TextureUsages {
        let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;

        // COPY_DST and COPY_SRC are required for FiltersBrush
        // But some backends like OpenGL might not support COPY_DST and COPY_SRC
        // https://github.com/emilk/egui/pull/3078

        if caps.usages.contains(wgpu::TextureUsages::COPY_DST) {
            usage |= wgpu::TextureUsages::COPY_DST;
        }

        if caps.usages.contains(wgpu::TextureUsages::COPY_SRC) {
            usage |= wgpu::TextureUsages::COPY_SRC;
        }

        usage
    }

    pub fn max_texture_dimension_2d(&self) -> u32 {
        self.max_texture_dimension_2d
    }

    #[inline]
    pub fn resize(&mut self, width: u32, height: u32) {
        self.size.width = width as f32;
        self.size.height = height as f32;

        // Configure view formats for wide color gamut support
        let view_formats = match self.colorspace {
            Colorspace::DisplayP3 | Colorspace::Rec2020 => {
                vec![self.format]
            }
            Colorspace::Srgb => {
                vec![]
            }
        };

        self.surface.configure(
            &self.device,
            &wgpu::SurfaceConfiguration {
                usage: Self::get_texture_usage(&self.surface_caps),
                format: self.format,
                width,
                height,
                view_formats,
                alpha_mode: self.alpha_mode,
                present_mode: wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: 2,
            },
        );
    }

    #[inline]
    pub fn surface_caps(&self) -> &wgpu::SurfaceCapabilities {
        &self.surface_caps
    }

    #[inline]
    pub fn supports_f16(&self) -> bool {
        self.supports_f16
    }

    #[inline]
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
    }

    pub fn get_optimal_texture_format(&self) -> wgpu::TextureFormat {
        // wgpu always uses f32 formats, not f16
        wgpu::TextureFormat::Rgba8Unorm
    }

    pub fn get_optimal_texture_sample_type(&self) -> wgpu::TextureSampleType {
        // wgpu uses Rgba8Unorm (f32) with Float sample type and filtering
        wgpu::TextureSampleType::Float { filterable: true }
    }

    pub fn convert_rgba8_to_optimal_format(&self, rgba8_data: &[u8]) -> Vec<u8> {
        // wgpu always uses f32 (Rgba8Unorm), no f16 conversion needed
        rgba8_data.to_vec()
    }
}

#[inline]
fn find_best_texture_format(
    formats: &[wgpu::TextureFormat],
    colorspace: Colorspace,
) -> wgpu::TextureFormat {
    let mut format: wgpu::TextureFormat = formats.first().unwrap().to_owned();

    let unsupported_formats = [
        wgpu::TextureFormat::Rgba8Snorm,
        wgpu::TextureFormat::R16Unorm,
        wgpu::TextureFormat::R16Snorm,
    ];

    // Bgra8Unorm is the most widely supported and guaranteed format in wgpu
    // Prefer it explicitly if available
    if formats.contains(&wgpu::TextureFormat::Bgra8Unorm) {
        format = wgpu::TextureFormat::Bgra8Unorm;
        tracing::info!(
            "Sugarloaf selected format: {format:?} from {:?} for colorspace {:?}",
            formats,
            colorspace
        );
        return format;
    }

    let filtered_formats: Vec<wgpu::TextureFormat> = formats
        .iter()
        .copied()
        .filter(|&x| {
            // On non-macOS platforms, always avoid sRGB formats
            // This maintains compatibility with existing Linux/Windows color handling
            !wgpu::TextureFormat::is_srgb(&x) && !unsupported_formats.contains(&x)
        })
        .collect();

    // If no compatible formats found, fall back to any non-unsupported format
    let final_formats = if filtered_formats.is_empty() {
        formats
            .iter()
            .copied()
            .filter(|&x| !unsupported_formats.contains(&x))
            .collect()
    } else {
        filtered_formats
    };

    if !final_formats.is_empty() {
        final_formats.first().unwrap().clone_into(&mut format);
    }

    tracing::info!(
        "Sugarloaf selected format: {format:?} from {:?} for colorspace {:?}",
        formats,
        colorspace
    );

    format
}
