//! Desktop PAL backend (Linux/Windows). Real winit+wgpu lands in build-order step 4; this
//! is a headless stand-in implementing the PAL traits so the `app` run loop compiles and
//! runs now, exercising the sim/render/embodiment seams without a GPU.

use gonedark_pal::{Input, InputFrame, Rhi, Window};

/// Headless window: runs for a bounded number of frames, then asks to close. Replaced by
/// a winit window in step 4.
#[derive(Default)]
pub struct DesktopWindow {
    width: u32,
    height: u32,
    frames: u32,
    max_frames: u32,
}

impl DesktopWindow {
    /// `max_frames == 0` means run forever (until something else closes it).
    pub fn new(width: u32, height: u32, max_frames: u32) -> Self {
        DesktopWindow {
            width,
            height,
            frames: 0,
            max_frames,
        }
    }
}

impl Window for DesktopWindow {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    fn should_close(&self) -> bool {
        self.max_frames != 0 && self.frames >= self.max_frames
    }
    fn pump(&mut self) -> bool {
        self.frames += 1;
        !self.should_close()
    }
}

/// Headless input: scripted intents for the harness. A real backend maps mouse+kbd/gamepad
/// onto `InputFrame` here.
#[derive(Default)]
pub struct DesktopInput {
    frame: u32,
}

impl Input for DesktopInput {
    fn poll(&mut self) -> InputFrame {
        self.frame += 1;
        // Scripted demo: embody at frame 30, surface at frame 120 — exercises the seam.
        InputFrame {
            embody_pressed: self.frame == 30,
            surface_pressed: self.frame == 120,
            ..InputFrame::default()
        }
    }
}

/// Stub RHI: no-op presentation until the wgpu device lands (step 4).
#[derive(Default)]
pub struct DesktopRhi;

impl Rhi for DesktopRhi {
    fn resize(&mut self, _width: u32, _height: u32) {}
    fn begin_frame(&mut self) -> bool {
        true
    }
    fn end_frame(&mut self) {}
}
