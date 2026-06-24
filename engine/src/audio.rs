//! Embodied audio mix (invariant #6, game-design §6) — the PURE, platform-free layer that
//! turns the deterministic per-tick [`SimEvent`] stream into the positioned [`AudioCue`]s the
//! backend renders. While embodied, strategic sound bleeding into the FPS view is the *primary*
//! directional-awareness system: "alerts, not intel" by ear.
//!
//! This is a presentation derivation: it reads sim events + the listener pose and produces
//! cues; it MUST NOT mutate sim state (it never desyncs lockstep — invariant #1). Floats are
//! fine here (presentation, not the sim). It is a free function so it is unit-testable without
//! a GPU or an audio device.
//!
//! IMPLEMENTATION OWNER: worker 3 (embodied audio). Compiling stub: returns no cues, so the
//! game is silent until you fill `mix_cues` + inline tests. KEEP the public signature intact.
//! You also own the two backend leaves `pal-desktop/src/audio.rs` and `pal-android/src/audio.rs`.

use gonedark_core::ecs::World;
use gonedark_core::event::SimEvent;
use gonedark_pal::AudioCue;

/// Build this frame's positioned audio mix from `events` (this tick's deterministic stream),
/// the `embodied` flag, the `listener` world position (the avatar, when embodied), the listener
/// `yaw` (radians, presentation-only), and a read-only `world` for any classification.
///
/// Contract for the implementation (worker 3):
/// - Map each relevant `SimEvent` → a [`gonedark_pal::SoundId`].
/// - `azimuth` = bearing of the event position relative to `yaw` (0 = ahead, +right).
/// - `gain` falls off with distance from `listener`.
/// - `muffled` is set for *strategic* sound while `embodied` (the off-map bleed).
/// - Read-only over `world`; never mutate sim state.
pub fn mix_cues(
    _events: &[SimEvent],
    _embodied: bool,
    _listener: (f32, f32),
    _yaw: f32,
    _world: &World,
) -> Vec<AudioCue> {
    Vec::new()
}
