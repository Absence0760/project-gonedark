//! Economy, camps, and production (invariant #1 — integer/fixed-point, deterministic).
//!
//! Holds the per-faction [`Resources`] purse and drives buildings: construction progress,
//! upgrades, territory-fed income, and FIFO unit production that spawns finished units into
//! the world. All command entry points ([`build`], [`upgrade`], [`queue_production`]) are
//! pure functions the sim calls from `Sim::apply`; the per-tick advance is [`economy_system`].
//!
//! Determinism: resource counts are plain `i64` (no float money), income/build/production all
//! advance by integer ticks in stable entity-index order, and produced units get their stats
//! from a fixed [`UnitKind`] table so every peer spawns the bit-identical unit.
//!
//! KEEP the `Resources` field shape (`amounts: [i64; FACTION_COUNT]`) and all public signatures
//! intact — the sim's checksum folds `Resources` by that shape.

use crate::components::{
    Armor, Army, Building, BuildingKind, EntityKind, Faction, Health, Order, ProductionItem,
    ShellKind, Stance, UnitKind, Vec2, Weapon, FACTION_COUNT,
};
use crate::ecs::{Entity, World};
use crate::event::SimEvent;
use crate::fixed::Fixed;
use crate::rng::Rng;
use crate::territory::Territory;

// ===========================================================================
// Cost / time / stat tables. All integer or fixed-point (invariant #1). These
// are the single source of truth every peer reads, so the same action costs and
// the same unit spawns identically everywhere (lockstep).
//
// MEASURED BALANCE BASELINE (D30 — supersedes the D26 first pass).
// ---------------------------------------------------------------------------
// Still a *playtest baseline*, NOT a locked design — but every combat/cost
// number here was moved against an objective, deterministic metric (the
// sim-runner `--metrics` harness: open 1v1 time-to-kill, equal-cost army
// trades, suppression pin-vs-kill timing, the economy ramp curve), so the
// shape is justified by measurement rather than vibe. Final *feel* still
// awaits human playtests. Reasoned in *seconds* (the sim runs at 60 Hz, so
// `seconds * 60 = ticks`) and against the demo's seed purse of
// `Resources::new(500)` (see sim-runner / engine). The economy must remain
// integer/fixed-point (invariant #1) and bit-identical dev==release.
//
// The shape of the design:
//   * Income drips per-tick. With base 1/tick that is 60 resources/second of
//     hands-off income; each held point adds 2/tick = 120/second. So holding
//     one point roughly *triples* your income — territory genuinely matters.
//   * Costs are sized in that 60/s frame so they read in seconds of saving:
//     a Rifleman (~1.7 s of base income) is cheap, spammable, long-ranged; a
//     Heavy (~3.7 s) is a real investment that buys 2.8x HP and 3x burst at
//     SHORTER range — a short-range bruiser, not a strict upgrade; a camp
//     (~4 s, half the seed purse) is a commitment but affordable turn-one.
//   * The Rifleman↔Heavy matchup is range-dependent by design: at point-blank
//     the cost-equal Heavy blob out-trades the rifles; at rifle range the
//     cheaper, longer-reaching rifles kite and win (harness-verified — the old
//     Heavy was strictly dominated and lost every equal-cost trade).
//   * Build/production times read in seconds: Rifleman a handful of seconds,
//     Heavy notably longer, camp construction longer still.
//   * A camp + one held point pays its 250 cost back in ~2 s of holding, so
//     "expand + bank a camp" is a real economic line against "spend it on
//     bodies now" — that fork is the intended decision.
// ===========================================================================

/// Cost (resources) to start building a [`Camp`](BuildingKind::Camp).
/// 250 = half the demo seed purse (500): a genuine commitment, yet you can
/// still afford exactly one turn-one (leaving 250 for an opening unit or two).
pub const CAMP_BUILD_COST: i64 = 250;

/// Resource cost to produce one [`Rifleman`](UnitKind::Rifleman).
/// 100 ≈ 1.7 s of base income (60/s): cheap and spammable, the bread-and-butter
/// body you mass.
pub const RIFLEMAN_COST: i64 = 100;
/// Resource cost to produce one [`Heavy`](UnitKind::Heavy).
/// 220 = 2.2x a Rifleman. The Heavy is a short-range *bruiser*: 3x HP (300 vs 100)
/// and 100-vs-30 burst at *shorter* range (11 vs 14). The 2.2x cost is tuned
/// (D30) so the equal-cost trade is genuinely range-dependent — at point-blank the
/// Heavy mass trades up, at rifle range the cheaper, longer-reaching Rifleman mass
/// wins — instead of the old strictly-dominated Heavy (measured rifle-mass-wipes-
/// heavy under D26's numbers). The HP/burst were re-tuned up from the D66 280/90 baseline
/// (combat-rebalance-plan WS-A): at the ×5 lethal kill speed 280/90 had flattened the RPS
/// (rifles won at *every* range); 300/100 restores heavy-wins-close / rifle-kites-at-range,
/// harness-measured (`--metrics`: sep5 → heavy +2, sep9 → rifle +2).
pub const HEAVY_COST: i64 = 220;

/// Ticks to finish a freshly-placed camp's construction. 1200 ticks = 20 s — a
/// camp is a slow, deliberate structural commitment, far longer than any unit.
pub const CAMP_BUILD_TICKS: u16 = 1200;

/// Base ticks to produce one [`Rifleman`](UnitKind::Rifleman) (before level
/// speedup). 300 ticks = 5 s: a handful of seconds, fast enough to reinforce.
pub const RIFLEMAN_BASE_TICKS: u16 = 300;
/// Base ticks to produce one [`Heavy`](UnitKind::Heavy) (before level speedup).
/// 660 ticks = 11 s: notably longer than a Rifleman, matching its higher cost
/// (2.2x) and battlefield value. Kept proportional to the 220 cost (3x the Rifleman
/// production time for 2.2x the cost) so producing Heavies stays a deliberate, slow
/// commitment rather than a spam option (D30).
pub const HEAVY_BASE_TICKS: u16 = 660;

/// Each upgrade level shaves this many ticks off production time...
/// 60 ticks = 1 s faster per level — a tangible, readable reward for investing
/// in a camp instead of (or alongside) more bodies.
pub const LEVEL_PROD_SPEEDUP: u16 = 60;
/// ...down to no faster than this floor (so a maxed camp can't produce instantly).
/// 120 ticks = 2 s: even a fully-upgraded camp still takes a beat per unit, so
/// production speed never trivializes the army-vs-economy tension.
pub const PROD_TICKS_FLOOR: u16 = 120;

/// Resources every faction accrues per tick regardless of held territory.
/// 1/tick = 60/second — a steady hands-off drip you always get.
pub const BASE_INCOME: i64 = 1;
/// Extra per-tick resources per controlled territory point.
/// 2/tick = 120/second per point — each point roughly *doubles* base income, so
/// holding ground is the dominant way to out-produce an opponent.
pub const PER_POINT_INCOME: i64 = 2;

/// Starting HP of a [`Camp`](BuildingKind::Camp). 1000 HP — ~4.5x a Rifleman and
/// ~4.5x a Heavy: a strategic structure that takes a sustained commitment to
/// raze, not something a stray squad deletes in passing.
const CAMP_HP: i32 = 1000;

// --- New content (D65): Tank, Medic, Barracks. A playtest BASELINE only — NOT D30-measured (D30
// covers Rifleman/Heavy); dial against a future `--metrics` pass. Same integer/fixed-point rules. ---

/// Cost to produce a [`Tank`](UnitKind::Tank) — a heavy vehicle, the priciest unit. 360 ≈ 3.6
/// Riflemen (~6 s of base income): massing armour is a real commitment.
pub const TANK_COST: i64 = 360;
/// Base ticks to produce a [`Tank`](UnitKind::Tank). 840 = 14 s — slow, deliberate armour.
pub const TANK_BASE_TICKS: u16 = 840;

// --- Produced-Tank directional armour (tank embodiment P9, the armour half — completes the D65
// unarmoured-tank stopgap). The produced `UnitKind::Tank` is now a real armoured vehicle: thick
// front, thinner side, thinnest rear, in the same `Fixed` units a `Weapon::penetration` is measured
// in and resolved by `combat::facing_penetration_multiplier`. Same scale as the duel-scene reference
// chassis (`scenario::DUEL_ARMOR_*`) so the project carries ONE armour vocabulary, not two: a
// penetration of 18 (the duel gun) bounces head-on (2·18 = 36 < 40) but pens the side (18 ≥ 16) and
// rear (18 ≥ 8) — *angle the hull at the enemy; flank to kill*. Held IDENTICAL across armies (no
// `unit_stats_for` tilt): armour is a snowball-sensitive combat axis, fenced by the same fairness
// band as damage/HP/penetration (factions-plan WS-B). No float (invariant #1).
//
// CONSEQUENCE (intended): infantry carry `penetration == 0`, and any positive armour facet bounces a
// zero-pen shot on EVERY facet (2·0 ≤ a). So a produced Tank now shrugs off all small-arms fire — it
// is an armoured vehicle, killed only by a penetrating gun (another tank's main gun). This replaces
// the D65 stopgap that kept the tank unarmoured precisely to avoid this, now that the tank IS meant
// to read as armour. No pre-placed shipping scenario produces a `UnitKind::Tank` (the duel chassis is
// a re-dressed Heavy), so no golden checksum moves.
/// Frontal armour of a produced Tank — the thickest facet. Sized so a duel-class penetration (18)
/// cannot crack it head-on (`2·18 = 36 < 40` ⇒ hard bounce) but a flank/rear shot lands.
pub const TANK_ARMOR_FRONT: Fixed = Fixed::from_int(40);
/// Side (flank) armour of a produced Tank — thinner; a duel-class penetration pens it (`18 ≥ 16`).
pub const TANK_ARMOR_SIDE: Fixed = Fixed::from_int(16);
/// Rear armour of a produced Tank — the thinnest facet; cracked by even a modest penetrating gun.
pub const TANK_ARMOR_REAR: Fixed = Fixed::from_int(8);

