//! Presentation-only unit animation floor (CP-3 / WS-B).
//!
//! This is the **floor** tier of the animation pass (`docs/plans/visual-design-plan.md` WS-B): a
//! coherent, *not-jarring* read of locomotion / firing / death on the greybox trooper — never UE5
//! parity, and never sim state. Everything here lives strictly on the render/float side of
//! [invariant #4](../../CLAUDE.md): it reads a *presentation* view of a unit (its speed this tick,
//! whether it is firing, whether it is alive — all copied into the render snapshot, never written
//! back) and returns pure float presentation data. No `core`/sim type is touched, nothing here
//! enters the checksum fold, and floats are fine (they are *forbidden* only in sim/core).
//!
//! Two seams, both pure + unit-tested off-GPU (mirroring [`crate::interpolate_instances`],
//! [`crate::token_icons`], [`crate::readout::readout_labels`]):
//!
//!  1. **Clip selection** — [`select_clip`] maps a small presentation [`AnimState`] to one of four
//!     [`AnimClip`]s. This is the load-bearing seam: the eventual skeletal-playback system slots in
//!     *behind* it (it will read the same [`AnimClip`] to pick a glTF animation track), so the
//!     classifier is stable even though today's runtime playback is procedural.
//!  2. **Procedural playback** — [`anim_pose`] samples a cheap per-instance [`AnimPose`] (a vertical
//!     bob, a forward pitch, a uniform scale) for a clip at a normalized `phase`, and
//!     [`pose_matrix`] folds that pose into the token's model matrix. This is the *stand-in* until a
//!     real rigid-part / skeletal player consumes the authored glTF clips
//!     (`tools/models/gen_trooper_rig.py`); it is deliberately subtle and applied **only to
//!     infantry** ([`is_infantry`]) so vehicles/structures render byte-identically to before.
//!
//! **Honest floor caveats.** Dead units are dropped from the render snapshot entirely
//! (`core::snapshot::Snapshot::capture` skips `!is_index_alive`), so at runtime the [`AnimClip::Death`]
//! branch is exercised by the selector + tests but is not *driven* to the screen — a visible death
//! topple needs cross-tick unit identity + a linger, which is the owed follow-up alongside real
//! skeletal skinning. The procedural pose is a placeholder for that skinning, not a substitute.

use crate::mesh::ModelKind;

/// Which locomotion / action clip a unit is playing. Presentation-derived (never sim state), stored
/// on [`crate::UnitInstance`] as a `u32` discriminant so the draw path can pick a pose without a
/// second snapshot read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimClip {
    /// Standing still and alive — a slow breathing bob.
    Idle,
    /// Moving over the ground faster than [`WALK_SPEED_THRESHOLD`] — a stride bob + slight lean.
    Walk,
    /// Discharging its weapon this tick — a recoil pitch (overrides locomotion).
    Fire,
    /// Not alive — a topple + sink (overrides everything).
    Death,
}

impl Default for AnimClip {
    fn default() -> Self {
        AnimClip::Idle
    }
}

impl AnimClip {
    /// Stable discriminant stored on [`crate::UnitInstance::anim_clip`]. Keep in lockstep with
    /// [`AnimClip::from_u32`].
    pub const fn as_u32(self) -> u32 {
        match self {
            AnimClip::Idle => 0,
            AnimClip::Walk => 1,
            AnimClip::Fire => 2,
            AnimClip::Death => 3,
        }
    }

    /// Inverse of [`AnimClip::as_u32`]. Any out-of-range value falls back to [`AnimClip::Idle`] (the
    /// `Default`), so a stale/zeroed field can never panic the draw path.
    pub fn from_u32(v: u32) -> AnimClip {
        match v {
            1 => AnimClip::Walk,
            2 => AnimClip::Fire,
            3 => AnimClip::Death,
            _ => AnimClip::Idle,
        }
    }
}

/// The presentation-visible inputs [`select_clip`] classifies from — all read off the interpolated
/// render snapshot at [`crate::interpolate_instances`], never written back to the sim.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimState {
    /// Ground speed this tick in world units/tick (magnitude of the unit's snapshot velocity).
    pub speed: f32,
    /// The unit fired within the muzzle-flash window (`core::snapshot::UnitSnapshot::firing`).
    pub firing: bool,
    /// The unit is alive (health fraction `> 0`). A dead unit plays [`AnimClip::Death`].
    pub alive: bool,
}

