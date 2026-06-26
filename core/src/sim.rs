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
    Building, BuildingKind, EntityKind, Faction, Health, InputSource, Order, Posture,
    ProductionItem, Stance, UnitKind, Vec2, Weapon,
};
use crate::economy::{self, Resources};
use crate::ecs::{Entity, World, WorldComponents};
use crate::event::SimEvent;
use crate::fixed::Fixed;
use crate::orders;
use crate::persist::{DeserializeError, Reader, StateSink, Writer};
use crate::rng::Rng;
use crate::snapshot::Snapshot;
use crate::systems;
use crate::trig::Angle;
use crate::terrain::{MapId, Terrain};
use crate::territory::{self, ControlPoint, Territory};

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
    /// An embodied unit fires its weapon along `dir` (a unit aim vector, already quantized to
    /// `Fixed` bits at the host boundary — invariant #1). The sim resolves a fixed-point cone
    /// hitscan ([`combat::resolve_fire`]); embodied units fire ONLY via this command, never the
    /// auto-combat resolver (combat skips `InputSource::Embodied`). Sim-authoritative: the hit is
    /// decided here, on every peer identically, not on the firing host.
    Fire { entity: Entity, dir: Vec2 },
    /// Drive an embodied unit one tick along `dir` — the first-person locomotion intent (the
    /// twin-stick / WASD avatar mover). `dir` is the desired heading already quantized to `Fixed`
    /// at the host boundary (invariant #1, exactly like [`Fire`](Self::Fire)'s aim); its magnitude
    /// is the analog deflection so a half-pushed stick walks at half speed. Applied via
    /// [`systems::step_along`](crate::systems::step_along) at the base
    /// [`MOVE_SPEED`](crate::systems::MOVE_SPEED) and **only** for a unit whose `input_source` is
    /// `Embodied` — a `Locomote` for an order-driven (or dead) unit is a no-op, mirroring how
    /// `combat` ignores embodied units. One such command is emitted per embodied unit per tick the
    /// stick is live; it enters the same lockstep stream as taps/fire and so stays bit-identical
    /// across peers (invariant #7).
    Locomote { entity: Entity, dir: Vec2 },
    /// An embodied unit starts reloading its weapon (the first-person Reload button). A no-op
    /// unless the unit is alive, has a magazine (`mag_size > 0`), is not already reloading, and
    /// its magazine is not already full — so a spurious tap costs nothing. Sets `reload_left`;
    /// `combat`'s upkeep counts it down and refills the magazine when it hits zero. Like
    /// [`Fire`](Self::Fire)/[`Locomote`](Self::Locomote) it rides the lockstep stream and is
    /// applied identically on every peer (invariant #7).
    Reload { entity: Entity },
    /// Set an embodied unit's body posture (the first-person Crouch toggle). The host carries the
    /// toggle state and emits this only on a change; the sim just writes `posture[entity]`.
    /// Crouching slows movement and tightens/extends the embodied aim (`combat`/`systems`). A
    /// no-op for a dead unit; harmless for an order-driven one (posture only affects the embodied
    /// fire/move paths). Lockstep-streamed like the other embodied intents.
    Crouch { entity: Entity, crouched: bool },
    /// Slew an embodied tank's turret toward the look-stick bearing `dir` (tank embodiment P2,
    /// D55). `dir` is the desired absolute aim, already quantized to `Fixed` at the host boundary
    /// (invariant #1, exactly like [`Fire`](Self::Fire)/[`Locomote`](Self::Locomote)). The sim
    /// turns `turret_yaw` one step (at the weapon's `turret_speed`) toward `atan2(dir.y, dir.x)`
    /// via [`trig::rotate_toward`](crate::trig::rotate_toward). EMBODIED-ONLY (`alive && Embodied`,
    /// else a no-op, mirroring `Locomote`); a zero `dir` is a no-op (no bearing to aim at). One per
    /// embodied tank per tick the look-stick is live; rides the lockstep stream (invariant #7).
    AimTurret { entity: Entity, dir: Vec2 },
    /// Drive an embodied tank's chassis one tick along `dir` — the vehicle locomotion intent that
    /// replaces [`Locomote`](Self::Locomote) for tanks (tank embodiment P2, D55). Unlike infantry's
    /// instant strafe, `dir` turns the **hull heading** (rate-limited) and drives forward along it
    /// with inertia (`hull_speed` accelerates/brakes toward the stick), so the tank turns-then-moves
    /// and a released stick coasts to a halt. `dir` is host-quantized to `Fixed` (invariant #1); its
    /// magnitude is the analog throttle. EMBODIED-ONLY (`alive && Embodied`, else a no-op).
    /// Lockstep-streamed like the other embodied intents (invariant #7).
    DriveHull { entity: Entity, dir: Vec2 },
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
    /// Which static map this sim is on. Terrain is static map data (not per-tick state, not
    /// checksummed), so the authoritative snapshot (D28) serializes this small id and re-derives
    /// `terrain` from it on resume, never the `GRID×GRID` grid. The scene is map id 0.
    map_id: MapId,
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
            map_id: Terrain::SCENE_MAP_ID,
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
        // Fixed system order (deterministic): move → orient → fight → capture → economy.
        orders::order_system(&mut self.world, &self.terrain);
        // Cosmetic AI hull/turret slew, AFTER movement sets this tick's velocity (D55 P2).
        systems::heading_system(&mut self.world);
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
            Command::Fire { entity, dir } => {
                if self.world.is_alive(entity) {
                    combat::resolve_fire(
                        &mut self.world,
                        &self.terrain,
                        entity.index as usize,
                        dir,
                        &mut self.events,
                    );
                }
            }
            Command::Locomote { entity, dir } => {
                // Embodied avatar only: an order-driven (or dead) unit ignores live locomotion,
                // exactly as `orders::order_system` skips embodied units. Applied here in the
                // command phase (before systems run); the order system won't overwrite it.
                let i = entity.index as usize;
                if self.world.is_alive(entity)
                    && self.world.input_source[i] == InputSource::Embodied
                {
                    // Crouch slows the avatar (the mobility cost of the marksman stance); standing
                    // moves at the base speed. Posture is a deterministic per-unit field, so the
                    // chosen speed is identical on every peer (invariant #1/#7).
                    let speed = if self.world.posture[i] == Posture::Crouched {
                        systems::CROUCH_MOVE_SPEED
                    } else {
                        systems::MOVE_SPEED
                    };
                    systems::step_along(&mut self.world, i, dir, speed);
                }
            }
            Command::Reload { entity } => {
                // Begin a reload on a live, magazine-armed unit that is not already reloading and
                // not already topped up. Combat's upkeep counts `reload_left` down and refills.
                if self.world.is_alive(entity) {
                    let w = &mut self.world.weapon[entity.index as usize];
                    if w.mag_size > 0 && w.reload_left == 0 && w.ammo < w.mag_size {
                        w.reload_left = w.reload_ticks;
                    }
                }
            }
            Command::Crouch { entity, crouched } => {
                if self.world.is_alive(entity) {
                    self.world.posture[entity.index as usize] = if crouched {
                        Posture::Crouched
                    } else {
                        Posture::Standing
                    };
                }
            }
            Command::AimTurret { entity, dir } => {
                // Embodied-only, exactly like Locomote/Fire. A zero stick has no bearing → no-op.
                let i = entity.index as usize;
                if dir != Vec2::ZERO
                    && self.world.is_alive(entity)
                    && self.world.input_source[i] == InputSource::Embodied
                {
                    let bearing = crate::trig::atan2(dir.y, dir.x);
                    let step = self.world.weapon[i].turret_speed as i32;
                    self.world.turret_yaw[i] =
                        crate::trig::rotate_toward(self.world.turret_yaw[i], bearing, step);
                }
            }
            Command::DriveHull { entity, dir } => {
                // Embodied-only vehicle locomotion (turn-then-drive + inertia). Order-driven (or
                // dead) units ignore it, as order_system skips embodied units. A zero/near-zero dir
                // brakes the hull to rest rather than no-opping (the stick was released).
                let i = entity.index as usize;
                if self.world.is_alive(entity)
                    && self.world.input_source[i] == InputSource::Embodied
                {
                    systems::drive_hull(&mut self.world, i, dir);
                }
            }
        }
    }

    /// Fold the whole world into a per-tick checksum in stable index order (invariant #7).
    /// Drives the shared [`fold`](Self::fold) walk through a [`Checksum`] sink; every per-entity
    /// component, plus the global resources, territory, and RNG state, is folded; the transient
    /// event stream and the derived fog/alerts are deliberately excluded.
    pub fn checksum(&self) -> u64 {
        let mut cs = Checksum::new();
        self.fold(&mut cs);
        cs.finish()
    }

    /// The single authoritative field-walk shared by [`checksum`](Self::checksum) and
    /// [`serialize`](Self::serialize) (D28 §4). It emits **exactly** the bytes the per-tick
    /// checksum has always folded, in the same order — `tick`, then each slot's `is_index_alive`
    /// byte + component data in `0..capacity()`, then per-faction resources, the territory points,
    /// and the RNG `(state, inc)`. Both a [`Checksum`] sink (which hashes the bytes) and a
    /// [`Writer`] sink (which records them) drive this, so anything folded into the checksum is
    /// serialized for free and the two can never silently drift.
    ///
    /// **It does NOT emit the liveness triple's `generation[]` or the free-list order** — the
    /// checksum has never hashed those, and adding them here would move the checksum stream and
    /// break lockstep compatibility. The resume-only liveness extras are written by
    /// [`serialize`](Self::serialize) *around* this walk, never inside it (D28's subtlety).
    fn fold<S: StateSink>(&self, sink: &mut S) {
        sink.write_u64(self.tick);
        for i in 0..self.world.capacity() {
            sink.write_u8(self.world.is_index_alive(i) as u8);
            let p = self.world.pos[i];
            let v = self.world.vel[i];
            sink.write_i32(p.x.to_bits());
            sink.write_i32(p.y.to_bits());
            sink.write_i32(v.x.to_bits());
            sink.write_i32(v.y.to_bits());
            write_order(sink, self.world.order[i]);
            sink.write_u8(stance_tag(self.world.stance[i]));
            sink.write_u8(input_tag(self.world.input_source[i]));
            sink.write_u8(posture_tag(self.world.posture[i]));
            sink.write_u8(faction_tag(self.world.faction[i]));
            sink.write_u8(kind_tag(self.world.kind[i]));
            let h = self.world.health[i];
            sink.write_i32(h.cur.to_bits());
            sink.write_i32(h.max.to_bits());
            let w = self.world.weapon[i];
            sink.write_i32(w.range.to_bits());
            sink.write_i32(w.damage.to_bits());
            sink.write_u32(w.cooldown_ticks as u32);
            sink.write_u32(w.cooldown_left as u32);
            sink.write_u32(w.mag_size as u32);
            sink.write_u32(w.ammo as u32);
            sink.write_u32(w.reload_ticks as u32);
            sink.write_u32(w.reload_left as u32);
            sink.write_u32(w.turret_speed as u32);
            sink.write_i32(self.world.suppression[i].to_bits());
            match self.world.last_attacker[i] {
                Some(e) => {
                    sink.write_u8(1);
                    sink.write_u32(e.index);
                    sink.write_u32(e.generation);
                }
                None => sink.write_u8(0),
            }
            sink.write_i32(self.world.retreat_below[i].to_bits());
            sink.write_i32(self.world.vision[i].to_bits());
            write_building(sink, &self.world.building[i]);
            // Tank embodiment P2 (D55): hull/turret headings + chassis speed (all sim state).
            sink.write_i32(self.world.hull_heading[i].0);
            sink.write_i32(self.world.turret_yaw[i].0);
            sink.write_i32(self.world.hull_speed[i].to_bits());
        }
        // Global per-faction resources, in fixed faction order.
        for f in Faction::ALL {
            sink.write_u64(self.resources.get(f) as u64);
        }
        // Territory control points, in stable vector order.
        sink.write_u32(self.territory.points.len() as u32);
        for cp in &self.territory.points {
            sink.write_i32(cp.pos.x.to_bits());
            sink.write_i32(cp.pos.y.to_bits());
            sink.write_u8(faction_tag(cp.owner));
            sink.write_i32(cp.progress.to_bits());
        }
        // RNG state — folds draw-count divergence in immediately (invariant #7).
        let (rng_state, rng_inc) = self.rng.checksum_state();
        sink.write_u64(rng_state);
        sink.write_u64(rng_inc);
    }

    /// Serialize the **authoritative** sim state a reconnecting peer resumes from (D28). The bytes
    /// capture everything the checksum hashes (via the shared [`fold`](Self::fold) walk) **plus**
    /// the resume-only liveness extras the checksum does not hash (`generation[]` and the
    /// free-list *order*), so a deserialized sim is byte-identical state — its checksum stream
    /// stays bit-identical to a never-interrupted run.
    ///
    /// Distinct from the lossy render [`snapshot`](Self::snapshot): that one is for interpolation
    /// and is unfit for resume. `Fixed` crosses as `to_bits` (invariant #1); terrain crosses as a
    /// `map_id`, not the grid (it is re-derived on resume). The transient `events` are excluded.
    pub fn serialize(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_u8(SNAPSHOT_VERSION);
        w.write_u32(self.map_id as u32);
        // Slot capacity, written in the wrapper (NOT in `fold`, so the checksum stream is
        // untouched). It is the number of slots `fold` emits and the length of `generation[]`;
        // deserialize needs it up front to drive the slot loop, since the fold's per-slot data is
        // not otherwise self-delimiting against the global block that follows.
        w.write_u32(self.world.capacity() as u32);
        // The shared checksum walk: tick + every slot's components + resources + territory + rng.
        self.fold(&mut w);
        // Liveness extras the checksum never hashes but a resume needs. `alive[]` is already in
        // the fold (the per-slot `is_index_alive` byte); `generation[]` and the free-list ORDER
        // are not — and the free-list order decides the next spawn's slot, so it is sim state.
        for &g in self.world.generations() {
            w.write_u32(g);
        }
        let free = self.world.free_list();
        w.write_u32(free.len() as u32);
        for &idx in free {
            w.write_u32(idx);
        }
        // Render-facing metadata that is NOT in the checksum (so it stays out of `fold`): each
        // slot's `unit_kind` (the producible archetype the renderer maps to a mesh). It is set
        // deterministically from the production queue, so a resume restores the same kind a
        // never-interrupted run holds — but its gameplay effect is already captured by the spawned
        // health/weapon stats, so it must never enter the checksum stream (invariant #7). One tag
        // byte per slot, in `0..capacity()` order (mirrors `generation[]`).
        for &k in &self.world.unit_kind {
            w.write_u8(unit_kind_tag(k));
        }
        w.into_bytes()
    }

    /// Reconstruct a [`Sim`] from [`serialize`](Self::serialize) bytes. The exact inverse of the
    /// serialize walk: it mirrors the shared [`fold`](Self::fold) field order to read back every
    /// component slot, the global state, and the RNG, then reads the liveness extras and rebuilds
    /// the world (re-deriving `terrain` from the `map_id`). Never panics — malformed input is a
    /// [`DeserializeError`] (bad version/tag, short buffer, inconsistent liveness, trailing bytes).
    pub fn deserialize(bytes: &[u8]) -> Result<Sim, DeserializeError> {
        let mut r = Reader::new(bytes);
        let ver = r.read_u8()?;
        if ver != SNAPSHOT_VERSION {
            return Err(DeserializeError::BadVersion(ver));
        }
        let map_id = MapId::try_from(r.read_u32()?).map_err(|_| DeserializeError::CorruptState)?;
        // Re-derive terrain from the id now, rejecting an unknown map loudly (a newer build's
        // snapshot must not silently fall back to the wrong terrain — invariant #7).
        let terrain = Terrain::from_map_id(map_id).ok_or(DeserializeError::UnknownMapId(map_id))?;

        // Slot capacity (written by `serialize` before the fold). Bound it against the remaining
        // bytes so a garbage value can't drive a huge pre-allocation; the smallest possible slot
        // encoding exceeds 1 byte, so `cap` cannot exceed the bytes left.
        let cap = r.read_u32()? as usize;
        if cap > r.remaining() {
            return Err(DeserializeError::LengthOverflow);
        }

        // --- mirror fold() exactly: tick, then each slot's components in 0..cap ---
        let tick = r.read_u64()?;

        let mut alive = Vec::with_capacity(cap);
        let mut pos = Vec::with_capacity(cap);
        let mut vel = Vec::with_capacity(cap);
        let mut order = Vec::with_capacity(cap);
        let mut stance = Vec::with_capacity(cap);
        let mut input_source = Vec::with_capacity(cap);
        let mut posture = Vec::with_capacity(cap);
        let mut faction = Vec::with_capacity(cap);
        let mut kind = Vec::with_capacity(cap);
        let mut health = Vec::with_capacity(cap);
        let mut weapon = Vec::with_capacity(cap);
        let mut suppression = Vec::with_capacity(cap);
        let mut last_attacker = Vec::with_capacity(cap);
        let mut retreat_below = Vec::with_capacity(cap);
        let mut vision = Vec::with_capacity(cap);
        let mut building = Vec::with_capacity(cap);
        let mut hull_heading = Vec::with_capacity(cap);
        let mut turret_yaw = Vec::with_capacity(cap);
        let mut hull_speed = Vec::with_capacity(cap);

        for _ in 0..cap {
            alive.push(r.read_u8()? != 0);
            pos.push(read_vec2(&mut r)?);
            vel.push(read_vec2(&mut r)?);
            order.push(read_order(&mut r)?);
            stance.push(read_stance(&mut r)?);
            input_source.push(read_input(&mut r)?);
            posture.push(read_posture(&mut r)?);
            faction.push(read_faction(&mut r)?);
            kind.push(read_kind(&mut r)?);
            health.push(Health {
                cur: Fixed::from_bits(r.read_i32()?),
                max: Fixed::from_bits(r.read_i32()?),
            });
            weapon.push(Weapon {
                range: Fixed::from_bits(r.read_i32()?),
                damage: Fixed::from_bits(r.read_i32()?),
                cooldown_ticks: read_u16(&mut r)?,
                cooldown_left: read_u16(&mut r)?,
                mag_size: read_u16(&mut r)?,
                ammo: read_u16(&mut r)?,
                reload_ticks: read_u16(&mut r)?,
                reload_left: read_u16(&mut r)?,
                turret_speed: read_u16(&mut r)?,
            });
            suppression.push(Fixed::from_bits(r.read_i32()?));
            last_attacker.push(read_opt_entity(&mut r)?);
            retreat_below.push(Fixed::from_bits(r.read_i32()?));
            vision.push(Fixed::from_bits(r.read_i32()?));
            building.push(read_building(&mut r)?);
            // Tank embodiment P2 (D55): mirror fold()'s hull/turret/speed trio, in the same order.
            hull_heading.push(Angle(r.read_i32()?));
            turret_yaw.push(Angle(r.read_i32()?));
            hull_speed.push(Fixed::from_bits(r.read_i32()?));
        }

        // Global per-faction resources, fixed faction order.
        let mut resources = Resources::default();
        for f in Faction::ALL {
            resources.amounts[f.index()] = r.read_u64()? as i64;
        }

        // Territory control points, stable vector order.
        let n_points = r.read_len(MIN_CONTROL_POINT_BYTES)?;
        let mut points = Vec::with_capacity(n_points);
        for _ in 0..n_points {
            points.push(ControlPoint {
                pos: read_vec2(&mut r)?,
                owner: read_faction(&mut r)?,
                progress: Fixed::from_bits(r.read_i32()?),
            });
        }
        let territory = Territory { points };

        // RNG (state, inc).
        let rng_state = r.read_u64()?;
        let rng_inc = r.read_u64()?;
        let rng = Rng::from_state(rng_state, rng_inc);

        // --- liveness extras (serialize-only; not in the checksum) ---
        let mut generation = Vec::with_capacity(cap);
        for _ in 0..cap {
            generation.push(r.read_u32()?);
        }
        let n_free = r.read_len(4)?;
        let mut free = Vec::with_capacity(n_free);
        for _ in 0..n_free {
            free.push(r.read_u32()?);
        }

        // Render-facing metadata (serialize-only; not in the checksum): one `unit_kind` tag per
        // slot, in 0..cap order — the exact inverse of the block `serialize` writes after the
        // free list.
        let mut unit_kind = Vec::with_capacity(cap);
        for _ in 0..cap {
            unit_kind.push(read_unit_kind(&mut r)?);
        }

        r.expect_end()?;

        let world = World::from_parts(
            generation,
            alive,
            free,
            WorldComponents {
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
                hull_heading,
                turret_yaw,
                hull_speed,
            },
        )
        .ok_or(DeserializeError::CorruptState)?;

        Ok(Sim {
            world,
            terrain,
            resources,
            territory,
            events: Vec::new(),
            map_id,
            rng,
            tick,
        })
    }

    /// Capture a read-only render snapshot (invariant #4).
    pub fn snapshot(&self) -> Snapshot {
        Snapshot::capture(&self.world, &self.territory, self.tick)
    }
}

