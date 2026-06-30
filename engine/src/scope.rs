//! Embodied **sniper / zoom gun-sight** seam (tank embodiment P9) — the PURE, GPU-free math that
//! turns the held aim-down-sight input into a narrowed embodied-camera FOV (+ a magnification
//! readout), plus the input→zoom-intent mapping. It is the zoom twin of [`fire`](crate::fire) and
//! [`locomote`](crate::locomote): a free-function seam the host drives once per frame, unit-tested
//! without a GPU.
//!
//! **Presentation/input only (invariants #4, #5).** Nothing here writes sim/`core` state, nothing
//! is checksummed, and the zoom is *not* a character system — it is a camera FOV change + a screen-
//! space scope reticle (the renderer's [`gonedark_render::scope`] half). Floats are fine: this is
//! the render/input boundary, never the sim. The zoom adds **no** desync surface (invariant #7) —
//! `aim_zoom_t` lives on `Game` as host presentation state and can never reach `&mut Sim`.
//!
//! Invariant #6 ("world goes dark stays fair"): the scope is **avatar-only vision** — it narrows
//! the same first-person frustum the player already sees and surfaces NO strategic-map intel. A
//! narrower FOV reveals *less* of the world, never more.

/// The fully-scoped embodied FOV (degrees). The gun-sight narrows from the embodied base FOV
/// (`EMBODIED_FOV_DEG`, 60°) down to this when aiming down sight — a ~3× optical zoom (see
/// [`zoom_magnification`]). Narrower = more magnified, but never below a sane floor. This is the
/// **tank gun-sight** target (an independent turret with optics); infantry use the gentler
/// [`ADS_FOV_DEG`] instead.
pub const SCOPED_FOV_DEG: f32 = 20.0;

/// The fully-aimed embodied FOV (degrees) for **infantry** iron-sight ADS (WS-A, CP-2 game-feel
/// bar). A modest narrowing from the 60° base — a ~1.7× magnification (vs the tank's ~3.3× scope) —
/// so aiming down sight steadies + tightens the shot without the disorienting tunnel of a sniper
/// scope. The host picks this target (not [`SCOPED_FOV_DEG`]) for a possessed rifleman, and skips
/// the [`crate::scope`]-overlay scope chrome (that stays tank-only). Same fair narrowing (invariant
/// #6: a narrower FOV reveals *less* of the world).
pub const ADS_FOV_DEG: f32 = 42.0;

/// Floor on the ADS look-sensitivity multiplier ([`ads_look_scale`]): even fully zoomed the look
/// never drops below 45% of hip sensitivity, so aiming down sight steadies the aim without ever
/// feeling glued/unresponsive on a phone-sized viewport. Tuned for the snappy-but-controllable feel
/// the WS-A floor calls for.
pub const ADS_SENS_FLOOR: f32 = 0.45;

/// Zoom interpolation rate (units of `t` per second): how fast the FOV eases between hip and full
/// ADS. `8.0` → a full transition in ~0.125 s, snappy but not instant (so the scope eases in, not
/// pops). Drives [`step_zoom_t`].
pub const ZOOM_RATE: f32 = 8.0;

/// Below this `aim_zoom_t` the scope overlay is not worth drawing (the FOV is essentially un-zoomed).
/// The host gates the `render::scope` pass on `aim_zoom_t > SCOPE_VISIBLE_T`.
pub const SCOPE_VISIBLE_T: f32 = 0.02;

/// Whether aiming down sight engages this frame: only while **embodied**, in a unit that *can* aim
/// down sight (`can_ads`), and while the ADS input is **held**. A pure gate (the zoom twin of
/// [`fire::fire_command`]'s trigger gate), so the command view never zooms and a dead avatar is
/// inert. As of WS-A `can_ads` is true for **any** living embodied unit (infantry iron-sight ADS to
/// [`ADS_FOV_DEG`] *and* the tank gun-sight to [`SCOPED_FOV_DEG`]) — the host chooses the target FOV
/// + whether to draw the scope-overlay chrome (tank only) from the possessed unit's kind. Holding
/// ADS while not embodied is inert.
#[inline]
pub fn zoom_active(embodied: bool, can_ads: bool, aim_held: bool) -> bool {
    embodied && can_ads && aim_held
}

/// Advance the zoom interpolation toward full ADS (`active`) or back to the hip (`!active`) by one
/// frame of `dt` seconds at `rate` units of `t`/second. Returns the next `aim_zoom_t`, **clamped to
/// `[0, 1]`** and **monotone per call** — strictly non-decreasing while `active`, non-increasing
/// otherwise (a negative/zero `dt` is a no-op, never a reversal). `t = 0` is hip (base FOV), `t = 1`
/// is full ADS (`SCOPED_FOV_DEG`). Pure → host-tested.
#[inline]
pub fn step_zoom_t(current: f32, active: bool, dt: f32, rate: f32) -> f32 {
    let step = (rate * dt).max(0.0);
    let next = if active { current + step } else { current - step };
    next.clamp(0.0, 1.0)
}