/// Ground speed (world units/tick) above which a unit reads as *walking* rather than *idling*. A
/// small dead-band so a unit jittering in place (numeric drift, being pushed) still reads as idle.
/// Presentation tuning only — not load-bearing, not a sim threshold.
pub const WALK_SPEED_THRESHOLD: f32 = 0.02;

/// Pick the clip for a unit's presentation state. **The load-bearing seam** — pure, total, and the
/// single place the priority order lives:
///
/// `Death` (not alive) **overrides** `Fire` (firing) **overrides** `Walk` (moving) **overrides**
/// `Idle`. So a unit that is firing while walking reads as firing; a unit that dies mid-stride reads
/// as dead. The eventual skeletal player reads this same [`AnimClip`] to select a glTF track.
pub fn select_clip(s: &AnimState) -> AnimClip {
    if !s.alive {
        return AnimClip::Death; // death overrides all
    }
    if s.firing {
        return AnimClip::Fire; // firing overrides locomotion
    }
    if s.speed > WALK_SPEED_THRESHOLD {
        AnimClip::Walk
    } else {
        AnimClip::Idle
    }
}

/// A cheap per-instance procedural pose — the *floor* stand-in for real skeletal playback. An
/// additive presentation transform folded into the token model matrix by [`pose_matrix`]:
/// a vertical `bob_z` (metres), a forward `pitch` (radians, about the body's side axis — positive
/// leans/topples toward the way it faces), and a uniform `scale_mul`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimPose {
    /// Metres added to the token's translation Z (bob / sink).
    pub bob_z: f32,
    /// Forward pitch in radians about the body's local side axis (lean / recoil / topple).
    pub pitch: f32,
    /// Uniform scale multiplier (1.0 = unchanged).
    pub scale_mul: f32,
}

impl AnimPose {
    /// The identity pose — [`pose_matrix`] with `REST` is byte-identical to
    /// [`crate::mesh::model_matrix`] (guaranteed by a unit test), so a unit at rest and every
    /// non-infantry token render exactly as before this pass.
    pub const REST: AnimPose = AnimPose {
        bob_z: 0.0,
        pitch: 0.0,
        scale_mul: 1.0,
    };
}

// --- Procedural pose tuning (all presentation floats, all subtle: "not jarring") -------------------
const IDLE_BOB: f32 = 0.015; // gentle breathing rise/fall (metres)
const IDLE_FREQ: f32 = 0.55; // slow breaths per phase cycle
const WALK_BOB: f32 = 0.07; // stride bob height (metres)
const WALK_FREQ: f32 = 2.2; // steps per phase cycle
const WALK_LEAN: f32 = 0.12; // steady forward lean while moving (radians)
const FIRE_KICK: f32 = 0.16; // peak recoil pitch back (radians)
const FIRE_BOB: f32 = 0.01; // slight settle down while firing (metres)
const DEATH_TOPPLE: f32 = std::f32::consts::FRAC_PI_2; // fully-fallen pitch (radians)
const DEATH_SINK: f32 = 0.4; // metres sunk into the ground when fallen
const DEATH_SHRINK: f32 = 0.08; // slight collapse in scale when fallen

/// Death progress in `[0, 1]` from the (unbounded, in "cycles") `phase`. Monotone and clamped so the
/// topple never overshoots. (At runtime death is not currently driven — see the module caveat — but
/// the curve is tested and ready for the linger the follow-up adds.)
fn death_progress(phase: f32) -> f32 {
    phase.clamp(0.0, 1.0)
}