/// Authoritative-snapshot format version (D28). Bumped on any layout change so a stale snapshot is
/// rejected ([`DeserializeError::BadVersion`]) rather than silently misparsed into a divergent
/// world. Independent of the lockstep wire version — different codec, different evolution.
const SNAPSHOT_VERSION: u8 = 4;

/// Smallest possible encoding of one `ControlPoint`: `pos` (2×i32) + owner tag (u8) + progress
/// (i32) = 13 bytes. Used to reject a garbage point count before allocating.
const MIN_CONTROL_POINT_BYTES: usize = 13;

fn write_order<S: StateSink>(sink: &mut S, o: Order) {
    match o {
        Order::Idle => sink.write_u8(0),
        Order::MoveTo(t) => {
            sink.write_u8(1);
            sink.write_i32(t.x.to_bits());
            sink.write_i32(t.y.to_bits());
        }
        Order::AttackMove(t) => {
            sink.write_u8(2);
            sink.write_i32(t.x.to_bits());
            sink.write_i32(t.y.to_bits());
        }
        Order::Patrol { a, b, toward_b } => {
            sink.write_u8(3);
            sink.write_i32(a.x.to_bits());
            sink.write_i32(a.y.to_bits());
            sink.write_i32(b.x.to_bits());
            sink.write_i32(b.y.to_bits());
            sink.write_u8(toward_b as u8);
        }
        Order::HoldPosition => sink.write_u8(4),
        Order::FallBack(t) => {
            sink.write_u8(5);
            sink.write_i32(t.x.to_bits());
            sink.write_i32(t.y.to_bits());
        }
    }
}

