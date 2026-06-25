//! Render quality tuning controller (Phase 4 WS-C) — the engine-side state that owns the active
//! [`QualityTier`], the running dynamic-resolution scale, and the thermal backoff, driving the pure
//! `render::tiers` policy fns each frame.
//!
//! **Everything here is a RENDERING choice (invariant #1/#4).** The controller reads only frame
//! timing (a host wall-clock `f32`, fine in this crate) and a [`ThermalState`] *reported through the
//! PAL* (invariant #2 — the signal crosses the platform seam, never `core`). It NEVER reads or
//! mutates sim state and NEVER changes `core::sim::TICK_HZ`: the sim ticks at the same fixed 60 Hz
//! whatever tier/scale/cap this picks, so the per-tick checksum stream is byte-identical at every
//! tier (the guard test `tier_choice_is_sim_independent` asserts it). The renderer's only job is to
//! draw its target at [`RenderTuning::resolution_scale`]; that GPU wiring is the thin glue left to
//! the backends, the decisions are all pure and tested here / in `render::tiers`.

use std::collections::VecDeque;

use gonedark_pal::ThermalState;
use gonedark_render::tiers::{next_resolution_scale, thermal_backoff, Backoff, QualityTier};

/// How many recent frame times the dyn-res controller averages over. A short window so it reacts to
/// a sustained over/under-budget trend without chasing single-frame spikes.
const FRAME_HISTORY: usize = 8;

/// The render quality-tuning controller. Construct with the device-class tier (the Settings
/// "graphics tiers" surface, surface 3, will set this); call [`RenderTuning::observe_frame`] once
/// per presented frame with the frame `dt` and the current thermal state; read
/// [`RenderTuning::resolution_scale`] / [`RenderTuning::fps_cap`] to drive the render target size +
/// frame pacing.
#[derive(Clone, Debug)]
pub struct RenderTuning {
    tier: QualityTier,
    /// Current dynamic-resolution scale in `(0, 1]` — the render target is drawn at this fraction
    /// of native then upscaled to the swapchain. Render-only (invariant #4).
    scale: f32,
    /// The latest thermal backoff (FPS cap + effective floor) computed from the reported state.
    backoff: Backoff,
    /// Sliding window of recent frame times (seconds); the dyn-res controller averages it. A
    /// `VecDeque` so the bounded window is O(1) to push_back/pop_front (no O(n) front-shift).
    recent: VecDeque<f32>,
}

impl RenderTuning {
    /// Build a controller for `tier`, starting at the tier's resolution ceiling (full quality until
    /// frame timing or heat forces it down).
    pub fn new(tier: QualityTier) -> Self {
        let params = tier.params();
        RenderTuning {
            tier,
            scale: params.res_scale_ceiling,
            backoff: thermal_backoff(ThermalState::Nominal, &params),
            recent: VecDeque::with_capacity(FRAME_HISTORY),
        }
    }

    /// The active quality tier (Settings reads/writes this).
    pub fn tier(&self) -> QualityTier {
        self.tier
    }

    /// Switch tiers (Settings "graphics tiers" surface). Re-clamps the running scale into the new
    /// tier's band so a downgrade takes effect immediately. Render-only — no sim effect.
    pub fn set_tier(&mut self, tier: QualityTier) {
        self.tier = tier;
        let params = tier.params();
        self.scale = self
            .scale
            .clamp(params.res_scale_floor, params.res_scale_ceiling);
    }

    /// The current dynamic-resolution scale to draw the render target at (`(0,1]`).
    pub fn resolution_scale(&self) -> f32 {
        self.scale
    }

    /// The current FPS cap presentation should pace to (`None` = uncapped). Driven by thermal
    /// backoff — the sim tick is unaffected (invariant #1/#4).
    pub fn fps_cap(&self) -> Option<u32> {
        self.backoff.fps_cap
    }

    /// The render-target pixel dimensions for this frame: the swapchain size scaled by
    /// [`resolution_scale`](Self::resolution_scale), each axis clamped to at least 1. Pure helper
    /// the backend's GPU glue uses to size its intermediate target.
    pub fn scaled_target(&self, width: u32, height: u32) -> (u32, u32) {
        let s = self.scale;
        let w = ((width as f32 * s).round() as u32).max(1);
        let h = ((height as f32 * s).round() as u32).max(1);
        (w, h)
    }

    /// Observe one presented frame: record its `dt`, recompute the thermal backoff from `thermal`,
    /// and ease the dyn-res scale toward the frame budget — clamped to the tier band *and* the
    /// thermal-tightened floor. Returns the new resolution scale. Pure w.r.t. the sim: it touches
    /// only `self` and reads `dt`/`thermal`, never the world.
    ///
    /// `budget_secs` is the per-frame budget to hold (`1/60` for the 60 Hz baseline; a host may
    /// pass `1/cap` when a thermal FPS cap is active so dyn-res paces to the *capped* rate).
    pub fn observe_frame(&mut self, dt: f32, thermal: ThermalState, budget_secs: f32) -> f32 {
        if self.recent.len() == FRAME_HISTORY {
            self.recent.pop_front();
        }
        // Ignore a non-finite / non-positive dt (first frame, a stall) — it would poison the average.
        if dt.is_finite() && dt > 0.0 {
            self.recent.push_back(dt);
        }

        let params = self.tier.params();
        self.backoff = thermal_backoff(thermal, &params);

        // The effective floor is the *tighter* of the tier floor and the thermal-tightened floor,
        // so heat can push quality below the comfortable tier floor. The ceiling is the tier's.
        let effective_floor = self.backoff.res_scale_floor.min(params.res_scale_floor);
        let mut effective = params;
        effective.res_scale_floor = effective_floor;

        // `next_resolution_scale` takes a `&[f32]`; the window only ever pushes/pops by one, so it
        // stays contiguous and `make_contiguous` is O(1) here.
        self.scale =
            next_resolution_scale(self.recent.make_contiguous(), budget_secs, self.scale, &effective);
        self.scale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-5;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < EPS
    }