/// Sample the procedural [`AnimPose`] for a `clip` at a normalized `phase` (in cycles; see
/// [`unit_phase`]). Pure + float-only, unit-tested off-GPU. Every branch is **bounded** (so the
/// stand-in can never fling a token off-screen) and reduces to something close to [`AnimPose::REST`]
/// at the neutral point.
pub fn anim_pose(clip: AnimClip, phase: f32) -> AnimPose {
    use std::f32::consts::TAU;
    match clip {
        AnimClip::Idle => AnimPose {
            bob_z: IDLE_BOB * (TAU * phase * IDLE_FREQ).sin(),
            pitch: 0.0,
            scale_mul: 1.0,
        },
        AnimClip::Walk => {
            // Two bobs per stride (feet strike): |sin| rides up on each step, never below 0.
            let step = (TAU * phase * WALK_FREQ).sin().abs();
            AnimPose {
                bob_z: WALK_BOB * step,
                pitch: WALK_LEAN,
                scale_mul: 1.0,
            }
        }
        AnimClip::Fire => {
            // A recoil pitch *back* (negative), pulsing with the fire cadence but always in
            // `[-FIRE_KICK, 0]` so the muzzle never dips forward.
            let pulse = 0.5 + 0.5 * (TAU * phase * WALK_FREQ).cos();
            AnimPose {
                bob_z: -FIRE_BOB,
                pitch: -FIRE_KICK * pulse,
                scale_mul: 1.0,
            }
        }
        AnimClip::Death => {
            let p = death_progress(phase);
            AnimPose {
                bob_z: -DEATH_SINK * p,
                pitch: DEATH_TOPPLE * p,
                scale_mul: 1.0 - DEATH_SHRINK * p,
            }
        }
    }
}

/// A continuous per-unit animation phase in *cycles* — the timeline [`anim_pose`] samples. Advances
/// with the sim clock (`tick` + inter-tick `alpha`) and is **staggered per unit** by its
/// `entity_index` so a rank of troopers doesn't bob in robotic lockstep. Unbounded on purpose
/// (`anim_pose` multiplies it by a per-clip frequency inside `sin`/`cos`, so there is no wrap seam).
/// Pure — unit-tested without a device.
pub fn unit_phase(tick: u64, alpha: f32, entity_index: u32) -> f32 {
    (tick as f32 + alpha) / ANIM_BASE_TICKS + entity_index as f32 * ANIM_STAGGER
}

/// Sim ticks per base phase cycle (at the locked 60 Hz tick, one cycle ≈ 1 s).
const ANIM_BASE_TICKS: f32 = 60.0;
/// Per-unit phase offset (cycles) applied by `entity_index` so units desync.
const ANIM_STAGGER: f32 = 0.11;

/// Only infantry troopers play the procedural locomotion floor — a bob/lean on a tank or a building
/// reads as a bug, so vehicles, structures, and scenery keep the plain [`crate::mesh::model_matrix`]
/// (byte-identical to before this pass). Faction trooper silhouettes (WS-C) animate too.
pub fn is_infantry(kind: ModelKind) -> bool {
    matches!(
        kind,
        ModelKind::Trooper | ModelKind::TrooperUs | ModelKind::TrooperFr
    )
}

