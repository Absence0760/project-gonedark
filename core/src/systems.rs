//! Sim systems — pure functions over component spans, iterated in stable index order.
//!
//! `movement_system` is the literal executor (invariant #3, D3): a unit holds its last
//! `Order` and does exactly that — here, step toward a `MoveTo` target at a fixed speed.
//! No autonomy, no pathing intelligence beyond following the (stubbed) flow-field
//! direction. Embodied units are skipped — their motion comes from live player input.

use crate::components::{InputSource, Order, Vec2};
use crate::ecs::World;
use crate::fixed::Fixed;

/// Base move speed in world units per tick (1/8). Tune via data later.
pub const MOVE_SPEED: Fixed = Fixed::from_ratio(1, 8);

/// Squared arrival epsilon: snap to the target when closer than this (1/256 units²).
pub const ARRIVE_EPS_SQ: Fixed = Fixed::from_ratio(1, 256);

/// Advance every order-driven unit one tick toward its `MoveTo` target.
pub fn movement_system(world: &mut World) {
    let n = world.capacity();
    for i in 0..n {
        if !world.is_index_alive(i) {
            continue;
        }
        // Possessed units are moved by live player input, not the order executor.
        if world.input_source[i] == InputSource::Embodied {
            continue;
        }
        match world.order[i] {
            Order::Idle => {
                world.vel[i] = Vec2::ZERO;
            }
            Order::MoveTo(target) => {
                let to = target - world.pos[i];
                if to.len_sq() <= ARRIVE_EPS_SQ {
                    world.pos[i] = target;
                    world.vel[i] = Vec2::ZERO;
                    world.order[i] = Order::Idle;
                } else {
                    // Flow-field STUB: direct unit direction. Real flow-field sampling
                    // replaces `to.normalized()` in Phase 1 build-order step 3.
                    let step = to.normalized().scale(MOVE_SPEED);
                    world.vel[i] = step;
                    world.pos[i] = world.pos[i] + step;
                }
            }
        }
    }
}
