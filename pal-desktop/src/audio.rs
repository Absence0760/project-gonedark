//! Desktop audio backend (the [`gonedark_pal::Audio`] seam). The *mix* — which sounds, where,
//! how loud, what's muffled — is computed platform-free in `engine::audio`; this backend only
//! renders it. The embodied "strategic sound bleeding into FPS" model (invariant #6) is the
//! same everywhere; only the output path is per-platform.
//!
//! IMPLEMENTATION OWNER: worker 3 (embodied audio, desktop backend). This is a compiling no-op
//! scaffold (it accepts cues and drops them), so the engine stays silent until you wire a real
//! output path (e.g. a `cpal`/PipeWire stream that pans by `azimuth`, scales by `gain`, and
//! low-passes `muffled` cues). KEEP `DesktopAudio` + its `Audio` impl; you own the internals
//! (add any stream/state fields you need; keep `new()` infallible or document the fallback).

use gonedark_pal::{Audio, AudioCue};

/// Desktop audio sink. Holds the platform output stream (worker 3 to add); the scaffold holds
/// nothing and renders silence.
#[derive(Default)]
pub struct DesktopAudio {
    // worker 3: output stream handle / mixer state.
}

impl DesktopAudio {
    /// Construct the desktop audio sink. The scaffold can't fail; if a real backend's device
    /// open can fail, fall back to a silent sink rather than panicking (audio is never load-
    /// bearing for the sim).
    pub fn new() -> Self {
        DesktopAudio::default()
    }
}

impl Audio for DesktopAudio {
    fn play_oneshot(&mut self, sound_id: u32) {
        let _ = sound_id;
    }

    fn submit_mix(&mut self, cues: &[AudioCue]) {
        // worker 3: render the positioned mix. Scaffold drops it (silence).
        let _ = cues;
    }
}