fn write_building<S: StateSink>(sink: &mut S, b: &Building) {
    sink.write_u8(building_kind_tag(b.kind));
    sink.write_u8(b.level);
    sink.write_u32(b.build_ticks_left as u32);
    sink.write_u32(b.queue.len() as u32);
    for item in &b.queue {
        sink.write_u8(unit_kind_tag(item.kind));
        sink.write_u32(item.ticks_left as u32);
    }
}

// --- decode helpers: the exact inverse of the encoders above (D28 deserialize path) ---

fn read_u16(r: &mut Reader) -> Result<u16, DeserializeError> {
    // Encoded as a u32 (matching the checksum's `cooldown_ticks as u32` / queue `ticks_left`),
    // so it round-trips through the same byte width; a value above u16 range is corruption.
    u16::try_from(r.read_u32()?).map_err(|_| DeserializeError::CorruptState)
}

fn read_vec2(r: &mut Reader) -> Result<Vec2, DeserializeError> {
    Ok(Vec2::new(
        Fixed::from_bits(r.read_i32()?),
        Fixed::from_bits(r.read_i32()?),
    ))
}

fn read_opt_entity(r: &mut Reader) -> Result<Option<Entity>, DeserializeError> {
    Ok(match r.read_u8()? {
        0 => None,
        1 => Some(Entity {
            index: r.read_u32()?,
            generation: r.read_u32()?,
        }),
        t => return Err(DeserializeError::BadTag(t)),
    })
}

