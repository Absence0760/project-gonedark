//! Embodied-fire input seam (W1): the host's float `yaw` → a deterministic [`Command::Fire`].
//!
//! While embodied, the player aims with a host-only `f32` yaw (integrated from look deltas; it
//! never enters the sim — D15). Firing is the one place that aim must cross into `core`, and it
//! crosses **exactly like the command-layer tap target**: the float direction `(cos yaw, sin yaw)`
//! is quantized to `Fixed` bits AT THIS BOUNDARY (`quantize`, the same round-to-bits rule as
//! [`world_to_fixed`](crate::world_to_fixed)) so the `Command` carries only fixed-point data into
//! the deterministic sim (invariant #1). The hit itself is resolved sim-side
//! ([`gonedark_core::combat::resolve_fire`]) — bit-identical on every peer, never on the firing
//! host. This is a pure free fn with no GPU/device dependency, so it is unit-tested directly.

use gonedark_core::components::Vec2;
use gonedark_core::ecs::Entity;
use gonedark_core::fixed::Fixed;
use gonedark_core::sim::Command;

/// Quantize one host float to `Fixed` Q16.16 bits — the single sanctioned float→sim boundary,
/// identical to [`world_to_fixed`](crate::world_to_fixed). Round-to-nearest so `+x` aim maps to
/// `Fixed::ONE` exactly.
#[inline]
fn quantize(v: f32) -> Fixed {
    Fixed::from_bits((v * Fixed::SCALE as f32).round() as i32)
}

/// Build the [`Command::Fire`] for an embodied unit this frame, or `None` if the fire button is
/// not pressed. The aim direction is the host yaw's unit vector `(cos yaw, sin yaw)` quantized to
/// `Fixed` here at the boundary — the only value that crosses into `core`. The sim decides whether
/// the shot lands (range / cone / line-of-sight / cooldown), so a pressed trigger always *emits*
/// the command; an out-of-arc or cooling-down weapon simply resolves to no damage sim-side.
#[inline]
pub fn fire_command(embodied_entity: Entity, yaw: f32, fire_pressed: bool) -> Option<Command> {
    if !fire_pressed {
        return None;
    }
    let dir = Vec2::new(quantize(yaw.cos()), quantize(yaw.sin()));
    Some(Command::Fire {
        entity: embodied_entity,
        dir,
    })
}

/// The embodied weapon's **select-fire mode** — a host-side input preference (never sim state). It
/// changes only how the host EMITS [`Command::Fire`] and which viewmodel animation plays; the sim's
/// authoritative rate of fire is still the weapon cooldown, identical on every peer either way.
///
/// - [`Semi`](FireMode::Semi): one shot per trigger *pull* (the rising edge). Holding does nothing
///   after the first shot; the viewmodel visibly works the action between shots (`semi_cycle_phase`).
/// - [`Auto`](FireMode::Auto): fires every frame the trigger is held (the cooldown paces the spray),
///   with a continuous muzzle-climb spray on the viewmodel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FireMode {
    #[default]
    Semi,
    Auto,
}

impl FireMode {
    /// The other mode — the select-fire toggle (X on desktop).
    pub fn toggled(self) -> Self {
        match self {
            FireMode::Semi => FireMode::Auto,
            FireMode::Auto => FireMode::Semi,
        }
    }
}

/// Whether to EMIT a `Command::Fire` this frame, given the fire `mode`, the current held-trigger
/// level (`held`), and last frame's held level (`prev_held`). Semi-auto fires only on the rising
/// edge (`held && !prev_held`) — one shot per pull, so holding the trigger can never "keep firing";
/// full-auto fires every held frame and lets the sim cooldown pace it. PURE → host-tested; this is
/// the single seam that gives the two modes their behaviour.
#[inline]
pub fn should_emit_fire(mode: FireMode, held: bool, prev_held: bool) -> bool {
    match mode {
        FireMode::Semi => held && !prev_held,
        FireMode::Auto => held,
    }
}

/// How many sim ticks the semi-auto **chambering cycle** animation plays for after a shot — the
/// window over which the viewmodel works the action and returns to ready. ~0.3 s at 60 Hz.
pub const CHAMBER_CYCLE_TICKS: u64 = 18;

