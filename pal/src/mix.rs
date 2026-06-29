//! Shared audio *render* math — the host-testable seam every concrete backend mixes through.
//!
//! The audio **mix derivation** (which sounds, where, how loud, what's muffled) is computed once,
//! platform-free, in `engine::audio::mix_cues`, producing [`AudioCue`](crate::AudioCue)s. Each
//! per-platform backend (`pal-desktop` cpal, `pal-android` oboe/AAudio) then *renders* those cues
//! to a stereo stream. The per-voice render math — equal-power pan from `azimuth`, gain clamp, the
//! one-pole low-pass that muffles off-map bleed (invariant #6), voice summation + soft-clamp, and
//! the [`MAX_VOICES`] eviction — is **identical on every platform**, so it lives here, NOT copied
//! into each backend. This is the same "pure logic behind a testable seam" pattern as
//! `render::interpolate_instances` and `engine`'s free fns: the realtime stream glue (a cpal /
//! oboe callback that can't be constructed in a host test) calls into this module, and this module
//! is fully unit-tested on the host with no audio device.
//!
//! Floats are fine here: this is the platform/presentation side, never the sim (invariant #1). The
//! determinism guard scopes its float greps to `core`/`sim` only — `pal` render math is out of
//! scope, exactly like [`AudioCue`](crate::AudioCue)'s own `f32` fields.

use std::collections::HashMap;
use std::f32::consts::PI;
use std::sync::Arc;

use crate::SoundId;

/// Max simultaneous voices; beyond this the oldest finished/started are dropped so a burst of fire
/// can't grow the mix unbounded. Shared by every backend so the cap behaves identically.
pub const MAX_VOICES: usize = 24;

/// The one-pole low-pass coefficient applied to a `muffled` (off-map strategic) voice. `< 1.0`
/// rolls off the highs so the bleed reads as "distant / off-map" (invariant #6); an unmuffled
/// voice uses `1.0` (a pure pass-through).
pub const MUFFLED_ALPHA: f32 = 0.12;

/// One playing sound: a shared synthesized buffer plus a cursor, per-ear gain, and a one-pole
/// low-pass state (`alpha == 1.0` is a pass-through; `< 1.0` muffles the off-map bleed).
pub struct Voice {
    samples: Arc<Vec<f32>>,
    pos: usize,
    gain_l: f32,
    gain_r: f32,
    lp_l: f32,
    lp_r: f32,
    alpha: f32,
}

impl Voice {
    /// True once the cursor has consumed the whole buffer — the voice contributes nothing further
    /// and is pruned lazily on [`Mixer::push`].
    #[inline]
    pub fn finished(&self) -> bool {
        self.pos >= self.samples.len()
    }
}

/// Build a [`Voice`] for `samples`, panned by `azimuth` (0 = ahead, + = right per the
/// [`AudioCue`](crate::AudioCue) contract), scaled by `gain` (clamped to `[0, 1]`), low-passed
/// when `muffled`. This is the exact render-derivation a backend applies to each cue before
/// pushing it into the [`Mixer`]; backends never reimplement it.
///
/// Equal-power pan: the lateral component of `azimuth` (`sin`) maps to an angle in `[0, PI/2]`,
/// and the two ear gains are `cos`/`sin` of that angle (so power is constant across the pan and
/// dead-ahead splits evenly, `g/√2` per ear).
pub fn voice_from_cue(samples: Arc<Vec<f32>>, azimuth: f32, gain: f32, muffled: bool) -> Voice {
    let pan = azimuth.sin().clamp(-1.0, 1.0);
    let angle = (pan + 1.0) * 0.25 * PI;
    let g = gain.clamp(0.0, 1.0);
    Voice {
        samples,
        pos: 0,
        gain_l: angle.cos() * g,
        gain_r: angle.sin() * g,
        lp_l: 0.0,
        lp_r: 0.0,
        alpha: if muffled { MUFFLED_ALPHA } else { 1.0 },
    }
}

/// The shared mix state read by a backend's realtime audio callback and written by the game
/// thread. A backend wraps this in an `Arc<Mutex<_>>`; the callback `try_lock`s it (never blocks
/// the audio thread) and pulls frames, the game thread pushes voices on `submit_mix`.
#[derive(Default)]
pub struct Mixer {
    voices: Vec<Voice>,
}

impl Mixer {
    /// A fresh, silent mixer.
    pub fn new() -> Self {
        Mixer::default()
    }

    /// Live voice count (test/diagnostic).
    pub fn len(&self) -> usize {
        self.voices.len()
    }

    /// True when no voices are queued.
    pub fn is_empty(&self) -> bool {
        self.voices.is_empty()
    }

