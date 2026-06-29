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
    Armor, Army, Building, BuildingKind, EntityKind, Faction, Health, InputSource, Order, Posture,
    ProductionItem, ShellKind, Stance, UnitKind, Vec2, Weapon, FACTION_COUNT,
};
use crate::economy::{self, Resources};
use crate::ecs::{Entity, World, WorldComponents};
use crate::event::SimEvent;
use crate::fixed::Fixed;
use crate::orders;
use crate::persist::{DeserializeError, Reader, StateSink, Writer};
use crate::projectile::{self, Projectile};
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
    /// **Match-setup**: select which real-army identity ([`Army`]) `faction` fields (factions-plan
    /// WS-A, [D68](../docs/decisions.md)). Writes the per-side army mapping
    /// ([`Sim::set_army`](Sim::set_army)) — a host/lobby intent (the [`shell`](crate::shell) seam's
    /// `SelectArmy` resolves to it, D34), normally issued at setup, before play. The army selection
    /// is match-config, not per-tick state, so it is **not** folded into the per-tick checksum (its
    /// gameplay effect — the per-army roster, WS-B — is what folds, invariant #7). Riding the
    /// lockstep stream still makes the *decode* identical on every peer (the wire codec carries the
    /// [`Army`] tag), so a setup command decodes to the same army everywhere (invariant #7), exactly
    /// like a `Build`'s `BuildingKind`.
    SelectArmy { faction: Faction, army: Army },
    /// Select the [`ShellKind`] an embodied tank's gun loads for its **next** shot (tank embodiment
    /// P6, D55). Cycles AP / APHE / HE, changing the launched shell's penetration / damage / splash
    /// ([`ShellKind::stats`], applied at fire time in
    /// [`projectile::fire_ballistic`](crate::projectile::fire_ballistic)). Writes the per-weapon
    /// `shell` field — folded per-tank sim state — so the decode is identical on every peer (the wire
    /// codec carries the [`ShellKind`] tag, invariant #7). A **no-op mid-reload** (you can't swap the
    /// chambered round while reloading) and on a dead/handle-stale unit. Harmless for a hitscan
    /// infantry weapon: the field rides along but the hitscan path never reads it.
    SelectShell { entity: Entity, shell: ShellKind },
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
    /// In-flight ballistic shells (tank embodiment P3, D55). A bounded pool advanced each tick by
    /// [`projectile::projectile_system`]; folded into the per-tick checksum + serialized (it is
    /// lockstep sim state — an in-flight shell decides a future hit). Empty in any scene with no
    /// embodied tank firing (every `muzzle_vel == 0` weapon stays hitscan), so it costs the
    /// existing checksum stream nothing.
    pub projectiles: Vec<Projectile>,
    /// Which static map this sim is on. Terrain is static map data (not per-tick state, not
    /// checksummed), so the authoritative snapshot (D28) serializes this small id and re-derives
    /// `terrain` from it on resume, never the `GRID×GRID` grid. The scene is map id 0.
    map_id: MapId,
    /// Ticks between income accruals — the scenario-local economy **pace lever** (the skirmish slows
    /// it; default `1` = accrue every tick, the unchanged full rate). Like [`map_id`](Self::map_id)
    /// it is static *per-match config*, not per-tick state: it is set at seed / deserialize time and
    /// never mutated by a system. So — exactly like `map_id` — it is serialized in the snapshot
    /// WRAPPER but **not** folded into the per-tick checksum. Its *effect* (the resource purse) IS
    /// folded, so two peers running different periods diverge in resources and the desync is caught
    /// on the next tick (invariant #7). A larger period stretches only the cadence, not the
    /// per-accrual amount, so a held point still ~triples income — the D30 cost/stat constants are
    /// untouched.
    income_period: u32,
    /// Per-side **army identity** selection — each [`Faction`] maps to one [`Army`] (factions-plan
    /// WS-A, [D68](decisions.md)). Indexed by [`Faction::index`]. Like [`income_period`](Self::income_period)
    /// /[`map_id`](Self::map_id) it is **static per-match config**: set at seed / `SelectArmy` /
    /// deserialize time and never mutated by a system. So — exactly like those — it is carried in the
    /// serialize WRAPPER but **not** folded into the per-tick checksum. Its *effect* (the per-army
    /// roster a faction draws from, WS-B) is what folds, so two peers that picked different armies
    /// diverge in spawned unit stats and the desync is caught there (invariant #7). Every faction
    /// defaults to [`Army::Neutral`], so a scene that selects no army is byte-identical to before.
    armies: [Army; FACTION_COUNT],
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
            projectiles: Vec::new(),
            map_id: Terrain::SCENE_MAP_ID,
            // Default full rate (accrue every tick) — identical income to before this lever existed,
            // so every existing scene's resources (and thus checksum) are byte-unchanged.
            income_period: 1,
            // Non-aligned by default (factions-plan WS-A): a fresh sim fields no real army, so every
            // existing scene's per-tick checksum is byte-unchanged until a scene/SelectArmy picks one.
            armies: [Army::Neutral; FACTION_COUNT],
            rng: Rng::new(seed),
            tick: 0,
        }
    }

    /// The [`Army`] identity a [`Faction`] fields (factions-plan WS-A). [`Army::Neutral`] until a
    /// scene seeder or a [`Command::SelectArmy`] selects one.
    #[inline]
    pub fn army_of(&self, faction: Faction) -> Army {
        self.armies[faction.index()]
    }

    /// Select which [`Army`] a [`Faction`] fields — the per-side match-setup choice (factions-plan
    /// WS-A). A scene seeder or the host (via [`Command::SelectArmy`] / the
    /// [`shell`](crate::shell) seam) calls this. Match-config: it never folds into the per-tick
    /// checksum (its roster effect, WS-B, is what folds — invariant #7).
    #[inline]
    pub fn set_army(&mut self, faction: Faction, army: Army) {
        self.armies[faction.index()] = army;
    }

    /// Set the income accrual **period** (ticks between income accruals) — the scenario-local economy
    /// pace lever. `1` (the default) accrues every tick (the full D30 rate); a larger value stretches
    /// the cadence proportionally, slowing the drip *without* touching the D30 cost/stat constants
    /// (the per-accrual amount is unchanged, so a held point still ~triples income). A scene seeder
    /// calls this; clamped to at least `1` so income always eventually accrues.
    pub fn set_income_period(&mut self, period: u32) {
        self.income_period = period.max(1);
    }

    /// The current income accrual period (ticks between accruals). `1` = the full per-tick rate.
    #[inline]
    pub fn income_period(&self) -> u32 {
        self.income_period
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
        // Fixed system order (deterministic): move → collide → orient → fight → capture → economy.
        orders::order_system(&mut self.world, &self.terrain);
        // Push movers (the embodied avatar moved in `apply`, AI units in `order_system`) out of any
        // building footprint — buildings are solid (you can't walk through them). Runs after ALL
        // movement, before the cosmetic slew/combat so positions are settled for the snapshot.
        systems::resolve_building_collisions(&mut self.world);
        // Cosmetic AI hull/turret slew, AFTER movement sets this tick's velocity (D55 P2).
        systems::heading_system(&mut self.world);
        // Tank aim-bloom settle (D55 P5): every tank gun's dispersion shrinks toward zero (pinpoint)
        // when it holds still and steady. Bloom is added separately at the embodied drive/aim sites
        // in `apply` (which ran above, in the command phase), so the per-tick net is bloom − settle.
        // Gated on `muzzle_vel > 0`, so a tank-free scene is byte-unchanged.
        crate::dispersion::dispersion_system(&mut self.world);
        combat::combat_system(
            &mut self.world,
            &self.terrain,
            &mut self.rng,
            &mut self.events,
        );
        // Advance in-flight shells + resolve impacts, AFTER auto-combat (D55 P3). Embodied tank
        // fire spawns shells in `apply`; this integrates their travel/drop and applies the same
        // cover-mitigated damage on impact.
        projectile::projectile_system(
            &mut self.world,
            &self.terrain,
            &mut self.projectiles,
            &mut self.events,
            projectile::GRAVITY,
        );
        // Medic healing (D65), AFTER combat/projectiles have settled this tick's damage and
        // despawned the dead (so a Medic never heals a corpse), before territory/economy. A no-op
        // when no Medic is present, so Medic-free scenes are byte-unchanged.
        crate::heal::heal_system(&mut self.world);
        // Ammo resupply (D67), AFTER combat/heal have settled this tick — a unit standing by a
        // friendly finished Camp/Barracks tops its carried reserve back up, the logistics half of
        // all-unit ammo. A building-free or ammo-free scene is a no-op (byte-unchanged checksum).
        crate::resupply::resupply_system(&mut self.world);
        territory::territory_system(&self.world, &mut self.territory, &mut self.events);
        economy::economy_system(
            &mut self.world,
            &mut self.resources,
            &self.territory,
            &mut self.events,
            &mut self.rng,
            self.tick,
            self.income_period,
            &self.armies,
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
                    let i = entity.index as usize;
                    // A ballistic gun (muzzle_vel > 0) on an embodied unit launches a shell instead
                    // of resolving an instant hitscan (D55 P3). `muzzle_vel == 0` (infantry, every
                    // existing unit) keeps the unchanged `resolve_fire` path — opt-in by a zero
                    // default. AI/auto fire never reaches here (combat skips embodied units), so the
                    // projectile path is embodied-only by construction (invariant #3).
                    if self.world.weapon[i].muzzle_vel > Fixed::ZERO
                        && self.world.input_source[i] == InputSource::Embodied
                    {
                        // The shell's launch direction is perturbed by the gun's current dispersion
                        // (D55 P5): a settled gun fires dead-on (zero scatter, no RNG draw), a
                        // moving/just-traversed one sprays — `fire_ballistic` draws the bounded
                        // offset from the reserved deterministic `rng` (invariant #7).
                        projectile::fire_ballistic(
                            &mut self.world,
                            i,
                            dir,
                            &mut self.projectiles,
                            &mut self.rng,
                        );
                    } else {
                        combat::resolve_fire(
                            &mut self.world,
                            &self.terrain,
                            i,
                            dir,
                            &mut self.events,
                        );
                    }
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
                    let before = self.world.turret_yaw[i];
                    let after = crate::trig::rotate_toward(before, bearing, step);
                    self.world.turret_yaw[i] = after;
                    // Tank embodiment P5 (D55): a turret that actually traversed this tick blooms the
                    // aim reticle (no-op for a fixed mount / non-tank gun — `bloom` self-gates).
                    if after != before {
                        crate::dispersion::bloom(
                            &mut self.world,
                            i,
                            crate::dispersion::DISPERSION_BLOOM_TRAVERSE,
                        );
                    }
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
                    // Tank embodiment P5 (D55): a hull that actually moved this tick blooms the aim
                    // reticle (no-op for a non-tank gun — `bloom` self-gates on `muzzle_vel > 0`).
                    if self.world.vel[i] != Vec2::ZERO {
                        crate::dispersion::bloom(
                            &mut self.world,
                            i,
                            crate::dispersion::DISPERSION_BLOOM_MOVE,
                        );
                    }
                }
            }
            Command::SelectArmy { faction, army } => {
                // Match-setup: record the per-side army identity (factions-plan WS-A). Writes only
                // the non-folded `armies` config, so applying it never moves the per-tick checksum —
                // its roster effect (WS-B) is what folds. Deterministic by construction (a plain tag
                // write, no float, no RNG).
                self.armies[faction.index()] = army;
            }
            Command::SelectShell { entity, shell } => {
                // Swap the loaded shell for the next shot (tank embodiment P6). A no-op while a
                // reload is in progress (`reload_left > 0`) — you can't change the chambered round
                // mid-reload — and for a dead/stale handle. Writes only the folded `shell` field, so
                // it is deterministic by construction (a plain tag write, no float, no RNG).
                if self.world.is_alive(entity) {
                    let w = &mut self.world.weapon[entity.index as usize];
                    if w.reload_left == 0 {
                        w.shell = shell;
                    }
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
            // All-unit ammo logistics (D67): carried reserve + its resupply cap. Both drive who can
            // fire (auto-reload + resupply), so they are sim state that folds into the checksum.
            sink.write_u32(w.reserve as u32);
            sink.write_u32(w.reserve_max as u32);
            sink.write_u32(w.turret_speed as u32);
            sink.write_i32(w.muzzle_vel.to_bits());
            // Tank embodiment P4 (D55): weapon armour penetration (sim state — drives the facing
            // multiplier). Zero for every existing weapon, so byte-neutral until a tank is fielded.
            sink.write_i32(w.penetration.to_bits());
            // Tank embodiment P5 (D55): current aim-time dispersion (reticle bloom). Real sim state —
            // it blooms/settles each tick and perturbs a launched shell (`dispersion::scatter_dir`).
            // Zero for every non-tank gun (the dispersion system gates on `muzzle_vel > 0`), so it
            // adds one zero word per slot and moves only an armoured-tank scene's stream value.
            sink.write_i32(w.dispersion.to_bits());
            // Tank embodiment P6 (D55): the loaded shell (sim state — decides the next shot's
            // pen/damage/splash). `Ap` (tag 0) for every existing weapon, so it appends one zero byte
            // per slot. APPENDED after `penetration` (keep this last among the weapon fields).
            sink.write_u8(shell_tag(w.shell));
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
            // Tank embodiment P4 (D55): directional armour (sim state). All-zero (unarmoured) for
            // every existing entity, so it adds three zero words per slot and moves nothing.
            let armor = self.world.armor[i];
            sink.write_i32(armor.front.to_bits());
            sink.write_i32(armor.side.to_bits());
            sink.write_i32(armor.rear.to_bits());
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
        // In-flight ballistic shells (tank embodiment P3, D55) — a global block AFTER the existing
        // globals, in stable pool order. An in-flight shell is lockstep sim state (it decides a
        // future impact), so it folds into the checksum + serializes. Empty (count 0) for every
        // scene with no embodied tank firing, so it is byte-neutral there.
        sink.write_u32(self.projectiles.len() as u32);
        for p in &self.projectiles {
            sink.write_i32(p.pos2d.x.to_bits());
            sink.write_i32(p.pos2d.y.to_bits());
            sink.write_i32(p.vel2d.x.to_bits());
            sink.write_i32(p.vel2d.y.to_bits());
            sink.write_i32(p.height.to_bits());
            sink.write_i32(p.vz.to_bits());
            sink.write_u32(p.owner.index);
            sink.write_u32(p.owner.generation);
            sink.write_u8(projectile::faction_tag(p.faction));
            sink.write_i32(p.damage.to_bits());
            sink.write_i32(p.penetration.to_bits());
            sink.write_u32(p.lifetime as u32);
            // Tank embodiment P6 (D55): the shell kind + its area burst, APPENDED after `lifetime`.
            // Empty pool for any scene without a firing ballistic tank, so it is byte-neutral there.
            sink.write_u8(shell_tag(p.shell));
            sink.write_i32(p.splash_radius.to_bits());
            sink.write_i32(p.splash_damage.to_bits());
        }
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
        // Income pace — scenario-local config, NOT in the fold (like map_id). Written here in the
        // wrapper so the order is: version, map_id, income_period, armies, capacity, fold(...).
        w.write_u32(self.income_period);
        // Per-side army identity (factions-plan WS-A) — match-setup config, like income_period: in
        // the WRAPPER, NOT the per-tick fold (so a no-army scene's checksum is byte-unchanged; the
        // roster effect, WS-B, is what folds — invariant #7). One tag byte per faction, in fixed
        // `Faction::ALL` order.
        for f in Faction::ALL {
            w.write_u8(army_tag(self.armies[f.index()]));
        }
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
        // Income pace — mirror serialize's wrapper order (after map_id, before capacity). Kept a
        // pure inverse (no clamp here, so re-serialize is byte-identical); economy_system clamps
        // 0 → 1 at the use site so a malformed period can never divide by zero.
        let income_period = r.read_u32()?;

        // Per-side army identity (factions-plan WS-A) — mirror serialize's wrapper order (after
        // income_period, before capacity). One tag byte per faction in fixed `Faction::ALL` order; a
        // tag outside the enum is corruption (`BadTag`), never a silent wrong army.
        let mut armies = [Army::Neutral; FACTION_COUNT];
        for f in Faction::ALL {
            armies[f.index()] = read_army(&mut r)?;
        }

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
        let mut armor = Vec::with_capacity(cap);

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
                // All-unit ammo logistics (D67): mirror fold()'s reserve + reserve_max, same order.
                reserve: read_u16(&mut r)?,
                reserve_max: read_u16(&mut r)?,
                turret_speed: read_u16(&mut r)?,
                muzzle_vel: Fixed::from_bits(r.read_i32()?),
                penetration: Fixed::from_bits(r.read_i32()?),
                // Tank embodiment P5 (D55): mirror fold()'s dispersion word, in the same order.
                dispersion: Fixed::from_bits(r.read_i32()?),
                // Tank embodiment P6 (D55): mirror fold()'s loaded-shell tag (after `penetration`).
                shell: read_shell(&mut r)?,
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
            // Tank embodiment P4 (D55): mirror fold()'s armour trio (front/side/rear), same order.
            armor.push(Armor {
                front: Fixed::from_bits(r.read_i32()?),
                side: Fixed::from_bits(r.read_i32()?),
                rear: Fixed::from_bits(r.read_i32()?),
            });
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

        // In-flight ballistic shells (tank embodiment P3, D55) — mirror fold()'s pool block,
        // field-for-field in the same order, right after the RNG and before the liveness extras.
        let n_proj = r.read_len(MIN_PROJECTILE_BYTES)?;
        let mut projectiles = Vec::with_capacity(n_proj);
        for _ in 0..n_proj {
            projectiles.push(Projectile {
                pos2d: read_vec2(&mut r)?,
                vel2d: read_vec2(&mut r)?,
                height: Fixed::from_bits(r.read_i32()?),
                vz: Fixed::from_bits(r.read_i32()?),
                owner: Entity {
                    index: r.read_u32()?,
                    generation: r.read_u32()?,
                },
                faction: read_faction(&mut r)?,
                damage: Fixed::from_bits(r.read_i32()?),
                penetration: Fixed::from_bits(r.read_i32()?),
                lifetime: read_u16(&mut r)?,
                // Tank embodiment P6 (D55): mirror fold()'s shell kind + area burst (after `lifetime`).
                shell: read_shell(&mut r)?,
                splash_radius: Fixed::from_bits(r.read_i32()?),
                splash_damage: Fixed::from_bits(r.read_i32()?),
            });
        }

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
                armor,
            },
        )
        .ok_or(DeserializeError::CorruptState)?;

        Ok(Sim {
            world,
            terrain,
            resources,
            territory,
            events: Vec::new(),
            projectiles,
            map_id,
            income_period,
            armies,
            rng,
            tick,
        })
    }

    /// Capture a read-only render snapshot (invariant #4).
    pub fn snapshot(&self) -> Snapshot {
        Snapshot::capture(&self.world, &self.territory, &self.projectiles, self.tick)
    }
}