fn read_order(r: &mut Reader) -> Result<Order, DeserializeError> {
    Ok(match r.read_u8()? {
        0 => Order::Idle,
        1 => Order::MoveTo(read_vec2(r)?),
        2 => Order::AttackMove(read_vec2(r)?),
        3 => Order::Patrol {
            a: read_vec2(r)?,
            b: read_vec2(r)?,
            toward_b: r.read_u8()? != 0,
        },
        4 => Order::HoldPosition,
        5 => Order::FallBack(read_vec2(r)?),
        t => return Err(DeserializeError::BadTag(t)),
    })
}

fn read_stance(r: &mut Reader) -> Result<Stance, DeserializeError> {
    Ok(match r.read_u8()? {
        0 => Stance::HoldFire,
        1 => Stance::ReturnFire,
        2 => Stance::FireAtWill,
        t => return Err(DeserializeError::BadTag(t)),
    })
}

fn read_input(r: &mut Reader) -> Result<InputSource, DeserializeError> {
    Ok(match r.read_u8()? {
        0 => InputSource::Orders,
        1 => InputSource::Embodied,
        t => return Err(DeserializeError::BadTag(t)),
    })
}

fn read_posture(r: &mut Reader) -> Result<Posture, DeserializeError> {
    Ok(match r.read_u8()? {
        0 => Posture::Standing,
        1 => Posture::Crouched,
        t => return Err(DeserializeError::BadTag(t)),
    })
}

