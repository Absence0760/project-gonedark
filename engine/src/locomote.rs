//! Embodied-locomotion input seam: the host's float `move_axis` + look `yaw` → a deterministic
//! [`Command::Locomote`] (the twin-stick / WASD avatar mover).
//!
//! This is the movement twin of [`fire`](crate::fire). While embodied the player walks with a
//! host-only analog stick (WASD on desktop, a virtual stick on touch) read into
//! [`InputFrame::move_axis`](gonedark_pal::InputFrame::move_axis), and aims with a host-only `f32`
//! `yaw` (integrated from look deltas; D15 — neither value enters the sim raw). Locomotion is the
//! place the *intended heading* must cross into `core`, and it crosses **exactly like the fire aim
//! and the command-layer tap target**: the float world-direction is quantized to `Fixed` bits AT
//! THIS BOUNDARY (`quantize`, the same round-to-bits rule as [`world_to_fixed`](crate::world_to_fixed))
//! so the `Command` carries only fixed-point data into the deterministic sim (invariant #1). The
//! move itself is resolved sim-side ([`gonedark_core::systems::step_along`], guarded to
//! `InputSource::Embodied`) — bit-identical on every peer, never on the moving host.
//!
//! The stick is **camera-relative twin-stick**: `W` walks forward along the current yaw, `S` back,
//! `A`/`D` strafe left/right of where the player is looking. The raw axis is rotated into world
//! space by `yaw` here on the host (floats are fine in this crate — it is not the sim), and only the
//! resulting unit-ish direction is quantized. Pure free fn, no GPU/device dependency → unit-tested
//! directly.

use gonedark_core::components::Vec2;
use gonedark_core::ecs::Entity;
use gonedark_core::fixed::Fixed;
use gonedark_core::sim::Command;

/// Quantize one host float to `Fixed` Q16.16 bits — the single sanctioned float→sim boundary,
/// identical to [`world_to_fixed`](crate::world_to_fixed) and [`fire`](crate::fire)'s aim
/// quantizer. Round-to-nearest so a `+x` heading maps to `Fixed::ONE` exactly.
#[inline]
fn quantize(v: f32) -> Fixed {
    Fixed::from_bits((v * Fixed::SCALE as f32).round() as i32)
}

/// Stick deflections at or below this magnitude are treated as neutral (no command). Small enough
/// that a fully-pressed WASD key (deflection 1) always passes, large enough to swallow analog-stick
/// drift / float noise so a resting stick emits nothing rather than a jittery near-zero heading.
const DEAD_ZONE: f32 = 1e-3;

