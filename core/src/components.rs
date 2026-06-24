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
    Patrol { a: Vec2, b: Vec2, toward_b: bool },
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
}

/// A constructable building archetype.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BuildingKind {
    /// Produces units and projects territory income; the upgradable core (game-design §3).
    #[default]
    Camp,
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
