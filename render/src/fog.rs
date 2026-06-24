//! Fog-of-war application at the render boundary (invariant #6 — going dark stays fair).
//!
//! The deterministic visibility *computation* lives in `core::fog` (a pure derivation, never sim
//! state). THIS module is the presentation half: given the per-frame interpolated instances and
//! a computed [`Visibility`] mask, decide which instances are drawn and how they are dimmed, so
//! unseen enemies vanish (command view) and the strategic map collapses to the avatar's sight
//! (embodied "world goes dark"). Float boundary — `f32` math is fine here.
//!
//! IMPLEMENTATION OWNER: worker 1 (fog rendering). This stub reproduces the EXISTING Phase-1
//! filter exactly (embodied → only the avatar; command view → everything), so the renderer's
//! behavior is unchanged until you wire `_fog` in. Fill `visible_instances` (+ inline tests on
//! the pure filter) and KEEP the signature intact.
//!
//! Implementation notes:
//! - Map an instance's `f32` world `(x, y)` to a `core` `Vec2` to query
//!   [`Visibility::is_visible`] — convert with `Fixed::from_bits((v * Fixed::SCALE as f32) as
//!   i32)`, the mirror of [`crate::fixed_to_f32`].
//! - Command view: friendly units + the avatar always draw; enemy/neutral/control-point
//!   instances draw only where the player faction has vision; consider dimming a thin "explored
//!   but not currently seen" band rather than a hard pop if it reads better.
//! - Embodied (`world_dark`): the avatar ([`FLAG_EMBODIED`]) always survives; other instances
//!   draw only inside the avatar's vision mask.

use crate::{UnitInstance, FLAG_EMBODIED, FLAG_RING};
use gonedark_core::components::Vec2;
use gonedark_core::fixed::Fixed;
use gonedark_core::fog::Visibility;

/// Map a render-space `f32` world coordinate back to a `core` `Fixed`. The exact inverse of
/// [`crate::fixed_to_f32`] (`bits / SCALE` ⇄ `round(v * SCALE)`), so an instance lands in the
/// same grid cell the sim used when `fog` was computed. Render-only — `f32` is fine here.
#[inline]
fn f32_to_fixed(v: f32) -> Fixed {
    Fixed::from_bits((v * Fixed::SCALE as f32).round() as i32)
}

