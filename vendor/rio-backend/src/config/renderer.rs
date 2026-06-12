use serde::{Deserialize, Serialize};
use std::fmt::Display;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Renderer {
    #[serde(default = "Backend::default", skip_serializing)]
    pub backend: Backend,
    #[serde(default = "bool::default", rename = "disable-unfocused-render")]
    pub disable_unfocused_render: bool,
    #[serde(
        default = "default_disable_occluded_render",
        rename = "disable-occluded-render"
    )]
    pub disable_occluded_render: bool,
    #[serde(default = "RendererStategy::default")]
    pub strategy: RendererStategy,
    /// Use the CPU rasterizer (tiny-skia) instead of the GPU pipeline.
    /// Experimental. v1 supports solid quads + glyphs only; image
    /// overlays, GPU filters, advanced underline styles, and corner radii
    /// are not yet implemented on the CPU path.
    #[serde(default = "default_use_cpu", rename = "use-cpu")]
    pub use_cpu: bool,
}

fn default_use_cpu() -> bool {
    false
}

fn default_disable_occluded_render() -> bool {
    false
}

#[derive(Default, Debug, Clone, PartialEq, Deserialize, Serialize)]
pub enum RendererStategy {
    #[default]
    #[serde(alias = "events")]
    Events,
    #[serde(alias = "game")]
    Game,
}

impl RendererStategy {
    #[inline]
    pub fn is_game(&self) -> bool {
        self == &RendererStategy::Game
    }

    #[inline]
    pub fn is_event_based(&self) -> bool {
        self == &RendererStategy::Events
    }
}

#[allow(clippy::derivable_impls)]
impl Default for Renderer {
    fn default() -> Renderer {
        Renderer {
            backend: Backend::default(),
            disable_unfocused_render: false,
            disable_occluded_render: default_disable_occluded_render(),
            strategy: RendererStategy::Events,
            use_cpu: default_use_cpu(),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq)]
pub enum Backend {
    #[serde(alias = "vulkan")]
    Vulkan,
    #[default]
    #[serde(alias = "webgpu", alias = "wgpu")]
    Webgpu,
}

impl Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Backend::Vulkan => write!(f, "Vulkan"),
            Backend::Webgpu => write!(f, "Webgpu"),
        }
    }
}
