//! Hand-rolled struct-of-arrays ECS (decisions.md D18).
//!
//! Components live in parallel dense `Vec`s indexed by entity index; systems iterate by
//! index, so iteration order is stable and deterministic *by construction* — never
//! HashMap iteration (invariant #1 / #7). Entity handles are index + generation, so a
//! stale handle to a recycled slot is detected, with no pointers in sim state.

use crate::components::{InputSource, Order, Stance, Vec2};

/// A generational handle to an entity. Cheap to copy; not a pointer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Entity {
    pub index: u32,
    pub generation: u32,
}

/// The world: SoA component storage + a generational free list.
#[derive(Default)]
pub struct World {
    generation: Vec<u32>,
    alive: Vec<bool>,
    free: Vec<u32>,

    // --- components (dense, indexed by entity index) ---
    pub pos: Vec<Vec2>,
    pub vel: Vec<Vec2>,
    pub order: Vec<Order>,
    pub stance: Vec<Stance>,
    pub input_source: Vec<InputSource>,
}

impl World {
    pub fn new() -> Self {
        World::default()
    }

    /// Allocate an entity, reusing a freed slot when possible (deterministic: the free
    /// list is a stack and spawn/despawn order is identical on every peer).
    pub fn spawn(&mut self) -> Entity {
        if let Some(index) = self.free.pop() {
            let i = index as usize;
            self.alive[i] = true;
            self.pos[i] = Vec2::ZERO;
            self.vel[i] = Vec2::ZERO;
            self.order[i] = Order::default();
            self.stance[i] = Stance::default();
            self.input_source[i] = InputSource::default();
            Entity {
                index,
                generation: self.generation[i],
            }
        } else {
            let index = self.generation.len() as u32;
            self.generation.push(0);
            self.alive.push(true);
            self.pos.push(Vec2::ZERO);
            self.vel.push(Vec2::ZERO);
            self.order.push(Order::default());
            self.stance.push(Stance::default());
            self.input_source.push(InputSource::default());
            Entity {
                index,
                generation: 0,
            }
        }
    }

    /// Free an entity (bumps its generation so stale handles are detected).
    pub fn despawn(&mut self, e: Entity) {
        if self.is_alive(e) {
            let i = e.index as usize;
            self.alive[i] = false;
            self.generation[i] = self.generation[i].wrapping_add(1);
            self.free.push(e.index);
        }
    }

    #[inline]
    pub fn is_alive(&self, e: Entity) -> bool {
        let i = e.index as usize;
        i < self.alive.len() && self.alive[i] && self.generation[i] == e.generation
    }

    /// Number of entity slots (live or recycled) — the iteration bound for systems.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.generation.len()
    }

    #[inline]
    pub fn is_index_alive(&self, i: usize) -> bool {
        self.alive[i]
    }
}
