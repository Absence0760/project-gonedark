//! Embodied **recoil / view-kick** seam (WS-A, CP-2 game-feel bar) — the PURE, GPU-free math that
//! turns the firing cadence into a presentation-only camera punch + a crosshair bloom that decay
//! back to rest. It is the recoil twin of [`scope`](crate::scope): a free-function seam the host
//! drives once per frame off the **wall-clock `dt`** (exactly like [`scope::step_zoom_t`]), unit-
//! tested without a GPU.
//!
//! **Presentation/input only (invariant #4).** Nothing here writes sim/`core` state, nothing is
//! checksummed, and the kick is *not* a character system — it is a transient camera-pitch offset +
//! a crosshair spread that the host applies in the render/look path. Floats are fine: this is the
//! render/input boundary, never the sim. The recoil adds **no** desync surface (invariant #7) — the
//! accumulator lives on `Game` as host presentation state and can never reach `&mut Sim`.
//!
//! **Why pitch, not yaw, into the camera.** The deterministic sim aim is 2-D yaw
//! (`fire::fire_command` quantizes `(cos yaw, sin yaw)`); the embodied *pitch* is already purely
//! cosmetic (it never enters fire/locomote). So an upward **pitch** kick is a free, fair view-punch:
//! it reads as the muzzle climbing without ever moving where the bullet goes, so the screen-center
//! crosshair stays aligned with the 2-D fire direction. The **horizontal** half of recoil — the
//! uncertainty a yaw kick would convey — is carried by the crosshair **bloom** (spread) instead, so
//! it never desynchronises the reticle from the shot (which a presentation-only camera-yaw offset
//! would). See the report / follow-ups for the deliberately-omitted camera-yaw kick.

/// Recoil added to the accumulator per shot (in abstract "recoil units"). One trigger pull bumps the
/// accumulator by this; sustained fire stacks it toward [`RECOIL_MAX`].
pub const RECOIL_PER_SHOT: f32 = 1.0;

/// The accumulator ceiling: sustained fire saturates here so the view-punch + bloom plateau at a
/// readable maximum instead of climbing without bound.
pub const RECOIL_MAX: f32 = 3.5;

/// Recovery rate (recoil units per second) the accumulator decays toward `0` while not firing — the
/// settle. `7.0` drains a fully-saturated gun (`RECOIL_MAX`) back to rest in ~0.5 s: a snappy
/// re-settle that still reads as a distinct recover, mirroring [`scope::ZOOM_RATE`]'s dt-based ease.
pub const RECOIL_RECOVERY: f32 = 7.0;

/// Upward camera-**pitch** offset (radians) per recoil unit. At [`RECOIL_MAX`] this is a
/// `3.5 * 0.012 ≈ 0.042 rad ≈ 2.4°` punch — a visible muzzle climb, not a disorienting lurch.
/// Cosmetic only (the sim aim is 2-D yaw), so it never moves the bullet.
pub const KICK_PITCH_PER_RECOIL: f32 = 0.012;

/// Crosshair **bloom** (extra reticle gap, in NDC half-height units) per recoil unit. At
/// [`RECOIL_MAX`] the four reticle arms spread an extra `3.5 * 0.016 ≈ 0.056` NDC outward from the
/// resting gap — the "your shots are spreading" read, decaying back as the gun settles.
pub const BLOOM_PER_RECOIL: f32 = 0.016;

/// Add `shots` trigger pulls' worth of recoil to the accumulator, **saturating at [`RECOIL_MAX`]**
/// (sustained fire plateaus, never runs away). A no-op for `shots == 0`. Pure → host-tested.
#[inline]
pub fn add_recoil(current: f32, shots: u32) -> f32 {
    (current + shots as f32 * RECOIL_PER_SHOT).min(RECOIL_MAX)
}

/// Decay the recoil accumulator toward rest (`0`) by one frame of `dt` seconds at `rate` units/sec.
/// Returns the next value, **clamped at `0`** and **monotone non-increasing** (a negative/zero `dt`
/// is a no-op, never a bump). Drives the settle; the dt-based step makes the recover frame-rate
/// independent (same wall-clock settle at 30/60/120 fps), exactly like [`scope::step_zoom_t`]. Pure.
#[inline]
pub fn decay_recoil(current: f32, dt: f32, rate: f32) -> f32 {
    let step = (rate * dt).max(0.0);
    (current - step).max(0.0)
}

