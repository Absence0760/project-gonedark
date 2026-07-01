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

/// A unit's engagement stance (the literal-executor firing vocabulary, invariant #3).
///
/// The default is [`FireAtWill`](Stance::FireAtWill): absent a specific order, a combat unit
/// engages the nearest enemy in weapon range + LoS each eligible tick. This is the literal
/// execution of "fire at will" (it never *moves* on its own — movement is still order-driven), and
/// it is what makes two AI units actually fight. [`ReturnFire`](Stance::ReturnFire) is NOT a safe
/// default: it only engages a recorded `last_attacker`, so two `ReturnFire` units facing off each
/// wait for the *other* to shoot first and deadlock — combat then never starts unless an embodied
/// player pulls the first trigger. (Discriminant order is the wire/persist contract — see
/// `sim::stance_tag` — so it is fixed: `HoldFire = 0`, `ReturnFire = 1`, `FireAtWill = 2`. Moving
/// `#[default]` does not change it.)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Stance {
    HoldFire,
    ReturnFire,
    #[default]
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

/// Which **real-army identity** a side fields — US Army vs French Army ([D68](../docs/decisions.md),
/// factions-plan WS-A). This is a **third** concept, deliberately DISTINCT from [`Faction`]: a
/// `Faction` is the *allegiance* tag combat resolves across (`Player`/`Enemy`/`Neutral`); an `Army`
/// is the *identity* a side wears — which per-faction roster, silhouettes, and gunsmith pool it
/// draws from. Each `Faction` in a match maps to one `Army` (the per-side selection lives in
/// [`Sim`](crate::sim::Sim), reachable through the [`shell`](crate::shell) seam, D34).
///
/// It is **match-setup config**, not per-tick state: it is chosen once at setup and never mutated by
/// a system, exactly like the income pace ([`Sim::set_income_period`](crate::sim::Sim::set_income_period)).
/// So — like that lever — it is carried in the persist **wrapper** and across the lockstep wire, but
/// it is **not** folded into the per-tick checksum: its *gameplay effect* (the per-army stat table,
/// WS-B) is what folds, so a peer that selected a different army diverges in spawned unit stats and
/// the desync is caught there (invariant #7), and a scene that never selects an army keeps the exact
/// pre-factions per-tick checksum byte-for-byte.
///
/// [`Neutral`](Army::Neutral) is the non-aligned default for legacy / debug scenes that field no
/// real army — `0`, the inert tag, mirroring [`Faction`]/[`UnitKind`]'s zero-is-default discipline.
/// Plain `repr`-stable data, no float (invariant #1).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Army {
    /// No real-army identity — non-aligned (legacy / debug scenes, neutral sides). The default, so a
    /// scene that never selects an army behaves (and checksums) exactly as before factions existed.
    #[default]
    Neutral,
    /// The US Army roster ([D68](../docs/decisions.md)).
    Us,
    /// The French Army roster ([D68](../docs/decisions.md)).
    Fr,
}

impl Army {
    /// Every army, in a fixed order — the stable index space for any per-army state (the WS-B stat
    /// table). Iterating this is deterministic by construction, mirroring [`Faction::ALL`].
    pub const ALL: [Army; 3] = [Army::Neutral, Army::Us, Army::Fr];

    /// Dense index into per-army arrays (`[_; ARMY_COUNT]`). The tag order is load-bearing: it MUST
    /// match the persist/wire codecs ([`sim`](crate::sim) `army_tag` + [`lockstep`](crate::lockstep)
    /// `put_army`), so a selection encoded on one peer decodes to the identical army on every other
    /// (invariant #7) — the same discipline as [`Faction::index`].
    #[inline]
    pub const fn index(self) -> usize {
        match self {
            Army::Neutral => 0,
            Army::Us => 1,
            Army::Fr => 2,
        }
    }
}