/// The embodied camera FOV (degrees) for a given `aim_zoom_t`: linearly interpolate from `base_deg`
/// (hip, `t = 0`) to `scoped_deg` (full ADS, `t = 1`). The result is **clamped to the
/// `[scoped, base]` interval** (with `scoped <= base` by design) so a stray out-of-range `t` can
/// never widen the frustum past the base FOV (which would reveal more world — invariant #6) nor
/// narrow it past the scope floor. Monotone in `t`. Pure → host-tested.
#[inline]
pub fn zoom_fov_deg(base_deg: f32, scoped_deg: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let fov = base_deg + (scoped_deg - base_deg) * t;
    let (lo, hi) = if scoped_deg <= base_deg {
        (scoped_deg, base_deg)
    } else {
        (base_deg, scoped_deg)
    };
    fov.clamp(lo, hi)
}

/// The optical magnification of the current FOV relative to the un-zoomed `base_deg`: the ratio of
/// the half-angle tangents, `tan(base/2) / tan(fov/2)`. `1.0×` at hip (`fov == base`), growing as
/// the FOV narrows (e.g. 60° → 20° ≈ `3.27×`). Used only for the HUD's magnification readout. FOVs
/// are clamped to a sane `(0°, 179°)` so the tangent never blows up. Floored at `1.0×` (the scope
/// never zooms *out*). Pure → host-tested.
#[inline]
pub fn zoom_magnification(base_deg: f32, fov_deg: f32) -> f32 {
    let half_tan = |d: f32| (d.clamp(1.0e-3, 179.0) * 0.5).to_radians().tan();
    (half_tan(base_deg) / half_tan(fov_deg)).max(1.0)
}

