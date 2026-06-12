// Copyright (c) 2023-present, Raphael Amorim.
//
// CPU rendering backend context.

use crate::sugarloaf::{SugarloafWindow, SugarloafWindowSize};

pub struct CpuContext {
    pub size: SugarloafWindowSize,
    pub scale: f32,
    pub width_px: u32,
    pub height_px: u32,
}

impl CpuContext {
    pub fn new(_window: SugarloafWindow) -> Self {
        unreachable!("CPU backend not supported on WASM")
    }

    pub fn resize(&mut self, _width: u32, _height: u32) {}

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
    }

    pub fn supports_f16(&self) -> bool {
        false
    }
}