/// Number of armies — the width of every per-army array (WS-B's stat table).
pub const ARMY_COUNT: usize = 3;

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
    /// turret (cosmetic, reusing the D55 hull/turret split + tank mesh). It is now a **real armoured
    /// vehicle** (wave-1 W1: directional facet armour) that small-arms cannot pen frontally — its
    /// dedicated counter is the [`AntiTank`](UnitKind::AntiTank) infantry, whose penetrating gun
    /// cracks the frontal facet ([D73](../docs/decisions.md), restoring the armour RPS triangle).
    Tank,
    /// A support unit: it carries no offensive weapon and instead **heals** nearby friendly units
    /// each tick (`crate::heal`). Built from a [`Barracks`](BuildingKind::Barracks).
    Medic,
    /// Dedicated **anti-tank infantry** (a bazooka / AT team) — the answer to armour ([D73]). Carries
    /// a slow, penetrating gun (`penetration ≥ TANK_ARMOR_FRONT`) so it cracks a produced
    /// [`Tank`](UnitKind::Tank)'s frontal facet head-on, but is **fragile** (low HP), **slow**
    /// (few ready rounds, long cooldown — D67 logistics), and **poor anti-personnel** (so equal-cost
    /// it loses to massed [`Rifleman`](UnitKind::Rifleman)). Unarmoured infantry; trained from a
    /// [`Barracks`](BuildingKind::Barracks) like the [`Medic`](UnitKind::Medic). The RPS triangle:
    /// AT-infantry beats armour, massed infantry beats AT-infantry, armour beats infantry.
    /// ([D73](../docs/decisions.md).)
    AntiTank,
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

/// The kind of shell an embodied tank gun has loaded (tank embodiment P6, D55). Selected with
/// [`Command::SelectShell`](crate::sim::Command::SelectShell) and carried as per-weapon sim state
/// ([`Weapon::shell`]); it decides the **penetration / damage / splash** trade of the *next* shell the
/// ballistic gun ([`projectile::fire_ballistic`](crate::projectile::fire_ballistic)) launches:
///
/// - [`Ap`](ShellKind::Ap) — armour-piercing solid shot: full penetration, point damage, **no
///   splash**. The default (the zero tag), so every existing weapon loads `Ap` and a hitscan infantry
///   weapon (which never reads `shell`) is behaviourally unaffected.
/// - [`Aphe`](ShellKind::Aphe) — AP with an HE filler: pens nearly as well, then a **post-penetration**
///   burst splashes a small radius — but only when the solid body actually penetrated (a bounce yields
///   no burst). The duellist's shell: pen *and* a little area.
/// - [`He`](ShellKind::He) — high-explosive: **low** penetration (it bounces off a tank's thick facets)
///   but a **large**, unconditional frag burst — the anti-infantry / anti-cluster shell.
///
/// A plain `repr`-stable tag like [`Stance`]/[`UnitKind`]; the tag order is the wire/persist contract
/// (it MUST match [`sim`](crate::sim)'s `shell_tag` + [`lockstep`](crate::lockstep)'s `put_shell`), so a
/// `SelectShell` encoded on one peer decodes to the identical shell on every other (invariant #7). No
/// float (invariant #1) — the per-shell numbers are exact `Fixed` ratios in [`ShellKind::stats`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ShellKind {
    /// Armour-piercing solid shot — full pen, point damage, no splash. The zero-tag default.
    #[default]
    Ap,
    /// AP + HE filler — strong pen plus a small **post-penetration** burst (only if it penned).
    Aphe,
    /// High-explosive — low pen, large unconditional frag burst (the anti-cluster shell).
    He,
}

/// The fixed-point penetration / damage / splash parameters of one [`ShellKind`] (tank embodiment P6,
/// D55) — the table-driven, exact-`Fixed` analogue of [`economy::unit_stats`](crate::economy::unit_stats).
/// `pen_mul`/`damage_mul` scale the firing [`Weapon`]'s `penetration`/`damage` (so a bigger gun fires a
/// proportionally bigger shell of every type); the `splash_*` pair describes the area burst. All exact
/// rationals, no float (invariant #1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ShellStats {
    /// Multiplier on the weapon's [`penetration`](Weapon::penetration) for this shell.
    pub pen_mul: Fixed,
    /// Multiplier on the weapon's [`damage`](Weapon::damage) for the **direct** hit.
    pub damage_mul: Fixed,
    /// Radius of the area burst, in world units. `0` ⇒ no splash (point damage only — `Ap`).
    pub splash_radius: Fixed,
    /// Multiplier on the weapon's [`damage`](Weapon::damage) applied to each hostile (other than the
    /// directly-hit body) within [`splash_radius`](ShellStats::splash_radius) of the impact.
    pub splash_damage_mul: Fixed,
}