/// Cost to produce a [`Medic`](UnitKind::Medic) — a cheap support body. 120 ≈ 1.2 Riflemen.
pub const MEDIC_COST: i64 = 120;
/// Base ticks to produce a [`Medic`](UnitKind::Medic). 360 = 6 s.
pub const MEDIC_BASE_TICKS: u16 = 360;

/// Cost to start building a [`Barracks`](BuildingKind::Barracks). 150 — cheaper than a Camp (250):
/// an affordable forward infantry / medic hub.
pub const BARRACKS_BUILD_COST: i64 = 150;
/// Ticks to finish a [`Barracks`](BuildingKind::Barracks). 600 = 10 s (faster than a Camp's 20 s).
pub const BARRACKS_BUILD_TICKS: u16 = 600;
/// Starting HP of a [`Barracks`](BuildingKind::Barracks). 600 — sturdier than a unit, softer than a
/// Camp (1000).
const BARRACKS_HP: i32 = 600;

/// Cost to upgrade a camp currently at `level` to the next tier: `200 * (level + 1)`.
/// Level 0→1 costs 200 (≈ two Riflemen), and each tier costs more (200, 400,
/// 600, …) so deep upgrades are a real resource sink competing with army size.
#[inline]
pub const fn upgrade_cost(level: u8) -> i64 {
    200 * (level as i64 + 1)
}

/// Resource cost to produce one unit of `kind`.
#[inline]
pub const fn unit_cost(kind: UnitKind) -> i64 {
    match kind {
        UnitKind::Rifleman => RIFLEMAN_COST,
        UnitKind::Heavy => HEAVY_COST,
        UnitKind::Tank => TANK_COST,
        UnitKind::Medic => MEDIC_COST,
    }
}

/// Production time (ticks) for `kind` at a camp of `level`. Higher tiers produce faster,
/// clamped to [`PROD_TICKS_FLOOR`] so production is always at least that many ticks.
#[inline]
pub const fn prod_time(kind: UnitKind, level: u8) -> u16 {
    let base = match kind {
        UnitKind::Rifleman => RIFLEMAN_BASE_TICKS,
        UnitKind::Heavy => HEAVY_BASE_TICKS,
        UnitKind::Tank => TANK_BASE_TICKS,
        UnitKind::Medic => MEDIC_BASE_TICKS,
    };
    let speedup = LEVEL_PROD_SPEEDUP.saturating_mul(level as u16);
    let reduced = base.saturating_sub(speedup);
    if reduced < PROD_TICKS_FLOOR {
        PROD_TICKS_FLOOR
    } else {
        reduced
    }
}

/// Build cost for a `kind` building.
#[inline]
pub const fn build_cost(kind: BuildingKind) -> i64 {
    match kind {
        BuildingKind::Camp => CAMP_BUILD_COST,
        BuildingKind::Barracks => BARRACKS_BUILD_COST,
    }
}

/// Construction time (ticks) for a `kind` building.
#[inline]
pub const fn build_ticks(kind: BuildingKind) -> u16 {
    match kind {
        BuildingKind::Camp => CAMP_BUILD_TICKS,
        BuildingKind::Barracks => BARRACKS_BUILD_TICKS,
    }
}

/// Starting HP for a `kind` building.
#[inline]
const fn building_hp(kind: BuildingKind) -> i32 {
    match kind {
        BuildingKind::Camp => CAMP_HP,
        BuildingKind::Barracks => BARRACKS_HP,
    }
}

/// Whether a `building` kind can produce a `unit` kind (the production-routing rule, D65). The Camp
/// (base) fields infantry and vehicles; the Barracks is infantry-only and is the **sole source of
/// the Medic**. `queue_production` enforces this, so a mismatched request is simply rejected.
#[inline]
pub const fn can_produce(building: BuildingKind, unit: UnitKind) -> bool {
    matches!(
        (building, unit),
        (BuildingKind::Camp, UnitKind::Rifleman | UnitKind::Heavy | UnitKind::Tank)
            | (BuildingKind::Barracks, UnitKind::Rifleman | UnitKind::Medic)
    )
}

/// Fixed combat stats a produced unit spawns with — looked up from [`UnitKind`] so every peer
/// spawns the bit-identical unit (determinism).
pub fn unit_stats(kind: UnitKind) -> (Health, Weapon) {
    match kind {
        // Modern-lethality re-tune (D66 — supersedes the D30 attrition baseline). Per-shot damage
        // is scaled ×5 across every weapon so a hit *matters* like a real rifle round: the old D30
        // numbers made a soldier a ~17-round bullet sponge (~8 s to drop one rifleman), which read
        // as unrealistic for the US-vs-France modern-army fantasy (game-design §3). Scaling every
        // weapon by the SAME factor preserves the whole D30 balance lattice (DPS *ratios*, the
        // range-trade rock-paper-scissors, cover swings) — it just makes the clock 5× faster:
        //   * Rifleman: 30 dmg / 30 ticks = 60 DPS → a symmetric open 1v1 now resolves in ~1-2 s
        //     (4 hits to drop a 100-HP soldier), and long-reaching (range 14) so rifle MASS still
        //     wins at range.
        //   * Heavy: a short-range BRUISER — 300 HP, 100 dmg / 48 ticks at range 11. Out-
        //     trades a cost-equal Rifleman blob at point-blank, still kited by the longer-ranged
        //     Rifleman (combat-rebalance-plan WS-A re-tune off the D66 280/90 baseline: at the new
        //     ×5 kill speed 280/90 lost the equal-cost trade at every range — rifle body-count +
        //     cadence dominated — so the D30 range RPS had flattened. 300/100 restores it,
        //     harness-measured: equal-cost sep5 → heavy wins +2, sep9 → rifle kites +2).
        // CAVEAT: with kills this fast, the per-*hit* suppression model (`combat::SUPPRESSION_PER_HIT`)
        // mostly stops biting before death — fire-and-maneuver suppression wants a per-near-miss
        // rework. Logged as an open question, not fixed here (D66).
        // Still a *playtest baseline* (measured targets, not final feel); dial against fresh
        // `--metrics` runs.
        UnitKind::Rifleman => (
            Health::full(Fixed::from_int(100)),
            Weapon {
                range: Fixed::from_int(14),
                damage: Fixed::from_int(30),
                cooldown_ticks: 30,
                cooldown_left: 0,
                // All-unit ammo (D67): a 30-round mag, 90-tick reload (≈1500 ms at 60 Hz), and six
                // spare mags carried (reserve 180 ≈ a real ~210-round rifle loadout). Rations both
                // the embodied player and AI units; rearmed at a friendly camp by `crate::resupply`.
                mag_size: 30,
                ammo: 30,
                reload_ticks: 90,
                reload_left: 0,
                reserve: 180,
                reserve_max: 180,
                // Infantry rifle: fixed mount, no independent turret (P2 default).
                turret_speed: 0,
                // Hitscan infantry weapon (P3 default): no shell flight, resolves instantly.
                muzzle_vel: Fixed::ZERO,
                // No armour penetration (P4 default): full damage vs the unarmoured units it fights
                // (multiplier 1.0); only bites against a future armoured kind. Balance unchanged.
                penetration: Fixed::ZERO,
                // Aim-time dispersion (P5 default): a hitscan infantry/vehicle gun never blooms
                // (the dispersion system gates on `muzzle_vel > 0`), so it stays settled at zero.
                dispersion: Fixed::ZERO,
                // Loads AP by default (P6, D55): inert for a hitscan unit (`muzzle_vel == 0` never
                // reads `shell`); the field just rides along, byte-folded as a zero tag.
                shell: ShellKind::Ap,
            },
        ),
        UnitKind::Heavy => (
            Health::full(Fixed::from_int(300)),
            Weapon {
                range: Fixed::from_int(11),
                damage: Fixed::from_int(100),
                cooldown_ticks: 48,
                cooldown_left: 0,
                // Bigger belt, slower 138-tick reload (≈2300 ms) — the bruiser sustains fire
                // longer but is punished harder for running dry. Four spare belts in reserve (200).
                mag_size: 50,
                ammo: 50,
                reload_ticks: 138,
                reload_left: 0,
                reserve: 200,
                reserve_max: 200,
                // Heavy infantry bruiser: still a fixed mount (the playable tank is the new
                // dedicated kind, plan §3). No independent turret here.
                turret_speed: 0,
                // Hitscan infantry weapon (P3 default): no shell flight, resolves instantly.
                muzzle_vel: Fixed::ZERO,
                // No armour penetration (P4 default) — unchanged balance vs unarmoured units.
                penetration: Fixed::ZERO,
                // Aim-time dispersion (P5 default): a hitscan infantry/vehicle gun never blooms
                // (the dispersion system gates on `muzzle_vel > 0`), so it stays settled at zero.
                dispersion: Fixed::ZERO,
                // Loads AP by default (P6, D55): inert for a hitscan unit (`muzzle_vel == 0` never
                // reads `shell`); the field just rides along, byte-folded as a zero tag.
                shell: ShellKind::Ap,
            },
        ),
        // A produced armoured vehicle (D65). High HP + a hard, slow gun + an independent turret slew
        // (cosmetic). UNARMOURED on purpose: with `penetration == 0` an armoured tank would bounce
        // every Rifleman shot (no anti-tank counter exists yet), which would break the rifle-centric
        // skirmish — the full armoured/ballistic tank stays the duel scene's. `muzzle_vel == 0` keeps
        // it hitscan, so auto-combat resolves it exactly like the other produced units.
        UnitKind::Tank => (
            Health::full(Fixed::from_int(300)),
            Weapon {
                range: Fixed::from_int(13),
                damage: Fixed::from_int(120),
                cooldown_ticks: 75,
                cooldown_left: 0,
                // The main gun stows finite shells too (D67): 6 ready, a slow 240-tick (≈4 s) reload,
                // 24 in reserve = 30 rounds before it must pull back to a camp to rearm.
                mag_size: 6,
                ammo: 6,
                reload_ticks: 240,
                reload_left: 0,
                reserve: 24,
                reserve_max: 24,
                turret_speed: 180,
                // BALLISTIC main gun (tank P9, D72): the produced tank's shot is a real traveling
                // shell, not hitscan — `muzzle_vel > 0` routes both AI auto-fire (`combat_system`)
                // and embodied `Fire` through `projectile::fire_ballistic`. `2` world-units/tick
                // matches the duel-tank gun (`scenario::DUEL_GUN_MUZZLE_VEL`): a readable arc at the
                // locked 60 Hz, so a moving target can out-run or dodge a shot in flight.
                muzzle_vel: Fixed::from_int(2),
                // Armour penetration (D55 P4 model), matching the duel gun (`DUEL_GUN_PENETRATION`):
                // against an UNARMOURED target it is multiplier 1.0 (full damage — balance vs rifles
                // is unchanged), and it pens flanks/rears while bouncing a thick frontal facet once a
                // real armoured tank exists to fire at. Float-free (invariant #1).
                penetration: Fixed::from_int(18),
                // Aim-time dispersion (P5): starts fully settled (`0`). An AI tank never blooms — the
                // dispersion system only *settles* toward zero, and bloom is added solely at the
                // embodied drive/aim sites — so an AI shot fires dead-on along the bearing (the
                // literal executor, invariant #3). An embodied driver's traverse/move blooms it.
                dispersion: Fixed::ZERO,
                // Loads AP by default (P6, D55): solid shot — full pen, full point damage, no splash.
                // The embodied player can `SelectShell` HE/APHE; AI tanks fire the loaded AP.
                shell: ShellKind::Ap,
            },
        ),
        // A support unit (D65): NO offensive weapon (range 0 ⇒ combat never acquires a target for
        // it), modest HP. It contributes through `crate::heal` (heals nearby friendlies), never
        // `combat`.
        UnitKind::Medic => (
            Health::full(Fixed::from_int(90)),
            Weapon {
                range: Fixed::ZERO,
                damage: Fixed::ZERO,
                cooldown_ticks: 0,
                cooldown_left: 0,
                mag_size: 0,
                ammo: 0,
                reload_ticks: 0,
                reload_left: 0,
                reserve: 0,
                reserve_max: 0,
                turret_speed: 0,
                muzzle_vel: Fixed::ZERO,
                penetration: Fixed::ZERO,
                // Aim-time dispersion (P5 default): a hitscan infantry/vehicle gun never blooms
                // (the dispersion system gates on `muzzle_vel > 0`), so it stays settled at zero.
                dispersion: Fixed::ZERO,
                // Loads AP by default (P6, D55): inert for a hitscan unit (`muzzle_vel == 0` never
                // reads `shell`); the field just rides along, byte-folded as a zero tag.
                shell: ShellKind::Ap,
            },
        ),
    }
}

