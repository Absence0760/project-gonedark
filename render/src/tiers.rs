//! Device quality tiers + dynamic-resolution + thermal-backoff policy — Phase 4 WS-C.
//!
//! **All of this is a RENDERING choice (invariant #1/#4).** A tier, a render-target scale, or a
//! thermal backoff changes only how the frame is *drawn* — it never touches `core`, never enters
//! the sim, and never changes `core::sim::TICK_HZ`. The sim runs the same fixed 60 Hz tick at every
//! tier, so the per-tick checksum stream is byte-identical regardless of what this module decides
//! (the guard test `tier_choice_is_sim_independent` in `engine` asserts exactly that). Floats are
//! fine here: this is the float boundary, not the sim (invariant #1).
//!
//! Three pure, host-testable pieces, no GPU and no `Game`/device needed:
//!  - [`QualityTier`] → [`TierParams`] ([`QualityTier::params`]): the per-device-class render
//!    budget (resolution-scale floor/ceiling, draw distance, effect density, instance budget).
//!    Feeds the future Settings "graphics tiers" surface (phase-4-plan surface 3).
//!  - [`next_resolution_scale`]: given recent frame times + the frame budget + the tier's
//!    floor/ceiling → a new render-target scale factor (presentation-only; reads frame timing,
//!    never sim state).
//!  - [`thermal_backoff`]: given a [`gonedark_pal::ThermalState`] + the tier → a [`Backoff`]
//!    (an FPS cap and a *tightened* dyn-res floor) the pacing/dyn-res loop applies under heat.
//!
//! The renderer's only coupling is that it scales its render target by the chosen factor; that
//! wiring lives in the GPU glue (`engine`/the backends). The *decisions* are all here, pure.
//!
//! ## What this module is deliberately NOT: physical UI scale
//! A [`QualityTier`] / [`TierParams`] is a **3D-cost** budget only. It must **never** carry a UI /
//! HUD scale factor. Quality tier (a thermal/performance knob — how expensive the *scene* is to
//! draw) and physical UI scale (a legibility knob — how large chrome/touch targets read in real
//! millimetres across displays of differing density/PPI) are **orthogonal**: a flagship on `High`
//! can be a physically small, very dense phone that needs *larger* UI, while a cheap `Low` tablet is
//! big and low-density and needs *smaller*. Folding one into the other would couple two unrelated
//! axes and mis-size the HUD. The physical UI scale is a separate scalar the host sources from the
//! platform (`winit` `scale_factor()` / Android `densityDpi`) and threads into the *chrome* passes
//! (`text`/`icon` `set_ui_scale`) and the touch layout (`engine::touch_controls::TouchLayout::with_density`,
//! with a physical-mm touch-target floor) — never through here.

use gonedark_pal::ThermalState;

/// A render quality tier, selected per device class. **Render-only** — never a sim input
/// (invariant #1/#4). `Low`/`Mid`/`High` map to concrete [`TierParams`]; `High` is the flagship
/// profile Phase 1 validated on (D22), `Low`/`Mid` retire that flagship-only caveat for mid-range
/// arm64 (phase-4-plan WS-C goal).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum QualityTier {
    /// Budget silicon: lowest resolution floor, shortest draw distance, fewest effects/instances.
    Low,
    /// Mid-range arm64 — the tier the Phase 4 tuning targets.
    Mid,
    /// Flagship (the S24/Adreno 750 class D22 validated): full resolution, longest draw distance.
    High,
}

impl QualityTier {
    /// The concrete render budget for this tier. Pure lookup — the single source of truth the
    /// Settings surface and the renderer both read.
    pub fn params(self) -> TierParams {
        match self {
            QualityTier::Low => TierParams {
                tier: self,
                res_scale_floor: 0.50,
                res_scale_ceiling: 0.85,
                draw_distance: 60.0,
                effect_density: 0.4,
                instance_budget: 120,
            },
            QualityTier::Mid => TierParams {
                tier: self,
                res_scale_floor: 0.65,
                res_scale_ceiling: 1.0,
                draw_distance: 110.0,
                effect_density: 0.7,
                instance_budget: 200,
            },
            QualityTier::High => TierParams {
                tier: self,
                res_scale_floor: 0.80,
                res_scale_ceiling: 1.0,
                draw_distance: 180.0,
                effect_density: 1.0,
                instance_budget: 400,
            },
        }
    }
}