impl ShellKind {
    /// This shell's fixed-point [`ShellStats`] — the per-kind pen/damage/splash table (P6). Playtest
    /// baselines (exact ratios, no float): `Ap` is the pure-pen point shot; `Aphe` trades a little
    /// penetration for a small post-pen burst; `He` trades most of its penetration for a big frag burst.
    #[inline]
    pub fn stats(self) -> ShellStats {
        match self {
            ShellKind::Ap => ShellStats {
                pen_mul: Fixed::ONE,
                damage_mul: Fixed::ONE,
                splash_radius: Fixed::ZERO,
                splash_damage_mul: Fixed::ZERO,
            },
            ShellKind::Aphe => ShellStats {
                pen_mul: Fixed::from_ratio(3, 4),
                damage_mul: Fixed::ONE,
                splash_radius: Fixed::from_int(2),
                splash_damage_mul: Fixed::from_ratio(1, 2),
            },
            ShellKind::He => ShellStats {
                pen_mul: Fixed::from_ratio(1, 8),
                damage_mul: Fixed::ONE,
                splash_radius: Fixed::from_int(4),
                splash_damage_mul: Fixed::from_ratio(3, 4),
            },
        }
    }

    /// Does this shell's splash fire **only after the direct hit penetrates** (the post-pen HE filler of
    /// [`Aphe`](ShellKind::Aphe))? `He` detonates on contact regardless; `Ap` has no splash at all.
    #[inline]
    pub fn splash_is_post_pen(self) -> bool {
        matches!(self, ShellKind::Aphe)
    }
}