/// Authoritative-snapshot format version (D28). Bumped on any layout change so a stale snapshot is
/// rejected ([`DeserializeError::BadVersion`]) rather than silently misparsed into a divergent
/// world. Independent of the lockstep wire version — different codec, different evolution. Bumped
/// 7→8 by factions-plan WS-A: the wrapper grew a per-faction [`Army`] tag block (after
/// `income_period`), so a pre-factions snapshot is rejected, never misparsed against the new layout.
/// Bumped 8→9 by tank embodiment P5 (D55): the per-slot `Weapon` fold grew a `dispersion` word, so a
/// pre-P5 snapshot is rejected rather than misparsed against the longer slot layout.
/// 8→9 by tank embodiment P6 (D55): the per-slot `Weapon` fold grew a loaded-shell tag and the
/// projectile fold grew a shell tag + splash pair, so a pre-P6 snapshot is rejected, not misparsed.
const SNAPSHOT_VERSION: u8 = 9;

/// Smallest possible encoding of one `ControlPoint`: `pos` (2×i32) + owner tag (u8) + progress
/// (i32) = 13 bytes. Used to reject a garbage point count before allocating.
const MIN_CONTROL_POINT_BYTES: usize = 13;

/// Smallest possible encoding of one in-flight [`Projectile`]: `pos2d` (2×i32) + `vel2d` (2×i32) +
/// `height` (i32) + `vz` (i32) + `owner` (2×u32) + faction tag (u8) + `damage` (i32) +
/// `penetration` (i32) + `lifetime` (u32) + shell tag (u8) + `splash_radius` (i32) + `splash_damage`
/// (i32) = 54 bytes. Rejects a garbage pool count before allocating (tank embodiment P3, P6 D55).
const MIN_PROJECTILE_BYTES: usize = 54;

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
        1 => BuildingKind::Barracks,
        t => return Err(DeserializeError::BadTag(t)),
    })
}