/// The concrete render budget a [`QualityTier`] maps to. **All render-only knobs** (floats and a
/// draw-count budget) — none of these is a sim input, so varying them leaves the checksum stream
/// untouched (invariant #1/#4). The dyn-res scale is clamped to `[res_scale_floor,
/// res_scale_ceiling]`; the rest parameterize the renderer's cost.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TierParams {
    /// Which tier produced these params (echoed back for callers that pass params around).
    pub tier: QualityTier,
    /// Lowest dynamic-resolution scale this tier will drop to (`(0,1]`). The dyn-res controller
    /// never scales below this, so quality has a tier-defined floor.
    pub res_scale_floor: f32,
    /// Highest dynamic-resolution scale (`<= 1.0`): native (`1.0`) on Mid/High, sub-native on Low.
    pub res_scale_ceiling: f32,
    /// Draw distance in world units — how far the renderer draws before culling. Render-only.
    pub draw_distance: f32,
    /// Effect density in `[0,1]` (particles/decals/etc.) — a cost dial, render-only.
    pub effect_density: f32,
    /// Soft cap on instances drawn per frame (the 200-unit power-budget knob, D22's deferral).
    /// Render-side culling target — NOT a sim entity cap (the sim simulates all units regardless).
    pub instance_budget: u32,
}

/// How aggressively [`next_resolution_scale`] reacts to a frame over/under budget. A fraction of
/// the headroom is corrected each step so the scale eases rather than oscillating frame-to-frame.
const DYNRES_ADJUST_GAIN: f32 = 0.5;

/// Dead-band around the budget (fraction): if the recent average frame time is within this much of
/// the target we hold the scale, so we don't hunt on noise.
const DYNRES_DEADBAND: f32 = 0.05;

/// Decide the next dynamic-resolution scale (presentation-only, invariant #4): given the recent
/// frame times (seconds), the frame-time `budget_secs` (e.g. `1/60`), the `current` scale, and the
/// `params` whose `[floor, ceiling]` clamp the result, return the new render-target scale.
///
/// Pure: reads frame timing only, never sim state. The rule:
///  - average the recent frames (empty slice → hold `current`, clamped into range);
///  - if we're **over** budget (too slow), scale *down* toward the floor proportional to the
///    overage; if comfortably **under** (faster than budget by more than the dead-band), scale
///    *up* toward the ceiling; within the dead-band, hold;
///  - always clamp to `[floor, ceiling]` so quality never leaves the tier's band.
///
/// Lowering the scale shrinks the render target → fewer fragments → faster frames, holding the
/// frame budget without ever touching the sim tick.
pub fn next_resolution_scale(
    recent_frame_times: &[f32],
    budget_secs: f32,
    current: f32,
    params: &TierParams,
) -> f32 {
    let (floor, ceiling) = (params.res_scale_floor, params.res_scale_ceiling);
    // Degenerate budget or no samples → nothing to pace against; hold, clamped into the band.
    // (`budget_secs.is_finite() && budget_secs > 0.0` also rejects NaN, which `> 0.0` alone lets
    // through with a confusing double-negative.)
    if !budget_secs.is_finite() || budget_secs <= 0.0 || recent_frame_times.is_empty() {
        return current.clamp(floor, ceiling);
    }

    let avg = recent_frame_times.iter().copied().sum::<f32>() / recent_frame_times.len() as f32;
    // Positive = over budget (too slow); negative = under (headroom to spare).
    let error = (avg - budget_secs) / budget_secs;

    let next = if error > DYNRES_DEADBAND {
        // Over budget: ease the scale down proportional to the overage.
        current - error * DYNRES_ADJUST_GAIN * current
    } else if error < -DYNRES_DEADBAND {
        // Under budget with margin: ease the scale up toward native.
        current + (-error) * DYNRES_ADJUST_GAIN * current
    } else {
        current
    };

    next.clamp(floor, ceiling)
}

/// The render-cost backoff a thermal/power state forces — an output of [`thermal_backoff`]. Both
/// fields are presentation knobs (invariant #2: the thermal *signal* came through the PAL, and the
/// *response* is render-only): cap the present rate and tighten the dyn-res floor so the GPU does
/// less work and cools. Neither touches the sim — the 60 Hz tick is unchanged.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Backoff {
    /// Frames-per-second cap to pace presentation to. The sim still ticks at 60 Hz; this only
    /// throttles how often we *draw/present*. `None` = no cap (present as fast as the surface).
    pub fps_cap: Option<u32>,
    /// The dyn-res floor to use *while under this thermal pressure*. It only ever **tightens
    /// downward** as heat rises — every thermal state yields a floor `<= the tier floor` (equal at
    /// `Nominal`/`Fair`, strictly below at `Serious`/`Critical`) — so the controller can shed more
    /// fragment cost under heat. `next_resolution_scale` should be fed `min(tier.floor, this)` as
    /// its effective floor, letting heat push quality below the comfortable tier floor.
    pub res_scale_floor: f32,
}