/// Directional [`Armor`] for a produced unit of `kind` (tank embodiment P9). Only the
/// [`Tank`](UnitKind::Tank) is armoured — every other archetype returns the all-zero
/// [`Armor::default()`] (unarmoured), so it takes byte-identical damage to today
/// ([`combat::facing_penetration_multiplier`](crate::combat::facing_penetration_multiplier) returns
/// exactly `1.0` for an unarmoured defender) and no checksum moves where the tank is absent
/// (invariant #7). Kept **separate** from [`unit_stats`]'s `(Health, Weapon)` tuple so the armour
/// concern doesn't disturb that signature's ~30 call sites, and is **not** per-army (`unit_stats_for`
/// has no armour tilt): armour is a snowball-sensitive combat axis, held identical across armies by
/// the same fairness band as damage/HP/penetration. No float (invariant #1).
#[inline]
pub fn unit_armor(kind: UnitKind) -> Armor {
    match kind {
        UnitKind::Tank => Armor {
            front: TANK_ARMOR_FRONT,
            side: TANK_ARMOR_SIDE,
            rear: TANK_ARMOR_REAR,
        },
        // Infantry and support carry no armour — the unarmoured default that preserves today's
        // balance exactly (multiplier 1.0 on every shot, regardless of facing or penetration).
        UnitKind::Rifleman | UnitKind::Heavy | UnitKind::Medic => Armor::default(),
    }
}

// ===========================================================================
// PER-FACTION ROSTERS (factions-plan WS-B, D68). The shared archetype skeleton
// (Rifleman / Heavy / Tank / Medic) is the same on every army; an army's identity
// is a **tilt** layered on the [`unit_stats`] baseline, NOT a separate roster —
// invariant #2 (one shared core, faction = content + a table).
//
// FAIRNESS BAND (pillar 4 / D30 unit-parity discipline — asymmetry of FEEL, never
// POWER), and WHY the tilt is on logistics, not the gun:
//   The equal-cost mass trade with `FireAtWill` is a **Lanchester square-law snowball** —
//   a tiny edge in per-shot damage or cadence compounds to a near-total wipe (MEASURED:
//   a 2-point damage gap on the Rifleman runs 10-vs-0). So the snowball-sensitive
//   combat axes — **damage, cooldown, HP, range, penetration** — are held IDENTICAL across
//   US and FR. There is no fair "soft gun tilt" on a mirror mass trade; the harness proves it.
//
//   The identity lives instead on the **logistics rhythm** — magazine depth, reload length,
//   reserve loadout, and (cosmetic, invariant #3) turret slew — scaled to keep **sustained
//   DPS and reload-depth invariant** (`mag` and `reload_ticks` move together by the same
//   ratio, and `reserve` keeps the same mag-count). US fields the sustained-fire doctrine
//   (deeper magazine, longer reload — the M249/M240 belt, M1 Abrams); FR the quick-swap
//   doctrine (shorter magazine, snappier reload — the FAMAS/HK416F bullpup, Leclerc
//   autoloader turret). Two same-role units put out the *same* fire and trade *evenly*; they
//   only diverge in reload cadence and loadout depth — a real feel/doctrine difference that
//   shows up in sustained and embodied play, never as combat power. The cross-faction
//   equal-cost "mirror of roles" trade therefore stays inside a TIGHT band (the per-faction
//   analogue of D30's unit-parity check), MEASURED against `sim-runner --metrics`
//   (`metrics::cross_faction_equal_cost`), not asserted by feel.
//
// All integer / fixed-point (invariant #1). The tilt touches `mag_size`/`ammo`,
// `reload_ticks`, `reserve`/`reserve_max` (all `u16`) and `turret_speed` (`u16`) — every one
// folds into the per-tick checksum via the spawned unit's `Weapon`, so two peers that picked
// different armies diverge in spawned-unit stats and the desync is caught there (invariant #7).
//
// The MEDIC carries NO tilt: it is a non-combatant (it heals via `crate::heal`, never fights)
// with no magazine and an HP-only sim surface, so any nudge would be *uncompensated power* on
// a unit that cannot trade — a fairness violation, not feel. Every army still fields the Medic;
// its identity is presentation-only (WS-C, silhouette/voicelines). `Army::Neutral` carries no
// tilt either: it is the shared baseline, so every legacy / non-aligned scene spawns
// byte-for-byte the pre-factions unit.
// ===========================================================================

/// A per-army **logistics tilt** over the shared [`unit_stats`] baseline: the magazine depth,
/// reload length, reserve loadout, and turret slew an army's variant of an archetype carries.
/// All other fields (damage, cooldown, range, HP, penetration) stay the shared baseline — the
/// fairness bound (see the module note). Zero-valued fields mean "no logistics tilt" (the Medic,
/// any `Neutral` unit), leaving the baseline untouched.
#[derive(Clone, Copy)]
struct LogisticsTilt {
    mag_size: u16,
    reload_ticks: u16,
    reserve: u16,
    turret_speed: u16,
}

/// The per-army logistics tilt for an `(army, kind)`, or `None` for the shared baseline (Neutral,
/// or any army's non-combatant Medic). Each tilt holds **sustained DPS and reload-depth invariant**
/// vs the baseline (`mag`/`reload` scale together; `reserve` keeps the same mag-count), so it is
/// power-neutral by construction and the mirror trade stays in the fairness band.
///
/// Integer-only (invariant #1); every field is a `u16`.
const fn faction_logistics_tilt(army: Army, kind: UnitKind) -> Option<LogisticsTilt> {
    match (army, kind) {
        // Shared baseline: the non-aligned default and every army's (non-combatant) Medic.
        (Army::Neutral, _) | (_, UnitKind::Medic) => None,
        // Rifleman (baseline mag 30 / reload 90 / reserve 180). US +20 % magazine (deeper belt,
        // longer reload); FR −20 % (snappier swap). Both keep 6 mags of reserve and the same
        // sustained rate (mag/reload scale together) → power-neutral.
        (Army::Us, UnitKind::Rifleman) => Some(LogisticsTilt { mag_size: 36, reload_ticks: 108, reserve: 216, turret_speed: 0 }),
        (Army::Fr, UnitKind::Rifleman) => Some(LogisticsTilt { mag_size: 24, reload_ticks: 72, reserve: 144, turret_speed: 0 }),
        // Heavy (baseline mag 50 / reload 138 / reserve 200, the support-weapon belt). US deeper
        // belt + longer reload (M249/M240); FR lighter (Minimi/AANF1). 4 reloads of reserve each.
        (Army::Us, UnitKind::Heavy) => Some(LogisticsTilt { mag_size: 60, reload_ticks: 166, reserve: 240, turret_speed: 0 }),
        (Army::Fr, UnitKind::Heavy) => Some(LogisticsTilt { mag_size: 40, reload_ticks: 110, reserve: 160, turret_speed: 0 }),
        // Tank: the main-gun magazine (baseline mag 6 / reload 240 / reserve 24) is too SHALLOW for a
        // fair logistics tilt — with only ~6 ready shells, any difference in shell count / reload
        // length shifts the reload PHASE enough that the Lanchester snowball lets the faster-reloading
        // side win a long fight (MEASURED: a US-7/280 vs FR-5/200 tilt handed the FR side a contrived
        // tank-in-cover standoff 2-0). So the tank keeps the SHARED gun/magazine and tilts only its
        // **turret slew** — cosmetic by invariant #3 (the slew never picks targets, the AI turret just
        // tracks the hull), so it is provably zero combat power: US the heavier manual-loader turret
        // (M1 Abrams, slower slew); FR the autoloader turret (Leclerc, quicker slew).
        (Army::Us, UnitKind::Tank) => Some(LogisticsTilt { mag_size: 6, reload_ticks: 240, reserve: 24, turret_speed: 160 }),
        (Army::Fr, UnitKind::Tank) => Some(LogisticsTilt { mag_size: 6, reload_ticks: 240, reserve: 24, turret_speed: 200 }),
    }
}