    /// Sum one stereo frame from all live voices, advancing + low-passing each. Finished voices
    /// contribute nothing (pruned lazily on [`push`](Self::push)). Output is soft-clamped to
    /// `[-1, 1]` so stacked cues never clip.
    pub fn next_frame(&mut self) -> (f32, f32) {
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for v in &mut self.voices {
            if v.pos >= v.samples.len() {
                continue;
            }
            let s = v.samples[v.pos];
            v.pos += 1;
            v.lp_l += v.alpha * (s * v.gain_l - v.lp_l);
            v.lp_r += v.alpha * (s * v.gain_r - v.lp_r);
            l += v.lp_l;
            r += v.lp_r;
        }
        (l.clamp(-1.0, 1.0), r.clamp(-1.0, 1.0))
    }

    /// Queue a voice. At [`MAX_VOICES`] it first prunes finished voices, then (if still full)
    /// drops the oldest, so `len()` never exceeds the cap.
    pub fn push(&mut self, v: Voice) {
        if self.voices.len() >= MAX_VOICES {
            self.voices.retain(|x| !x.finished());
        }
        if self.voices.len() >= MAX_VOICES {
            self.voices.remove(0); // still full → drop the oldest
        }
        self.voices.push(v);
    }
}

// --- procedural cue synthesis (no audio assets yet) --------------------------------------------
//
// Synthesized once per backend at the device sample rate, shared (`Arc`) into every voice playing
// that sound. Identical on every platform so a cue sounds the same everywhere; amplitudes stay
// ~0.5 so a few stacked cues don't clip. Audio noise need not be deterministic (presentation, not
// the sim) — the xorshift here is just for a recognizable texture.

/// Synthesize a short buffer per [`SoundId`] at sample rate `sr`. Backends call this once on
/// stream-open and look voices up by id in `submit_mix` / `play_oneshot`.
pub fn synth_bank(sr: u32) -> HashMap<SoundId, Arc<Vec<f32>>> {
    let mut bank = HashMap::new();
    bank.insert(SoundId::Gunfire, Arc::new(gunfire(sr)));
    bank.insert(SoundId::UnitDown, Arc::new(unit_down(sr)));
    bank.insert(SoundId::BaseHit, Arc::new(base_hit(sr)));
    bank.insert(SoundId::Capture, Arc::new(capture(sr)));
    bank.insert(SoundId::ProductionReady, Arc::new(production_ready(sr)));
    bank.insert(SoundId::HitConfirm, Arc::new(hit_confirm(sr)));
    bank
}

fn secs(sr: u32, s: f32) -> usize {
    (sr as f32 * s) as usize
}
fn sine(sr: u32, i: usize, freq: f32) -> f32 {
    (2.0 * PI * freq * i as f32 / sr as f32).sin()
}
/// Tiny xorshift noise (audio noise need not be deterministic).
fn noise(state: &mut u32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// A snappy noise burst with a fast exponential decay.
fn gunfire(sr: u32) -> Vec<f32> {
    let n = secs(sr, 0.09);
    let tau = sr as f32 * 0.02;
    let mut st = 0x1234_5678u32;
    (0..n)
        .map(|i| noise(&mut st) * 0.5 * (-(i as f32) / tau).exp())
        .collect()
}

/// A descending tone — a unit falling.
fn unit_down(sr: u32) -> Vec<f32> {
    let n = secs(sr, 0.28);
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let freq = 380.0 - 240.0 * t; // 380 → 140 Hz
            sine(sr, i, freq) * 0.45 * (1.0 - t)
        })
        .collect()
}

/// A low thud + noise — a building being hit.
fn base_hit(sr: u32) -> Vec<f32> {
    let n = secs(sr, 0.2);
    let tau = sr as f32 * 0.06;
    let mut st = 0x9E37_79B9u32;
    (0..n)
        .map(|i| {
            let env = (-(i as f32) / tau).exp();
            (sine(sr, i, 90.0) * 0.5 + noise(&mut st) * 0.2) * env
        })
        .collect()
}

/// A rising two-tone chime — a control point captured.
fn capture(sr: u32) -> Vec<f32> {
    let n = secs(sr, 0.22);
    let half = n / 2;
    (0..n)
        .map(|i| {
            let freq = if i < half { 620.0 } else { 930.0 };
            let t = i as f32 / n as f32;
            sine(sr, i, freq) * 0.4 * (1.0 - t * 0.5)
        })
        .collect()
}

/// A short high blip — production finished.
fn production_ready(sr: u32) -> Vec<f32> {
    let n = secs(sr, 0.07);
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            sine(sr, i, 1050.0) * 0.4 * (1.0 - t)
        })
        .collect()
}