/// The upward camera-pitch offset (radians) for the current recoil level — a positive (look-up)
/// value the host adds to the embodied pitch when building the view-projection. `0` at rest. Pure.
#[inline]
pub fn view_pitch_kick(recoil: f32) -> f32 {
    recoil.max(0.0) * KICK_PITCH_PER_RECOIL
}

/// The crosshair bloom (extra reticle gap, NDC half-height units) for the current recoil level — the
/// host adds it to the resting reticle gap so the arms spread under fire and pull back in as the gun
/// settles. `0` at rest. Pure.
#[inline]
pub fn crosshair_bloom(recoil: f32) -> f32 {
    recoil.max(0.0) * BLOOM_PER_RECOIL
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- accumulator: saturating add, clamped decay ----

    #[test]
    fn a_shot_adds_recoil_and_sustained_fire_saturates() {
        let one = add_recoil(0.0, 1);
        assert!((one - RECOIL_PER_SHOT).abs() < 1e-6, "one shot adds RECOIL_PER_SHOT");
        // Stacking many shots never exceeds the ceiling.
        let mut r = 0.0;
        for _ in 0..100 {
            r = add_recoil(r, 1);
        }
        assert!((r - RECOIL_MAX).abs() < 1e-6, "sustained fire saturates at RECOIL_MAX, got {r}");
        // A multi-shot bump is the same as several single bumps, still clamped.
        assert!((add_recoil(0.0, 1000) - RECOIL_MAX).abs() < 1e-6);
    }

    #[test]
    fn zero_shots_is_a_noop() {
        assert_eq!(add_recoil(1.25, 0), 1.25);
    }

    #[test]
    fn decay_eases_toward_zero_and_clamps() {
        let dt = 1.0 / 60.0;
        let mut r = RECOIL_MAX;
        let mut prev = RECOIL_MAX + 1.0;
        for _ in 0..240 {
            let next = decay_recoil(r, dt, RECOIL_RECOVERY);
            assert!(next <= prev + 1e-9, "monotone non-increasing");
            assert!(next >= 0.0, "never below rest, got {next}");
            prev = next;
            r = next;
        }
        assert!(r.abs() < 1e-6, "settles to rest (0)");
        // Already at rest → stays at rest.
        assert_eq!(decay_recoil(0.0, dt, RECOIL_RECOVERY), 0.0);
    }

    #[test]
    fn decay_is_a_noop_for_nonpositive_dt() {
        assert_eq!(decay_recoil(2.0, 0.0, RECOIL_RECOVERY), 2.0);
        assert_eq!(decay_recoil(2.0, -0.1, RECOIL_RECOVERY), 2.0);
    }

    #[test]
    fn a_full_gun_settles_in_about_half_a_second() {
        // RECOIL_MAX / RECOIL_RECOVERY ≈ 0.5 s — pin the felt settle time so a tuning drift is caught.
        let dt = 1.0 / 60.0;
        let mut r = RECOIL_MAX;
        let mut frames = 0;
        while r > 1e-4 && frames < 600 {
            r = decay_recoil(r, dt, RECOIL_RECOVERY);
            frames += 1;
        }
        let secs = frames as f32 * dt;
        assert!((0.40..=0.65).contains(&secs), "settle ~0.5 s, got {secs} s");
    }

    // ---- derived presentation quantities ----

    #[test]
    fn pitch_kick_is_zero_at_rest_and_grows_upward_with_recoil() {
        assert_eq!(view_pitch_kick(0.0), 0.0, "no kick at rest");
        assert!(view_pitch_kick(RECOIL_MAX) > 0.0, "kick looks UP (positive pitch)");
        assert!(
            view_pitch_kick(RECOIL_MAX) > view_pitch_kick(1.0),
            "more recoil ⇒ more climb"
        );
        // Saturated punch stays within a sane, non-disorienting band (~2.4°).
        let max_deg = view_pitch_kick(RECOIL_MAX).to_degrees();
        assert!((1.0..=4.0).contains(&max_deg), "max punch ~2.4°, got {max_deg}°");
        // A stray negative accumulator never produces a downward kick.
        assert_eq!(view_pitch_kick(-5.0), 0.0);
    }

    #[test]
    fn bloom_is_zero_at_rest_and_spreads_with_recoil() {
        assert_eq!(crosshair_bloom(0.0), 0.0, "tight crosshair at rest");
        assert!(crosshair_bloom(RECOIL_MAX) > crosshair_bloom(1.0), "more recoil ⇒ more spread");
        assert_eq!(crosshair_bloom(-1.0), 0.0, "no negative spread");
    }
}