/// Per-(army, archetype) combat stats a produced unit spawns with (factions-plan WS-B). The
/// [`Army::Neutral`] baseline is exactly [`unit_stats`] (every legacy scene unchanged); `Us`/`Fr`
/// apply the fairness-banded **logistics tilt** ([`faction_logistics_tilt`]) on top — magazine,
/// reload, reserve, and turret only; the snowball-sensitive gun stats (damage/cooldown/range/HP)
/// stay shared so the mirror trade stays fair (see the module note). Every army fields every
/// archetype — there is no missing role. Determinism: the table is fixed-point and identical on
/// every peer, so a given `(army, kind)` spawns the bit-identical unit everywhere (invariant #1/#7).
pub fn unit_stats_for(army: Army, kind: UnitKind) -> (Health, Weapon) {
    let (health, mut weapon) = unit_stats(kind);
    if let Some(tilt) = faction_logistics_tilt(army, kind) {
        weapon.mag_size = tilt.mag_size;
        weapon.ammo = tilt.mag_size; // spawns with a full magazine (mirrors `unit_stats`)
        weapon.reload_ticks = tilt.reload_ticks;
        weapon.reserve = tilt.reserve;
        weapon.reserve_max = tilt.reserve;
        weapon.turret_speed = tilt.turret_speed;
    }
    (health, weapon)
}

/// Per-faction resource purse. Indexed by [`Faction::index`]; plain `i64` so there is no
/// float money in the deterministic sim. SHAPE IS PINNED (checksum folds `amounts`).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Resources {
    pub amounts: [i64; FACTION_COUNT],
}

impl Resources {
    /// Start every faction with `initial` resources.
    pub fn new(initial: i64) -> Self {
        Resources {
            amounts: [initial; FACTION_COUNT],
        }
    }

    #[inline]
    pub fn get(&self, faction: Faction) -> i64 {
        self.amounts[faction.index()]
    }

    #[inline]
    pub fn add(&mut self, faction: Faction, delta: i64) {
        self.amounts[faction.index()] += delta;
    }

    /// Spend `cost` if affordable; returns whether the spend happened (no debt allowed).
    #[inline]
    pub fn try_spend(&mut self, faction: Faction, cost: i64) -> bool {
        let i = faction.index();
        if self.amounts[i] >= cost {
            self.amounts[i] -= cost;
            true
        } else {
            false
        }
    }
}

/// Start construction of a `kind` building for `faction` at `pos`, spending its cost. Returns
/// the new building entity, or `None` if unaffordable. STUB (worker 3).
pub fn build(
    world: &mut World,
    resources: &mut Resources,
    faction: Faction,
    kind: BuildingKind,
    pos: Vec2,
) -> Option<Entity> {
    if !resources.try_spend(faction, build_cost(kind)) {
        return None;
    }
    let e = world.spawn();
    let i = e.index as usize;
    world.kind[i] = EntityKind::Building;
    world.faction[i] = faction;
    world.pos[i] = pos;
    world.health[i] = Health::full(Fixed::from_int(building_hp(kind)));
    world.order[i] = Order::Idle;
    world.building[i] = Building {
        kind,
        level: 0,
        build_ticks_left: build_ticks(kind),
        queue: Vec::new(),
    };
    Some(e)
}

/// Upgrade a built camp one level, spending the upgrade cost. Returns whether it happened.
/// STUB (worker 3).
pub fn upgrade(world: &mut World, resources: &mut Resources, camp: Entity) -> bool {
    if !world.is_alive(camp) {
        return false;
    }
    let i = camp.index as usize;
    if world.kind[i] != EntityKind::Building || world.building[i].build_ticks_left != 0 {
        return false;
    }
    let level = world.building[i].level;
    if !resources.try_spend(world.faction[i], upgrade_cost(level)) {
        return false;
    }
    world.building[i].level = level.saturating_add(1);
    true
}

/// Enqueue a `unit` for production at a built `camp`, spending its cost. Returns whether it
/// was queued. STUB (worker 3).
pub fn queue_production(
    world: &mut World,
    resources: &mut Resources,
    camp: Entity,
    unit: UnitKind,
) -> bool {
    if !world.is_alive(camp) {
        return false;
    }
    let i = camp.index as usize;
    if world.kind[i] != EntityKind::Building || world.building[i].build_ticks_left != 0 {
        return false;
    }
    // Production routing (D65): the building must be able to make this unit (Camp = infantry +
    // vehicles; Barracks = infantry + Medic). A mismatched request is rejected without spending.
    if !can_produce(world.building[i].kind, unit) {
        return false;
    }
    if !resources.try_spend(world.faction[i], unit_cost(unit)) {
        return false;
    }
    let level = world.building[i].level;
    world.building[i].queue.push(ProductionItem {
        kind: unit,
        ticks_left: prod_time(unit, level),
    });
    true
}

