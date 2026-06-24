//! Platform Abstraction Layer — trait definitions ONLY (invariant #2, D9).
//!
//! Concrete backends (`pal-desktop`, `pal-android`, later `pal-ios`) implement these;
//! `core` never sees them. Keep this seam *thin* — only what genuinely differs per
//! platform crosses it. Floats are fine here: this is the platform side, not the sim.

/// Monotonic clock for the run loop's frame timing.
pub trait Clock {
    /// Monotonic seconds since some fixed epoch.
    fn now_secs(&self) -> f64;
}

/// Per-frame input, already translated from the platform's native scheme into the
/// engine's intent vocabulary. Each backend maps touch+gyro / mouse+kbd / gamepad onto
/// this SAME struct, so the core stays input-agnostic (platforms.md §5).
#[derive(Clone, Debug, Default)]
pub struct InputFrame {
    /// Pointer position in window pixels, if any (command-layer tap/click).
    pub pointer: Option<(f32, f32)>,
    pub pointer_down: bool,
    /// Edge-triggered embody / surface intents.
    pub embody_pressed: bool,
    pub surface_pressed: bool,
    /// Embodied locomotion + look axes (left stick / right stick or WASD + mouse).
    pub move_axis: (f32, f32),
    pub look_axis: (f32, f32),
    pub fire: bool,
}

/// A platform window / surface + lifecycle.
pub trait Window {
    fn size(&self) -> (u32, u32);
    fn should_close(&self) -> bool;
    /// Pump native events; returns false when the app should exit.
    fn pump(&mut self) -> bool;
}

/// Per-platform input source.
pub trait Input {
    fn poll(&mut self) -> InputFrame;
}

/// Render Hardware Interface — the GPU seam. Concrete impls wrap wgpu (→ Vulkan/D3D12/
/// Metal per device). The renderer talks to this, never to a specific GPU API.
pub trait Rhi {
    /// (Re)create the swapchain for a new surface size.
    fn resize(&mut self, width: u32, height: u32);
    /// Begin a frame; false means skip (e.g. surface lost — recreate next frame).
    fn begin_frame(&mut self) -> bool;
    /// Present the frame.
    fn end_frame(&mut self);
}

/// Low-latency audio backend (AAudio/CoreAudio/PipeWire). Stubbed for Phase 1.
pub trait Audio {
    fn play_oneshot(&mut self, sound_id: u32);
}

/// Persistent storage / VFS seam (settings, replays).
pub trait Storage {
    fn read(&self, key: &str) -> Option<Vec<u8>>;
    fn write(&mut self, key: &str, bytes: &[u8]);
}