/// Map a thermal/power state (read through the PAL — invariant #2) + the active tier onto the
/// render backoff to apply. Pure + host-testable: the policy is here; the PAL only *reports* the
/// state, and the engine only *applies* the result.
///
/// Policy (more heat → less render cost):
///  - [`ThermalState::Nominal`]: no cap, the tier's own floor — full freedom.
///  - [`ThermalState::Fair`]: cap to 60 (don't render faster than the sim ticks — wasted heat),
///    hold the tier floor.
///  - [`ThermalState::Serious`]: cap to 45 and allow dropping below the tier floor (a touch) to
///    shed fragment cost.
///  - [`ThermalState::Critical`]: cap to 30 and drop the floor hard — survival mode; keep the game
///    running and cool rather than pretty. (If 200 units at 60 Hz forces Critical on mid-range
///    silicon, that is the on-device datum that reopens D21 dual-rate — recorded via `/decision`,
///    per phase-4-plan WS-C step 3. This fn does NOT change the tick; it only backs off render.)
pub fn thermal_backoff(state: ThermalState, params: &TierParams) -> Backoff {
    let tier_floor = params.res_scale_floor;
    match state {
        ThermalState::Nominal => Backoff {
            fps_cap: None,
            res_scale_floor: tier_floor,
        },
        ThermalState::Fair => Backoff {
            fps_cap: Some(60),
            res_scale_floor: tier_floor,
        },
        ThermalState::Serious => Backoff {
            fps_cap: Some(45),
            // Allow a small dip below the tier floor under sustained heat.
            res_scale_floor: (tier_floor - 0.10).max(0.40),
        },
        ThermalState::Critical => Backoff {
            fps_cap: Some(30),
            // Survival: shed as much fragment cost as the clamp allows.
            res_scale_floor: 0.40,
        },
    }
}

#[cfg(test)]
mod tests {
    //! `tiers` is render-side (the float boundary, invariant #1), so `f32` math + epsilon
    //! comparisons are fair game — these exercise pure decision logic, never the GPU and never
    //! the sim. Table-driven where it pays.

    use super::*;

