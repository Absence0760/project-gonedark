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