/// Advance one tick of economy: income from held territory, construction, upgrades, and
/// production (spawning finished units). STUB (worker 3) — no-op so the scaffold compiles.
#[allow(clippy::too_many_arguments)] // honest sim inputs; bundling them buys no clarity
pub fn economy_system(
    world: &mut World,
    resources: &mut Resources,
    territory: &Territory,
    events: &mut Vec<SimEvent>,
    rng: &mut Rng,
    tick: u64,
    income_period: u32,
    armies: &[Army; FACTION_COUNT],
) {
    let _ = rng;

    // --- INCOME (integer accrual; Neutral never earns) ---
    // Income accrues once every `income_period` ticks (default 1 = every tick, the full D30 rate).
    // A larger period is the scenario-local pace lever (the skirmish slows the drip without touching
    // the D30 cost/stat constants): the per-accrual amount is unchanged, so a held point still
    // ~triples income — only the cadence stretches. `tick` is the pre-increment counter (folded into
    // the checksum), so the gate fires identically on every peer (invariant #7). Clamp 0 → 1 so a
    // malformed period can never divide by zero.
    let period = income_period.max(1) as u64;
    if tick.is_multiple_of(period) {
        for &faction in Faction::ALL.iter() {
            if faction == Faction::Neutral {
                continue;
            }
            let count = territory.controlled_count(faction) as i64;
            resources.add(faction, BASE_INCOME + PER_POINT_INCOME * count);
        }
    }

    // --- BUILDINGS: construction + production countdown ---
    // First scan (index order): advance construction, count down the front production item,
    // and record any camp whose front item completed THIS tick. We do not spawn here —
    // `world.spawn()` may reallocate the SoA Vecs, so we collect completions and spawn after,
    // still in index order (deterministic).
    let mut completed: Vec<(usize, UnitKind)> = Vec::new();
    let cap = world.capacity();
    for i in 0..cap {
        if !world.is_index_alive(i) || world.kind[i] != EntityKind::Building {
            continue;
        }
        if world.building[i].build_ticks_left > 0 {
            world.building[i].build_ticks_left -= 1;
            continue;
        }
        // Any operational building serves its production queue (Camp or Barracks, D65); what may be
        // queued at each is gated upstream by `can_produce` in `queue_production`.
        if let Some(front) = world.building[i].queue.first_mut() {
            if front.ticks_left > 0 {
                front.ticks_left -= 1;
            }
            if front.ticks_left == 0 {
                let item = world.building[i].queue.remove(0);
                completed.push((i, item.kind));
            }
        }
    }

    // Second pass: spawn finished units (index order preserved).
    for (camp_i, unit_kind) in completed {
        let faction = world.faction[camp_i];
        let pos = world.pos[camp_i];
        // Draw the spawned unit's stats from the PRODUCING faction's army roster (factions-plan
        // WS-B): a US camp fields the US variant, an FR camp the FR variant; a non-aligned camp
        // (`Army::Neutral`) spawns the shared baseline, byte-identical to before factions. These
        // per-army stats fold into the checksum, so two peers that picked different armies diverge
        // here and the desync is caught (invariant #7).
        let (health, weapon) = unit_stats_for(armies[faction.index()], unit_kind);
        let e = world.spawn();
        let ei = e.index as usize;
        world.kind[ei] = EntityKind::Unit;
        world.unit_kind[ei] = unit_kind;
        world.faction[ei] = faction;
        world.pos[ei] = pos;
        world.health[ei] = health;
        world.weapon[ei] = weapon;
        // Directional armour (tank embodiment P9): a produced Tank enters armoured; every other
        // archetype draws the unarmoured default, so non-tank production is byte-identical to before
        // and the checksum only moves once an armoured tank is actually on the field (invariant #7).
        world.armor[ei] = unit_armor(unit_kind);
        world.order[ei] = Order::Idle;
        // A produced unit enters the match on FireAtWill (the engagement default) so it actually
        // fights — it engages any enemy in weapon range + LoS (invariant #3: firing in place, never
        // autonomous movement). ReturnFire here would deadlock: a fresh reinforcement only shoots
        // back once hit, so two opposing fresh units would stand and stare until something else fired.
        world.stance[ei] = Stance::FireAtWill;
        events.push(SimEvent::UnitProduced { faction, pos });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::territory::ControlPoint;

    /// The non-aligned per-side army map — every faction fields the shared baseline roster, so a
    /// production run through `economy_system` spawns byte-identical pre-factions units. The
    /// per-army roster draw is covered by the dedicated WS-B tests below.
    const NEUTRAL_ARMIES: [Army; FACTION_COUNT] = [Army::Neutral; FACTION_COUNT];

    fn empty_terr() -> Territory {
        Territory::empty()
    }

    fn tick(world: &mut World, res: &mut Resources, terr: &Territory) -> Vec<SimEvent> {
        let mut events = Vec::new();
        let mut rng = Rng::new(1);
        // Full income rate (tick 0, period 1 ⇒ accrue every call), the pre-lever behaviour these
        // tests were written against. The income-period gate is covered separately.
        economy_system(world, res, terr, &mut events, &mut rng, 0, 1, &NEUTRAL_ARMIES);
        events
    }

    fn alive_units(world: &World, faction: Faction) -> usize {
        let mut n = 0;
        for i in 0..world.capacity() {
            if world.is_index_alive(i)
                && world.kind[i] == EntityKind::Unit
                && world.faction[i] == faction
            {
                n += 1;
            }
        }
        n
    }

    #[test]
    fn try_spend_rejects_when_poor_and_debits_when_affordable() {
        let mut res = Resources::new(40);
        assert!(!res.try_spend(Faction::Player, 50));
        assert_eq!(
            res.get(Faction::Player),
            40,
            "rejected spend must not debit"
        );
        assert!(res.try_spend(Faction::Player, 30));
        assert_eq!(res.get(Faction::Player), 10);
        // Exact-balance spend succeeds.
        assert!(res.try_spend(Faction::Player, 10));
        assert_eq!(res.get(Faction::Player), 0);
    }

    #[test]
    fn build_creates_under_construction_building_and_debits() {
        let mut world = World::new();
        let mut res = Resources::new(CAMP_BUILD_COST);
        let e = build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::ZERO,
        )
        .expect("affordable build should return Some");
        let i = e.index as usize;
        assert_eq!(res.get(Faction::Player), 0, "build must debit cost");
        assert_eq!(world.kind[i], EntityKind::Building);
        assert_eq!(world.faction[i], Faction::Player);
        assert_eq!(world.building[i].build_ticks_left, CAMP_BUILD_TICKS);
        assert_eq!(world.building[i].level, 0);
        assert!(world.building[i].queue.is_empty());
        assert_eq!(world.health[i], Health::full(Fixed::from_int(CAMP_HP)));
    }

    #[test]
    fn build_too_poor_returns_none_and_does_not_debit() {
        let mut world = World::new();
        let mut res = Resources::new(CAMP_BUILD_COST - 1);
        let r = build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::ZERO,
        );
        assert!(r.is_none());
        assert_eq!(res.get(Faction::Player), CAMP_BUILD_COST - 1);
        assert_eq!(world.capacity(), 0, "no entity should have spawned");
    }

    #[test]
    fn economy_system_ticks_construction_to_built() {
        let mut world = World::new();
        let mut res = Resources::new(CAMP_BUILD_COST);
        let e = build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::ZERO,
        )
        .unwrap();
        let i = e.index as usize;
        let terr = empty_terr();
        for _ in 0..CAMP_BUILD_TICKS {
            assert!(world.building[i].build_ticks_left > 0);
            tick(&mut world, &mut res, &terr);
        }
        assert_eq!(
            world.building[i].build_ticks_left, 0,
            "camp should be built after CAMP_BUILD_TICKS ticks"
        );
    }

    #[test]
    fn queue_production_then_run_spawns_one_unit_and_debits() {
        let mut world = World::new();
        let mut res = Resources::new(CAMP_BUILD_COST + RIFLEMAN_COST);
        let camp = build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::ZERO,
        )
        .unwrap();
        let terr = empty_terr();

        // Finish construction (income would distort balances, so use empty territory and
        // measure against the income we know we accrue).
        for _ in 0..CAMP_BUILD_TICKS {
            tick(&mut world, &mut res, &terr);
        }
        let before = res.get(Faction::Player);
        assert!(queue_production(
            &mut world,
            &mut res,
            camp,
            UnitKind::Rifleman
        ));
        assert_eq!(
            res.get(Faction::Player),
            before - RIFLEMAN_COST,
            "queueing must debit the unit cost"
        );

        assert_eq!(alive_units(&world, Faction::Player), 0);
        let ptime = prod_time(UnitKind::Rifleman, 0);
        let mut produced_events = 0;
        for _ in 0..ptime {
            let evs = tick(&mut world, &mut res, &terr);
            produced_events += evs
                .iter()
                .filter(|e| matches!(e, SimEvent::UnitProduced { .. }))
                .count();
        }
        assert_eq!(alive_units(&world, Faction::Player), 1, "exactly one unit");
        assert_eq!(produced_events, 1, "exactly one UnitProduced event");

        // Verify the spawned unit's stats.
        let mut found = false;
        for i in 0..world.capacity() {
            if world.is_index_alive(i) && world.kind[i] == EntityKind::Unit {
                let (h, w) = unit_stats(UnitKind::Rifleman);
                assert_eq!(world.faction[i], Faction::Player);
                assert_eq!(world.health[i], h);
                assert_eq!(world.weapon[i], w);
                assert_eq!(world.stance[i], Stance::FireAtWill);
                assert_eq!(world.order[i], Order::Idle);
                found = true;
            }
        }
        assert!(found);
    }

    #[test]
    fn production_spawns_unit_with_its_queued_kind() {
        // The load-bearing render-metadata seam: a Heavy queued through production must spawn
        // carrying `UnitKind::Heavy`, a Rifleman `UnitKind::Rifleman`. Set deterministically from
        // the queue item, so it is identical on every peer (it is NOT in the checksum).
        let mut world = World::new();
        let mut res = Resources::new(CAMP_BUILD_COST + RIFLEMAN_COST + HEAVY_COST);
        let camp = build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::ZERO,
        )
        .unwrap();
        let terr = empty_terr();
        for _ in 0..CAMP_BUILD_TICKS {
            tick(&mut world, &mut res, &terr);
        }

        // Produce a Rifleman, then verify the single spawned unit carries Rifleman.
        assert!(queue_production(&mut world, &mut res, camp, UnitKind::Rifleman));
        for _ in 0..prod_time(UnitKind::Rifleman, 0) {
            tick(&mut world, &mut res, &terr);
        }
        let rifle_idx = (0..world.capacity())
            .find(|&i| world.is_index_alive(i) && world.kind[i] == EntityKind::Unit)
            .expect("a rifleman should have spawned");
        assert_eq!(world.unit_kind[rifle_idx], UnitKind::Rifleman);

        // Produce a Heavy, then verify the new unit carries Heavy (and the rifleman is untouched).
        assert!(queue_production(&mut world, &mut res, camp, UnitKind::Heavy));
        for _ in 0..prod_time(UnitKind::Heavy, 0) {
            tick(&mut world, &mut res, &terr);
        }
        let heavy_idx = (0..world.capacity())
            .find(|&i| {
                world.is_index_alive(i)
                    && world.kind[i] == EntityKind::Unit
                    && i != rifle_idx
            })
            .expect("a heavy should have spawned");
        assert_eq!(world.unit_kind[heavy_idx], UnitKind::Heavy);
        assert_eq!(
            world.unit_kind[rifle_idx],
            UnitKind::Rifleman,
            "spawning the heavy must not disturb the rifleman's kind"
        );
    }

    #[test]
    fn upgrade_raises_level_and_debits() {
        let mut world = World::new();
        let mut res = Resources::new(CAMP_BUILD_COST + upgrade_cost(0));
        let camp = build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::ZERO,
        )
        .unwrap();
        let i = camp.index as usize;

        // Unbuilt camp can't upgrade.
        assert!(!upgrade(&mut world, &mut res, camp));

        let terr = empty_terr();
        for _ in 0..CAMP_BUILD_TICKS {
            tick(&mut world, &mut res, &terr);
        }
        // Drain income so we can test the too-poor path precisely: spend down to exactly
        // upgrade_cost(0).
        let surplus = res.get(Faction::Player) - upgrade_cost(0);
        assert!(surplus >= 0);
        res.try_spend(Faction::Player, surplus);
        assert_eq!(res.get(Faction::Player), upgrade_cost(0));

        assert!(upgrade(&mut world, &mut res, camp));
        assert_eq!(world.building[i].level, 1);
        assert_eq!(res.get(Faction::Player), 0);

        // Now too poor for the next (more expensive) upgrade.
        assert!(!upgrade(&mut world, &mut res, camp));
        assert_eq!(world.building[i].level, 1);
    }

    #[test]
    fn upgrade_fails_on_dead_or_non_building() {
        let mut world = World::new();
        let mut res = Resources::new(10_000);
        // A plain unit entity is not a building.
        let u = world.spawn();
        let ui = u.index as usize;
        world.kind[ui] = EntityKind::Unit;
        assert!(!upgrade(&mut world, &mut res, u));

        // A despawned/stale handle.
        let camp = build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::ZERO,
        )
        .unwrap();
        world.despawn(camp);
        assert!(!upgrade(&mut world, &mut res, camp));
    }

    #[test]
    fn queue_production_fails_when_unbuilt_or_poor() {
        let mut world = World::new();
        let mut res = Resources::new(CAMP_BUILD_COST);
        let camp = build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::ZERO,
        )
        .unwrap();
        // Unbuilt: cannot queue.
        assert!(!queue_production(
            &mut world,
            &mut res,
            camp,
            UnitKind::Rifleman
        ));
        let terr = empty_terr();
        for _ in 0..CAMP_BUILD_TICKS {
            tick(&mut world, &mut res, &terr);
        }
        // Built but drain resources to 0 → too poor.
        let bal = res.get(Faction::Player);
        res.try_spend(Faction::Player, bal);
        assert_eq!(res.get(Faction::Player), 0);
        assert!(!queue_production(
            &mut world,
            &mut res,
            camp,
            UnitKind::Heavy
        ));
        assert!(world.building[camp.index as usize].queue.is_empty());
    }

    /// The income-period gate: with `income_period = N` income accrues once every `N` ticks (on
    /// `tick % N == 0`) at the unchanged per-accrual amount, so the effective drip is `1/N` the full
    /// rate. This is the scenario-local pace lever; the D30 constants are untouched.
    #[test]
    fn income_accrues_only_on_period_boundaries() {
        let mut world = World::new();
        let mut res = Resources::new(0);
        // One held point so each accrual is BASE_INCOME + PER_POINT_INCOME (a non-trivial amount).
        let terr = Territory {
            points: vec![ControlPoint {
                pos: Vec2::ZERO,
                owner: Faction::Player,
                progress: Fixed::ZERO,
            }],
        };
        let per_accrual = BASE_INCOME + PER_POINT_INCOME;
        let period: u32 = 18;
        let mut rng = Rng::new(1);

        // Drive ticks 0..(3*period). Income lands only on ticks 0, period, 2*period → 3 accruals.
        let mut accruals = 0i64;
        for t in 0..(3 * period as u64) {
            let before = res.get(Faction::Player);
            let mut events = Vec::new();
            economy_system(&mut world, &mut res, &terr, &mut events, &mut rng, t, period, &NEUTRAL_ARMIES);
            let gained = res.get(Faction::Player) - before;
            if t.is_multiple_of(period as u64) {
                assert_eq!(gained, per_accrual, "tick {t} is a boundary → full accrual");
                accruals += 1;
            } else {
                assert_eq!(gained, 0, "tick {t} is off-boundary → no income");
            }
        }
        assert_eq!(accruals, 3);
        assert_eq!(res.get(Faction::Player), per_accrual * 3);

        // A period of 0 is clamped to 1 (every tick), and never panics on the modulo.
        let mut r2 = Resources::new(0);
        let mut ev = Vec::new();
        economy_system(&mut world, &mut r2, &terr, &mut ev, &mut rng, 7, 0, &NEUTRAL_ARMIES);
        assert_eq!(r2.get(Faction::Player), per_accrual, "period 0 clamps to full rate");
    }

    #[test]
    fn income_grows_with_owned_territory() {
        let mut world = World::new();
        let mut res = Resources::new(0);
        let terr = Territory {
            points: vec![ControlPoint {
                pos: Vec2::ZERO,
                owner: Faction::Player,
                progress: Fixed::ZERO,
            }],
        };
        let n: i64 = 10;
        for _ in 0..n {
            tick(&mut world, &mut res, &terr);
        }
        let expected = (BASE_INCOME + PER_POINT_INCOME) * n;
        assert_eq!(res.get(Faction::Player), expected);
        // Enemy owns nothing → only base income.
        assert_eq!(res.get(Faction::Enemy), BASE_INCOME * n);
        // Neutral never earns.
        assert_eq!(res.get(Faction::Neutral), 0);
    }

    #[test]
    fn higher_level_camp_produces_faster_with_floor() {
        assert!(prod_time(UnitKind::Rifleman, 1) < prod_time(UnitKind::Rifleman, 0));
        // Each level shaves exactly LEVEL_PROD_SPEEDUP off the base.
        assert_eq!(
            prod_time(UnitKind::Rifleman, 1),
            RIFLEMAN_BASE_TICKS - LEVEL_PROD_SPEEDUP
        );
        assert_eq!(
            prod_time(UnitKind::Heavy, 2),
            HEAVY_BASE_TICKS - 2 * LEVEL_PROD_SPEEDUP
        );
        // Floor is respected at a very high (saturated) level.
        assert_eq!(prod_time(UnitKind::Rifleman, 255), PROD_TICKS_FLOOR);
        assert_eq!(prod_time(UnitKind::Heavy, 255), PROD_TICKS_FLOOR);
    }

    /// Anchor the measured baseline (D30) in seconds (60 Hz) so an accidental edit that
    /// breaks the intended "reads in seconds" shape trips a test. These assertions are
    /// expected to move when the numbers are next rebalanced.
    #[test]
    fn balance_baseline_reads_in_seconds() {
        const HZ: u16 = 60;
        // Camp build is the slowest action; units are a handful of seconds.
        assert_eq!(CAMP_BUILD_TICKS, 20 * HZ, "camp construction is 20 s");
        assert_eq!(RIFLEMAN_BASE_TICKS, 5 * HZ, "rifleman is 5 s");
        assert_eq!(HEAVY_BASE_TICKS, 11 * HZ, "heavy is 11 s (D30)");
        // A camp is buildable turn-one from the 500-resource demo purse, with
        // resources to spare. (Bound to locals so the check is on values, not a
        // const expression — clippy flags `assert!` on a constant condition.)
        let (camp_cost, rifle_cost, heavy_cost) = (CAMP_BUILD_COST, RIFLEMAN_COST, HEAVY_COST);
        assert!(camp_cost < 500, "camp affordable at the seed purse");
        // Holding one point ~doubles base income (territory matters).
        assert_eq!(PER_POINT_INCOME, 2 * BASE_INCOME);
        // Heavy is a real investment over the spammable Rifleman (220 vs 100 cost — D30).
        assert!(heavy_cost > rifle_cost, "heavy costs more than a rifleman");
        assert_eq!(heavy_cost, 220, "heavy costs 220 = 11/5 of a rifleman (D30)");
    }

    /// Lock the measured combat stats so a stray edit that re-breaks the tuned
    /// Rifleman/Heavy relationship (TTK band, Heavy-as-bruiser) trips a test. These are
    /// the values the `--metrics` harness was tuned against; expected to move on the next
    /// measured re-tune. D66 scaled per-shot damage ×5 over the D30 baseline for modern
    /// lethality (HP + cooldown + range unchanged), so the *ratios* the harness checks hold.
    #[test]
    fn unit_stats_match_measured_baseline() {
        let (rh, rw) = unit_stats(UnitKind::Rifleman);
        assert_eq!(rh, Health::full(Fixed::from_int(100)), "rifleman 100 HP");
        assert_eq!(rw.range, Fixed::from_int(14), "rifleman range 14");
        assert_eq!(rw.damage, Fixed::from_int(30), "rifleman 30 dmg (D66 lethal: ~4 hits to kill)");
        assert_eq!(rw.cooldown_ticks, 30, "rifleman 30-tick cooldown -> 60 DPS, ~1-2 s 1v1");

        let (hh, hw) = unit_stats(UnitKind::Heavy);
        assert_eq!(hh, Health::full(Fixed::from_int(300)), "heavy 300 HP (3x the 100-HP rifle, rebalance WS-A)");
        assert_eq!(hw.range, Fixed::from_int(11), "heavy range 11 (shorter than rifle -> kiteable)");
        assert_eq!(hw.damage, Fixed::from_int(100), "heavy 100 dmg (100 vs 30 rifle burst, rebalance WS-A)");
        assert_eq!(hw.cooldown_ticks, 48, "heavy 48-tick cooldown -> 100 dmg per 48 ticks");

        // The Heavy is a bruiser, not a strict upgrade: shorter range than the Rifleman is the
        // load-bearing weakness that makes the matchup range-dependent (the old Heavy was
        // strictly dominated). Guard that relationship explicitly.
        assert!(hw.range < rw.range, "heavy must out-range LESS than the rifleman");
        assert!(hh.max > rh.max, "heavy is tankier");
        assert!(hw.damage > rw.damage, "heavy hits harder per shot");

        // Magazines are armed + start full so a freshly possessed unit can fire (embodied-only
        // gate). The bruiser carries the bigger belt and the longer reload.
        assert_eq!(rw.mag_size, 30, "rifleman 30-round mag");
        assert_eq!(rw.ammo, rw.mag_size, "spawns with a full magazine");
        assert_eq!(rw.reload_ticks, 90, "rifleman 90-tick reload");
        assert_eq!(hw.mag_size, 50, "heavy 50-round belt");
        assert_eq!(hw.ammo, hw.mag_size, "spawns with a full magazine");
        assert!(hw.mag_size > rw.mag_size, "heavy sustains fire longer");
        assert!(hw.reload_ticks > rw.reload_ticks, "heavy reload is slower");
        assert_eq!(rw.reload_left, 0, "not reloading at spawn");
        assert_eq!(hw.reload_left, 0, "not reloading at spawn");

        // All-unit ammo loadouts (D67): a full reserve at spawn; the rifle carries six spare mags
        // (~210-round loadout), the bruiser four spare belts. Drawn on reload, rearmed at a camp.
        assert_eq!(rw.reserve, 180, "rifleman carries 6 spare mags");
        assert_eq!(rw.reserve, rw.reserve_max, "spawns with a full reserve loadout");
        assert_eq!(hw.reserve, 200, "heavy carries 4 spare belts");
        assert_eq!(hw.reserve, hw.reserve_max, "spawns with a full reserve loadout");
    }

    // --- New content (D65): Tank, Medic, Barracks ------------------------------------------------

    #[test]
    fn d65_costs_times_and_stats_are_defined() {
        // Tables answer for the new kinds (the exhaustive matches would not compile otherwise, but
        // pin the intended shape: tank = priciest, medic = cheap, barracks = cheaper/faster camp).
        assert_eq!(unit_cost(UnitKind::Tank), TANK_COST);
        assert_eq!(unit_cost(UnitKind::Medic), MEDIC_COST);
        assert_eq!(prod_time(UnitKind::Tank, 0), TANK_BASE_TICKS);
        assert_eq!(prod_time(UnitKind::Medic, 0), MEDIC_BASE_TICKS);
        assert_eq!(build_cost(BuildingKind::Barracks), BARRACKS_BUILD_COST);
        assert_eq!(build_ticks(BuildingKind::Barracks), BARRACKS_BUILD_TICKS);
        assert!(unit_cost(UnitKind::Tank) > unit_cost(UnitKind::Heavy), "tank is the priciest unit");
        assert!(unit_cost(UnitKind::Medic) < unit_cost(UnitKind::Heavy), "medic is cheap");
        assert!(build_cost(BuildingKind::Barracks) < build_cost(BuildingKind::Camp), "barracks cheaper");
        assert!(build_ticks(BuildingKind::Barracks) < build_ticks(BuildingKind::Camp), "barracks faster");

        let (th, tw) = unit_stats(UnitKind::Tank);
        assert!(th.max > unit_stats(UnitKind::Rifleman).0.max, "tank out-HPs a rifleman");
        assert!(tw.damage > Fixed::ZERO && tw.range > Fixed::ZERO, "tank has a gun");
        assert!(tw.turret_speed > 0, "tank has an independent turret slew");
        // P9/D72: the produced tank now carries a real BALLISTIC main gun — a traveling shell with
        // armour penetration, no longer the D65 hitscan stand-in. (Its `Armor` facets are a separate
        // concern; this asserts only the gun.)
        assert!(tw.muzzle_vel > Fixed::ZERO, "produced tank fires a ballistic shell (P9, D72)");
        assert!(tw.penetration > Fixed::ZERO, "the ballistic gun carries armour penetration (P9, D72)");
        // The tank's main gun stows finite shells too (D67) — no more infinite ammo.
        assert!(tw.mag_size > 0, "tank rations its main-gun shells");
        assert_eq!(tw.ammo, tw.mag_size, "tank spawns with a loaded gun");
        assert!(tw.reserve > 0 && tw.reserve == tw.reserve_max, "tank carries a full shell reserve");

        let (mh, mw) = unit_stats(UnitKind::Medic);
        assert!(mh.max > Fixed::ZERO, "medic is alive");
        assert_eq!(mw.range, Fixed::ZERO, "medic has no weapon range → combat never engages it");
        assert_eq!(mw.damage, Fixed::ZERO, "medic deals no damage (it heals, via crate::heal)");
    }

    #[test]
    fn can_produce_routes_units_to_the_right_building() {
        use BuildingKind::{Barracks, Camp};
        use UnitKind::{Heavy, Medic, Rifleman, Tank};
        // Camp (base): infantry + vehicles, but NOT the Medic.
        assert!(can_produce(Camp, Rifleman));
        assert!(can_produce(Camp, Heavy));
        assert!(can_produce(Camp, Tank));
        assert!(!can_produce(Camp, Medic), "the Medic comes only from a Barracks");
        // Barracks: infantry + Medic, but NOT vehicles.
        assert!(can_produce(Barracks, Rifleman));
        assert!(can_produce(Barracks, Medic));
        assert!(!can_produce(Barracks, Tank), "the Barracks cannot build vehicles");
        assert!(!can_produce(Barracks, Heavy));
    }

    #[test]
    fn queue_production_enforces_routing_without_spending_on_a_reject() {
        let mut world = World::new();
        let mut res = Resources::new(100_000);
        let camp = build(&mut world, &mut res, Faction::Player, BuildingKind::Camp, Vec2::ZERO)
            .unwrap();
        let barracks = build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Barracks,
            Vec2::new(Fixed::from_int(8), Fixed::ZERO),
        )
        .unwrap();
        world.building[camp.index as usize].build_ticks_left = 0;
        world.building[barracks.index as usize].build_ticks_left = 0;

        let before = res.get(Faction::Player);
        assert!(
            !queue_production(&mut world, &mut res, camp, UnitKind::Medic),
            "a Camp cannot make a Medic"
        );
        assert!(
            !queue_production(&mut world, &mut res, barracks, UnitKind::Tank),
            "a Barracks cannot make a Tank"
        );
        assert_eq!(res.get(Faction::Player), before, "a rejected queue never spends");

        // The valid routes succeed and spend exactly their cost.
        assert!(queue_production(&mut world, &mut res, camp, UnitKind::Tank));
        assert!(queue_production(&mut world, &mut res, barracks, UnitKind::Medic));
        assert_eq!(res.get(Faction::Player), before - TANK_COST - MEDIC_COST);
    }

    #[test]
    fn barracks_builds_with_its_own_hp_and_produces_a_medic() {
        let mut world = World::new();
        let mut res = Resources::new(BARRACKS_BUILD_COST + MEDIC_COST);
        let bar = build(&mut world, &mut res, Faction::Player, BuildingKind::Barracks, Vec2::ZERO)
            .unwrap();
        let i = bar.index as usize;
        assert_eq!(world.building[i].kind, BuildingKind::Barracks);
        assert_eq!(world.building[i].build_ticks_left, BARRACKS_BUILD_TICKS);
        assert_eq!(world.health[i], Health::full(Fixed::from_int(600)), "barracks HP is its own");

        let terr = empty_terr();
        for _ in 0..BARRACKS_BUILD_TICKS {
            tick(&mut world, &mut res, &terr);
        }
        assert_eq!(world.building[i].build_ticks_left, 0, "barracks finished constructing");
        assert!(queue_production(&mut world, &mut res, bar, UnitKind::Medic));
        for _ in 0..prod_time(UnitKind::Medic, 0) {
            tick(&mut world, &mut res, &terr);
        }
        let medic = (0..world.capacity()).find(|&j| {
            world.is_index_alive(j)
                && world.kind[j] == EntityKind::Unit
                && world.unit_kind[j] == UnitKind::Medic
        });
        assert!(medic.is_some(), "the Barracks produced a Medic into the world");
    }

    // --- factions WS-B: per-faction rosters --------------------------------------------------------

    /// Run one economy tick with an explicit per-side army map (production draws the producing
    /// faction's roster). Float-free, like [`tick`].
    fn tick_armies(
        world: &mut World,
        res: &mut Resources,
        terr: &Territory,
        armies: &[Army; FACTION_COUNT],
    ) -> Vec<SimEvent> {
        let mut events = Vec::new();
        let mut rng = Rng::new(1);
        economy_system(world, res, terr, &mut events, &mut rng, 0, 1, armies);
        events
    }

    /// The number of full magazines a baseline loadout carries in reserve (`reserve / mag_size`).
    fn mag_count(kind: UnitKind) -> u16 {
        let w = unit_stats(kind).1;
        w.reserve / w.mag_size
    }

    /// The [`Army::Neutral`] roster is byte-for-byte the shared [`unit_stats`] baseline, for EVERY
    /// archetype — so a legacy / non-aligned scene spawns exactly the pre-factions unit (its golden
    /// checksum is unmoved).
    #[test]
    fn neutral_army_roster_is_the_shared_baseline() {
        for kind in [UnitKind::Rifleman, UnitKind::Heavy, UnitKind::Tank, UnitKind::Medic] {
            assert_eq!(
                unit_stats_for(Army::Neutral, kind),
                unit_stats(kind),
                "{kind:?}: Neutral must equal the shared baseline"
            );
        }
    }

    /// The US and FR rosters are genuinely DISTINCT (a real roster, not a reskin) but the asymmetry
    /// is **feel, never power**: the snowball-sensitive combat axes — per-shot damage, cadence, range,
    /// and HP — are SHARED; only the logistics axis (magazine / reload / reserve / turret slew) tilts.
    /// This is the per-stat fairness bound the `--metrics` swap-invariance proves at the trade level.
    #[test]
    fn us_and_fr_differ_only_on_the_logistics_axis() {
        for kind in [UnitKind::Rifleman, UnitKind::Heavy, UnitKind::Tank] {
            let (uh, uw) = unit_stats_for(Army::Us, kind);
            let (fh, fw) = unit_stats_for(Army::Fr, kind);
            assert_ne!(uw, fw, "{kind:?}: US and FR loadouts must differ");
            // Shared combat-power axes (the fairness bound).
            assert_eq!(uh, fh, "{kind:?}: HP is shared");
            assert_eq!(uw.damage, fw.damage, "{kind:?}: per-shot damage is shared");
            assert_eq!(uw.cooldown_ticks, fw.cooldown_ticks, "{kind:?}: cadence is shared");
            assert_eq!(uw.range, fw.range, "{kind:?}: range is shared");
            assert_eq!(uw.penetration, fw.penetration, "{kind:?}: penetration is shared");
        }
        // The Medic is non-combatant: shared across every army (no fair combat surface to tilt).
        for army in [Army::Neutral, Army::Us, Army::Fr] {
            assert_eq!(unit_stats_for(army, UnitKind::Medic), unit_stats(UnitKind::Medic));
        }
    }

    /// The infantry logistics tilt is **power-neutral by construction**: US carries the deeper
    /// magazine / longer reload (sustained-fire doctrine), FR the shorter / snappier one, but both
    /// keep the SAME number of reserve magazines AND the same reload/magazine ratio — so sustained DPS
    /// and reload-depth are invariant (the magazine only changes the reload *rhythm*, never the rate).
    #[test]
    fn infantry_logistics_tilt_is_dps_and_depth_neutral() {
        for kind in [UnitKind::Rifleman, UnitKind::Heavy] {
            let base = unit_stats(kind).1;
            let us = unit_stats_for(Army::Us, kind).1;
            let fr = unit_stats_for(Army::Fr, kind).1;
            // Doctrine direction: US deeper magazine, FR shallower.
            assert!(us.mag_size > base.mag_size && base.mag_size > fr.mag_size, "{kind:?}: US deep, FR shallow magazine");
            assert!(us.reload_ticks > base.reload_ticks && base.reload_ticks > fr.reload_ticks, "{kind:?}: reload tracks magazine depth");
            // A full magazine at spawn (mirrors `unit_stats`), and the SAME reserve mag-count for both.
            for w in [us, fr] {
                assert_eq!(w.ammo, w.mag_size, "{kind:?}: spawns with a full magazine");
                assert_eq!(w.reserve, w.reserve_max, "{kind:?}: spawns with a full reserve");
                assert_eq!(w.reserve / w.mag_size, mag_count(kind), "{kind:?}: same reserve mag-count → same depth");
            }
            // Reload/magazine ratio preserved (sustained DPS invariant) to within the integer
            // rounding of the reload tick (≤1 tick: e.g. the Heavy's 138/50 ratio at mag 60 rounds
            // 165.6→166). Cross-multiplied so the check is pure integer (invariant #1).
            for w in [us, fr] {
                let lhs = (w.reload_ticks as i64) * (base.mag_size as i64);
                let rhs = (base.reload_ticks as i64) * (w.mag_size as i64);
                assert!((lhs - rhs).abs() <= (w.mag_size as i64), "{kind:?}: reload/mag ratio held (sustained DPS invariant)");
            }
        }
    }

    /// The tank tilt is **turret-slew only** — cosmetic per invariant #3 (the slew never picks
    /// targets), so its gun, magazine, reload, and reserve stay the shared baseline. (The shallow
    /// 6-shell magazine makes any logistics tilt on the tank unfair under reload pressure — see the
    /// module note and the `--metrics` reload-pressure test — so the identity lives on the turret.)
    #[test]
    fn tank_tilt_is_cosmetic_turret_only() {
        let base = unit_stats(UnitKind::Tank).1;
        let us = unit_stats_for(Army::Us, UnitKind::Tank).1;
        let fr = unit_stats_for(Army::Fr, UnitKind::Tank).1;
        for w in [us, fr] {
            assert_eq!(w.mag_size, base.mag_size, "tank magazine is shared");
            assert_eq!(w.reload_ticks, base.reload_ticks, "tank reload is shared");
            assert_eq!(w.reserve, base.reserve, "tank reserve is shared");
            assert_eq!(w.damage, base.damage, "tank gun is shared");
        }
        // Only the (cosmetic) turret slew differs: US slower manual-loader, FR quicker autoloader.
        assert!(us.turret_speed < fr.turret_speed, "US turret slews slower than FR (cosmetic identity)");
        assert_ne!(us.turret_speed, base.turret_speed);
    }

    /// PRODUCTION draws the **producing faction's army roster** (the canonical "every peer spawns the
    /// bit-identical unit from a fixed table" seam): a US camp fields the US Rifleman variant, an FR
    /// camp the FR one, a non-aligned camp the baseline — and the spawned `Weapon` (which folds into
    /// the checksum) reflects it. This is what makes two peers with mismatched armies diverge in a way
    /// the per-tick checksum catches (invariant #7).
    #[test]
    fn production_spawns_the_producing_armys_roster() {
        let produce_for = |army: Army| -> Weapon {
            let mut world = World::new();
            let mut res = Resources::new(CAMP_BUILD_COST + RIFLEMAN_COST);
            let camp = build(&mut world, &mut res, Faction::Player, BuildingKind::Camp, Vec2::ZERO).unwrap();
            let terr = empty_terr();
            let armies = {
                let mut a = [Army::Neutral; FACTION_COUNT];
                a[Faction::Player.index()] = army;
                a
            };
            for _ in 0..CAMP_BUILD_TICKS {
                tick_armies(&mut world, &mut res, &terr, &armies);
            }
            assert!(queue_production(&mut world, &mut res, camp, UnitKind::Rifleman));
            for _ in 0..prod_time(UnitKind::Rifleman, 0) {
                tick_armies(&mut world, &mut res, &terr, &armies);
            }
            let u = (0..world.capacity())
                .find(|&i| world.is_index_alive(i) && world.kind[i] == EntityKind::Unit)
                .expect("a rifleman spawned");
            world.weapon[u]
        };
        assert_eq!(produce_for(Army::Us), unit_stats_for(Army::Us, UnitKind::Rifleman).1, "US camp fields the US variant");
        assert_eq!(produce_for(Army::Fr), unit_stats_for(Army::Fr, UnitKind::Rifleman).1, "FR camp fields the FR variant");
        assert_eq!(produce_for(Army::Neutral), unit_stats(UnitKind::Rifleman).1, "non-aligned camp fields the baseline");
        // The point of the roster: the US and FR produced units genuinely differ.
        assert_ne!(produce_for(Army::Us), produce_for(Army::Fr), "the two armies produce distinct units");
    }

    // =======================================================================
    // PRODUCED-TANK ARMOUR (tank embodiment P9 — the armour half). The produced `UnitKind::Tank`
    // is now an armoured vehicle; every other archetype stays unarmoured so today's balance is
    // byte-identical. Facet behaviour is pinned against the shared
    // `combat::facing_penetration_multiplier` resolver (invariant #7 safety).
    // =======================================================================

    /// The golden no-regression property: every NON-tank archetype draws the all-zero
    /// [`Armor::default()`], so `facing_penetration_multiplier` returns exactly `1.0` for it and
    /// the entire pre-P9 combat/economy balance is untouched.
    #[test]
    fn non_tank_archetypes_stay_unarmoured() {
        for kind in [UnitKind::Rifleman, UnitKind::Heavy, UnitKind::Medic] {
            assert!(
                unit_armor(kind).is_unarmored(),
                "{kind:?} must carry no armour — preserves today's damage exactly",
            );
            assert_eq!(unit_armor(kind), Armor::default(), "{kind:?} is the unarmoured default");
        }
    }

    /// The produced Tank carries a real, well-ordered tank armour block: thick front, thinner side,
    /// thinnest rear, all strictly positive. *Angle the hull at the enemy; flank to kill.*
    #[test]
    fn produced_tank_carries_ordered_directional_armour() {
        let a = unit_armor(UnitKind::Tank);
        assert!(!a.is_unarmored(), "the produced tank is armoured");
        assert!(a.front > a.side, "front is the thickest facet");
        assert!(a.side > a.rear, "side is thicker than rear");
        assert!(a.rear > Fixed::ZERO, "every facet is real armour (positive)");
        assert_eq!(a.front, TANK_ARMOR_FRONT);
        assert_eq!(a.side, TANK_ARMOR_SIDE);
        assert_eq!(a.rear, TANK_ARMOR_REAR);
    }

    /// The production seam itself: a Tank pushed through `economy_system` spawns wearing its armour,
    /// while a Rifleman produced the same way stays unarmoured (the no-regression guarantee, proven
    /// at the real spawn site, not just the table).
    #[test]
    fn production_assigns_tank_armour_and_leaves_infantry_unarmoured() {
        let mut world = World::new();
        let mut res = Resources::new(CAMP_BUILD_COST + TANK_COST + RIFLEMAN_COST);
        let camp = build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::ZERO,
        )
        .unwrap();
        let terr = empty_terr();
        for _ in 0..CAMP_BUILD_TICKS {
            tick(&mut world, &mut res, &terr);
        }

        // Produce a Tank → it spawns armoured.
        assert!(queue_production(&mut world, &mut res, camp, UnitKind::Tank));
        for _ in 0..prod_time(UnitKind::Tank, 0) {
            tick(&mut world, &mut res, &terr);
        }
        let tank_idx = (0..world.capacity())
            .find(|&i| world.is_index_alive(i) && world.kind[i] == EntityKind::Unit)
            .expect("a tank should have spawned");
        assert_eq!(world.unit_kind[tank_idx], UnitKind::Tank);
        assert_eq!(
            world.armor[tank_idx],
            unit_armor(UnitKind::Tank),
            "the produced tank wears the tank armour block",
        );
        assert!(!world.armor[tank_idx].is_unarmored());

        // Produce a Rifleman → it spawns unarmoured (no regression at the spawn site).
        assert!(queue_production(&mut world, &mut res, camp, UnitKind::Rifleman));
        for _ in 0..prod_time(UnitKind::Rifleman, 0) {
            tick(&mut world, &mut res, &terr);
        }
        let rifle_idx = (0..world.capacity())
            .find(|&i| {
                world.is_index_alive(i)
                    && world.kind[i] == EntityKind::Unit
                    && i != tank_idx
            })
            .expect("a rifleman should have spawned");
        assert!(
            world.armor[rifle_idx].is_unarmored(),
            "a produced rifleman carries no armour",
        );
    }

    /// The load-bearing hitbox property on the PRODUCED tank's armour: a head-on shot bounces, a
    /// flank/rear shot pens — pinned against the shared resolver. A duel-class penetration (18)
    /// bounces the front (`2·18 = 36 < 40`) but pens the side (`18 ≥ 16`) and rear (`18 ≥ 8`).
    #[test]
    fn produced_tank_front_bounces_while_flank_and_rear_penetrate() {
        use crate::combat::{facing_penetration_multiplier, shot_facet, Facet};
        use crate::trig::{Angle, ANGLE_FULL};

        let armor = unit_armor(UnitKind::Tank);
        let hull = Angle(ANGLE_FULL / 2); // tank faces −X → front meets a +X shot
        let pen = Fixed::from_int(18); // a duel-class penetrating gun (the tank's own gun, W4)

        let plus_x = Vec2::new(Fixed::ONE, Fixed::ZERO);
        let from_flank = Vec2::new(Fixed::ZERO, Fixed::ONE);
        let from_rear = Vec2::new(-Fixed::ONE, Fixed::ZERO);

        // Front: the thick facet overmatches the shot → hard bounce (0×).
        assert_eq!(shot_facet(plus_x, hull), Facet::Front);
        assert_eq!(
            facing_penetration_multiplier(plus_x, hull, pen, armor),
            Fixed::ZERO,
            "a head-on shot bounces off the produced tank's frontal armour",
        );
        // Side: the gun out-pens the thinner flank → full damage (1×).
        assert_eq!(shot_facet(from_flank, hull), Facet::Side);
        assert_eq!(
            facing_penetration_multiplier(from_flank, hull, pen, armor),
            Fixed::ONE,
            "a flank shot pens the produced tank's side",
        );
        // Rear: thinnest facet → full damage (1×).
        assert_eq!(shot_facet(from_rear, hull), Facet::Rear);
        assert_eq!(
            facing_penetration_multiplier(from_rear, hull, pen, armor),
            Fixed::ONE,
            "a rear shot pens the produced tank's tail",
        );
    }

    /// Facet selection is correct AT the front/side and side/rear arc boundaries (the off-by-one
    /// trap the P4 facet math guards). The shared `shot_facet` arc is `FACET_ARC` wide; here we only
    /// assert that shots clearly inside each arc bucket onto the expected facet — and that an
    /// infantry-class **zero-penetration** shot bounces on EVERY facet (the intended consequence:
    /// a produced tank shrugs off all small arms; only a penetrating gun kills it).
    #[test]
    fn zero_penetration_bounces_every_facet_on_the_produced_tank() {
        use crate::combat::facing_penetration_multiplier;
        use crate::trig::Angle;

        let armor = unit_armor(UnitKind::Tank);
        let hull = Angle(0); // faces +X
        let zero_pen = Fixed::ZERO; // every infantry weapon (Rifleman/Heavy)

        // Probe shots from the four cardinals: whatever facet each lands on, a zero-pen shot must
        // bounce (2·0 = 0 ≤ a for every positive facet) — infantry cannot scratch the tank.
        for dir in [
            Vec2::new(Fixed::ONE, Fixed::ZERO),
            Vec2::new(-Fixed::ONE, Fixed::ZERO),
            Vec2::new(Fixed::ZERO, Fixed::ONE),
            Vec2::new(Fixed::ZERO, -Fixed::ONE),
        ] {
            assert_eq!(
                facing_penetration_multiplier(dir, hull, zero_pen, armor),
                Fixed::ZERO,
                "a zero-penetration (infantry) shot bounces off every facet of the produced tank",
            );
        }
    }
}