/// A crisp, very short two-tone tick — the embodied "I hit him" hitmarker confirmation. Higher and
/// shorter than every other cue so it reads as a UI feedback blip, not a world event.
fn hit_confirm(sr: u32) -> Vec<f32> {
    let n = secs(sr, 0.05);
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let freq = if t < 0.5 { 1400.0 } else { 1900.0 }; // up-tick: confirmed
            sine(sr, i, freq) * 0.5 * (1.0 - t)
        })
        .collect()
}

/// Map the legacy opaque `play_oneshot` id onto a [`SoundId`]. Shared so every backend's
/// fire-and-forget path agrees (desktop/Android both call this).
pub fn oneshot_sound(sound_id: u32) -> SoundId {
    match sound_id {
        1 => SoundId::UnitDown,
        2 => SoundId::BaseHit,
        3 => SoundId::Capture,
        4 => SoundId::ProductionReady,
        5 => SoundId::HitConfirm,
        _ => SoundId::Gunfire,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    /// A constant-1.0 buffer of `n` samples — lets a test read a voice's effective per-ear gain
    /// straight off `next_frame()` (the low-pass converges toward `s * gain`).
    fn ones(n: usize) -> Arc<Vec<f32>> {
        Arc::new(vec![1.0f32; n])
    }

    // --- voice_from_cue: equal-power pan -------------------------------------------------------

    #[test]
    fn pan_dead_ahead_is_balanced() {
        let v = voice_from_cue(ones(8), 0.0, 1.0, false);
        // azimuth 0 → both ears at g/√2.
        assert!((v.gain_l - v.gain_r).abs() < EPS, "l {} r {}", v.gain_l, v.gain_r);
        assert!((v.gain_l - 1.0 / 2.0_f32.sqrt()).abs() < EPS);
    }

    #[test]
    fn pan_hard_right_favors_right_ear() {
        // azimuth +PI/2 → source to the right (cue contract: positive = right).
        let v = voice_from_cue(ones(8), PI / 2.0, 1.0, false);
        assert!(v.gain_r > v.gain_l);
        assert!(v.gain_l.abs() < EPS, "left ear {} should be ~0", v.gain_l);
    }

    #[test]
    fn pan_hard_left_favors_left_ear() {
        let v = voice_from_cue(ones(8), -PI / 2.0, 1.0, false);
        assert!(v.gain_l > v.gain_r);
        assert!(v.gain_r.abs() < EPS, "right ear {} should be ~0", v.gain_r);
    }

    // --- voice_from_cue: gain clamp -----------------------------------------------------------

    #[test]
    fn gain_above_one_clamps_to_one() {
        let v = voice_from_cue(ones(8), 0.0, 5.0, false);
        // Each ear is cos/sin(angle) * 1.0; combined power == 1.0, neither ear exceeds 1.0.
        assert!(v.gain_l <= 1.0 + EPS && v.gain_r <= 1.0 + EPS);
        // Equivalent to gain == 1.0 (not 5.0).
        let at_one = voice_from_cue(ones(8), 0.0, 1.0, false);
        assert!((v.gain_l - at_one.gain_l).abs() < EPS);
    }

    #[test]
    fn negative_gain_clamps_to_zero() {
        let v = voice_from_cue(ones(8), 0.0, -1.0, false);
        assert!(v.gain_l.abs() < EPS && v.gain_r.abs() < EPS);
    }

    // --- muffled → low-pass alpha -------------------------------------------------------------

    #[test]
    fn muffled_sets_lowpass_alpha_unmuffled_passes_through() {
        let muffled = voice_from_cue(ones(8), 0.0, 1.0, true);
        let plain = voice_from_cue(ones(8), 0.0, 1.0, false);
        assert!((muffled.alpha - MUFFLED_ALPHA).abs() < EPS);
        assert!((plain.alpha - 1.0).abs() < EPS);
        assert!(muffled.alpha < 1.0);
    }

    #[test]
    fn muffled_voice_attenuates_first_sample_vs_unmuffled() {
        // First frame of a one-pole filter from rest: out = alpha * (s*gain). A muffled voice's
        // first sample is therefore much smaller than an unmuffled (alpha == 1.0) voice's.
        let mut muffled = Mixer::new();
        muffled.push(voice_from_cue(ones(8), 0.0, 1.0, true));
        let (ml, _) = muffled.next_frame();

        let mut plain = Mixer::new();
        plain.push(voice_from_cue(ones(8), 0.0, 1.0, false));
        let (pl, _) = plain.next_frame();

        assert!(ml < pl, "muffled first sample {ml} should be < unmuffled {pl}");
        // The muffled output is exactly MUFFLED_ALPHA of the unmuffled one on the first frame.
        assert!((ml - MUFFLED_ALPHA * pl).abs() < EPS);
    }

    // --- Mixer::next_frame: summation, soft-clamp, finished, empty -----------------------------

    #[test]
    fn empty_mixer_is_silent() {
        let mut m = Mixer::new();
        assert_eq!(m.next_frame(), (0.0, 0.0));
    }

    #[test]
    fn two_unmuffled_voices_sum_per_ear() {
        // Two dead-ahead voices, each ear gain g/√2 ≈ 0.707; pass-through (alpha 1) so the first
        // frame is exactly the gain. Summed L ≈ 1.414 but clamped to 1.0.
        let mut m = Mixer::new();
        m.push(voice_from_cue(ones(8), 0.0, 1.0, false));
        m.push(voice_from_cue(ones(8), 0.0, 1.0, false));
        let (l, r) = m.next_frame();
        assert_eq!((l, r), (1.0, 1.0)); // clamped
    }

    #[test]
    fn output_stays_clamped_when_many_loud_voices_stack() {
        let mut m = Mixer::new();
        for _ in 0..MAX_VOICES {
            m.push(voice_from_cue(ones(8), 0.0, 1.0, false));
        }
        let (l, r) = m.next_frame();
        assert!((-1.0..=1.0).contains(&l) && (-1.0..=1.0).contains(&r));
        assert_eq!((l, r), (1.0, 1.0));
    }

    #[test]
    fn finished_voice_contributes_zero() {
        // A 1-sample buffer: after one frame it's finished and adds nothing.
        let mut m = Mixer::new();
        m.push(voice_from_cue(ones(1), 0.0, 1.0, false));
        let first = m.next_frame();
        assert!(first.0 > 0.0);
        let second = m.next_frame();
        assert_eq!(second, (0.0, 0.0));
    }

    // --- Mixer::push: MAX_VOICES eviction ------------------------------------------------------

    #[test]
    fn push_never_exceeds_max_voices() {
        let mut m = Mixer::new();
        for _ in 0..(MAX_VOICES * 3) {
            m.push(voice_from_cue(ones(64), 0.0, 1.0, false));
        }
        assert!(m.len() <= MAX_VOICES, "len {} exceeded cap", m.len());
    }

    #[test]
    fn push_prunes_finished_before_dropping_oldest() {
        let mut m = Mixer::new();
        // Fill to the cap with 1-sample voices, then exhaust them all.
        for _ in 0..MAX_VOICES {
            m.push(voice_from_cue(ones(1), 0.0, 1.0, false));
        }
        m.next_frame(); // advance every voice past its single sample → all finished
        // The next push should prune the finished ones rather than evict a live voice.
        m.push(voice_from_cue(ones(64), 0.0, 1.0, false));
        // Only the fresh, live voice should remain audible; the rest were pruned as finished.
        assert!(m.len() <= MAX_VOICES);
        let (l, _) = m.next_frame();
        assert!(l > 0.0, "the freshly pushed live voice should be audible");
    }

    // --- synth_bank ---------------------------------------------------------------------------

    #[test]
    fn synth_bank_has_every_sound_nonempty_and_unclipped() {
        let bank = synth_bank(48_000);
        for id in [
            SoundId::Gunfire,
            SoundId::UnitDown,
            SoundId::BaseHit,
            SoundId::Capture,
            SoundId::ProductionReady,
            SoundId::HitConfirm,
        ] {
            let buf = bank.get(&id).expect("sound present");
            assert!(!buf.is_empty(), "{id:?} buffer empty");
            for &s in buf.iter() {
                assert!((-0.8..=0.8).contains(&s), "{id:?} sample {s} too hot");
            }
        }
    }

    #[test]
    fn synth_bank_length_scales_with_sample_rate() {
        let lo = synth_bank(24_000);
        let hi = synth_bank(48_000);
        let lo_gun = lo.get(&SoundId::Gunfire).unwrap().len();
        let hi_gun = hi.get(&SoundId::Gunfire).unwrap().len();
        assert!(hi_gun > lo_gun, "{hi_gun} should exceed {lo_gun}");
    }

    #[test]
    fn oneshot_sound_maps_ids() {
        assert_eq!(oneshot_sound(1), SoundId::UnitDown);
        assert_eq!(oneshot_sound(2), SoundId::BaseHit);
        assert_eq!(oneshot_sound(3), SoundId::Capture);
        assert_eq!(oneshot_sound(4), SoundId::ProductionReady);
        assert_eq!(oneshot_sound(5), SoundId::HitConfirm);
        assert_eq!(oneshot_sound(0), SoundId::Gunfire);
        assert_eq!(oneshot_sound(99), SoundId::Gunfire);
    }
}
