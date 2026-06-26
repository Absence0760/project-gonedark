//! Sim components — plain data, fixed-point only. Stored struct-of-arrays in the ECS.

use crate::fixed::Fixed;
use crate::trig;
use core::ops::{Add, Sub};

/// 2D vector in Q16.16 world units.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Vec2 {
    pub x: Fixed,
    pub y: Fixed,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 {
        x: Fixed::ZERO,
        y: Fixed::ZERO,
    };

    #[inline]
    pub const fn new(x: Fixed, y: Fixed) -> Self {
        Vec2 { x, y }
    }

    #[inline]
    pub fn scale(self, s: Fixed) -> Vec2 {
        Vec2::new(self.x * s, self.y * s)
    }

    #[inline]
    pub fn dot(self, o: Vec2) -> Fixed {
        self.x * o.x + self.y * o.y
    }

    /// Squared length (cheap; no sqrt). Prefer this for distance comparisons.
    #[inline]
    pub fn len_sq(self) -> Fixed {
        self.dot(self)
    }

    /// Length via fixed-point sqrt.
    #[inline]
    pub fn len(self) -> Fixed {
        trig::sqrt(self.len_sq())
    }

    /// Unit vector; a zero vector returns zero (never divides by zero).
    #[inline]
    pub fn normalized(self) -> Vec2 {
        let l = self.len();
        if l == Fixed::ZERO {
            Vec2::ZERO
        } else {
            Vec2::new(self.x / l, self.y / l)
        }
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    #[inline]
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    #[inline]
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}

/// A unit's current order. The literal executor (invariant #3, D3) holds exactly this and
/// does it — no autonomy, no strategy. Phase 2 widens the *vocabulary* (D23): the depth of
/// the game lives in the orders a player can pre-program, not in unit cleverness.
///
/// - [`Idle`](Order::Idle) — stand still, zero velocity.
/// - [`MoveTo`](Order::MoveTo) — walk to a point via the flow field, then go idle.
/// - [`AttackMove`](Order::AttackMove) — walk to a point but stop and let the weapon engage
///   any enemy that comes into range along the way (movement still literal — it does not
///   chase; combat fires under the unit's stance).
/// - [`Patrol`](Order::Patrol) — bounce between two points forever (`toward_b` tracks the
///   current leg). The canonical pre-dive "watch this lane" order.
/// - [`HoldPosition`](Order::HoldPosition) — never move; fight only from here (stance still
///   governs firing). Distinct from `Idle`: a held unit is deliberately rooted.
/// - [`FallBack`](Order::FallBack) — retreat to a rally point; the retreat *trigger* (fall
///   back below X% health) installs this order (D23) — the unit itself never decides to flee.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Order {
    #[default]
    Idle,
    MoveTo(Vec2),
    AttackMove(Vec2),
    Patrol {
        a: Vec2,
        b: Vec2,
        toward_b: bool,
    },
    HoldPosition,
    FallBack(Vec2),
}

/// A unit's engagement stance (stubbed vocabulary for Phase 1).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Stance {
    HoldFire,
    #[default]
    ReturnFire,
    FireAtWill,
}

/// Where a unit's input comes from (invariant #5, D6/D7). `Orders` = command layer /
/// literal-executor AI; `Embodied` = live player input while possessed. Flipping this is
/// the *entirety* of possession — there is no separate character object and no respawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum InputSource {
    #[default]
    Orders,
    Embodied,
}

// ===========================================================================
// Phase 2 components (D23). All fixed-point / integer only (invariant #1). New
// per-entity arrays live in `ecs::World`; these are the plain-data payloads.
// ===========================================================================

/// Which side an entity belongs to. Combat only engages across factions; production and
/// resources are tracked per faction. `Neutral` is for uncontrolled props/control points.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Faction {
    #[default]
    Player,
    Enemy,
    Neutral,
}

impl Faction {
    /// Every faction, in a fixed order — the stable index space for per-faction state
    /// (resources, territory counts). Iterating this is deterministic by construction.
    pub const ALL: [Faction; 3] = [Faction::Player, Faction::Enemy, Faction::Neutral];

    /// Dense index into per-faction arrays (`[_; FACTION_COUNT]`).
    #[inline]
    pub const fn index(self) -> usize {
        match self {
            Faction::Player => 0,
            Faction::Enemy => 1,
            Faction::Neutral => 2,
        }
    }
}

/// Number of factions — the width of every per-faction array.
pub const FACTION_COUNT: usize = 3;

/// What an entity *is*. Determines which systems act on it: `Unit`s move and fight,
/// `Building`s are produced/upgraded and can be attacked (driving "base under attack").
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum EntityKind {
    #[default]
    Unit,
    Building,
}

