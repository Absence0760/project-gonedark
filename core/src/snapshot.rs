//! Read-only render snapshot (invariant #4). The renderer interpolates between two of
//! these and converts the Q16.16 positions to float at *its* boundary — it never calls
//! back into the sim to mutate state. Carrying raw `Fixed` keeps `core` float-free.

use crate::components::{InputSource, Vec2};
use crate::ecs::World;

/// One unit's renderable state at a tick.
#[derive(Clone, Debug)]
pub struct UnitSnapshot {
    pub pos: Vec2,
    pub vel: Vec2,
    pub embodied: bool,
}

/// An immutable copy of the renderable world at one sim tick.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub tick: u64,
    pub units: Vec<UnitSnapshot>,
}

impl Snapshot {
    pub fn capture(world: &World, tick: u64) -> Self {
        let mut units = Vec::new();
        for i in 0..world.capacity() {
            if !world.is_index_alive(i) {
                continue;
            }
            units.push(UnitSnapshot {
                pos: world.pos[i],
                vel: world.vel[i],
                embodied: world.input_source[i] == InputSource::Embodied,
            });
        }
        Snapshot { tick, units }
    }
}
