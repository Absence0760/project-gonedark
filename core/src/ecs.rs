//! Hand-rolled struct-of-arrays ECS (decisions.md D18).
//!
//! Components live in parallel dense `Vec`s indexed by entity index; systems iterate by
//! index, so iteration order is stable and deterministic *by construction* — never
//! HashMap iteration (invariant #1 / #7). Entity handles are index + generation, so a
//! stale handle to a recycled slot is detected, with no pointers in sim state.

use crate::components::{
    Building, EntityKind, Faction, Health, InputSource, Order, Posture, Stance, UnitKind, Vec2,
    Weapon,
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
    /// Body posture (the embodied crouch toggle). Player-only sim state; AI units stay
    /// `Standing` (invariant #3). Folded into the checksum — it affects movement speed + aim.
    pub posture: Vec<Posture>,

    // --- Phase 2 components (D23) ---
    /// Which side the entity is on (combat engages only across factions).
    pub faction: Vec<Faction>,
    /// Whether the entity is a unit or a building.
    pub kind: Vec<EntityKind>,
    /// The producible archetype a unit was spawned as (render-facing metadata: the renderer maps
    /// `Heavy` → tank, `Rifleman` → infantry). Set deterministically from the production queue, so
    /// it is identical on every peer — but its *gameplay* effect is already captured by the spawned
    /// `health`/`weapon` stats, so it is **NOT** folded into the per-tick checksum (invariant #7).
    /// Defaults to `Rifleman`; meaningless for buildings.
    pub unit_kind: Vec<UnitKind>,
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
            self.posture[i] = Posture::default();
            self.faction[i] = Faction::default();
            self.kind[i] = EntityKind::default();
            self.unit_kind[i] = UnitKind::default();
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
            self.posture.push(Posture::default());
            self.faction.push(Faction::default());
            self.kind.push(EntityKind::default());
            self.unit_kind.push(UnitKind::default());
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

    // --- Authoritative snapshot support (D28) ----------------------------------------------
    //
    // The liveness triple (`generation` / `alive` / the `free` list, in order) is private sim
    // state the per-tick checksum does NOT fully hash: the checksum folds each slot's
    // `is_index_alive` (== `alive[i]`) and component data, but not `generation[i]` nor the
    // free-list *order*. A resume needs both — `generation[]` for stale-handle detection, and
    // the free-list order because `spawn` pops it to choose the next reused slot, so a wrong
    // order makes the next spawn land on a different slot than its peers (an instant desync,
    // D28 §3). These read accessors + the `from_parts` reconstructor let `persist`/`sim`
    // capture and rebuild the triple while keeping it encapsulated here (the World owns the
    // invariant that all its arrays are the same length).

    /// The per-slot generation array (`generation[i]` is slot `i`'s current generation).
    #[inline]
    pub fn generations(&self) -> &[u32] {
        &self.generation
    }

    /// The per-slot liveness array (`alive[i]` is whether slot `i` currently holds a live
    /// entity).
    #[inline]
    pub fn alive_flags(&self) -> &[bool] {
        &self.alive
    }

    /// The free list — slot indices available for reuse, **in stack order** (`spawn` pops the
    /// last). The order is sim state: it decides which slot the next spawn lands in.
    #[inline]
    pub fn free_list(&self) -> &[u32] {
        &self.free
    }

    /// Rebuild a [`World`] from its decoded parts (the inverse of the accessors above + the
    /// public component `Vec`s). Used only by the authoritative-snapshot deserialize (D28).
    ///
    /// `components` carries every per-slot component array already filled (length == capacity);
    /// this fn supplies the private liveness triple and validates the whole structure is
    /// self-consistent, returning `None` on any mismatch rather than building a corrupt world:
    /// - all arrays share one length (`capacity`),
    /// - the free list references only in-range, **dead** slots with no duplicates, and
    /// - every dead slot appears in the free list (dead ⇔ free), so the resumed world's
    ///   spawn/despawn bookkeeping is exactly what a never-interrupted run would hold.
    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        generation: Vec<u32>,
        alive: Vec<bool>,
        free: Vec<u32>,
        components: WorldComponents,
    ) -> Option<World> {
        let cap = generation.len();
        let WorldComponents {
            pos,
            vel,
            order,
            stance,
            input_source,
            posture,
            faction,
            kind,
            unit_kind,
            health,
            weapon,
            suppression,
            last_attacker,
            retreat_below,
            vision,
            building,
        } = components;
        // Every array must be the same length as the liveness arrays.
        if alive.len() != cap
            || pos.len() != cap
            || vel.len() != cap
            || order.len() != cap
            || stance.len() != cap
            || input_source.len() != cap
            || posture.len() != cap
            || faction.len() != cap
            || kind.len() != cap
            || unit_kind.len() != cap
            || health.len() != cap
            || weapon.len() != cap
            || suppression.len() != cap
            || last_attacker.len() != cap
            || retreat_below.len() != cap
            || vision.len() != cap
            || building.len() != cap
        {
            return None;
        }
        // The free list must reference only in-range, dead, non-duplicate slots, and must list
        // every dead slot exactly once (dead ⇔ free) — the invariant `spawn`/`despawn` maintain.
        let mut seen = vec![false; cap];
        for &idx in &free {
            let i = idx as usize;
            if i >= cap || alive[i] || seen[i] {
                return None;
            }
            seen[i] = true;
        }
        for i in 0..cap {
            // A dead slot not in the free list (seen) would leak — never spawnable again.
            if !alive[i] && !seen[i] {
                return None;
            }
        }
        Some(World {
            generation,
            alive,
            free,
            pos,
            vel,
            order,
            stance,
            input_source,
            posture,
            faction,
            kind,
            unit_kind,
            health,
            weapon,
            suppression,
            last_attacker,
            retreat_below,
            vision,
            building,
        })
    }
}

/// The full set of per-slot component arrays, decoded from an authoritative snapshot and handed
/// to [`World::from_parts`]. A plain bag of `Vec`s mirroring the public component fields on
/// [`World`] — used only on the deserialize path (D28).
pub struct WorldComponents {
    pub pos: Vec<Vec2>,
    pub vel: Vec<Vec2>,
    pub order: Vec<Order>,
    pub stance: Vec<Stance>,
    pub input_source: Vec<InputSource>,
    pub posture: Vec<Posture>,
    pub faction: Vec<Faction>,
    pub kind: Vec<EntityKind>,
    pub unit_kind: Vec<UnitKind>,
    pub health: Vec<Health>,
    pub weapon: Vec<Weapon>,
    pub suppression: Vec<Fixed>,
    pub last_attacker: Vec<Option<Entity>>,
    pub retreat_below: Vec<Fixed>,
    pub vision: Vec<Fixed>,
    pub building: Vec<Building>,
}