/// A producible unit archetype. Stats are looked up from this in `economy`/`combat` so the
/// same kind spawns identically on every peer (determinism).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum UnitKind {
    #[default]
    Rifleman,
    Heavy,
    /// A produced armoured vehicle — high HP, a hard-hitting gun, and an independently-slewing
    /// turret (cosmetic, reusing the D55 hull/turret split + tank mesh). For balance in the
    /// rifle-centric skirmish it is **unarmoured** (no facet immunity): the full armoured + ballistic
    /// tank, which infantry cannot pen frontally, remains the duel scene's domain until an anti-tank
    /// counter exists ([D65](../docs/decisions.md)).
    Tank,
    /// A support unit: it carries no offensive weapon and instead **heals** nearby friendly units
    /// each tick (`crate::heal`). Built from a [`Barracks`](BuildingKind::Barracks).
    Medic,
}

/// A constructable building archetype.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BuildingKind {
    /// Produces units and projects territory income; the upgradable core (game-design §3). The base
    /// fields infantry and vehicles (Rifleman / Heavy / Tank).
    #[default]
    Camp,
    /// A cheaper, faster forward production building for **infantry** — the only source of the
    /// [`Medic`](UnitKind::Medic). Cannot build vehicles ([D65](../docs/decisions.md)).
    Barracks,
}

/// Hit points. `cur <= max`; an entity with `cur <= 0` is dead and gets despawned by combat.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Health {
    pub cur: Fixed,
    pub max: Fixed,
}

impl Health {
    /// Full health at a given maximum.
    #[inline]
    pub const fn full(max: Fixed) -> Self {
        Health { cur: max, max }
    }

    /// `cur / max` as a Fixed fraction in `[0, 1]` (0 when `max <= 0`, never divides by zero).
    #[inline]
    pub fn fraction(self) -> Fixed {
        if self.max.to_bits() <= 0 {
            Fixed::ZERO
        } else {
            self.cur / self.max
        }
    }

    #[inline]
    pub fn is_dead(self) -> bool {
        self.cur.to_bits() <= 0
    }

    /// Restore `amount` HP, never above `max`. A no-op on a dead entity (you cannot heal a corpse —
    /// only `combat`'s despawn handles death, and `heal` must never revive). Fixed-point only
    /// (invariant #1), so it is deterministic and folds into the checksum via `cur`.
    #[inline]
    pub fn heal(&mut self, amount: Fixed) {
        if self.is_dead() {
            return;
        }
        let healed = self.cur + amount;
        self.cur = if healed > self.max { self.max } else { healed };
    }
}

impl Default for Health {
    /// A generic entity spawns at 100 HP so a freshly spawned, weapon-less unit is alive and
    /// inert — keeping the Phase 1 single-mover tests untouched.
    fn default() -> Self {
        Health::full(Fixed::from_int(100))
    }
}

/// A weapon: how far it reaches, how hard it hits, and how often. A default (range 0) weapon
/// never fires, so non-combatants and the Phase 1 mover are inert in `combat_system`.
///
/// The magazine fields drive the **embodied** reload mechanic (the first-person Reload button)
/// and are deliberately **opt-in**: a weapon with `mag_size == 0` has *no* magazine and fires
/// without an ammo gate — this is what every AI / auto-combat unit and every Phase-1/2 test
/// uses, so the existing `combat_system` engage pass is untouched. Only weapons produced with a
/// real `mag_size` (the playable archetypes in `economy::unit_stats`) ration ammo, and only the
/// embodied fire path ([`combat::resolve_fire`]) enforces it (AI units are literal executors —
/// they never reload, invariant #3). All `u16`/`Fixed`, no float (invariant #1).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Weapon {
    /// Maximum engagement distance in world units.
    pub range: Fixed,
    /// Damage per shot, before cover mitigation.
    pub damage: Fixed,
    /// Ticks between shots once fired.
    pub cooldown_ticks: u16,
    /// Ticks left until the weapon may fire again (0 = ready).
    pub cooldown_left: u16,
    /// Magazine capacity. `0` = no magazine system (infinite ammo, no reload) — the default for
    /// AI/auto units and tests. `> 0` enables the embodied ammo+reload gate.
    pub mag_size: u16,
    /// Rounds left in the current magazine (only meaningful when `mag_size > 0`).
    pub ammo: u16,
    /// How many ticks a reload takes once started (config; only meaningful when `mag_size > 0`).
    pub reload_ticks: u16,
    /// Ticks left in an in-progress reload (`0` = not reloading). Set by `Command::Reload`,
    /// counted down in combat upkeep; on reaching zero the magazine refills to `mag_size`.
    pub reload_left: u16,
    /// Maximum turret slew in angle-units per tick (tank embodiment P2, D55). `0` = a fixed mount
    /// locked to the hull — the default for infantry and every existing unit, so their
    /// `turret_yaw` never moves and the new field costs the checksum nothing. A real tank gun has
    /// `turret_speed > 0`: the embodied `AimTurret` slews `turret_yaw` toward the look-stick at
    /// this rate, and the AI turret tracks the hull at it (cosmetic — invariant #3).
    pub turret_speed: u16,
    /// Shell muzzle velocity in world units per tick (tank embodiment P3, D55). `0` (the infantry
    /// default) means **hitscan**: an embodied [`Fire`](crate::sim::Command::Fire) resolves
    /// instantly through [`combat::resolve_fire`](crate::combat::resolve_fire), exactly as today —
    /// so the new field costs every existing unit nothing and moves the checksum by nothing. A
    /// real tank gun has `muzzle_vel > 0`: an embodied shot instead launches a fixed-point
    /// **ballistic projectile** ([`projectile`](crate::projectile)) that travels at this speed,
    /// drops under gravity, and resolves its hit on impact (travel time + leading + drop). Opt-in
    /// by a zero default, the same pattern as `mag_size`/`turret_speed`. `Fixed`, no float
    /// (invariant #1).
    pub muzzle_vel: Fixed,
    /// Armour penetration this weapon's shot carries, compared against the defender's [`Armor`]
    /// facet at the damage step (tank embodiment P4, D55). `0` (the default for infantry and every
    /// existing weapon) penetrates an **unarmoured** defender fully (`armor == {0,0,0}` ⇒
    /// multiplier `1.0`, see [`combat::facing_penetration_multiplier`](crate::combat::facing_penetration_multiplier)),
    /// so the new field is byte-neutral for the existing balance: it only bites against an armoured
    /// target, where a low-penetration shot bounces off the thick frontal facet but pens the
    /// thinner flank/rear. `Fixed`, no float (invariant #1).
    pub penetration: Fixed,
}