/// Build the [`Command::Locomote`] for an embodied unit this frame, or `None` if the stick is
/// neutral. `move_axis` is the raw host stick `(mx, my)` in **screen convention** (the same one
/// `pal-desktop` fills from WASD): `+mx` = right (`D`), `+my` = down/back, so `W` (up) is `-my`.
///
/// The axis is interpreted **camera-relative** to the host look `yaw`:
/// - forward `F = (cos yaw, sin yaw)` — the direction the embodied camera faces,
/// - right   `R = (sin yaw, -cos yaw)` — F rotated −90° about world +Z (the screen-right strafe),
///
/// and combined as `dir = F·(−my) + R·(mx)` (W → forward, S → back, D → strafe-right, A →
/// strafe-left). The deflection magnitude is clamped to `1`, so a keyboard diagonal isn't faster
/// than a cardinal while a partially-pushed analog stick still walks proportionally slower (the
/// `step_along` "analog deflection" contract). The world direction is quantized to `Fixed` here at
/// the boundary — the only value that crosses into `core` (invariant #1). The sim ignores the
/// command for any unit that isn't `InputSource::Embodied`, so a stray heading is harmless.
#[inline]
pub fn locomote_command(
    embodied_entity: Entity,
    yaw: f32,
    move_axis: (f32, f32),
) -> Option<Command> {
    let (mx, my) = move_axis;
    // Local twin-stick deflection: forward is −my (screen +Y is down, so W/up = −my), strafe is mx.
    let forward = -my;
    let strafe = mx;
    let mag_sq = forward * forward + strafe * strafe;
    if mag_sq <= DEAD_ZONE * DEAD_ZONE {
        return None;
    }
    // Clamp deflection to a unit disc: a cardinal stays full-speed, a keyboard diagonal (|·| = √2)
    // normalises to 1, and a partial analog push is preserved (≤ 1 → no scaling).
    let scale = if mag_sq > 1.0 {
        1.0 / mag_sq.sqrt()
    } else {
        1.0
    };
    let forward = forward * scale;
    let strafe = strafe * scale;

    // Rotate the local (forward, strafe) deflection into world space by the look yaw.
    let (c, s) = (yaw.cos(), yaw.sin());
    let world_x = c * forward + s * strafe;
    let world_y = s * forward - c * strafe;

    let dir = Vec2::new(quantize(world_x), quantize(world_y));
    Some(Command::Locomote {
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
            index: 4,
            generation: 0,
        }
    }

    /// Pull the `(x, y)` Fixed bits out of a `Locomote`, panicking on any other command.
    fn dir_of(cmd: Command) -> (Fixed, Fixed) {
        match cmd {
            Command::Locomote { dir, .. } => (dir.x, dir.y),
            other => panic!("expected Command::Locomote, got {other:?}"),
        }
    }

    #[test]
    fn neutral_stick_emits_nothing() {
        assert!(locomote_command(entity(), 0.0, (0.0, 0.0)).is_none());
        // Sub-dead-zone drift is also neutral.
        assert!(locomote_command(entity(), 1.3, (0.0005, -0.0005)).is_none());
    }

    #[test]
    fn locomote_carries_the_embodied_entity() {
        let e = entity();
        let cmd = locomote_command(e, 0.0, (0.0, -1.0)).expect("a live stick emits Locomote");
        match cmd {
            Command::Locomote { entity, .. } => assert_eq!(entity, e),
            other => panic!("expected Locomote, got {other:?}"),
        }
    }

    #[test]
    fn wasd_at_yaw_zero_is_camera_relative() {
        // yaw 0 → facing +x. W = forward (+x), S = back (−x), D = strafe-right (−y), A = (+y).
        // W: move_axis (0,-1).
        let (x, y) = dir_of(locomote_command(entity(), 0.0, (0.0, -1.0)).unwrap());
        assert_eq!(x, Fixed::ONE);
        assert_eq!(y, Fixed::ZERO);
        // S: move_axis (0,1).
        let (x, y) = dir_of(locomote_command(entity(), 0.0, (0.0, 1.0)).unwrap());
        assert_eq!(x, Fixed::from_int(-1));
        assert_eq!(y, Fixed::ZERO);
        // D: move_axis (1,0) → strafe right of +x facing is −y.
        let (x, y) = dir_of(locomote_command(entity(), 0.0, (1.0, 0.0)).unwrap());
        assert_eq!(x, Fixed::ZERO);
        assert_eq!(y, Fixed::from_int(-1));
        // A: move_axis (-1,0) → +y.
        let (x, y) = dir_of(locomote_command(entity(), 0.0, (-1.0, 0.0)).unwrap());
        assert_eq!(x, Fixed::ZERO);
        assert_eq!(y, Fixed::ONE);
    }

    #[test]
    fn wasd_at_quarter_turn_rotates_with_the_camera() {
        // yaw 90° → facing +y. W = forward (+y), D = strafe-right (+x). cos 90° rounds to 0 bits
        // (exactly as the fire seam's quarter-turn case), so the cross-axis is Fixed::ZERO.
        // W: move_axis (0,-1) → +y.
        let (x, y) = dir_of(locomote_command(entity(), FRAC_PI_2, (0.0, -1.0)).unwrap());
        assert_eq!(x, Fixed::ZERO);
        assert_eq!(y, Fixed::ONE);
        // D: move_axis (1,0) → +x.
        let (x, y) = dir_of(locomote_command(entity(), FRAC_PI_2, (1.0, 0.0)).unwrap());
        assert_eq!(x, Fixed::ONE);
        assert_eq!(y, Fixed::ZERO);
    }

    #[test]
    fn cardinal_heading_is_a_quantized_unit_vector() {
        // A single-key heading must quantize to ~length-1 so the sim walks at the full base speed
        // (the boundary preserves a usable unit heading, like the fire seam's aim check).
        let (x, y) = dir_of(locomote_command(entity(), 0.7, (0.0, -1.0)).unwrap());
        let fx = x.to_bits() as f32 / Fixed::SCALE as f32;
        let fy = y.to_bits() as f32 / Fixed::SCALE as f32;
        let len_sq = fx * fx + fy * fy;
        assert!((len_sq - 1.0).abs() < 1e-3, "len_sq={len_sq} should be ~1");
    }

    #[test]
    fn diagonal_keyboard_input_is_clamped_to_unit_speed() {
        // W+D pressed together is deflection √2; without the clamp the avatar would walk faster
        // diagonally than along a cardinal. The emitted heading must stay ~length 1.
        let (x, y) = dir_of(locomote_command(entity(), 0.0, (1.0, -1.0)).unwrap());
        let fx = x.to_bits() as f32 / Fixed::SCALE as f32;
        let fy = y.to_bits() as f32 / Fixed::SCALE as f32;
        let len_sq = fx * fx + fy * fy;
        assert!(
            (len_sq - 1.0).abs() < 1e-3,
            "diagonal len_sq={len_sq} should be clamped to ~1, not ~2"
        );
    }
}
