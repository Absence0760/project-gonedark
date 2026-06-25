//! The deterministic fixed-tick simulation (invariants #1, #4, #7).
//!
//! [`Sim::step`] advances the world by exactly one tick: it clears the per-tick event stream,
//! applies that tick's commands, then runs the game systems in a fixed order —
//! [`orders`](crate::orders) (literal-executor movement + retreat triggers) →
//! [`combat`](crate::combat) (fire/suppress/die) → [`territory`](crate::territory) (capture) →
//! [`economy`](crate::economy) (income/build/production). The renderer reads
//! [`Sim::snapshot`] and interpolates but never mutates state. Fog/alerts are derived
//! presentation views computed outside the tick (see [`fog`](crate::fog),
//! [`alerts`](crate::alerts)) and are deliberately not part of the checksum.
//!
//! The tick rate is the locked global 60 Hz ([`TICK_HZ`], D21).

use crate::checksum::Checksum;
use crate::combat;
use crate::components::{
    Building, BuildingKind, EntityKind, Faction, InputSource, Order, Stance, UnitKind, Vec2,
};
use crate::economy::{self, Resources};
use crate::ecs::{Entity, World};
use crate::event::SimEvent;
use crate::fixed::Fixed;
use crate::orders;
use crate::rng::Rng;
use crate::snapshot::Snapshot;
use crate::terrain::Terrain;
use crate::territory::{self, Territory};

/// Sim tick rate (Hz). Locked at a single global 60 Hz for Phase 1 ([`decisions.md`] D21,
/// closing Q10); 30 Hz proved too coarse for embodied combat (D16). Dual-rate is deferred to
/// Phase 3's 200-unit thermal re-evaluation, not killed — kept a single named constant so the
/// rate stays trivially re-tunable.
pub const TICK_HZ: u32 = 60;

/// A command fed into the sim on a tick — the lockstep "order" unit. Commands are applied in
/// the order given (stable), before systems run. All payloads are `Copy` fixed-point/handle
/// data so a command carries no float into the deterministic sim (invariant #1).
#[derive(Clone, Copy, Debug)]
pub enum Command {
    /// Issue a move order (literal executor follows it via the flow field).
    Move { entity: Entity, target: Vec2 },
    /// Move toward a point but engage enemies that come into range en route.
    AttackMove { entity: Entity, target: Vec2 },
    /// Install an arbitrary order from the Phase 2 vocabulary (patrol, hold, fall back, …).
    SetOrder { entity: Entity, order: Order },
    /// Change a unit's engagement stance.
    SetStance { entity: Entity, stance: Stance },
    /// Set the retreat trigger: fall back when health drops below this fraction (`0` = never).
    SetRetreatThreshold { entity: Entity, fraction: Fixed },
    /// Possess a unit: swap its input source to live player input + go dark (invariant #5).
    Embody { entity: Entity },
    /// Release a possessed unit back to order-driven control.
    Surface { entity: Entity },
    /// Start constructing a building for `faction` at `pos` (spends resources).
    Build {
        faction: Faction,
        kind: BuildingKind,
        pos: Vec2,
    },
    /// Upgrade a built camp one tier (spends resources).
    Upgrade { camp: Entity },
    /// Queue a unit for production at a built camp (spends resources).
    QueueProduction { camp: Entity, unit: UnitKind },
}

/// The simulation: the deterministic world, the static terrain, per-faction resources, the
/// territory control state, the per-tick event stream, the seeded RNG, and the tick counter.
pub struct Sim {
    pub world: World,
    pub terrain: Terrain,
    pub resources: Resources,
    pub territory: Territory,
    /// Facts emitted this tick (combat/territory/economy); cleared at the top of every step.
    /// Derived, transient signal for alerts/audio — NOT folded into the checksum.
    pub events: Vec<SimEvent>,
    rng: Rng,
    tick: u64,
}

impl Sim {
    pub fn new(seed: u64) -> Self {
        Sim {
            world: World::new(),
            terrain: Terrain::open(),
            resources: Resources::default(),
            territory: Territory::empty(),
            events: Vec::new(),
            rng: Rng::new(seed),
            tick: 0,
        }
    }

