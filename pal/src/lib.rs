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
    /// Edge-triggered pointer-release (a tap/drag completed this frame). Touch UI uses this to
    /// close a drag-select rectangle or commit a tap. Latched + cleared like the other edges.
    pub pointer_up: bool,
    /// Edge-triggered embody / surface intents.
    pub embody_pressed: bool,
    pub surface_pressed: bool,
    /// Edge-triggered "open the order/stance context" intent (touch: long-press; desktop:
    /// right-button down). The command-UI layer turns this into a vocabulary action on the
    /// current selection.
    pub long_press: bool,
    /// A discrete order/stance vocabulary slot chosen from the on-screen command UI this frame,
    /// if any (touch: a radial/toolbar button; desktop: number keys). The command-UI layer maps
    /// the slot → a [`Command`](../../core) for the current selection. Kept as an opaque small
    /// integer so the PAL backend needn't know the vocabulary.
    pub command_slot: Option<u8>,
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

/// A distinct game sound. The embodied audio mix (game-design §6, invariant #6) is built from
/// these: strategic sound bleeding into the FPS view is the *primary* directional-awareness
/// system while the map is dark. Plain `repr` enum so the mix layer (engine) and the backends
/// (`pal-*`) share one vocabulary without either pulling a dependency.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum SoundId {
    /// A weapon firing somewhere on the field.
    Gunfire,
    /// One of your units died.
    UnitDown,
    /// A building of yours is being hit.
    BaseHit,
    /// A control point changed hands.
    Capture,
    /// A queued unit finished production.
    ProductionReady,
}

/// One positioned sound for this frame's mix. Floats are fine here (platform side, not the sim):
/// `azimuth`/`gain` are derived in the engine's *presentation* path from the deterministic event
/// stream + the listener pose — they never feed back into `core` (invariant #1).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AudioCue {
    pub sound: SoundId,
    /// Bearing to the source relative to the listener's facing, radians. 0 = dead ahead,
    /// positive = to the right. Lets a backend pan the cue so direction reads by ear.
    pub azimuth: f32,
    /// Linear gain in `[0, 1]` after distance attenuation.
    pub gain: f32,
    /// When set, this is *strategic* sound leaking into the embodied mix — a backend should
    /// low-pass / duck it so it reads as "distant / off-map", not as a local event.
    pub muffled: bool,
}

/// Low-latency audio backend (AAudio/CoreAudio/PipeWire). `play_oneshot` is the simple fire-
/// and-forget path; `submit_mix` takes the per-frame positioned mix the embodied view needs
/// (the strategic-sound-into-FPS bleed, invariant #6). Output fidelity is per-backend; the
/// *mix* (which sounds, where, how loud) is computed once, platform-free, in `engine::audio`.
pub trait Audio {
    fn play_oneshot(&mut self, sound_id: u32);
    /// Submit this frame's positioned cues. Called every presented frame (often empty). A
    /// backend renders/pans/ducks them; an empty slice means silence this frame.
    fn submit_mix(&mut self, cues: &[AudioCue]);
}

/// Persistent storage / VFS seam (settings, replays).
pub trait Storage {
    fn read(&self, key: &str) -> Option<Vec<u8>>;
    fn write(&mut self, key: &str, bytes: &[u8]);
}