fn read_unit_kind(r: &mut Reader) -> Result<UnitKind, DeserializeError> {
    Ok(match r.read_u8()? {
        0 => UnitKind::Rifleman,
        1 => UnitKind::Heavy,
        2 => UnitKind::Tank,
        3 => UnitKind::Medic,
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
        BuildingKind::Barracks => 1,
    }
}

fn unit_kind_tag(k: UnitKind) -> u8 {
    match k {
        UnitKind::Rifleman => 0,
        UnitKind::Heavy => 1,
        UnitKind::Tank => 2,
        UnitKind::Medic => 3,
    }
}

/// Army identity tag (factions-plan WS-A). MUST match lockstep.rs's `put_army`/`get_army` and
/// [`Army::index`] (the tag order is the wire/persist contract) — a `SelectArmy` command encoded on
/// one peer has to decode to the identical army on every other (invariant #7).
fn army_tag(a: Army) -> u8 {
    match a {
        Army::Neutral => 0,
        Army::Us => 1,
        Army::Fr => 2,
    }
}

fn read_army(r: &mut Reader) -> Result<Army, DeserializeError> {
    Ok(match r.read_u8()? {
        0 => Army::Neutral,
        1 => Army::Us,
        2 => Army::Fr,
        t => return Err(DeserializeError::BadTag(t)),
    })
}

/// Shell-kind tag (tank embodiment P6, D55). MUST match lockstep.rs's `put_shell`/`get_shell` and the
/// `ShellKind` discriminant order (the wire/persist contract) — a `SelectShell` command, the loaded
/// `Weapon::shell`, and an in-flight `Projectile::shell` must all decode to the identical shell on
/// every peer (invariant #7).
fn shell_tag(s: ShellKind) -> u8 {
    match s {
        ShellKind::Ap => 0,
        ShellKind::Aphe => 1,
        ShellKind::He => 2,
    }
}

fn read_shell(r: &mut Reader) -> Result<ShellKind, DeserializeError> {
    Ok(match r.read_u8()? {
        0 => ShellKind::Ap,
        1 => ShellKind::Aphe,
        2 => ShellKind::He,
        t => return Err(DeserializeError::BadTag(t)),
    })
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

    #[test]
    fn step_pushes_a_unit_out_of_a_building_it_walked_into() {
        // Integration: building collision must resolve *through the full `Sim::step` pipeline*, not
        // just the unit-tested `resolve_building_collisions` in isolation. The lockstep determinism
        // scenes never sit a unit on a building, so this is the only cross-arch coverage that the
        // collide step is wired into `step` in the right order (after movement, before the snapshot)
        // — it rides `determinism.yml`'s `cargo test -p gonedark-core --release` on every arch.
        //
        // The avatar starts ON the building centre (the zero-delta degenerate case → the most
        // determinism-sensitive path: `normalized()` returns zero, so the resolver must eject along
        // +X identically on every peer) and locomotes straight into it. One tick must leave it on
        // the footprint boundary at exactly BUILDING_RADIUS + UNIT_RADIUS = 2 m along +X.
        let mut sim = Sim::new(0);
        let bldg = sim.world.spawn();
        sim.world.kind[bldg.index as usize] = EntityKind::Building;
        sim.world.pos[bldg.index as usize] = Vec2::ZERO;
        let unit = sim.world.spawn();
        sim.world.input_source[unit.index as usize] = InputSource::Embodied;
        sim.world.pos[unit.index as usize] = Vec2::ZERO;

        sim.step(&[Command::Locomote {
            entity: unit,
            dir: Vec2::new(Fixed::ONE, Fixed::ZERO),
        }]);
        let pushed = sim.world.pos[unit.index as usize];
        assert_eq!(pushed, Vec2::new(Fixed::from_int(2), Fixed::ZERO));

        // A second tick standing still keeps it on the boundary (idempotent through the pipeline).
        sim.step(&[]);
        assert_eq!(sim.world.pos[unit.index as usize], pushed);
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

    // --- factions WS-A: Army selection as match-setup config ---------------------------------

    #[test]
    fn army_defaults_to_neutral_for_every_faction() {
        // A fresh sim fields no real army — so a pre-factions scene is identity-neutral (and its
        // per-tick checksum is byte-unchanged, since `armies` is not folded).
        let sim = Sim::new(0);
        for f in Faction::ALL {
            assert_eq!(sim.army_of(f), Army::Neutral);
        }
    }

    #[test]
    fn select_army_command_sets_the_per_side_identity() {
        // The match-setup command writes the per-faction army mapping (US vs FR — D68). It reaches
        // each side independently and is a plain config write (no entity, no float, no RNG).
        let mut sim = Sim::new(0);
        sim.step(&[
            Command::SelectArmy {
                faction: Faction::Player,
                army: Army::Us,
            },
            Command::SelectArmy {
                faction: Faction::Enemy,
                army: Army::Fr,
            },
        ]);
        assert_eq!(sim.army_of(Faction::Player), Army::Us);
        assert_eq!(sim.army_of(Faction::Enemy), Army::Fr);
        // The unselected side stays Neutral.
        assert_eq!(sim.army_of(Faction::Neutral), Army::Neutral);
    }

    #[test]
    fn select_army_is_checksum_neutral() {
        // The army selection is match-config, NOT per-tick sim state: selecting an army must not move
        // the per-tick checksum (its roster effect, WS-B, is what folds — invariant #1/#7). Two sims
        // step identically; one also picks armies. Their checksum streams must stay bit-identical.
        let seed = 0xFAC_A11u64;
        let mut with = Sim::new(seed);
        let mut without = Sim::new(seed);
        with.step(&[
            Command::SelectArmy {
                faction: Faction::Player,
                army: Army::Us,
            },
            Command::SelectArmy {
                faction: Faction::Enemy,
                army: Army::Fr,
            },
        ]);
        without.step(&[]);
        assert_eq!(
            with.checksum(),
            without.checksum(),
            "selecting an army must not perturb the per-tick checksum"
        );
        // But the selection IS there (it just lives outside the fold).
        assert_eq!(with.army_of(Faction::Player), Army::Us);
    }

    #[test]
    fn army_selection_survives_the_snapshot_round_trip() {
        // The per-side army identity rides the persist WRAPPER (like income_period/map_id), so a
        // reconnecting peer resumes the same matchup — not silently reset to Neutral. fold↔deserialize
        // round trip (factions-plan WS-A): re-serialize is byte-identical and the armies come back.
        let mut sim = Sim::new(7);
        sim.set_army(Faction::Player, Army::Us);
        sim.set_army(Faction::Enemy, Army::Fr);
        for _ in 0..10 {
            sim.step(&[]);
        }
        let bytes = sim.serialize();
        let restored = Sim::deserialize(&bytes).expect("army-selected sim round-trips");
        assert_eq!(restored.army_of(Faction::Player), Army::Us, "US must survive resume");
        assert_eq!(restored.army_of(Faction::Enemy), Army::Fr, "FR must survive resume");
        assert_eq!(restored.army_of(Faction::Neutral), Army::Neutral);
        assert_eq!(restored.checksum(), sim.checksum());
        assert_eq!(restored.serialize(), bytes, "re-serialize is byte-identical");
    }

    #[test]
    fn deserialize_rejects_a_bad_army_tag() {
        // A garbage army tag in the wrapper is corruption (BadTag), never a silently-wrong identity.
        let mut bytes = Sim::new(0).serialize();
        // Wrapper layout: version(1) + map_id(4) + income_period(4) + armies(3). The first army tag
        // is at byte index 9. Stuff an out-of-range tag there.
        bytes[9] = 0x7F;
        match Sim::deserialize(&bytes).err() {
            Some(DeserializeError::BadTag(0x7F)) => {}
            other => panic!("expected BadTag(0x7F) for a corrupt army tag, got {other:?}"),
        }
    }

    // --- factions WS-B: per-faction rosters fold into the checksum --------------------------------

    /// Build a deterministic two-camp production scene where the Player fields `pa` and the Enemy
    /// fields `ea`, each operational camp queued to produce one Rifleman. Pure / fixed-point, so two
    /// peers constructing it bit-identically diverge only where the per-army roster legitimately does.
    fn production_scene(pa: Army, ea: Army) -> (Sim, u16) {
        let mut sim = Sim::new(0x600D_A2A0);
        sim.set_army(Faction::Player, pa);
        sim.set_army(Faction::Enemy, ea);
        sim.resources = Resources::new(100_000);
        let p = Vec2::new(Fixed::from_int(-10), Fixed::ZERO);
        let e = Vec2::new(Fixed::from_int(10), Fixed::ZERO);
        let pcamp =
            economy::build(&mut sim.world, &mut sim.resources, Faction::Player, BuildingKind::Camp, p)
                .expect("player camp affordable");
        let ecamp =
            economy::build(&mut sim.world, &mut sim.resources, Faction::Enemy, BuildingKind::Camp, e)
                .expect("enemy camp affordable");
        // Make both operational, then queue one Rifleman each (the produced unit's per-army stats are
        // what folds into the checksum).
        sim.world.building[pcamp.index as usize].build_ticks_left = 0;
        sim.world.building[ecamp.index as usize].build_ticks_left = 0;
        assert!(economy::queue_production(&mut sim.world, &mut sim.resources, pcamp, UnitKind::Rifleman));
        assert!(economy::queue_production(&mut sim.world, &mut sim.resources, ecamp, UnitKind::Rifleman));
        (sim, economy::prod_time(UnitKind::Rifleman, 0))
    }

    /// The per-tick checksum stream of a scene over `ticks` ticks (no commands — the production
    /// countdown drives it).
    fn checksum_stream(mut sim: Sim, ticks: u16) -> Vec<u64> {
        let mut out = Vec::with_capacity(ticks as usize + 1);
        out.push(sim.checksum());
        for _ in 0..ticks {
            sim.step(&[]);
            out.push(sim.checksum());
        }
        out
    }

    /// **2-peer lockstep with MISMATCHED armies (factions-plan WS-B).** Two peers each run the
    /// identical scene where the Player fields the US Army and the Enemy the French Army — different
    /// rosters on the two sides. Their per-tick checksum streams must stay **bit-identical**: the
    /// per-army stat table is the same fixed-point data on every peer, so a US camp and an FR camp
    /// produce the bit-identical unit on both peers (invariant #1/#7). This is the lockstep agreement
    /// the cross-platform matrix rests on, now exercised with two armies in the same match.
    #[test]
    fn two_peers_agree_with_mismatched_armies() {
        let ticks = production_scene(Army::Us, Army::Fr).1 + 4;
        let peer_a = checksum_stream(production_scene(Army::Us, Army::Fr).0, ticks);
        let peer_b = checksum_stream(production_scene(Army::Us, Army::Fr).0, ticks);
        assert_eq!(peer_a, peer_b, "two peers with the same US-vs-FR matchup must agree every tick");
        // And the stream actually advances (the rifles spawn, state changes) — not a frozen sim.
        assert_ne!(peer_a.first(), peer_a.last(), "production must move the checksum");
    }

    /// The per-army roster genuinely **folds into the checksum** (the desync-catch of invariant #7):
    /// a US-vs-FR match and a non-aligned (Neutral) match share the byte-identical checksum until the
    /// rifles spawn — then DIVERGE, because the US/FR units carry the army-tilted (logistics) loadout
    /// while the Neutral units carry the baseline. So two peers that picked different armies are
    /// caught at the production tick, exactly as intended.
    #[test]
    fn per_army_roster_diverges_the_checksum_at_production() {
        let (_, ptime) = production_scene(Army::Us, Army::Fr);
        let ticks = ptime + 4;
        let armed = checksum_stream(production_scene(Army::Us, Army::Fr).0, ticks);
        let neutral = checksum_stream(production_scene(Army::Neutral, Army::Neutral).0, ticks);
        // Identical before any unit spawns (armies are not folded; only their spawned-unit stats are).
        assert_eq!(armed[0], neutral[0], "pre-production state is army-agnostic");
        // But the final state differs — the produced units' per-army stats moved the fold.
        assert_ne!(
            armed.last(),
            neutral.last(),
            "US/FR produced units must fold differently from the Neutral baseline"
        );
    }

    // --- tank embodiment P6 (D55): SelectShell + per-shell fire ---------------------------------

    /// Build a fresh sim with one **embodied ballistic tank** at the origin (facing +X) and two
    /// clustered enemy infantry downrange — so an HE shell's splash has a neighbour to catch. The tank
    /// already has `InputSource::Embodied`, so a `Fire` launches a real shell (`muzzle_vel > 0`).
    fn ballistic_tank_scene() -> (Sim, Entity) {
        let mut sim = Sim::new(0xB6B6_0006);
        let tank = sim.world.spawn();
        let ti = tank.index as usize;
        sim.world.kind[ti] = EntityKind::Unit;
        sim.world.faction[ti] = Faction::Player;
        sim.world.input_source[ti] = InputSource::Embodied;
        sim.world.pos[ti] = Vec2::ZERO;
        sim.world.weapon[ti] = Weapon {
            range: Fixed::from_int(40),
            damage: Fixed::from_int(40),
            cooldown_ticks: 4,
            muzzle_vel: Fixed::from_int(2),
            penetration: Fixed::from_int(20),
            ..Default::default()
        };
        for (x, y) in [(6, 0), (6, 1)] {
            let e = sim.world.spawn();
            let i = e.index as usize;
            sim.world.kind[i] = EntityKind::Unit;
            sim.world.faction[i] = Faction::Enemy;
            sim.world.pos[i] = Vec2::new(Fixed::from_int(x), Fixed::from_int(y));
            sim.world.health[i] = Health::full(Fixed::from_int(1000));
            sim.world.stance[i] = Stance::HoldFire;
        }
        (sim, tank)
    }

    #[test]
    fn select_shell_changes_the_loaded_shell() {
        let (mut sim, tank) = ballistic_tank_scene();
        let i = tank.index as usize;
        assert_eq!(sim.world.weapon[i].shell, ShellKind::Ap, "defaults to AP");
        sim.step(&[Command::SelectShell { entity: tank, shell: ShellKind::He }]);
        assert_eq!(sim.world.weapon[i].shell, ShellKind::He, "SelectShell loads HE");
        sim.step(&[Command::SelectShell { entity: tank, shell: ShellKind::Aphe }]);
        assert_eq!(sim.world.weapon[i].shell, ShellKind::Aphe, "and switches again");
    }

    #[test]
    fn select_shell_is_a_noop_mid_reload() {
        let (mut sim, tank) = ballistic_tank_scene();
        let i = tank.index as usize;
        // Arm a magazine and put it mid-reload: the chambered round can't be swapped now.
        sim.world.weapon[i].mag_size = 6;
        sim.world.weapon[i].reload_left = 30;
        sim.step(&[Command::SelectShell { entity: tank, shell: ShellKind::He }]);
        assert_eq!(sim.world.weapon[i].shell, ShellKind::Ap, "no swap while reloading");
        // Once the reload finishes, the swap takes.
        sim.world.weapon[i].reload_left = 0;
        sim.step(&[Command::SelectShell { entity: tank, shell: ShellKind::He }]);
        assert_eq!(sim.world.weapon[i].shell, ShellKind::He, "swaps once loaded");
    }

    #[test]
    fn select_shell_for_a_dead_handle_is_a_noop() {
        // A stale handle must not panic or write anything — just advance the tick.
        let mut sim = Sim::new(0);
        let stale = Entity { index: 0, generation: 9 };
        sim.step(&[Command::SelectShell { entity: stale, shell: ShellKind::He }]);
        assert_eq!(sim.tick_count(), 1);
    }

    #[test]
    fn selected_shell_flows_into_the_launched_projectile() {
        // SelectShell then Fire in the same tick: the command order applies the selection first, so the
        // shell launched this tick is the selected kind, carrying its splash.
        let (mut sim, tank) = ballistic_tank_scene();
        // Move the enemies far away so the shell does not impact (and despawn) on this first tick.
        for i in 0..sim.world.capacity() {
            if sim.world.is_index_alive(i) && sim.world.faction[i] == Faction::Enemy {
                sim.world.pos[i] = Vec2::new(Fixed::from_int(30), Fixed::ZERO);
            }
        }
        sim.step(&[
            Command::SelectShell { entity: tank, shell: ShellKind::He },
            Command::Fire { entity: tank, dir: Vec2::new(Fixed::ONE, Fixed::ZERO) },
        ]);
        assert_eq!(sim.projectiles.len(), 1, "the HE shell is in flight");
        let p = sim.projectiles[0];
        assert_eq!(p.shell, ShellKind::He);
        assert_eq!(p.splash_radius, Fixed::from_int(4), "HE carries its frag radius");
        assert_eq!(p.penetration, Fixed::from_int(20) * Fixed::from_ratio(1, 8), "HE pen ×1/8");
    }

    /// 2-peer lockstep agreement with a `SelectShell` in the stream (invariant #7): two peers seed the
    /// identical ballistic-tank scene and apply the identical command set each tick — selecting HE then
    /// firing on a cadence. Their per-tick checksum streams must stay **bit-identical**: the selected
    /// shell folds per-tank, and the HE shells + their splash resolve through the same fixed-point path
    /// on both peers, so the streams never diverge.
    #[test]
    fn select_shell_two_peers_agree_in_lockstep() {
        let cmds_for = |tank: Entity, tick: u64| -> Vec<Command> {
            let mut c = Vec::new();
            if tick == 0 {
                c.push(Command::SelectShell { entity: tank, shell: ShellKind::He });
            }
            if tick.is_multiple_of(5) {
                c.push(Command::Fire {
                    entity: tank,
                    dir: Vec2::new(Fixed::ONE, Fixed::ZERO),
                });
            }
            c
        };
        let (mut a, ta) = ballistic_tank_scene();
        let (mut b, tb) = ballistic_tank_scene();
        assert_eq!(ta, tb, "identical seeding yields identical handles");
        assert_eq!(a.checksum(), b.checksum(), "pre-step states agree");
        for tick in 0..60u64 {
            a.step(&cmds_for(ta, tick));
            b.step(&cmds_for(tb, tick));
            assert_eq!(a.checksum(), b.checksum(), "checksums must agree at tick {tick}");
        }
        // The HE selection actually bit (shells flew and splashed): at least one enemy lost health.
        let hurt = (0..a.world.capacity()).any(|i| {
            a.world.is_index_alive(i)
                && a.world.faction[i] == Faction::Enemy
                && a.world.health[i].cur < Fixed::from_int(1000)
        });
        assert!(hurt, "the HE shells should have damaged the enemy cluster");
    }

    #[test]
    fn select_shell_survives_serialize_round_trip() {
        // The loaded shell is folded per-tank sim state, so a serialize→deserialize restores it and the
        // checksum is unmoved (it is the persist analogue of the fold round-trip).
        let (mut sim, tank) = ballistic_tank_scene();
        sim.step(&[Command::SelectShell { entity: tank, shell: ShellKind::Aphe }]);
        let bytes = sim.serialize();
        let restored = Sim::deserialize(&bytes).expect("round-trips");
        assert_eq!(
            restored.world.weapon[tank.index as usize].shell,
            ShellKind::Aphe,
            "deserialize restores the loaded shell",
        );
        assert_eq!(restored.checksum(), sim.checksum(), "checksum is byte-identical");
    }
}