    #[inline]
    pub fn tick_count(&self) -> u64 {
        self.tick
    }

    #[inline]
    pub fn rng(&mut self) -> &mut Rng {
        &mut self.rng
    }

    /// This tick's emitted events (read-only). Valid until the next [`Sim::step`].
    #[inline]
    pub fn events(&self) -> &[SimEvent] {
        &self.events
    }

    /// Apply this tick's commands, then advance every system one tick in a fixed order.
    pub fn step(&mut self, commands: &[Command]) {
        self.events.clear();
        for c in commands {
            self.apply(*c);
        }
        // Fixed system order (deterministic): move → fight → capture → economy.
        orders::order_system(&mut self.world, &self.terrain);
        combat::combat_system(
            &mut self.world,
            &self.terrain,
            &mut self.rng,
            &mut self.events,
        );
        territory::territory_system(&self.world, &mut self.territory, &mut self.events);
        economy::economy_system(
            &mut self.world,
            &mut self.resources,
            &self.territory,
            &mut self.events,
            &mut self.rng,
        );
        self.tick += 1;
    }

    fn apply(&mut self, c: Command) {
        match c {
            Command::Move { entity, target } => {
                if self.world.is_alive(entity) {
                    self.world.order[entity.index as usize] = Order::MoveTo(target);
                }
            }
            Command::AttackMove { entity, target } => {
                if self.world.is_alive(entity) {
                    self.world.order[entity.index as usize] = Order::AttackMove(target);
                }
            }
            Command::SetOrder { entity, order } => {
                if self.world.is_alive(entity) {
                    self.world.order[entity.index as usize] = order;
                }
            }
            Command::SetStance { entity, stance } => {
                if self.world.is_alive(entity) {
                    self.world.stance[entity.index as usize] = stance;
                }
            }
            Command::SetRetreatThreshold { entity, fraction } => {
                if self.world.is_alive(entity) {
                    self.world.retreat_below[entity.index as usize] = fraction;
                }
            }
            Command::Embody { entity } => {
                if self.world.is_alive(entity) {
                    self.world.input_source[entity.index as usize] = InputSource::Embodied;
                }
            }
            Command::Surface { entity } => {
                if self.world.is_alive(entity) {
                    self.world.input_source[entity.index as usize] = InputSource::Orders;
                }
            }
            Command::Build { faction, kind, pos } => {
                economy::build(&mut self.world, &mut self.resources, faction, kind, pos);
            }
            Command::Upgrade { camp } => {
                economy::upgrade(&mut self.world, &mut self.resources, camp);
            }
            Command::QueueProduction { camp, unit } => {
                economy::queue_production(&mut self.world, &mut self.resources, camp, unit);
            }
        }
    }