fn read_faction(r: &mut Reader) -> Result<Faction, DeserializeError> {
    Ok(match r.read_u8()? {
        0 => Faction::Player,
        1 => Faction::Enemy,
        2 => Faction::Neutral,
        t => return Err(DeserializeError::BadTag(t)),
    })
}

fn read_kind(r: &mut Reader) -> Result<EntityKind, DeserializeError> {
    Ok(match r.read_u8()? {
        0 => EntityKind::Unit,
        1 => EntityKind::Building,
        t => return Err(DeserializeError::BadTag(t)),
    })
}

fn read_building_kind(r: &mut Reader) -> Result<BuildingKind, DeserializeError> {
    Ok(match r.read_u8()? {
        0 => BuildingKind::Camp,
        t => return Err(DeserializeError::BadTag(t)),
    })
}

fn read_unit_kind(r: &mut Reader) -> Result<UnitKind, DeserializeError> {
    Ok(match r.read_u8()? {
        0 => UnitKind::Rifleman,
        1 => UnitKind::Heavy,
        t => return Err(DeserializeError::BadTag(t)),
    })
}

fn read_building(r: &mut Reader) -> Result<Building, DeserializeError> {
    let kind = read_building_kind(r)?;
    let level = r.read_u8()?;
    let build_ticks_left = read_u16(r)?;
    let n = r.read_len(MIN_PRODUCTION_ITEM_BYTES)?;
    let mut queue = Vec::with_capacity(n);
    for _ in 0..n {
        queue.push(ProductionItem {
            kind: read_unit_kind(r)?,
            ticks_left: read_u16(r)?,
        });
    }
    Ok(Building {
        kind,
        level,
        build_ticks_left,
        queue,
    })
}