    #[test]
    fn new_starts_at_tier_ceiling() {
        let t = RenderTuning::new(QualityTier::Low);
        assert!(approx(
            t.resolution_scale(),
            QualityTier::Low.params().res_scale_ceiling
        ));
        assert_eq!(t.fps_cap(), None, "nominal start is uncapped");
        assert_eq!(t.tier(), QualityTier::Low);
    }

    #[test]
    fn sustained_over_budget_drops_scale_toward_floor() {
        let mut t = RenderTuning::new(QualityTier::Mid);
        let budget = 1.0 / 60.0;
        let start = t.resolution_scale();
        // Feed many 25 ms (40 fps) frames at nominal heat → scale eases down.
        for _ in 0..30 {
            t.observe_frame(0.025, ThermalState::Nominal, budget);
        }
        assert!(t.resolution_scale() < start, "over budget must drop scale");
        assert!(
            t.resolution_scale() >= QualityTier::Mid.params().res_scale_floor - EPS,
            "never below the (nominal) tier floor"
        );
    }

    #[test]
    fn sustained_under_budget_climbs_back_to_ceiling() {
        let mut t = RenderTuning::new(QualityTier::Mid);
        let budget = 1.0 / 60.0;
        // First drive it down…
        for _ in 0..30 {
            t.observe_frame(0.030, ThermalState::Nominal, budget);
        }
        assert!(t.resolution_scale() < QualityTier::Mid.params().res_scale_ceiling);
        // …then feed fast 5 ms frames → it climbs back to the ceiling.
        for _ in 0..40 {
            t.observe_frame(0.005, ThermalState::Nominal, budget);
        }
        assert!(approx(
            t.resolution_scale(),
            QualityTier::Mid.params().res_scale_ceiling
        ));
    }

    #[test]
    fn thermal_critical_caps_fps_and_lets_scale_drop_below_tier_floor() {
        let mut t = RenderTuning::new(QualityTier::High);
        let tier_floor = QualityTier::High.params().res_scale_floor; // 0.80
        // Critical heat + over-budget frames: the cap appears and the scale is allowed below the
        // comfortable High floor (survival).
        for _ in 0..40 {
            t.observe_frame(0.040, ThermalState::Critical, 1.0 / 30.0);
        }
        assert_eq!(t.fps_cap(), Some(30));
        assert!(
            t.resolution_scale() < tier_floor,
            "critical heat may drop below the tier floor, got {}",
            t.resolution_scale()
        );
    }

    #[test]
    fn nominal_heat_never_caps_and_respects_tier_floor() {
        let mut t = RenderTuning::new(QualityTier::High);
        for _ in 0..50 {
            t.observe_frame(0.050, ThermalState::Nominal, 1.0 / 60.0);
        }
        assert_eq!(t.fps_cap(), None);
        assert!(t.resolution_scale() >= QualityTier::High.params().res_scale_floor - EPS);
    }

    #[test]
    fn set_tier_reclamps_running_scale() {
        let mut t = RenderTuning::new(QualityTier::High); // scale starts 1.0
        // Drop to Low (ceiling 0.85): the running scale must re-clamp into Low's band at once.
        t.set_tier(QualityTier::Low);
        assert_eq!(t.tier(), QualityTier::Low);
        assert!(t.resolution_scale() <= QualityTier::Low.params().res_scale_ceiling + EPS);
    }

    #[test]
    fn scaled_target_scales_and_clamps_to_one() {
        let mut t = RenderTuning::new(QualityTier::Mid);
        // At full ceiling (1.0) the target equals the swapchain.
        assert_eq!(t.scaled_target(1920, 1080), (1920, 1080));
        // Drive the scale down and confirm the target shrinks but never to 0.
        for _ in 0..40 {
            t.observe_frame(0.050, ThermalState::Critical, 1.0 / 30.0);
        }
        let (w, h) = t.scaled_target(1920, 1080);
        assert!(w < 1920 && h < 1080 && w >= 1 && h >= 1);
        // A tiny swapchain never produces a zero-area target.
        assert_eq!(t.scaled_target(0, 0), (1, 1));
    }

    #[test]
    fn non_positive_dt_is_ignored() {
        let mut t = RenderTuning::new(QualityTier::Mid);
        let start = t.resolution_scale();
        // A zero / NaN dt (first frame, stall) must not move the scale.
        t.observe_frame(0.0, ThermalState::Nominal, 1.0 / 60.0);
        t.observe_frame(f32::NAN, ThermalState::Nominal, 1.0 / 60.0);
        assert!(approx(t.resolution_scale(), start));
    }
}
