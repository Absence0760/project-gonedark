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

use crate::{UnitInstance, FLAG_EMBODIED};
use gonedark_core::fog::Visibility;

/// Filter (and optionally dim) the frame's instances against the visibility `_fog` mask.
///
/// STUB BEHAVIOR (worker 1 to replace): ignores the mask and reproduces the Phase-1 rule —
/// embodied frames keep only the avatar; lit frames keep everything.
pub fn visible_instances(
    instances: &[UnitInstance],
    _fog: &Visibility,
    world_dark: bool,
) -> Vec<UnitInstance> {
    if world_dark {
        instances
            .iter()
            .copied()
            .filter(|u| u.flags & FLAG_EMBODIED != 0)
            .collect()
    } else {
        instances.to_vec()
    }
}
