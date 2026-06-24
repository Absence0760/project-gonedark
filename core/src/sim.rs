//! The deterministic fixed-tick simulation (invariants #1, #4).
//!
//! [`Sim::step`] advances the world by exactly one tick from the commands applied that
//! tick. The renderer reads [`Sim::snapshot`] and interpolates, but never calls in here to
//! mutate state. The tick rate is parameterized (see [`TICK_HZ`]) and not yet locked.

use crate::checksum::Checksum;
use crate::components::{InputSource, Order, Stance, Vec2};
use crate::ecs::{Entity, World};
use crate::rng::Rng;
use crate::snapshot::Snapshot;
use crate::systems;

/// Sim tick rate (Hz). TARGET ~60 for embodied combat (decisions.md D16); whether the
/// whole sim runs global-60 or a dual-rate split is OPEN (open-questions.md Q10) and MUST
/// be profiled on real arm64 before being locked. Parameterized on purpose — this is a
/// provisional value, NOT a settled decision.
pub const TICK_HZ: u32 = 60;

/// A command fed into the sim on a tick — the lockstep "order" unit. Commands are applied
/// in the order given (stable), before systems run.
#[derive(Clone, Copy, Debug)]
pub enum Command {
    /// Issue a move order (literal executor follows it).
    Move { entity: Entity, target: Vec2 },
    /// Change a unit's engagement stance.
    SetStance { entity: Entity, stance: Stance },
    /// Possess a unit: swap its input source to live player input + go dark (invariant #5).
    Embody { entity: Entity },
    /// Release a possessed unit back to order-driven control.
    Surface { entity: Entity },
}

/// The simulation: the deterministic world, its seeded RNG, and the tick counter.
pub struct Sim {
    pub world: World,
    rng: Rng,
    tick: u64,
}

impl Sim {
    pub fn new(seed: u64) -> Self {
        Sim {
            world: World::new(),
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

    /// Apply this tick's commands, then advance every system one tick.
    pub fn step(&mut self, commands: &[Command]) {
        for c in commands {
            self.apply(*c);
        }
        systems::movement_system(&mut self.world);
        self.tick += 1;
    }

    fn apply(&mut self, c: Command) {
        match c {
            Command::Move { entity, target } => {
                if self.world.is_alive(entity) {
                    self.world.order[entity.index as usize] = Order::MoveTo(target);
                }
            }
            Command::SetStance { entity, stance } => {
                if self.world.is_alive(entity) {
                    self.world.stance[entity.index as usize] = stance;
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
        }
    }

    /// Fold the whole world into a per-tick checksum in stable index order (invariant #7).
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
            cs.write_u8(order_tag(self.world.order[i]));
            if let Order::MoveTo(t) = self.world.order[i] {
                cs.write_i32(t.x.to_bits());
                cs.write_i32(t.y.to_bits());
            }
            cs.write_u8(stance_tag(self.world.stance[i]));
            cs.write_u8(input_tag(self.world.input_source[i]));
        }
        cs.finish()
    }

    /// Capture a read-only render snapshot (invariant #4).
    pub fn snapshot(&self) -> Snapshot {
        Snapshot::capture(&self.world, self.tick)
    }
}

fn order_tag(o: Order) -> u8 {
    match o {
        Order::Idle => 0,
        Order::MoveTo(_) => 1,
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
