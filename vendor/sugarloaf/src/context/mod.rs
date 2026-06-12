pub mod cpu;
#[cfg(feature = "wgpu")]
pub mod webgpu;

use crate::sugarloaf::{SugarloafBackend, SugarloafWindow};
use crate::{SugarloafRenderer, SugarloafWindowSize};
#[cfg(all(target_arch = "wasm32", feature = "wgpu"))]
use wgpu;

pub struct Context<'a> {
    pub inner: ContextType<'a>,
}

#[allow(clippy::large_enum_variant)]
pub enum ContextType<'a> {
    #[cfg(feature = "wgpu")]
    Wgpu(webgpu::WgpuContext<'a>),
    Cpu(cpu::CpuContext),
    #[cfg(not(feature = "wgpu"))]
    #[doc(hidden)]
    _Phantom(std::marker::PhantomData<&'a ()>),
}

impl Context<'_> {
    #[cfg(all(target_arch = "wasm32", feature = "wgpu"))]
    pub async fn new_wasm_async(
        canvas: web_sys::HtmlCanvasElement,
        renderer_config: SugarloafRenderer,
    ) -> Context<'static> {
        let backends = match &renderer_config.backend {
            SugarloafBackend::Wgpu(b) => *b,
            _ => wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL,
        };
        let inner = ContextType::Wgpu(
            webgpu::WgpuContext::new_async(canvas, renderer_config, backends).await,
        );
        Context { inner }
    }

    pub fn new<'a>(
        sugarloaf_window: SugarloafWindow,
        renderer_config: SugarloafRenderer,
    ) -> Context<'a> {
        let inner = match renderer_config.backend {
            #[cfg(feature = "wgpu")]
            SugarloafBackend::Wgpu(backends) => ContextType::Wgpu(
                webgpu::WgpuContext::new(sugarloaf_window, renderer_config, backends),
            ),
            SugarloafBackend::Cpu => {
                ContextType::Cpu(cpu::CpuContext::new(sugarloaf_window))
            }
        };

        Context { inner }
    }

    #[inline]
    pub fn scale(&self) -> f32 {
        match &self.inner {
            #[cfg(feature = "wgpu")]
            ContextType::Wgpu(ctx) => ctx.scale,
            ContextType::Cpu(ctx) => ctx.scale,
            #[cfg(not(feature = "wgpu"))]
            ContextType::_Phantom(_) => unreachable!(),
        }
    }

    #[inline]
    pub fn set_scale(&mut self, scale: f32) {
        match &mut self.inner {
            #[cfg(feature = "wgpu")]
            ContextType::Wgpu(ctx) => {
                ctx.set_scale(scale);
            }
            ContextType::Cpu(ctx) => {
                ctx.set_scale(scale);
            }
            #[cfg(not(feature = "wgpu"))]
            ContextType::_Phantom(_) => unreachable!(),
        }
    }

    #[inline]
    pub fn size(&self) -> SugarloafWindowSize {
        match &self.inner {
            #[cfg(feature = "wgpu")]
            ContextType::Wgpu(ctx) => ctx.size,
            ContextType::Cpu(ctx) => ctx.size,
            #[cfg(not(feature = "wgpu"))]
            ContextType::_Phantom(_) => unreachable!(),
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }

        match &mut self.inner {
            #[cfg(feature = "wgpu")]
            ContextType::Wgpu(ctx) => ctx.resize(width, height),
            ContextType::Cpu(ctx) => ctx.resize(width, height),
            #[cfg(not(feature = "wgpu"))]
            ContextType::_Phantom(_) => unreachable!(),
        }
    }

    #[inline]
    pub fn supports_f16(&self) -> bool {
        match &self.inner {
            #[cfg(feature = "wgpu")]
            ContextType::Wgpu(ctx) => ctx.supports_f16(),
            ContextType::Cpu(ctx) => ctx.supports_f16(),
            #[cfg(not(feature = "wgpu"))]
            ContextType::_Phantom(_) => unreachable!(),
        }
    }
}