/// The look-sensitivity multiplier while aiming down sight at interpolation `t` and optical
/// `magnification` (WS-A, CP-2). Eases from `1.0` at hip (`t = 0`) toward `1 / magnification` at full
/// ADS (`t = 1`) — the standard "match the angular travel" ramp that keeps the mouse/stick feeling
/// consistent as the FOV narrows — but **floored at [`ADS_SENS_FLOOR`]** so the aim never feels
/// glued even at a high tank magnification. The host multiplies its look deltas by this each frame.
/// PRESENTATION/INPUT ONLY (invariant #4): it scales the host-side yaw/pitch integration, never the
/// sim. Monotone non-increasing in `t`; clamped so an out-of-range `t`/`magnification` stays sane.
/// Pure → host-tested.
#[inline]
pub fn ads_look_scale(t: f32, magnification: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    // The fully-zoomed target multiplier (never below the responsiveness floor).
    let target = (1.0 / magnification.max(1.0)).max(ADS_SENS_FLOOR);
    // Lerp hip (1.0) → target by t.
    1.0 + (target - 1.0) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embodied base FOV the host pairs with [`SCOPED_FOV_DEG`] (kept in sync with
    /// `lib::EMBODIED_FOV_DEG`; duplicated here only so the seam tests are self-contained).
    const BASE: f32 = 60.0;

    // ---- input → zoom intent gate ----

    #[test]
    fn zoom_engages_only_embodied_scoped_and_held() {
        assert!(zoom_active(true, true, true), "embodied tank holding ADS zooms");
        assert!(!zoom_active(false, true, true), "command view never zooms");
        assert!(!zoom_active(true, false, true), "a scope-less unit never zooms");
        assert!(!zoom_active(true, true, false), "released ADS does not zoom");
    }

    // ---- t interpolation: monotone + clamped ----

    #[test]
    fn step_eases_in_when_active_and_clamps_at_one() {
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        let mut prev = -1.0;
        for _ in 0..240 {
            let next = step_zoom_t(t, true, dt, ZOOM_RATE);
            assert!(next >= prev - 1e-9, "monotone non-decreasing while active");
            assert!((0.0..=1.0).contains(&next), "t stays in [0,1], got {next}");
            prev = next;
            t = next;
        }
        assert!((t - 1.0).abs() < 1e-6, "reaches and saturates at full ADS");
        // Already at 1.0 → stays at 1.0 (clamped, never overshoots).
        assert_eq!(step_zoom_t(1.0, true, dt, ZOOM_RATE), 1.0);
    }

    #[test]
    fn step_eases_out_when_inactive_and_clamps_at_zero() {
        let dt = 1.0 / 60.0;
        let mut t = 1.0;
        let mut prev = 2.0;
        for _ in 0..240 {
            let next = step_zoom_t(t, false, dt, ZOOM_RATE);
            assert!(next <= prev + 1e-9, "monotone non-increasing while inactive");
            assert!((0.0..=1.0).contains(&next), "t stays in [0,1], got {next}");
            prev = next;
            t = next;
        }
        assert!(t.abs() < 1e-6, "returns to hip (0)");
        assert_eq!(step_zoom_t(0.0, false, dt, ZOOM_RATE), 0.0);
    }

    #[test]
    fn step_is_a_noop_for_nonpositive_dt() {
        // A zero or negative wall-clock dt never moves (or reverses) the zoom.
        assert_eq!(step_zoom_t(0.5, true, 0.0, ZOOM_RATE), 0.5);
        assert_eq!(step_zoom_t(0.5, true, -0.1, ZOOM_RATE), 0.5);
        assert_eq!(step_zoom_t(0.5, false, -0.1, ZOOM_RATE), 0.5);
    }

    // ---- FOV mapping: narrows on, restores off, clamped ----

    #[test]
    fn fov_is_base_at_hip_and_scoped_at_full_ads() {
        assert!((zoom_fov_deg(BASE, SCOPED_FOV_DEG, 0.0) - BASE).abs() < 1e-6, "t=0 → base FOV");
        assert!(
            (zoom_fov_deg(BASE, SCOPED_FOV_DEG, 1.0) - SCOPED_FOV_DEG).abs() < 1e-6,
            "t=1 → scoped FOV (narrowed)"
        );
    }

    #[test]
    fn fov_narrows_monotonically_with_t() {
        let mut prev = BASE + 1.0;
        for i in 0..=20 {
            let t = i as f32 / 20.0;
            let fov = zoom_fov_deg(BASE, SCOPED_FOV_DEG, t);
            assert!(fov <= prev + 1e-6, "FOV is non-increasing in t: {fov} !<= {prev}");
            assert!(
                (SCOPED_FOV_DEG..=BASE).contains(&fov),
                "FOV {fov} escaped [{SCOPED_FOV_DEG}, {BASE}]"
            );
            prev = fov;
        }
    }

    #[test]
    fn fov_clamps_out_of_range_t() {
        // A stray t below 0 or above 1 can never widen past base nor narrow past the scope floor.
        assert!((zoom_fov_deg(BASE, SCOPED_FOV_DEG, -5.0) - BASE).abs() < 1e-6);
        assert!((zoom_fov_deg(BASE, SCOPED_FOV_DEG, 5.0) - SCOPED_FOV_DEG).abs() < 1e-6);
    }

    // ---- magnification readout ----

    #[test]
    fn magnification_is_unity_at_hip_and_grows_when_scoped() {
        assert!((zoom_magnification(BASE, BASE) - 1.0).abs() < 1e-6, "hip is 1.0×");
        let scoped = zoom_magnification(BASE, SCOPED_FOV_DEG);
        assert!(scoped > 1.0, "scoped magnifies, got {scoped}×");
        // 60° → 20° is ~3.27× (tan30 / tan10).
        assert!((scoped - 3.27).abs() < 0.1, "≈3.27× from 60°→20°, got {scoped}×");
        // Narrower FOV ⇒ strictly more magnification.
        assert!(
            zoom_magnification(BASE, 10.0) > scoped,
            "a narrower FOV magnifies more"
        );
    }

    #[test]
    fn magnification_never_zooms_out() {
        // A wider-than-base FOV would compute < 1×; the floor keeps the readout at 1.0× (the scope
        // never de-magnifies the world).
        assert!((zoom_magnification(BASE, 90.0) - 1.0).abs() < 1e-6);
    }

    // ---- ADS look-sensitivity ramp (WS-A) ----

    #[test]
    fn ads_look_scale_is_unity_at_hip_and_drops_when_scoped() {
        // The infantry ADS magnification (60°→42° ≈ 1.7×).
        let mag = zoom_magnification(BASE, ADS_FOV_DEG);
        assert!((ads_look_scale(0.0, mag) - 1.0).abs() < 1e-6, "hip is full sensitivity");
        let scoped = ads_look_scale(1.0, mag);
        assert!(scoped < 1.0, "ADS slows the look, got {scoped}");
        // ~1/1.7 ≈ 0.59 (above the floor), matched to the angular-travel ramp.
        assert!((scoped - 1.0 / mag).abs() < 1e-6, "full ADS scales to 1/mag at this magnification");
    }

    #[test]
    fn ads_look_scale_is_monotone_nonincreasing_in_t() {
        let mag = zoom_magnification(BASE, SCOPED_FOV_DEG); // the high tank magnification
        let mut prev = 2.0;
        for i in 0..=20 {
            let s = ads_look_scale(i as f32 / 20.0, mag);
            assert!(s <= prev + 1e-9, "non-increasing in t: {s} !<= {prev}");
            assert!((0.0..=1.0).contains(&s), "scale {s} out of (0,1]");
            prev = s;
        }
    }

    #[test]
    fn ads_look_scale_never_drops_below_the_floor() {
        // Even at an absurd magnification the look stays at least ADS_SENS_FLOOR responsive.
        assert!(
            (ads_look_scale(1.0, 100.0) - ADS_SENS_FLOOR).abs() < 1e-6,
            "fully zoomed at huge mag floors at ADS_SENS_FLOOR"
        );
        // Degenerate magnification (< 1) is clamped so the scale never exceeds 1.0 (never speeds up).
        assert!(ads_look_scale(1.0, 0.1) <= 1.0 + 1e-6, "ADS never speeds the look up");
    }
}