/// Filter the frame's instances against the visibility `fog` mask (invariant #6 — going dark
/// stays fair). The drawn set is a hard visibility cut; no dimming.
///
/// - Embodied (`world_dark`): the avatar ([`FLAG_EMBODIED`]) ALWAYS survives, and every other
///   instance is drawn only if its cell is visible in `fog` (the avatar's own sight). The
///   strategic map collapses to what the avatar can see.
/// - Command view (`!world_dark`): control-point rings ([`FLAG_RING`]) and the avatar always
///   draw — objectives are known map markers — while units and buildings draw only where the
///   player's union vision (`fog`) reaches, so enemies outside it stay hidden (fog of war).
///   Friendly units sit inside their own vision and therefore show naturally.
pub fn visible_instances(
    instances: &[UnitInstance],
    fog: &Visibility,
    world_dark: bool,
) -> Vec<UnitInstance> {
    instances
        .iter()
        .copied()
        .filter(|u| {
            // The possessed avatar is always drawn (the camera anchor in both views).
            if u.flags & FLAG_EMBODIED != 0 {
                return true;
            }
            // Command view: control points are always-known map markers.
            if !world_dark && u.flags & FLAG_RING != 0 {
                return true;
            }
            // Everything else is gated on the visibility mask.
            fog.is_visible(Vec2::new(f32_to_fixed(u.x), f32_to_fixed(u.y)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary, so `f32` instance coords are fine here. We build a REAL
    //! [`Visibility`] mask from a tiny `core` world (same spawn pattern as `core::fog`'s tests)
    //! and assert the pure filter keeps/drops the right instances — no GPU involved.

    use super::*;
    use gonedark_core::components::{EntityKind, Faction, Vec2};
    use gonedark_core::ecs::{Entity, World};
    use gonedark_core::fixed::Fixed;
    use gonedark_core::fog::{command_visibility, embodied_visibility};
    use gonedark_core::terrain::Terrain;

    /// Spawn a unit at integer world `(x, y)` with the given vision radius (cf. core::fog tests).
    fn spawn_unit(world: &mut World, faction: Faction, x: i32, y: i32, vision: i32) -> Entity {
        let e = world.spawn();
        let i = e.index as usize;
        world.pos[i] = Vec2::new(Fixed::from_int(x), Fixed::from_int(y));
        world.faction[i] = faction;
        world.kind[i] = EntityKind::Unit;
        world.vision[i] = Fixed::from_int(vision);
        e
    }

    /// A plain unit instance at world `(x, y)` (no flags).
    fn inst(x: f32, y: f32) -> UnitInstance {
        UnitInstance {
            x,
            y,
            ..Default::default()
        }
    }

    // ---- embodied (world goes dark) ----

    #[test]
    fn embodied_keeps_avatar_even_when_cell_is_dark() {
        // Avatar at (-30,-30); its instance sits far from any lit cell (use the blank mask to
        // make "dark" unambiguous) yet must always survive while embodied.
        let fog = Visibility::blank();
        let avatar = UnitInstance {
            x: 50.0,
            y: 50.0,
            flags: FLAG_EMBODIED,
            ..Default::default()
        };
        let out = visible_instances(&[avatar], &fog, true);
        assert_eq!(out.len(), 1, "avatar always survives the dark frame");
        assert_eq!(out[0].flags & FLAG_EMBODIED, FLAG_EMBODIED);
    }

    #[test]
    fn embodied_keeps_inside_mask_drops_outside() {
        // Embodied vision around an avatar at origin (radius 12): a non-avatar instance at (5,0)
        // is inside the avatar's sight and kept; one at (40,0) is outside and dropped.
        let mut world = World::new();
        let avatar = spawn_unit(&mut world, Faction::Player, 0, 0, 12);
        let terrain = Terrain::open();
        let fog = embodied_visibility(&world, &terrain, avatar);

        let inside = inst(5.0, 0.0);
        let outside = inst(40.0, 0.0);
        let out = visible_instances(&[inside, outside], &fog, true);

        assert_eq!(out.len(), 1, "only the in-sight instance survives");
        assert!((out[0].x - 5.0).abs() < 1e-4);
    }

    // ---- command view (fog of war) ----

    #[test]
    fn command_keeps_ring_always() {
        // A control-point ring is a known objective marker: drawn even from a fully-blank mask.
        let fog = Visibility::blank();
        let ring = UnitInstance {
            x: 99.0,
            y: -99.0,
            flags: FLAG_RING,
            ..Default::default()
        };
        let out = visible_instances(&[ring], &fog, false);
        assert_eq!(out.len(), 1, "rings always draw in command view");
        assert_eq!(out[0].flags & FLAG_RING, FLAG_RING);
    }

    #[test]
    fn command_keeps_unit_inside_drops_outside() {
        // Player union vision around a unit at origin (radius 24): a unit instance at (10,0) is
        // seen and kept; an enemy-equivalent instance at (40,0) is in the dark and dropped.
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Player, 0, 0, 24);
        let terrain = Terrain::open();
        let fog = command_visibility(&world, &terrain, Faction::Player);

        let inside = inst(10.0, 0.0);
        let outside = inst(40.0, 0.0);
        let out = visible_instances(&[inside, outside], &fog, false);

        assert_eq!(out.len(), 1, "fog of war hides the out-of-sight unit");
        assert!((out[0].x - 10.0).abs() < 1e-4);
    }

    // ---- blank mask: only the always-keep cases survive ----

    #[test]
    fn blank_mask_drops_everything_except_always_keep() {
        let fog = Visibility::blank();
        let avatar = UnitInstance {
            x: 1.0,
            y: 2.0,
            flags: FLAG_EMBODIED,
            ..Default::default()
        };
        let ring = UnitInstance {
            x: 3.0,
            y: 4.0,
            flags: FLAG_RING,
            ..Default::default()
        };
        let plain = inst(5.0, 6.0);

        // Command view: avatar + ring kept, plain unit dropped.
        let cmd = visible_instances(&[avatar, ring, plain], &fog, false);
        assert_eq!(cmd.len(), 2);
        assert!(cmd.iter().any(|u| u.flags & FLAG_EMBODIED != 0));
        assert!(cmd.iter().any(|u| u.flags & FLAG_RING != 0));

        // Embodied: only the avatar survives (rings are map intel, gone in the dark).
        let dark = visible_instances(&[avatar, ring, plain], &fog, true);
        assert_eq!(dark.len(), 1);
        assert_eq!(dark[0].flags & FLAG_EMBODIED, FLAG_EMBODIED);
    }
}