/// The semi-auto chambering phase `[0,1]` for the viewmodel this frame: `0` right after the shot
/// (`last_fire_tick == tick`, action just opened) ramping to `1` (fully chambered / ready) over
/// [`CHAMBER_CYCLE_TICKS`]. `1` when the player hasn't fired, or the shot is older than the window,
/// or the tick predates it. PRESENTATION ONLY (fed to `render::world::WeaponPose.cycle`); pure →
/// host-tested. The host only uses this in semi-auto — full-auto pins the pose's cycle to `1`.
pub fn semi_cycle_phase(last_fire_tick: Option<u64>, tick: u64) -> f32 {
    let Some(fired) = last_fire_tick else {
        return 1.0;
    };
    if tick < fired {
        return 1.0;
    }
    ((tick - fired) as f32 / CHAMBER_CYCLE_TICKS as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f32::consts::FRAC_PI_2;

    fn entity() -> Entity {
        Entity {
            index: 3,
            generation: 0,
        }
    }

    #[test]
    fn no_command_when_trigger_released() {
        assert!(fire_command(entity(), 0.0, false).is_none());
    }

    #[test]
    fn pressed_trigger_emits_fire_for_the_embodied_entity() {
        let e = entity();
        let cmd = fire_command(e, 0.0, true).expect("a pressed trigger emits a Fire command");
        match cmd {
            Command::Fire { entity, .. } => assert_eq!(entity, e),
            other => panic!("expected Command::Fire, got {other:?}"),
        }
    }

    #[test]
    fn yaw_zero_quantizes_to_plus_x_unit_vector() {
        // cos 0 = 1, sin 0 = 0 → exactly (Fixed::ONE, Fixed::ZERO).
        let cmd = fire_command(entity(), 0.0, true).unwrap();
        let Command::Fire { dir, .. } = cmd else {
            panic!("expected Fire");
        };
        assert_eq!(dir.x, Fixed::ONE);
        assert_eq!(dir.y, Fixed::ZERO);
    }

    #[test]
    fn yaw_quarter_turn_quantizes_to_plus_y_unit_vector() {
        // cos 90° ≈ 0, sin 90° = 1 → (~0, Fixed::ONE). The cosine rounds to 0 bits.
        let cmd = fire_command(entity(), FRAC_PI_2, true).unwrap();
        let Command::Fire { dir, .. } = cmd else {
            panic!("expected Fire");
        };
        assert_eq!(dir.x, Fixed::ZERO);
        assert_eq!(dir.y, Fixed::ONE);
    }

    #[test]
    fn semi_auto_fires_only_on_the_press_edge() {
        // Holding the trigger (held stays true) emits exactly one shot: the frame the press begins.
        assert!(should_emit_fire(FireMode::Semi, true, false), "rising edge fires");
        assert!(!should_emit_fire(FireMode::Semi, true, true), "still-held does NOT keep firing");
        assert!(!should_emit_fire(FireMode::Semi, false, true), "release does not fire");
        assert!(!should_emit_fire(FireMode::Semi, false, false), "idle does not fire");
    }

    #[test]
    fn full_auto_fires_every_held_frame() {
        assert!(should_emit_fire(FireMode::Auto, true, false), "auto fires on the edge");
        assert!(should_emit_fire(FireMode::Auto, true, true), "and keeps firing while held (spray)");
        assert!(!should_emit_fire(FireMode::Auto, false, true), "but not once released");
    }

    #[test]
    fn select_fire_toggles_between_the_two_modes() {
        assert_eq!(FireMode::default(), FireMode::Semi, "default is semi-auto");
        assert_eq!(FireMode::Semi.toggled(), FireMode::Auto);
        assert_eq!(FireMode::Auto.toggled(), FireMode::Semi);
    }

    #[test]
    fn chamber_cycle_ramps_from_zero_at_the_shot_to_ready() {
        assert_eq!(semi_cycle_phase(None, 100), 1.0, "no shot → ready");
        assert_eq!(semi_cycle_phase(Some(100), 100), 0.0, "just fired → action open");
        let mid = semi_cycle_phase(Some(100), 100 + CHAMBER_CYCLE_TICKS / 2);
        assert!(mid > 0.0 && mid < 1.0, "mid-cycle is working the action ({mid})");
        assert_eq!(
            semi_cycle_phase(Some(100), 100 + CHAMBER_CYCLE_TICKS),
            1.0,
            "chambered by the end of the window",
        );
        assert_eq!(semi_cycle_phase(Some(100), 100 + CHAMBER_CYCLE_TICKS + 50), 1.0, "and stays ready");
        assert_eq!(semi_cycle_phase(Some(100), 90), 1.0, "a future-stamped shot reads as ready");
    }

    #[test]
    fn quantized_direction_is_a_near_unit_vector() {
        // For an arbitrary yaw the quantized (x,y) must stay close to length 1 — confirming the
        // boundary preserves a usable aim vector for the sim's cone test. Pure host-float check.
        let yaw = 0.9_f32;
        let cmd = fire_command(entity(), yaw, true).unwrap();
        let Command::Fire { dir, .. } = cmd else {
            panic!("expected Fire");
        };
        let x = dir.x.to_bits() as f32 / Fixed::SCALE as f32;
        let y = dir.y.to_bits() as f32 / Fixed::SCALE as f32;
        let len_sq = x * x + y * y;
        assert!((len_sq - 1.0).abs() < 1e-3, "len_sq={len_sq} should be ~1");
    }
}