/// Smallest encoding of one queued `ProductionItem`: unit-kind tag (u8) + ticks_left (u32) = 5
/// bytes. Used to reject a garbage queue length before allocating.
const MIN_PRODUCTION_ITEM_BYTES: usize = 5;

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

fn posture_tag(p: Posture) -> u8 {
    match p {
        Posture::Standing => 0,
        Posture::Crouched => 1,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::MOVE_SPEED;

    /// Spawn one embodied unit at the origin and return its entity + the sim.
    fn sim_with_embodied_unit() -> (Sim, Entity) {
        let mut sim = Sim::new(0);
        let e = sim.world.spawn();
        sim.world.input_source[e.index as usize] = InputSource::Embodied;
        (sim, e)
    }

    #[test]
    fn locomote_walks_the_embodied_unit_one_tick() {
        let (mut sim, e) = sim_with_embodied_unit();
        let dir = Vec2::new(Fixed::ONE, Fixed::ZERO);
        sim.step(&[Command::Locomote { entity: e, dir }]);
        // The avatar advances exactly dir * MOVE_SPEED; nothing else moves it (orders skips
        // embodied units, combat has no targets).
        assert_eq!(
            sim.world.pos[e.index as usize],
            Vec2::new(MOVE_SPEED, Fixed::ZERO)
        );
    }

    #[test]
    fn locomote_is_ignored_for_an_order_driven_unit() {
        let (mut sim, e) = sim_with_embodied_unit();
        // Surface the unit back to order-driven control: live locomotion must no-op now.
        sim.world.input_source[e.index as usize] = InputSource::Orders;
        let before = sim.world.pos[e.index as usize];
        sim.step(&[Command::Locomote {
            entity: e,
            dir: Vec2::new(Fixed::ONE, Fixed::ZERO),
        }]);
        assert_eq!(sim.world.pos[e.index as usize], before);
    }

    #[test]
    fn locomote_for_a_dead_entity_is_a_no_op() {
        // A stale handle (never spawned / wrong generation) must not panic or move anything. In a
        // zero-spawn sim the component spans are empty, so the `is_alive` guard is load-bearing:
        // without it `step_along(world, 0, …)` would index out of bounds and panic. The real
        // assertion is therefore "no panic"; the tick advancing to 1 proves `step` ran to
        // completion with the guard correctly suppressing the move.
        let mut sim = Sim::new(0);
        let stale = Entity {
            index: 0,
            generation: 7,
        };
        sim.step(&[Command::Locomote {
            entity: stale,
            dir: Vec2::new(Fixed::ONE, Fixed::ZERO),
        }]);
        assert_eq!(sim.tick_count(), 1);
    }

    #[test]
    fn locomote_speed_matches_an_ordered_move() {
        // The embodied mover and the order-driven mover share MOVE_SPEED, so a straight-line dash
        // covers the same ground per tick whether you possess the unit or order it. Compare one
        // tick of Locomote against one tick of step_along at the base speed.
        let (mut sim, e) = sim_with_embodied_unit();
        sim.step(&[Command::Locomote {
            entity: e,
            dir: Vec2::new(Fixed::ZERO, Fixed::ONE),
        }]);
        let mut reference = World::new();
        let r = reference.spawn();
        crate::systems::step_along(
            &mut reference,
            r.index as usize,
            Vec2::new(Fixed::ZERO, Fixed::ONE),
            MOVE_SPEED,
        );
        assert_eq!(
            sim.world.pos[e.index as usize],
            reference.pos[r.index as usize]
        );
    }

    // --- tank embodiment P2 (D55): AimTurret / DriveHull command routing --------------------

    #[test]
    fn aim_turret_slews_toward_the_bearing_by_turret_speed_then_holds() {
        let (mut sim, e) = sim_with_embodied_unit();
        let i = e.index as usize;
        sim.world.weapon[i].turret_speed = 200;
        let north = Vec2::new(Fixed::ZERO, Fixed::ONE); // bearing = ANGLE_FULL/4
        // First tick steps the turret toward +Y by exactly turret_speed.
        sim.step(&[Command::AimTurret { entity: e, dir: north }]);
        assert_eq!(sim.world.turret_yaw[i], crate::trig::Angle(200));
        // Held long enough, it reaches the bearing and then holds (no overshoot, no drift).
        let quarter = crate::trig::ANGLE_FULL / 4;
        for _ in 0..(quarter / 200 + 4) {
            sim.step(&[Command::AimTurret { entity: e, dir: north }]);
        }
        assert_eq!(sim.world.turret_yaw[i], crate::trig::Angle(quarter), "reaches the bearing");
        sim.step(&[Command::AimTurret { entity: e, dir: north }]);
        assert_eq!(sim.world.turret_yaw[i], crate::trig::Angle(quarter), "and holds it");
    }

    #[test]
    fn aim_turret_is_embodied_only_and_zero_dir_is_a_noop() {
        let (mut sim, e) = sim_with_embodied_unit();
        let i = e.index as usize;
        sim.world.weapon[i].turret_speed = 200;
        // A zero look-stick has no bearing → no slew.
        sim.step(&[Command::AimTurret { entity: e, dir: Vec2::ZERO }]);
        assert_eq!(sim.world.turret_yaw[i], crate::trig::Angle(0));
        // Surfaced (order-driven) → the live aim no-ops, and the AI slew leaves a held turret put.
        sim.world.input_source[i] = InputSource::Orders;
        sim.step(&[Command::AimTurret {
            entity: e,
            dir: Vec2::new(Fixed::ZERO, Fixed::ONE),
        }]);
        assert_eq!(sim.world.turret_yaw[i], crate::trig::Angle(0));
    }

    #[test]
    fn drive_hull_moves_an_embodied_tank_and_no_ops_when_surfaced() {
        let (mut sim, e) = sim_with_embodied_unit();
        let i = e.index as usize;
        let east = Vec2::new(Fixed::ONE, Fixed::ZERO); // along the initial +X heading
        sim.step(&[Command::DriveHull { entity: e, dir: east }]);
        // Accelerated from rest by one step and advanced along the hull heading.
        assert_eq!(sim.world.hull_speed[i], systems::HULL_ACCEL);
        assert!(sim.world.pos[i].x > Fixed::ZERO);
        assert_eq!(sim.world.pos[i].y, Fixed::ZERO);
        // Surface → DriveHull is ignored (order-driven units never take live locomotion).
        sim.world.input_source[i] = InputSource::Orders;
        let before = sim.world.pos[i];
        sim.step(&[Command::DriveHull { entity: e, dir: east }]);
        assert_eq!(sim.world.pos[i], before);
    }

    #[test]
    fn drive_hull_release_brakes_the_embodied_tank_to_rest() {
        let (mut sim, e) = sim_with_embodied_unit();
        let i = e.index as usize;
        let east = Vec2::new(Fixed::ONE, Fixed::ZERO);
        for _ in 0..50 {
            sim.step(&[Command::DriveHull { entity: e, dir: east }]);
        }
        assert_eq!(sim.world.hull_speed[i], MOVE_SPEED, "spun up to the cap");
        // Release the stick: a zero-dir DriveHull brakes the chassis to a full stop.
        for _ in 0..200 {
            sim.step(&[Command::DriveHull {
                entity: e,
                dir: Vec2::ZERO,
            }]);
        }
        assert_eq!(sim.world.hull_speed[i], Fixed::ZERO);
        assert_eq!(sim.world.vel[i], Vec2::ZERO);
    }

    #[test]
    fn aim_turret_for_a_dead_entity_is_a_no_op() {
        // A stale handle must not panic or index out of bounds: the embodied-only `is_alive` guard
        // is load-bearing in a zero-spawn sim (empty component spans). No panic + tick advancing to
        // 1 proves `step` ran the guarded arm to completion.
        let mut sim = Sim::new(0);
        let stale = Entity {
            index: 0,
            generation: 99,
        };
        sim.step(&[Command::AimTurret {
            entity: stale,
            dir: Vec2::new(Fixed::ZERO, Fixed::ONE),
        }]);
        assert_eq!(sim.tick_count(), 1);
    }

    #[test]
    fn drive_hull_for_a_dead_entity_is_a_no_op() {
        // Same contract as Locomote/AimTurret: a stale handle is dropped by the `is_alive` guard
        // before `drive_hull` would index the empty spans. The assertion is "no panic"; the tick
        // reaching 1 proves the guard suppressed the move and `step` completed.
        let mut sim = Sim::new(0);
        let stale = Entity {
            index: 0,
            generation: 99,
        };
        sim.step(&[Command::DriveHull {
            entity: stale,
            dir: Vec2::new(Fixed::ONE, Fixed::ZERO),
        }]);
        assert_eq!(sim.tick_count(), 1);
    }
}
