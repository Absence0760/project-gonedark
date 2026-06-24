//! Hand-rolled struct-of-arrays ECS (decisions.md D18).
//!
//! Components live in parallel dense `Vec`s indexed by entity index; systems iterate by
//! index, so iteration order is stable and deterministic *by construction* — never
//! HashMap iteration (invariant #1 / #7). Entity handles are index + generation, so a
//! stale handle to a recycled slot is detected, with no pointers in sim state.

use crate::components::{
    Building, EntityKind, Faction, Health, InputSource, Order, Stance, Vec2, Weapon,
};
use crate::fixed::Fixed;

/// Default sight radius (world units) every entity spawns with (fog-of-war input). Kept here
/// (not in `fog`) so the ECS has no dependency on a worker-owned module.
const DEFAULT_VISION: Fixed = Fixed::from_int(24);

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

    // --- Phase 2 components (D23) ---
    /// Which side the entity is on (combat engages only across factions).
    pub faction: Vec<Faction>,
    /// Whether the entity is a unit or a building.
    pub kind: Vec<EntityKind>,
    /// Hit points; an entity at/under zero is despawned by combat.
    pub health: Vec<Health>,
    /// Weapon (a default range-0 weapon never fires).
    pub weapon: Vec<Weapon>,
    /// Accumulated suppression (`combat`); pins/slows the unit, decays over time.
    pub suppression: Vec<Fixed>,
    /// The last entity to damage this one (drives the `ReturnFire` stance). `None` until hit.
    pub last_attacker: Vec<Option<Entity>>,
    /// Retreat trigger: health fraction in `[0, 1]` below which `orders` installs `FallBack`.
    /// Zero (default) = never retreat.
    pub retreat_below: Vec<Fixed>,
    /// Sight radius (world units) for fog-of-war visibility derivation.
    pub vision: Vec<Fixed>,
    /// Per-building state (construction/upgrade/production); inert for units.
    pub building: Vec<Building>,
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
            self.faction[i] = Faction::default();
            self.kind[i] = EntityKind::default();
            self.health[i] = Health::default();
            self.weapon[i] = Weapon::default();
            self.suppression[i] = Fixed::ZERO;
            self.last_attacker[i] = None;
            self.retreat_below[i] = Fixed::ZERO;
            self.vision[i] = DEFAULT_VISION;
            self.building[i] = Building::default();
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
            self.faction.push(Faction::default());
            self.kind.push(EntityKind::default());
            self.health.push(Health::default());
            self.weapon.push(Weapon::default());
            self.suppression.push(Fixed::ZERO);
            self.last_attacker.push(None);
            self.retreat_below.push(Fixed::ZERO);
            self.vision.push(DEFAULT_VISION);
            self.building.push(Building::default());
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

    /// The live generational [`Entity`] handle occupying slot `i`, or `None` if the slot is
    /// dead. The O(1) inverse of "index → handle" that systems need to put a real handle into
    /// an event or a component (e.g. combat's `last_attacker` / `SimEvent`), since the snapshot
    /// and component arrays are addressed by bare index. Reads the slot's current generation.
    #[inline]
    pub fn entity(&self, i: usize) -> Option<Entity> {
        if i < self.alive.len() && self.alive[i] {
            Some(Entity {
                index: i as u32,
                generation: self.generation[i],
            })
        } else {
            None
        }
    }
}