    /// Fold the whole world into a per-tick checksum in stable index order (invariant #7).
    /// Every per-entity component, plus the global resources and territory, is folded; the
    /// transient event stream and the derived fog/alerts are deliberately excluded.
    pub fn checksum(&self) -> u64 {
        let mut cs = Checksum::new();
        cs.write_u64(self.tick);
        for i in 0..self.world.capacity() {
            cs.write_u8(self.world.is_index_alive(i) as u8);
            let p = self.world.pos[i];
            let v = self.world.vel[i];
            cs.write_i32(p.x.to_bits());
            cs.write_i32(p.y.to_bits());
            cs.write_i32(v.x.to_bits());
            cs.write_i32(v.y.to_bits());
            write_order(&mut cs, self.world.order[i]);
            cs.write_u8(stance_tag(self.world.stance[i]));
            cs.write_u8(input_tag(self.world.input_source[i]));
            cs.write_u8(faction_tag(self.world.faction[i]));
            cs.write_u8(kind_tag(self.world.kind[i]));
            let h = self.world.health[i];
            cs.write_i32(h.cur.to_bits());
            cs.write_i32(h.max.to_bits());
            let w = self.world.weapon[i];
            cs.write_i32(w.range.to_bits());
            cs.write_i32(w.damage.to_bits());
            cs.write_u32(w.cooldown_ticks as u32);
            cs.write_u32(w.cooldown_left as u32);
            cs.write_i32(self.world.suppression[i].to_bits());
            match self.world.last_attacker[i] {
                Some(e) => {
                    cs.write_u8(1);
                    cs.write_u32(e.index);
                    cs.write_u32(e.generation);
                }
                None => cs.write_u8(0),
            }
            cs.write_i32(self.world.retreat_below[i].to_bits());
            cs.write_i32(self.world.vision[i].to_bits());
            write_building(&mut cs, &self.world.building[i]);
        }
        // Global per-faction resources, in fixed faction order.
        for f in Faction::ALL {
            cs.write_u64(self.resources.get(f) as u64);
        }
        // Territory control points, in stable vector order.
        cs.write_u32(self.territory.points.len() as u32);
        for cp in &self.territory.points {
            cs.write_i32(cp.pos.x.to_bits());
            cs.write_i32(cp.pos.y.to_bits());
            cs.write_u8(faction_tag(cp.owner));
            cs.write_i32(cp.progress.to_bits());
        }
        // RNG state — folds draw-count divergence in immediately (invariant #7).
        let (rng_state, rng_inc) = self.rng.checksum_state();
        cs.write_u64(rng_state);
        cs.write_u64(rng_inc);
        cs.finish()
    }

    /// Capture a read-only render snapshot (invariant #4).
    pub fn snapshot(&self) -> Snapshot {
        Snapshot::capture(&self.world, &self.territory, self.tick)
    }
}

fn write_order(cs: &mut Checksum, o: Order) {
    match o {
        Order::Idle => cs.write_u8(0),
        Order::MoveTo(t) => {
            cs.write_u8(1);
            cs.write_i32(t.x.to_bits());
            cs.write_i32(t.y.to_bits());
        }
        Order::AttackMove(t) => {
            cs.write_u8(2);
            cs.write_i32(t.x.to_bits());
            cs.write_i32(t.y.to_bits());
        }
        Order::Patrol { a, b, toward_b } => {
            cs.write_u8(3);
            cs.write_i32(a.x.to_bits());
            cs.write_i32(a.y.to_bits());
            cs.write_i32(b.x.to_bits());
            cs.write_i32(b.y.to_bits());
            cs.write_u8(toward_b as u8);
        }
        Order::HoldPosition => cs.write_u8(4),
        Order::FallBack(t) => {
            cs.write_u8(5);
            cs.write_i32(t.x.to_bits());
            cs.write_i32(t.y.to_bits());
        }
    }
}

fn write_building(cs: &mut Checksum, b: &Building) {
    cs.write_u8(building_kind_tag(b.kind));
    cs.write_u8(b.level);
    cs.write_u32(b.build_ticks_left as u32);
    cs.write_u32(b.queue.len() as u32);
    for item in &b.queue {
        cs.write_u8(unit_kind_tag(item.kind));
        cs.write_u32(item.ticks_left as u32);
    }
}

fn stance_tag(s: Stance) -> u8 {
    match s {
        Stance::HoldFire => 0,
        Stance::ReturnFire => 1,
        Stance::FireAtWill => 2,
    }
}

fn input_tag(s: InputSource) -> u8 {
    match s {
        InputSource::Orders => 0,
        InputSource::Embodied => 1,
    }
}

fn faction_tag(f: Faction) -> u8 {
    match f {
        Faction::Player => 0,
        Faction::Enemy => 1,
        Faction::Neutral => 2,
    }
}

fn kind_tag(k: EntityKind) -> u8 {
    match k {
        EntityKind::Unit => 0,
        EntityKind::Building => 1,
    }
}

fn building_kind_tag(k: BuildingKind) -> u8 {
    match k {
        BuildingKind::Camp => 0,
    }
}

fn unit_kind_tag(k: UnitKind) -> u8 {
    match k {
        UnitKind::Rifleman => 0,
        UnitKind::Heavy => 1,
    }
}