/// A weapon: how far it reaches, how hard it hits, and how often. A default (range 0) weapon
/// never fires, so non-combatants and the Phase 1 mover are inert in `combat_system`.
///
/// **Ammo is all-unit logistics (D67), not an embodied-only toggle.** A weapon with `mag_size > 0`
/// rations rounds for *both* the embodied player ([`combat::resolve_fire`]) **and** every
/// AI/auto-combat unit ([`combat::combat_system`]): firing spends a round, an empty magazine can't
/// fire, and a depleted unit must rearm (`reserve` → magazine via reload; an empty `reserve` is
/// refilled at a friendly camp by `crate::resupply`). `mag_size == 0` still means *no* magazine —
/// infinite ammo, no reload — which is what the Medic (disarmed) and the float-free combat unit
/// tests use, so a zero-mag weapon keeps the old engage pass exactly. AI auto-reload stays inside
/// the literal-executor rule (invariant #3): it reloads the gun it already holds, it does not pick
/// targets or maneuver. All `u16`/`Fixed`, no float (invariant #1).
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
    /// Ticks left in an in-progress reload (`0` = not reloading). Started by `Command::Reload`
    /// (embodied) or auto-started in combat upkeep when an AI unit's magazine runs dry; counted
    /// down in combat upkeep; on reaching zero the magazine draws up to `mag_size` rounds from
    /// `reserve`.
    pub reload_left: u16,
    /// Rounds carried in reserve, drawn into the magazine on each reload (D67 logistics). `0` means
    /// the unit is out of carried ammo — once the loaded magazine is also spent it is combat-
    /// ineffective until `crate::resupply` rearms it at a friendly camp. Only meaningful when
    /// `mag_size > 0`.
    pub reserve: u16,
    /// The reserve loadout cap — what `crate::resupply` refills `reserve` toward at a friendly camp
    /// (D67). A unit spawns with `reserve == reserve_max` (a full loadout). Only meaningful when
    /// `mag_size > 0`.
    pub reserve_max: u16,
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
    /// Current **aim-time dispersion** (reticle bloom), in `Fixed` (tank embodiment P5, D55). Grows
    /// while the tank's hull moves or its turret traverses ([`dispersion::bloom`](crate::dispersion::bloom))
    /// and settles back toward zero when it holds still and steady
    /// ([`dispersion::dispersion_system`](crate::dispersion::dispersion_system)); a launched shell's
    /// direction is perturbed in proportion to it
    /// ([`dispersion::scatter_dir`](crate::dispersion::scatter_dir)). A **fully settled** gun
    /// (`dispersion == 0`) fires **dead-on** with **zero scatter** — mastery is waiting for the
    /// reticle to settle. Meaningful only for a **ballistic tank gun** (`muzzle_vel > 0`): the
    /// dispersion system gates on that, so every infantry/hitscan weapon keeps `dispersion == 0`
    /// (the default) and is byte-neutral. `Fixed`, no float (invariant #1).
    pub dispersion: Fixed,
    /// The shell this gun currently has loaded (tank embodiment P6, D55). Switched by
    /// [`Command::SelectShell`](crate::sim::Command::SelectShell) and read only by the ballistic fire
    /// path ([`projectile::fire_ballistic`](crate::projectile::fire_ballistic)) to scale the launched
    /// shell's pen/damage and set its splash ([`ShellKind::stats`]). [`ShellKind::Ap`] is the zero-tag
    /// default, so every existing weapon — and every hitscan infantry weapon, which never reads it —
    /// is behaviourally unchanged. It is per-tank sim state, so it **folds** into the checksum
    /// (appended after `penetration`); a hitscan weapon's inert `Ap` adds one zero byte per slot.
    pub shell: ShellKind,
    /// **Stock** gunsmith move-speed offset (gunsmith breadth, CP-1 / D85). Added to the unit's
    /// base locomotion speed at **every** mover — the AI order mover
    /// ([`orders::order_system`](crate::orders)) and the embodied `Locomote`/crouch path — via the
    /// shared [`systems::with_move_delta`](crate::systems::with_move_delta), floored at
    /// [`systems::MIN_MOVE_SPEED`](crate::systems::MIN_MOVE_SPEED). `0` (the default for every
    /// legacy / Standard-stock weapon) returns the base speed **unchanged** — the zero-delta fast
    /// path — so a Standard unit stays bit-identical. Higher is faster (the Stock polarity).
    /// `Fixed`, no float (invariant #1).
    pub move_speed_delta: Fixed,
    /// **Stock** gunsmith aim-cone offset (gunsmith breadth, CP-1 / D85). Added to the embodied
    /// hitscan half-cone **cosine** in [`combat::resolve_fire`](crate::combat::resolve_fire)
    /// (higher cosine ⇒ **tighter** cone), clamped to `[0, 1]`. Embodied-only by nature — the AI
    /// `can_engage` path has no aim cone. `0` (the default) leaves the cone exactly as today (the
    /// zero-delta fast path). Higher is tighter/steadier (the Stock polarity). `Fixed`, no float
    /// (invariant #1).
    pub cone_cos_delta: Fixed,
    /// **Muzzle** gunsmith suppression-out offset (gunsmith breadth, CP-1 / D85). Added to the
    /// per-direct-hit suppression this weapon deals at **both** hit sites (AI
    /// [`combat_system`](crate::combat::combat_system) engage pass **and** embodied
    /// [`resolve_fire`](crate::combat::resolve_fire)), floored at zero. `0` (the default) deals
    /// exactly [`combat::SUPPRESSION_PER_HIT`](crate::combat::SUPPRESSION_PER_HIT) — the zero-delta
    /// fast path. Higher is more suppression (the Muzzle polarity). `Fixed`, no float (invariant #1).
    pub supp_out_delta: Fixed,
    /// **Muzzle** gunsmith downrange-falloff amount (gunsmith breadth, CP-1 / D85). Drives a
    /// **sqrt-free**, `dist_sq`-bucketed damage multiplier beyond half weapon range in
    /// [`combat::falloff_multiplier`](crate::combat::falloff_multiplier): within `range/2`, full
    /// damage; beyond it, `ONE − falloff_delta` (floored at zero). `0` (the default) yields a
    /// multiplier of **exactly [`Fixed::ONE`] at every range**, so it is byte-neutral for every
    /// existing weapon. **Lower** is better downrange retention (the Muzzle polarity, mirroring
    /// `cooldown`/`reload`). `Fixed`, no float (invariant #1).
    pub falloff_delta: Fixed,
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
