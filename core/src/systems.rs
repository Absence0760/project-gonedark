//! Sim systems — pure functions over component spans, iterated in stable index order.
//!
//! Movement is the literal executor (invariant #3, D3): a unit holds its last `Order` and
//! does exactly that — step toward its target at a fixed speed, no autonomy. The full
//! order/stance vocabulary lives in [`orders::order_system`](crate::orders); this module owns
//! only the shared stepping primitive both it (and any future mover) call.
//!
//! Pathing uses a real deterministic [`FlowField`](crate::flow_field): a unit steps along the
//! sampled downhill direction toward its target. Fields come from a per-tick
//! [`FlowFieldCache`](crate::flow_field::FlowFieldCache) — units sharing a goal share one build,
//! which is bit-identical to each building its own (a field is a pure function of its goal) but
//! turns a 200-unit shared push from ~200 builds into a handful (the measured 60 Hz bottleneck;
//! `docs/phase-3-plan.md` §"Workstream A"). Phase 1 had no obstacles, so the field points at the
//! goal; the structure generalises to Phase 2 terrain.

use crate::components::Vec2;
use crate::ecs::World;
use crate::fixed::Fixed;
use crate::flow_field::FlowFieldCache;

/// Base move speed in world units per tick (1/8). Tune via data later.
pub const MOVE_SPEED: Fixed = Fixed::from_ratio(1, 8);

/// Squared arrival epsilon: snap to the target when closer than this (1/256 units²).
pub const ARRIVE_EPS_SQ: Fixed = Fixed::from_ratio(1, 256);

/// Step a single unit toward `target` via the flow field at an explicit `speed` (world units
/// per tick). The field is fetched from `cache` (built once per distinct goal per tick), so the
/// sampled direction is bit-identical to building a fresh field here. Returns `true` once it has
/// arrived (within [`ARRIVE_EPS_SQ`]), snapping it onto the target and zeroing velocity. The one
/// movement implementation `orders::order_system` builds on (invariant #3 — the unit only
/// follows the field, it does not strategize). A `speed` of zero pins the unit in place (e.g.
/// fully suppressed) without completing its order — and without forcing a field build.
pub fn step_toward_speed(
    world: &mut World,
    cache: &mut FlowFieldCache,
    i: usize,
    target: Vec2,
    speed: Fixed,
) -> bool {
    let to = target - world.pos[i];
    if to.len_sq() <= ARRIVE_EPS_SQ {
        world.pos[i] = target;
        world.vel[i] = Vec2::ZERO;
        true
    } else if speed == Fixed::ZERO {
        // Pinned: hold position, but the order is not yet complete.
        world.vel[i] = Vec2::ZERO;
        false
    } else {
        let dir = cache.get(target).sample(world.pos[i]);
        let step = dir.scale(speed);
        world.vel[i] = step;
        world.pos[i] = world.pos[i] + step;
        false
    }
}

/// Step a single unit toward `target` at the base [`MOVE_SPEED`].
#[inline]
pub fn step_toward(world: &mut World, cache: &mut FlowFieldCache, i: usize, target: Vec2) -> bool {
    step_toward_speed(world, cache, i, target, MOVE_SPEED)
}
