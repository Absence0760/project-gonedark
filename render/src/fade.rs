//! Shared linear fade-out clock for tick-stamped presentation effects.
//!
//! Several HUD/VFX cues follow the same shape: an event stamps a sim tick, and the visual
//! reads full-bright on that tick then ramps linearly to dark over a fixed window — the
//! muzzle flash ([`crate::world::muzzle_flash_intensity`]), the impact burst
//! ([`crate::impact::impact_intensity`]), and the hitmarker
//! ([`crate::hud::hitmarker_marker`]). This module owns that one fade curve so the three
//! stay byte-identical instead of hand-duplicating it.
//!
//! Pure float math (the presentation boundary, invariant #1/#4): the fade clock is derived
//! from read-only snapshot ticks and never feeds back into the sim, so `f32` is fair game
//! and it is unit-testable without a GPU.

/// Linear fade-out intensity in `[0, 1]` for the current `tick`, given the optional tick the
/// effect started on (`None` → never started → dark).
///
/// The starting tick reads `1.0`, then ramps linearly to `0.0` over `duration_ticks`. Dark
/// (`0.0`) whenever the fade is not live: never started (`None`), a future-stamped start
/// (`tick < started`), an age at or past the window, or a degenerate `duration_ticks == 0`
/// (the window check catches it before the division, so there is no `0/0`).
pub fn fade_out_since(started_tick: Option<u64>, tick: u64, duration_ticks: u64) -> f32 {
    let Some(started) = started_tick else {
        return 0.0;
    };
    if tick < started {
        return 0.0; // future-stamped start is not yet live
    }
    let age = tick - started;
    if age >= duration_ticks {
        return 0.0;
    }
    1.0 - age as f32 / duration_ticks as f32
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary (invariant #1), so `f32` math is fair game here.

    use super::*;

    const EPS: f32 = 1e-4;
    const DUR: u64 = 8;

    #[test]
    fn not_started_is_dark() {
        assert_eq!(fade_out_since(None, 100, DUR), 0.0);
    }

    #[test]
    fn start_tick_is_full_intensity() {
        assert!((fade_out_since(Some(50), 50, DUR) - 1.0).abs() < EPS);
    }

    #[test]
    fn mid_fade_decays_monotonically() {
        let young = fade_out_since(Some(0), 1, DUR);
        let mid = fade_out_since(Some(0), DUR / 2, DUR);
        let old = fade_out_since(Some(0), DUR - 1, DUR);
        assert!(young > mid && mid > old, "intensity decreases with age");
        assert!(old > 0.0, "still lit just before the cutoff");
        assert!(young < 1.0, "already dimmer one tick in");
        // The ramp is exactly linear: 1 - age/duration.
        assert!((mid - (1.0 - (DUR / 2) as f32 / DUR as f32)).abs() < EPS);
    }

    #[test]
    fn fully_faded_at_and_past_the_window() {
        assert_eq!(fade_out_since(Some(0), DUR, DUR), 0.0, "gone exactly at the cutoff");
        assert_eq!(fade_out_since(Some(0), DUR + 100, DUR), 0.0, "stays gone after");
    }

    #[test]
    fn future_stamped_start_is_dark() {
        // tick < started (clock not yet there) reads dark rather than a negative age.
        assert_eq!(fade_out_since(Some(100), 50, DUR), 0.0);
    }

    #[test]
    fn zero_duration_is_always_dark() {
        // Degenerate window: even the start tick is already past it — and no 0/0 division.
        assert_eq!(fade_out_since(Some(50), 50, 0), 0.0);
        assert_eq!(fade_out_since(Some(50), 51, 0), 0.0);
    }
}