    const EPS: f32 = 1e-5;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < EPS
    }

    // ---- tier → params mapping ----

    #[test]
    fn tier_params_are_distinct_and_ordered() {
        let lo = QualityTier::Low.params();
        let mid = QualityTier::Mid.params();
        let hi = QualityTier::High.params();
        // Each tier echoes its own enum.
        assert_eq!(lo.tier, QualityTier::Low);
        assert_eq!(mid.tier, QualityTier::Mid);
        assert_eq!(hi.tier, QualityTier::High);
        // Monotonic in the budget knobs: higher tier draws further, denser, more instances.
        assert!(lo.draw_distance < mid.draw_distance && mid.draw_distance < hi.draw_distance);
        assert!(lo.effect_density < mid.effect_density && mid.effect_density <= hi.effect_density);
        assert!(lo.instance_budget < mid.instance_budget && mid.instance_budget < hi.instance_budget);
        // Floors rise with the tier; ceilings never exceed native.
        assert!(lo.res_scale_floor < mid.res_scale_floor && mid.res_scale_floor < hi.res_scale_floor);
        for p in [lo, mid, hi] {
            assert!(p.res_scale_floor > 0.0 && p.res_scale_floor <= p.res_scale_ceiling);
            assert!(p.res_scale_ceiling <= 1.0);
        }
    }

    #[test]
    fn mid_tier_carries_the_200_unit_budget() {
        // The 200-unit power budget D22 deferred is the Mid-tier instance budget.
        assert_eq!(QualityTier::Mid.params().instance_budget, 200);
    }

    // ---- dynamic resolution ----

    #[test]
    fn dynres_over_budget_scales_down() {
        let p = QualityTier::Mid.params();
        // 20 ms frames against a 16.67 ms budget → too slow → scale drops.
        let next = next_resolution_scale(&[0.020, 0.020, 0.020], 1.0 / 60.0, 1.0, &p);
        assert!(next < 1.0, "over budget must scale down, got {next}");
        assert!(next >= p.res_scale_floor, "never below the floor");
    }

    #[test]
    fn dynres_under_budget_scales_up() {
        let p = QualityTier::Mid.params();
        // 8 ms frames against 16.67 ms → headroom → scale rises toward the ceiling.
        let next = next_resolution_scale(&[0.008, 0.008], 1.0 / 60.0, 0.8, &p);
        assert!(next > 0.8, "under budget must scale up, got {next}");
        assert!(next <= p.res_scale_ceiling, "never above the ceiling");
    }

    #[test]
    fn dynres_within_deadband_holds() {
        let p = QualityTier::Mid.params();
        let budget = 1.0 / 60.0;
        // Exactly on budget → hold.
        let next = next_resolution_scale(&[budget, budget], budget, 0.9, &p);
        assert!(approx(next, 0.9), "on-budget holds, got {next}");
    }

    #[test]
    fn dynres_clamps_to_floor_and_ceiling() {
        let p = QualityTier::Low.params(); // floor 0.50, ceiling 0.85
        // Wildly over budget → clamp to floor, not below.
        let down = next_resolution_scale(&[0.100, 0.100], 1.0 / 60.0, 0.55, &p);
        assert!(approx(down, p.res_scale_floor), "clamps to floor, got {down}");
        // Wildly under budget from the ceiling → clamp to ceiling, not above.
        let up = next_resolution_scale(&[0.001], 1.0 / 60.0, 0.85, &p);
        assert!(approx(up, p.res_scale_ceiling), "clamps to ceiling, got {up}");
    }

    #[test]
    fn dynres_empty_or_degenerate_holds_clamped() {
        let p = QualityTier::High.params(); // floor 0.80
        // No samples → hold current, clamped into the band.
        assert!(approx(
            next_resolution_scale(&[], 1.0 / 60.0, 0.5, &p),
            p.res_scale_floor
        ));
        // Non-positive budget → hold, clamped.
        assert!(approx(
            next_resolution_scale(&[0.016], 0.0, 0.9, &p),
            0.9
        ));
    }

    #[test]
    fn dynres_is_monotone_toward_budget() {
        // Repeatedly applying the controller to over-budget frames is monotone non-increasing and
        // converges to the floor — no oscillation past it.
        let p = QualityTier::Mid.params();
        let mut scale = 1.0;
        for _ in 0..50 {
            let next = next_resolution_scale(&[0.030], 1.0 / 60.0, scale, &p);
            assert!(next <= scale + EPS, "never rises while over budget");
            scale = next;
        }
        assert!(approx(scale, p.res_scale_floor), "converges to floor");
    }

    // ---- thermal backoff ----

    #[test]
    fn thermal_backoff_tightens_with_heat() {
        let p = QualityTier::High.params();
        let nominal = thermal_backoff(ThermalState::Nominal, &p);
        let fair = thermal_backoff(ThermalState::Fair, &p);
        let serious = thermal_backoff(ThermalState::Serious, &p);
        let critical = thermal_backoff(ThermalState::Critical, &p);

        assert_eq!(nominal.fps_cap, None, "nominal is uncapped");
        // The cap tightens monotonically as heat rises.
        assert_eq!(fair.fps_cap, Some(60));
        assert_eq!(serious.fps_cap, Some(45));
        assert_eq!(critical.fps_cap, Some(30));

        // The floor only ever drops (sheds more cost) as heat rises — never rises.
        assert!(fair.res_scale_floor <= nominal.res_scale_floor + EPS);
        assert!(serious.res_scale_floor <= fair.res_scale_floor + EPS);
        assert!(critical.res_scale_floor <= serious.res_scale_floor + EPS);
    }

    #[test]
    fn thermal_backoff_nominal_keeps_tier_floor() {
        for t in [QualityTier::Low, QualityTier::Mid, QualityTier::High] {
            let p = t.params();
            let b = thermal_backoff(ThermalState::Nominal, &p);
            assert!(approx(b.res_scale_floor, p.res_scale_floor));
            assert_eq!(b.fps_cap, None);
        }
    }

    #[test]
    fn thermal_backoff_critical_is_survival() {
        let p = QualityTier::High.params(); // tier floor 0.80
        let b = thermal_backoff(ThermalState::Critical, &p);
        // Critical drops the floor well below the comfy High floor to shed fragment cost.
        assert!(b.res_scale_floor < p.res_scale_floor);
        assert_eq!(b.fps_cap, Some(30));
    }
}