/// Build a token's column-major model matrix with an [`AnimPose`] folded in: uniform `scale` ×
/// `pose.scale_mul`, a forward `pose.pitch` about the body's local side (+Y) axis, then `yaw` about
/// world up (+Z), then a translation with `pose.bob_z` added to Z. Pure scalar `f32` (no `glam`
/// dep, D19), so it is unit-testable and mirrors [`crate::mesh::model_matrix`] — with which it is
/// **byte-identical** when `pose == AnimPose::REST`.
///
/// Column-major `[[f32; 4]; 4]` (outer index = column) matching the host's `glam` convention, so the
/// mesh shader computes `view_proj * model * vec4(pos, 1)`.
pub fn pose_matrix(translation: [f32; 3], scale: f32, yaw: f32, pose: AnimPose) -> [[f32; 4]; 4] {
    let s = scale * pose.scale_mul;
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pose.pitch.sin_cos();
    // R = Rz(yaw) * Ry(pitch); columns are the images of the scaled local basis vectors.
    // Reduces to model_matrix's yaw-only columns when pitch == 0 (cp = 1, sp = 0).
    [
        [s * cy * cp, s * sy * cp, -s * sp, 0.0], // image of local +X (forward)
        [-s * sy, s * cy, 0.0, 0.0],              // image of local +Y (side)
        [s * cy * sp, s * sy * sp, s * cp, 0.0],  // image of local +Z (up)
        [
            translation[0],
            translation[1],
            translation[2] + pose.bob_z,
            1.0,
        ],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{model_matrix, ModelKind};

    const EPS: f32 = 1e-5;

    fn st(speed: f32, firing: bool, alive: bool) -> AnimState {
        AnimState {
            speed,
            firing,
            alive,
        }
    }

    // ---- select_clip: every branch + the priority order ----

    #[test]
    fn idle_when_still_alive_and_not_firing() {
        assert_eq!(select_clip(&st(0.0, false, true)), AnimClip::Idle);
        // Just at (not above) the walk threshold still reads as idle (the dead-band).
        assert_eq!(
            select_clip(&st(WALK_SPEED_THRESHOLD, false, true)),
            AnimClip::Idle
        );
    }

    #[test]
    fn walk_when_moving_above_threshold() {
        assert_eq!(
            select_clip(&st(WALK_SPEED_THRESHOLD + 0.01, false, true)),
            AnimClip::Walk
        );
        assert_eq!(select_clip(&st(5.0, false, true)), AnimClip::Walk);
    }

    #[test]
    fn fire_overrides_walk_and_idle() {
        // Firing while moving → Fire, not Walk.
        assert_eq!(select_clip(&st(5.0, true, true)), AnimClip::Fire);
        // Firing while still → Fire, not Idle.
        assert_eq!(select_clip(&st(0.0, true, true)), AnimClip::Fire);
    }

    #[test]
    fn death_overrides_everything() {
        assert_eq!(select_clip(&st(0.0, false, false)), AnimClip::Death);
        assert_eq!(select_clip(&st(5.0, false, false)), AnimClip::Death); // dead while moving
        assert_eq!(select_clip(&st(5.0, true, false)), AnimClip::Death); // dead while firing+moving
    }

    // ---- AnimClip discriminant round-trip ----

    #[test]
    fn clip_u32_round_trips_and_defaults_to_idle() {
        for c in [
            AnimClip::Idle,
            AnimClip::Walk,
            AnimClip::Fire,
            AnimClip::Death,
        ] {
            assert_eq!(AnimClip::from_u32(c.as_u32()), c);
        }
        assert_eq!(AnimClip::from_u32(999), AnimClip::Idle); // out-of-range → default
        assert_eq!(AnimClip::default(), AnimClip::Idle);
    }

    // ---- anim_pose: bounded + shaped per clip ----

    #[test]
    fn idle_pose_is_a_small_bounded_bob() {
        for i in 0..64 {
            let p = i as f32 * 0.05;
            let pose = anim_pose(AnimClip::Idle, p);
            assert!(pose.bob_z.abs() <= IDLE_BOB + EPS);
            assert_eq!(pose.pitch, 0.0);
            assert!((pose.scale_mul - 1.0).abs() < EPS);
        }
    }

    #[test]
    fn walk_pose_bobs_up_and_leans_forward() {
        let mut saw_bob = false;
        for i in 0..64 {
            let p = i as f32 * 0.05;
            let pose = anim_pose(AnimClip::Walk, p);
            assert!(pose.bob_z >= -EPS, "walk bob never dips below ground");
            assert!(pose.bob_z <= WALK_BOB + EPS);
            assert!((pose.pitch - WALK_LEAN).abs() < EPS, "steady forward lean");
            if pose.bob_z > 0.5 * WALK_BOB {
                saw_bob = true;
            }
        }
        assert!(saw_bob, "walk actually rises over the cycle");
    }

    #[test]
    fn fire_pose_recoils_back_and_stays_bounded() {
        for i in 0..64 {
            let p = i as f32 * 0.05;
            let pose = anim_pose(AnimClip::Fire, p);
            assert!(
                pose.pitch <= EPS && pose.pitch >= -FIRE_KICK - EPS,
                "recoil pitches back, within [-FIRE_KICK, 0]"
            );
        }
    }

    #[test]
    fn death_pose_topples_and_sinks_monotonically() {
        let a = anim_pose(AnimClip::Death, 0.0);
        let mid = anim_pose(AnimClip::Death, 0.5);
        let end = anim_pose(AnimClip::Death, 1.0);
        // Starts at rest, ends fully fallen.
        assert!(a.pitch.abs() < EPS && a.bob_z.abs() < EPS);
        assert!((end.pitch - DEATH_TOPPLE).abs() < EPS);
        assert!((end.bob_z + DEATH_SINK).abs() < EPS);
        // Monotone topple + sink.
        assert!(a.pitch < mid.pitch && mid.pitch < end.pitch);
        assert!(a.bob_z > mid.bob_z && mid.bob_z > end.bob_z);
        // Past the end the clamp holds it fallen (no overshoot).
        let over = anim_pose(AnimClip::Death, 3.0);
        assert!((over.pitch - DEATH_TOPPLE).abs() < EPS);
    }

    // ---- pose_matrix: REST equivalence + pitch effect ----

    #[test]
    fn rest_pose_matrix_equals_model_matrix() {
        for &(t, s, y) in &[
            ([0.0f32, 0.0, 0.0], 1.0f32, 0.0f32),
            ([5.0, 7.0, 1.0], 1.0, 0.9),
            ([-3.0, 2.0, 0.0], 2.5, std::f32::consts::FRAC_PI_2),
            ([1.0, -1.0, 0.5], 0.5, -1.7),
        ] {
            let a = pose_matrix(t, s, y, AnimPose::REST);
            let b = model_matrix(t, s, y);
            for col in 0..4 {
                for row in 0..4 {
                    assert!(
                        (a[col][row] - b[col][row]).abs() < EPS,
                        "REST pose_matrix must equal model_matrix at [{col}][{row}]"
                    );
                }
            }
        }
    }

    #[test]
    fn pose_matrix_bob_shifts_translation_z_only() {
        let pose = AnimPose {
            bob_z: 0.3,
            pitch: 0.0,
            scale_mul: 1.0,
        };
        let m = pose_matrix([2.0, 3.0, 1.0], 1.0, 0.0, pose);
        assert!((m[3][0] - 2.0).abs() < EPS);
        assert!((m[3][1] - 3.0).abs() < EPS);
        assert!((m[3][2] - 1.3).abs() < EPS, "bob adds to translation Z");
    }

    #[test]
    fn pose_matrix_pitch_tilts_the_up_axis_forward() {
        // At yaw 0 (facing +X), a positive pitch tips the local +Z image toward +X (forward).
        let pose = AnimPose {
            bob_z: 0.0,
            pitch: 0.4,
            scale_mul: 1.0,
        };
        let m = pose_matrix([0.0, 0.0, 0.0], 1.0, 0.0, pose);
        let up_x = m[2][0]; // x-component of the image of local +Z
        assert!(up_x > 0.0, "topple leans the up-axis toward +X");
        // Scale multiplier shrinks the basis uniformly.
        let shrunk = pose_matrix(
            [0.0, 0.0, 0.0],
            2.0,
            0.0,
            AnimPose {
                scale_mul: 0.5,
                ..pose
            },
        );
        let col0_len = (shrunk[0][0] * shrunk[0][0]
            + shrunk[0][1] * shrunk[0][1]
            + shrunk[0][2] * shrunk[0][2])
            .sqrt();
        assert!((col0_len - 1.0).abs() < 1e-3, "2.0 * 0.5 == unit basis");
    }

    // ---- unit_phase + is_infantry ----

    #[test]
    fn unit_phase_advances_with_the_clock_and_staggers_per_unit() {
        let p0 = unit_phase(0, 0.0, 0);
        let p1 = unit_phase(60, 0.0, 0);
        assert!((p1 - p0 - 1.0).abs() < EPS, "one base cycle per 60 ticks");
        // Two units at the same instant sit at different phases (desync).
        assert!((unit_phase(10, 0.5, 0) - unit_phase(10, 0.5, 1)).abs() > EPS);
    }

    #[test]
    fn only_troopers_are_infantry() {
        assert!(is_infantry(ModelKind::Trooper));
        assert!(is_infantry(ModelKind::TrooperUs));
        assert!(is_infantry(ModelKind::TrooperFr));
        assert!(!is_infantry(ModelKind::Tank));
        assert!(!is_infantry(ModelKind::TankTurret));
        assert!(!is_infantry(ModelKind::CampHq));
        assert!(!is_infantry(ModelKind::Crate));
    }
}