/// Directional armour, in the same `Fixed` units a [`Weapon::penetration`] is measured in (tank
/// embodiment P4, D55). A shot is bucketed onto one facet by the angle between its travel
/// direction and the defender's [`hull_heading`](crate::ecs::World::hull_heading), then its
/// penetration is compared against that facet's value
/// ([`combat::facing_penetration_multiplier`](crate::combat::facing_penetration_multiplier)).
///
/// **The default is all-zero = unarmoured**, which yields a damage multiplier of exactly `1.0` on
/// every facet regardless of penetration — so every Rifleman, Heavy, and building takes *identical*
/// damage to today and the entire existing balance + combat-test suite is unchanged (invariant #7
/// safety: armour only perturbs the checksum where a unit is actually armoured). A real tank carries
/// thick `front`, thinner `side`, thinnest `rear` — *angle the hull at the enemy; flank to kill*.
/// All `Fixed`, no float (invariant #1).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Armor {
    /// Armour facing the hull heading — the thickest facet on a tank.
    pub front: Fixed,
    /// Armour on either flank (90° off the hull heading).
    pub side: Fixed,
    /// Armour at the tail — the thinnest facet.
    pub rear: Fixed,
}

impl Armor {
    /// Is this an unarmoured entity (every facet zero)? The common case — every non-tank — and the
    /// fast path that guarantees a `1.0` damage multiplier without any facet/trig work.
    #[inline]
    pub fn is_unarmored(self) -> bool {
        self.front == Fixed::ZERO && self.side == Fixed::ZERO && self.rear == Fixed::ZERO
    }
}

/// A unit's body posture (the embodied crouch toggle). Crouching trades mobility for accuracy:
/// it moves at [`systems::CROUCH_MOVE_SPEED`](crate::systems::CROUCH_MOVE_SPEED) and fires through
/// the tighter [`combat::FIRE_CONE_COS_HALF_CROUCHED`](crate::combat::FIRE_CONE_COS_HALF_CROUCHED)
/// aim cone. Player-only sim state (set by `Command::Crouch`); AI units stay `Standing`
/// (literal executor — invariant #3). A plain `repr`-stable tag like [`Stance`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Posture {
    #[default]
    Standing,
    Crouched,
}

/// One queued unit at a production building, counting down to completion.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ProductionItem {
    pub kind: UnitKind,
    pub ticks_left: u16,
}

/// Per-entity building state (meaningful only when `EntityKind::Building`). Carries
/// construction progress, an upgrade level, and a FIFO production queue. The queue is a
/// `Vec` (deterministic order); empty for every non-building entity.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Building {
    pub kind: BuildingKind,
    /// Upgrade tier (0 = base). Higher tiers produce faster / yield more (economy's call).
    pub level: u8,
    /// Ticks left until construction finishes (0 = built and operational).
    pub build_ticks_left: u16,
    /// FIFO production queue; the front item counts down each tick.
    pub queue: Vec<ProductionItem>,
}
