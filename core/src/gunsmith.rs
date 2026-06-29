//! Gunsmith loadout — fixed-point weapon attachment deltas (WS-C, D60 / `customization.md` §1).
//!
//! This is the **one** customization surface that crosses into the simulation, so it gets the
//! full determinism treatment (invariants #1/#7). An attachment is a **sidegrade**: it spends one
//! tracked weapon stat to buy another, never a flat upgrade. A player picks a *shape*
//! (long-range marksman vs. close-quarters runner), not a *tier* — the same anti-degeneracy
//! discipline the D30 balance harness enforces on units ([`economy::unit_stats`] — a strictly
//! dominated Heavy was a *bug*).
//!
//! ## How it reaches the sim
//!
//! A [`Loadout`] is chosen on the command layer **before** the dive (the `engine::loadout_ui`
//! seam) and applied to the unit's [`Weapon`] component **at match start** as deterministic
//! match-setup input ([`Loadout::apply_to_weapon`]). The resulting stats live in the weapon
//! component, which `Sim::fold` already hashes field-by-field — so a loadout difference flows
//! into the per-tick checksum automatically and a loadout desync would be caught by the cross-arch
//! matrix (invariant #7) like any other sim divergence. **There is no new fold surface and no new
//! per-tick code path**: the loadout is applied once, then the existing combat/economy systems run
//! on the modified weapon exactly as they do on any other weapon.
//!
//! ## Why "no strictly-dominant build" is provable, not just measured
//!
//! Each attachment **slot** trades within a *unique pair* of tracked stat axes
//! ([`Optic`] = range ↔ fire-rate, [`Barrel`] = damage ↔ reserve, [`Magazine`] = capacity ↔
//! handling), and within a slot every option is a pure trade: one axis strictly better, the paired
//! axis strictly worse. Because the slot pairs are **disjoint**, any two distinct loadouts differ
//! in at least one slot, and that slot contributes one strictly-good and one strictly-bad component
//! that **no other slot can cancel** — so neither loadout is "at least as good on every axis."
//! That is the definition of *no strict domination*. [`tests`] proves it exhaustively over the full
//! build space as well, so a future re-tune that breaks the property trips a test.
//!
//! Fixed-point only (range/damage are [`Fixed`]; the count stats are integer ticks/rounds), no
//! floats anywhere — the determinism guard greps this file and its tests.

use crate::components::{Army, Weapon};
use crate::fixed::Fixed;

/// A **goodness-signed** delta a loadout applies to a weapon across the six tracked stat axes.
///
/// Each field is expressed in the weapon's own native units (`range`/`damage` as [`Fixed`], the
/// rest as integer counts). The *polarity* per axis — whether "more" is better — is baked into
/// [`StatDelta::strictly_dominates`]: `range`, `damage`, `mag_size`, and `reserve` are
/// better-when-higher; `cooldown_ticks` and `reload_ticks` are better-when-**lower** (a smaller
/// cooldown is a faster gun; a smaller reload is snappier handling). No floats (invariant #1).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct StatDelta {
    /// Engagement range, in world units. Higher is better.
    pub range: Fixed,
    /// Damage per shot. Higher is better.
    pub damage: Fixed,
    /// Ticks between shots. **Lower** is better (a faster rate of fire).
    pub cooldown_ticks: i32,
    /// Magazine capacity, in rounds. Higher is better.
    pub mag_size: i32,
    /// Ticks a reload takes. **Lower** is better (snappier handling).
    pub reload_ticks: i32,
    /// Carried reserve rounds. Higher is better.
    pub reserve: i32,
}

impl StatDelta {
    /// The no-op delta (the `Standard` option in every slot).
    pub const ZERO: StatDelta = StatDelta {
        range: Fixed::ZERO,
        damage: Fixed::ZERO,
        cooldown_ticks: 0,
        mag_size: 0,
        reload_ticks: 0,
        reserve: 0,
    };

    /// Sum two deltas axis-by-axis. Integer axes saturate (they never realistically come near the
    /// `i32` bound, but saturating keeps it total and float-free); the [`Fixed`] axes use
    /// `Fixed`'s wrapping add, same as the rest of the sim.
    #[inline]
    #[allow(clippy::should_implement_trait)] // intentionally inherent: saturating/wrapping, not std Add semantics
    pub fn add(self, o: StatDelta) -> StatDelta {
        StatDelta {
            range: self.range.wrapping_add(o.range),
            damage: self.damage.wrapping_add(o.damage),
            cooldown_ticks: self.cooldown_ticks.saturating_add(o.cooldown_ticks),
            mag_size: self.mag_size.saturating_add(o.mag_size),
            reload_ticks: self.reload_ticks.saturating_add(o.reload_ticks),
            reserve: self.reserve.saturating_add(o.reserve),
        }
    }

    /// Is `self` **no worse than** `other` on every tracked axis (polarity-aware)?
    #[inline]
    fn no_axis_worse(&self, other: &StatDelta) -> bool {
        self.range >= other.range
            && self.damage >= other.damage
            && self.cooldown_ticks <= other.cooldown_ticks // lower = better
            && self.mag_size >= other.mag_size
            && self.reload_ticks <= other.reload_ticks // lower = better
            && self.reserve >= other.reserve
    }

    /// Is `self` **strictly better** than `other` on at least one tracked axis (polarity-aware)?
    #[inline]
    fn some_axis_better(&self, other: &StatDelta) -> bool {
        self.range > other.range
            || self.damage > other.damage
            || self.cooldown_ticks < other.cooldown_ticks // lower = better
            || self.mag_size > other.mag_size
            || self.reload_ticks < other.reload_ticks // lower = better
            || self.reserve > other.reserve
    }

    /// Does `self` **strictly dominate** `other` — at least as good on every tracked axis and
    /// strictly better on at least one? This is the relation the sidegrade rule forbids between any
    /// two real loadouts ([`tests::no_loadout_strictly_dominates_another`]).
    #[inline]
    pub fn strictly_dominates(&self, other: &StatDelta) -> bool {
        self.no_axis_worse(other) && self.some_axis_better(other)
    }
}

/// One small whole-unit step on a [`Fixed`] stat axis (range/damage). A const helper so the
/// attachment table stays float-free and readable.
const fn fx(n: i32) -> Fixed {
    Fixed::from_int(n)
}

/// Build a macro-free pair of slot enums would be noisy; instead each slot is its own enum with a
/// `Standard` no-op plus two opposed trades, and a shared shape: `ALL`, `delta`, `label`, and
/// `next`/`prev` for the UI to cycle through. The doc on each enum names its unique axis pair.
macro_rules! slot_enum {
    (
        $(#[$emeta:meta])*
        $name:ident { $standard:ident, $plus:ident => $plusd:expr, $minus:ident => $minusd:expr }
        labels { $sl:expr, $pl:expr, $ml:expr }
    ) => {
        $(#[$emeta])*
        #[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
        pub enum $name {
            /// The neutral, no-trade option (a [`StatDelta::ZERO`]).
            #[default]
            $standard,
            #[doc = "The `+`-axis trade for this slot (see the enum docs for which axes)."]
            $plus,
            #[doc = "The opposed `-`-axis trade for this slot."]
            $minus,
        }

        impl $name {
            /// Every option, in a fixed order (the order the UI cycles through). Iterating this is
            /// deterministic by construction.
            pub const ALL: [$name; 3] = [$name::$standard, $name::$plus, $name::$minus];

            /// This option's [`StatDelta`]. `Standard` is the no-op; the other two are opposed
            /// trades on this slot's unique axis pair.
            #[inline]
            pub const fn delta(self) -> StatDelta {
                match self {
                    $name::$standard => StatDelta::ZERO,
                    $name::$plus => $plusd,
                    $name::$minus => $minusd,
                }
            }

            /// A short human label for the loadout UI.
            #[inline]
            pub const fn label(self) -> &'static str {
                match self {
                    $name::$standard => $sl,
                    $name::$plus => $pl,
                    $name::$minus => $ml,
                }
            }

            /// The next option, wrapping (the UI "cycle forward" on this slot).
            #[inline]
            pub const fn next(self) -> Self {
                match self {
                    $name::$standard => $name::$plus,
                    $name::$plus => $name::$minus,
                    $name::$minus => $name::$standard,
                }
            }

            /// The previous option, wrapping (the UI "cycle back" on this slot).
            #[inline]
            pub const fn prev(self) -> Self {
                match self {
                    $name::$standard => $name::$minus,
                    $name::$plus => $name::$standard,
                    $name::$minus => $name::$plus,
                }
            }
        }
    };
}

/// Build a [`StatDelta`] with all six axes explicit (kept out of FRU `..` so the slot tables are
/// const-evaluable on every toolchain). Order: range, damage, cooldown, mag, reload, reserve.
const fn delta(
    range: Fixed,
    damage: Fixed,
    cooldown_ticks: i32,
    mag_size: i32,
    reload_ticks: i32,
    reserve: i32,
) -> StatDelta {
    StatDelta {
        range,
        damage,
        cooldown_ticks,
        mag_size,
        reload_ticks,
        reserve,
    }
}

slot_enum! {
    /// **Optic** — trades **range ↔ fire-rate** (its unique axis pair). A `Marksman` glass reaches
    /// further but is slower to reacquire (a longer cooldown); a `CloseQuarters` reflex sight gives
    /// up reach for a faster effective rate of fire.
    Optic {
        Standard,
        // Marksman: +range, slower fire (higher cooldown = worse rate).
        Marksman => delta(fx(2), Fixed::ZERO, 5, 0, 0, 0),
        // CloseQuarters: -range, faster fire (lower cooldown = better rate).
        CloseQuarters => delta(fx(-2), Fixed::ZERO, -5, 0, 0, 0)
    }
    labels { "Standard", "Marksman", "Close-Quarters" }
}

slot_enum! {
    /// **Barrel** — trades **damage ↔ reserve** (its unique axis pair). A `Heavy` barrel hits
    /// harder per shot but its heavier ammunition means fewer rounds carried; a `Light` barrel
    /// trades hitting power for a deeper reserve to stay in the fight longer between resupplies.
    Barrel {
        Standard,
        // Heavy: +damage, -reserve.
        Heavy => delta(Fixed::ZERO, fx(6), 0, 0, 0, -60),
        // Light: -damage, +reserve.
        Light => delta(Fixed::ZERO, fx(-6), 0, 0, 0, 60)
    }
    labels { "Standard", "Heavy", "Light" }
}

slot_enum! {
    /// **Magazine** — trades **capacity ↔ handling** (its unique axis pair). An `Extended` mag
    /// holds more rounds before a reload but is slower to swap (longer reload); a `Quickdraw` mag
    /// holds fewer rounds but snaps in faster (shorter reload).
    Magazine {
        Standard,
        // Extended: +capacity, slower reload (worse handling).
        Extended => delta(Fixed::ZERO, Fixed::ZERO, 0, 10, 30, 0),
        // Quickdraw: -capacity, faster reload (better handling).
        Quickdraw => delta(Fixed::ZERO, Fixed::ZERO, 0, -10, -30, 0)
    }
    labels { "Standard", "Extended", "Quickdraw" }
}

// ---- Per-faction gunsmith pools (factions WS-E, layers on D60 / D68 / D71) ----------------------
//
// Each [`Army`] gunsmiths a **different weapon pool**: the same slot vocabulary (Optic / Barrel /
// Magazine, the shape a player picks), but the *magnitude* each non-Standard option trades is
// per-army. This is identity, **not** power — a faction is "content + a table, never a logic fork"
// (invariant #2), so WS-E adds a table and reuses every line of the D60 machinery above
// ([`StatDelta`], [`StatDelta::strictly_dominates`], the disjoint-axis slot structure). It is the
// gunsmith analogue of WS-B's per-army [`unit_stats_for`](crate::economy::unit_stats_for): the
// [`Army::Neutral`] pool reproduces the slot enums' own [`Optic::delta`]/etc. **byte-for-byte**
// (every legacy scene unchanged), while `Us`/`Fr` tilt the trade magnitudes for flavour.
//
// **Why this stays sidegrade-only per pool (D60).** A pool only scales the trade magnitudes; it
// never changes the *structure* that makes the no-strict-domination proof hold. Every option still
// touches **only** its slot's unique axis pair (range↔fire-rate / damage↔reserve / capacity↔
// handling — disjoint), and every non-Standard option is still a pure trade (strictly better on one
// axis of that pair, strictly worse on the other). So the disjoint-slot argument from the module
// docs holds **independently inside each army's pool** — proven exhaustively per army in
// [`tests::no_pool_build_strictly_dominates_another`]. The asymmetry between pools is FLAVOUR
// (pillar 4 / D68 / D71): US gunsmiths in heavier, deeper steps; FR in lighter, snappier ones — a
// rhythm difference, not a stat-line that out-guns the other army (D71).
//
// **Determinism (invariant #1/#7).** Every pool delta is fixed-point ([`Fixed`] range/damage,
// integer counts) — no float. The pool changes *which delta* a loadout contributes, and that delta
// lands in the weapon component via [`Loadout::apply_to_weapon_for`] at match setup exactly as the
// army-blind [`Loadout::apply_to_weapon`] does — so it rides the existing `Sim::fold` weapon hash
// with **no new fold surface**. Two peers that pick the same army + the same loadout fold identical
// checksums ([`tests::same_army_same_loadout_two_peers_stay_bit_identical`]).

/// A per-army gunsmith **pool**: the [`StatDelta`] each non-`Standard` attachment option contributes
/// for this army. `Standard` is always the no-op ([`StatDelta::ZERO`]) and so is not stored. The
/// fields are named for the slot options they back ([`Optic::Marksman`]/[`Optic::CloseQuarters`],
/// [`Barrel::Heavy`]/[`Barrel::Light`], [`Magazine::Extended`]/[`Magazine::Quickdraw`]).
///
/// A pool only varies the *magnitudes* of the D60 trades; it preserves the disjoint-axis sidegrade
/// structure, so the no-strict-domination property holds inside every pool (see the module-level
/// note above and [`tests::no_pool_build_strictly_dominates_another`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GunsmithPool {
    /// [`Optic::Marksman`] — +range ↔ slower fire (longer cooldown).
    pub marksman: StatDelta,
    /// [`Optic::CloseQuarters`] — -range ↔ faster fire (shorter cooldown).
    pub close_quarters: StatDelta,
    /// [`Barrel::Heavy`] — +damage ↔ -reserve.
    pub heavy: StatDelta,
    /// [`Barrel::Light`] — -damage ↔ +reserve.
    pub light: StatDelta,
    /// [`Magazine::Extended`] — +capacity ↔ slower reload.
    pub extended: StatDelta,
    /// [`Magazine::Quickdraw`] — -capacity ↔ faster reload.
    pub quickdraw: StatDelta,
}

/// The gunsmith pool an [`Army`] draws from. The tag → pool mapping is the WS-E content table.
///
/// - [`Army::Neutral`] reproduces the slot enums' own [`Optic::delta`]/[`Barrel::delta`]/
///   [`Magazine::delta`] **exactly**, so a loadout on a Neutral/legacy scene behaves and checksums
///   byte-for-byte as it did before WS-E existed.
/// - [`Army::Us`] gunsmiths in **heavier, deeper** steps (a longer-reach marksman optic, a
///   harder-hitting heavy barrel paid for with a deeper reserve swing, a bigger extended mag) — the
///   US logistics rhythm of WS-B/D71, now on the weapon bench.
/// - [`Army::Fr`] gunsmiths in **lighter, snappier** steps (smaller magnitudes, quicker handling) —
///   the French rhythm. Same slots, same trade *axes*, different feel.
///
/// All three obey the per-pool sidegrade rule by construction (each option a pure trade on its
/// slot's disjoint axis pair); the property is proven exhaustively per army in [`tests`].
pub const fn pool_for(army: Army) -> GunsmithPool {
    match army {
        // The baseline pool IS the slot enums' own deltas — zero behavioural change off-faction.
        Army::Neutral => GunsmithPool {
            marksman: Optic::Marksman.delta(),
            close_quarters: Optic::CloseQuarters.delta(),
            heavy: Barrel::Heavy.delta(),
            light: Barrel::Light.delta(),
            extended: Magazine::Extended.delta(),
            quickdraw: Magazine::Quickdraw.delta(),
        },
        // US — heavier/deeper trades. (range±3, cooldown±7; damage±8, reserve±80; mag±14, reload±40.)
        Army::Us => GunsmithPool {
            marksman: delta(fx(3), Fixed::ZERO, 7, 0, 0, 0),
            close_quarters: delta(fx(-3), Fixed::ZERO, -7, 0, 0, 0),
            heavy: delta(Fixed::ZERO, fx(8), 0, 0, 0, -80),
            light: delta(Fixed::ZERO, fx(-8), 0, 0, 0, 80),
            extended: delta(Fixed::ZERO, Fixed::ZERO, 0, 14, 40, 0),
            quickdraw: delta(Fixed::ZERO, Fixed::ZERO, 0, -14, -40, 0),
        },
        // FR — lighter/snappier trades. (range±2, cooldown±4; damage±5, reserve±40; mag±8, reload±20.)
        Army::Fr => GunsmithPool {
            marksman: delta(fx(2), Fixed::ZERO, 4, 0, 0, 0),
            close_quarters: delta(fx(-2), Fixed::ZERO, -4, 0, 0, 0),
            heavy: delta(Fixed::ZERO, fx(5), 0, 0, 0, -40),
            light: delta(Fixed::ZERO, fx(-5), 0, 0, 0, 40),
            extended: delta(Fixed::ZERO, Fixed::ZERO, 0, 8, 20, 0),
            quickdraw: delta(Fixed::ZERO, Fixed::ZERO, 0, -8, -20, 0),
        },
    }
}

/// The [`StatDelta`] an [`Optic`] selection contributes **within a pool**. `Standard` is the no-op.
#[inline]
const fn optic_delta_in(o: Optic, pool: &GunsmithPool) -> StatDelta {
    match o {
        Optic::Standard => StatDelta::ZERO,
        Optic::Marksman => pool.marksman,
        Optic::CloseQuarters => pool.close_quarters,
    }
}

/// The [`StatDelta`] a [`Barrel`] selection contributes **within a pool**. `Standard` is the no-op.
#[inline]
const fn barrel_delta_in(b: Barrel, pool: &GunsmithPool) -> StatDelta {
    match b {
        Barrel::Standard => StatDelta::ZERO,
        Barrel::Heavy => pool.heavy,
        Barrel::Light => pool.light,
    }
}

/// The [`StatDelta`] a [`Magazine`] selection contributes **within a pool**. `Standard` is the no-op.
#[inline]
const fn magazine_delta_in(m: Magazine, pool: &GunsmithPool) -> StatDelta {
    match m {
        Magazine::Standard => StatDelta::ZERO,
        Magazine::Extended => pool.extended,
        Magazine::Quickdraw => pool.quickdraw,
    }
}

/// Range can never be driven to or below zero by a loadout (that would *disarm* the weapon, which
/// is a different thing — a Medic — not a sidegrade). The floor keeps every applied weapon armed.
const MIN_RANGE: Fixed = Fixed::ONE;
/// Damage likewise floors at a positive value so a `Light` barrel never neuters a gun to zero.
const MIN_DAMAGE: Fixed = Fixed::ONE;

/// A complete pre-match weapon loadout: one option per slot. Chosen on the command layer, applied
/// to the unit's weapon at match start. `Default` is all-`Standard` (the neutral baseline a player
/// with no unlocks fields).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Loadout {
    pub optic: Optic,
    pub barrel: Barrel,
    pub magazine: Magazine,
}

impl Loadout {
    /// The neutral all-`Standard` loadout (identical to [`Loadout::default`]); a named const for
    /// call sites and tests.
    pub const STANDARD: Loadout = Loadout {
        optic: Optic::Standard,
        barrel: Barrel::Standard,
        magazine: Magazine::Standard,
    };

    /// The summed [`StatDelta`] this loadout applies **in the baseline ([`Army::Neutral`]) pool** —
    /// the per-slot deltas added across the three slots. This is the value the no-strict-domination
    /// property is proven on (it is a pure function of the *selection*, independent of any base
    /// weapon, so the fairness guarantee holds regardless of which weapon the loadout is bolted
    /// onto). Equivalent to [`Loadout::total_delta_for(Army::Neutral)`](Loadout::total_delta_for).
    #[inline]
    pub fn total_delta(self) -> StatDelta {
        self.total_delta_for(Army::Neutral)
    }

    /// The summed [`StatDelta`] this loadout applies **in a given army's pool** (factions WS-E) —
    /// the per-slot deltas drawn from [`pool_for`] and added across the three slots. For
    /// [`Army::Neutral`] this is byte-identical to [`Loadout::total_delta`] (the pool reproduces the
    /// slot enums' own deltas); `Us`/`Fr` draw their per-faction magnitudes. Still a pure function
    /// of the *(army, selection)* pair — the per-pool no-strict-domination property is proven on it
    /// in [`tests::no_pool_build_strictly_dominates_another`].
    #[inline]
    pub fn total_delta_for(self, army: Army) -> StatDelta {
        let pool = pool_for(army);
        optic_delta_in(self.optic, &pool)
            .add(barrel_delta_in(self.barrel, &pool))
            .add(magazine_delta_in(self.magazine, &pool))
    }

    /// Apply this loadout to a weapon **at match start** (deterministic match-setup input). The
    /// modified fields (`range`, `damage`, `cooldown_ticks`, `mag_size`, `ammo`, `reload_ticks`,
    /// `reserve`, `reserve_max`) are all already in `Sim::fold`, so the change rides the per-tick
    /// checksum with no new fold surface (invariant #7).
    ///
    /// Guards that keep it well-formed and scoped:
    /// - A **disarmed** weapon (`range <= 0`, e.g. the Medic) carries no loadout and is returned
    ///   untouched — a sidegrade never *arms* a non-combatant.
    /// - The magazine/handling/reserve axes apply **only** to a magazine weapon (`mag_size > 0`);
    ///   a magazine-less weapon (infinite ammo) keeps that property.
    /// - Every field saturates to a sensible floor ([`MIN_RANGE`]/[`MIN_DAMAGE`] for the [`Fixed`]
    ///   axes; `0`/`1` for the counts) so an extreme stack can never produce an invalid weapon.
    ///   For the Rifleman this WS targets, no floor is ever hit (see [`tests`]).
    ///
    /// Ammo bookkeeping: if the weapon was at a **full** magazine going in (the just-spawned case),
    /// it stays full at the new capacity; otherwise the loaded count is only clamped down to the
    /// new capacity. The reserve cap (`reserve_max`) moves with the carried reserve, since the
    /// loadout *is* the full carried complement at match start.
    pub fn apply_to_weapon(self, w: &mut Weapon) {
        self.apply_to_weapon_for(Army::Neutral, w);
    }

    /// Apply this loadout to a weapon **drawing from `army`'s gunsmith pool** (factions WS-E). Same
    /// match-setup contract and guards as [`Loadout::apply_to_weapon`] — the only difference is the
    /// per-slot trade magnitudes come from [`pool_for(army)`](pool_for). For [`Army::Neutral`] it is
    /// identical to [`Loadout::apply_to_weapon`]. The modified weapon fields are already in
    /// `Sim::fold`, so a per-army loadout rides the per-tick checksum with no new fold surface
    /// (invariant #7) — two peers with the same `(army, loadout)` fold identically.
    pub fn apply_to_weapon_for(self, army: Army, w: &mut Weapon) {
        // A disarmed weapon is not a gunsmith target — leave it exactly as it was.
        if w.range <= Fixed::ZERO {
            return;
        }
        let d = self.total_delta_for(army);

        w.range = (w.range.wrapping_add(d.range)).max(MIN_RANGE);
        w.damage = (w.damage.wrapping_add(d.damage)).max(MIN_DAMAGE);
        w.cooldown_ticks = apply_count(w.cooldown_ticks, d.cooldown_ticks, 0);

        // Magazine/handling/reserve only mean anything for a magazine weapon; a magazine-less
        // weapon (mag_size == 0 ⇒ infinite ammo, no reload) stays magazine-less.
        if w.mag_size > 0 {
            let was_full = w.ammo == w.mag_size;
            w.mag_size = apply_count(w.mag_size, d.mag_size, 1);
            w.ammo = if was_full {
                w.mag_size
            } else {
                w.ammo.min(w.mag_size)
            };
            w.reload_ticks = apply_count(w.reload_ticks, d.reload_ticks, 1);
            w.reserve = apply_count(w.reserve, d.reserve, 0);
            // The carried reserve at match start IS the loadout's full complement, so the resupply
            // cap tracks it.
            w.reserve_max = w.reserve;
        }
    }
}

/// Apply an integer goodness/raw delta to a `u16` weapon count, clamped to `[floor, u16::MAX]`.
/// Pure integer arithmetic (invariant #1) — no float, total, saturating at both ends.
#[inline]
fn apply_count(base: u16, delta: i32, floor: u16) -> u16 {
    let v = base as i32 + delta;
    if v < floor as i32 {
        floor
    } else if v > u16::MAX as i32 {
        u16::MAX
    } else {
        v as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::UnitKind;
    use crate::components::{Faction, Health, Stance, Vec2, Weapon};
    use crate::economy::unit_stats;
    use crate::ecs::Entity;
    use crate::sim::Sim;

    /// Every loadout in the full build space (3 slots × 3 options = 27).
    fn all_loadouts() -> Vec<Loadout> {
        let mut v = Vec::new();
        for &optic in &Optic::ALL {
            for &barrel in &Barrel::ALL {
                for &magazine in &Magazine::ALL {
                    v.push(Loadout {
                        optic,
                        barrel,
                        magazine,
                    });
                }
            }
        }
        v
    }

    /// THE fairness invariant (the WS-C / D60 / D30 anti-degeneracy rule): no attachment
    /// combination strictly dominates another on the tracked stat axes. Proven exhaustively over
    /// the entire 27-build space — a future re-tune that accidentally makes one build a flat upgrade
    /// trips this immediately.
    #[test]
    fn no_loadout_strictly_dominates_another() {
        let builds: Vec<StatDelta> = all_loadouts().iter().map(|l| l.total_delta()).collect();
        for (i, a) in builds.iter().enumerate() {
            for (j, b) in builds.iter().enumerate() {
                if i == j {
                    continue;
                }
                assert!(
                    !a.strictly_dominates(b),
                    "build {i} ({a:?}) strictly dominates build {j} ({b:?}) — not a sidegrade",
                );
            }
        }
    }

    /// The structural reason the property above holds: every *non-Standard* option is a genuine
    /// trade — strictly better on one tracked axis AND strictly worse on another — versus the
    /// neutral `Standard` (`StatDelta::ZERO`). A pure upside (or pure downside) option would be a
    /// tier, not a sidegrade.
    #[test]
    fn every_option_is_a_pure_trade() {
        let check = |d: StatDelta, name: &str| {
            if d == StatDelta::ZERO {
                return; // the Standard option is the neutral baseline.
            }
            assert!(
                d.some_axis_better(&StatDelta::ZERO),
                "{name} must improve at least one axis: {d:?}"
            );
            // "Worse somewhere" = ZERO is better than `d` on some axis.
            assert!(
                StatDelta::ZERO.some_axis_better(&d),
                "{name} must cost at least one axis: {d:?}"
            );
        };
        for o in Optic::ALL {
            check(o.delta(), o.label());
        }
        for b in Barrel::ALL {
            check(b.delta(), b.label());
        }
        for m in Magazine::ALL {
            check(m.delta(), m.label());
        }
    }

    /// The slot axis pairs are disjoint — the load-bearing premise of the no-domination proof. Each
    /// slot touches exactly its two named axes and nothing else, and no two slots share an axis.
    #[test]
    fn slot_axis_pairs_are_disjoint() {
        // Optic touches only {range, cooldown_ticks}.
        for o in Optic::ALL {
            let d = o.delta();
            assert_eq!(d.damage, Fixed::ZERO);
            assert_eq!((d.mag_size, d.reload_ticks, d.reserve), (0, 0, 0));
        }
        // Barrel touches only {damage, reserve}.
        for b in Barrel::ALL {
            let d = b.delta();
            assert_eq!(d.range, Fixed::ZERO);
            assert_eq!((d.cooldown_ticks, d.mag_size, d.reload_ticks), (0, 0, 0));
        }
        // Magazine touches only {mag_size, reload_ticks}.
        for m in Magazine::ALL {
            let d = m.delta();
            assert_eq!((d.range, d.damage), (Fixed::ZERO, Fixed::ZERO));
            assert_eq!((d.cooldown_ticks, d.reserve), (0, 0));
        }
    }

    /// `next`/`prev` cycle through all three options and are inverses — the UI cycling contract.
    #[test]
    fn options_cycle_through_all_and_invert() {
        assert_eq!(Optic::Standard.next(), Optic::Marksman);
        assert_eq!(Optic::Marksman.next(), Optic::CloseQuarters);
        assert_eq!(Optic::CloseQuarters.next(), Optic::Standard);
        for o in Optic::ALL {
            assert_eq!(o.next().prev(), o, "next then prev is identity");
            assert_eq!(o.prev().next(), o);
        }
        for b in Barrel::ALL {
            assert_eq!(b.next().prev(), b);
        }
        for m in Magazine::ALL {
            assert_eq!(m.next().prev(), m);
        }
    }

    /// Applying a loadout to the real Rifleman weapon moves exactly the intended fields, by the
    /// intended amounts, and never trips a floor (the Rifleman is the in-scope loadout-bearing
    /// unit). Magazine stays full at the new capacity; the reserve cap tracks the carried reserve.
    #[test]
    fn apply_to_rifleman_moves_the_intended_stats() {
        let (_, base) = unit_stats(UnitKind::Rifleman);
        let lo = Loadout {
            optic: Optic::Marksman,       // range +2, cooldown +5
            barrel: Barrel::Heavy,        // damage +6, reserve -60
            magazine: Magazine::Extended, // mag +10, reload +30
        };
        let mut w = base;
        lo.apply_to_weapon(&mut w);
        assert_eq!(w.range, base.range + fx(2));
        assert_eq!(w.damage, base.damage + fx(6));
        assert_eq!(w.cooldown_ticks, base.cooldown_ticks + 5);
        assert_eq!(w.mag_size, base.mag_size + 10);
        assert_eq!(
            w.ammo, w.mag_size,
            "spawned full → stays full at the new capacity"
        );
        assert_eq!(w.reload_ticks, base.reload_ticks + 30);
        assert_eq!(w.reserve, base.reserve - 60);
        assert_eq!(
            w.reserve_max, w.reserve,
            "reserve cap tracks the carried reserve"
        );
    }

    /// The opposed extreme: a Close-Quarters / Light / Quickdraw runner. Confirms the negative
    /// trades land and still stay well above every floor on the Rifleman.
    #[test]
    fn apply_runner_loadout_moves_the_other_way() {
        let (_, base) = unit_stats(UnitKind::Rifleman);
        let lo = Loadout {
            optic: Optic::CloseQuarters,
            barrel: Barrel::Light,
            magazine: Magazine::Quickdraw,
        };
        let mut w = base;
        lo.apply_to_weapon(&mut w);
        assert_eq!(w.range, base.range - fx(2));
        assert_eq!(w.damage, base.damage - fx(6));
        assert_eq!(w.cooldown_ticks, base.cooldown_ticks - 5);
        assert_eq!(w.mag_size, base.mag_size - 10);
        assert_eq!(w.reload_ticks, base.reload_ticks - 30);
        assert_eq!(w.reserve, base.reserve + 60);
    }

    /// A disarmed weapon (the Medic, `range == 0`) is never touched — a loadout doesn't arm a
    /// non-combatant.
    #[test]
    fn disarmed_weapon_is_left_untouched() {
        let (_, base) = unit_stats(UnitKind::Medic);
        assert_eq!(base.range, Fixed::ZERO, "medic is disarmed");
        let mut w = base;
        Loadout {
            optic: Optic::Marksman,
            barrel: Barrel::Heavy,
            magazine: Magazine::Extended,
        }
        .apply_to_weapon(&mut w);
        assert_eq!(w, base, "a disarmed weapon carries no loadout");
    }

    /// A magazine-less but armed weapon keeps `mag_size == 0` (infinite ammo) — the magazine/reserve
    /// axes only apply to a magazine weapon. Range/damage/cooldown still move.
    #[test]
    fn magazineless_weapon_keeps_infinite_ammo() {
        let mut w = Weapon {
            range: fx(10),
            damage: fx(20),
            cooldown_ticks: 30,
            ..Weapon::default()
        };
        assert_eq!(w.mag_size, 0);
        Loadout {
            optic: Optic::Marksman,
            barrel: Barrel::Standard,
            magazine: Magazine::Extended,
        }
        .apply_to_weapon(&mut w);
        assert_eq!(w.range, fx(12), "range still moves");
        assert_eq!(w.cooldown_ticks, 35);
        assert_eq!(
            w.mag_size, 0,
            "stays magazine-less (no reload gate created)"
        );
        assert_eq!(w.reserve, 0);
    }

    // ---- Determinism / checksum coverage (invariant #7) -------------------------------------

    /// Build a tiny deterministic fight: a player shooter (FireAtWill) facing an enemy dummy, the
    /// shooter carrying `loadout`. Returns the sim ready to step. The loadout is applied to the
    /// spawned weapon — match-setup input — so it rides the existing checksum fold.
    fn fight_with_loadout(seed: u64, loadout: Loadout) -> (Sim, Entity) {
        let mut sim = Sim::new(seed);
        // Shooter at origin with a full Rifleman kit, possessed by orders (FireAtWill).
        let shooter = sim.world.spawn();
        let si = shooter.index as usize;
        let (sh, sw) = unit_stats(UnitKind::Rifleman);
        sim.world.pos[si] = Vec2::ZERO;
        sim.world.faction[si] = Faction::Player;
        sim.world.health[si] = sh;
        sim.world.weapon[si] = sw;
        sim.world.stance[si] = Stance::FireAtWill;
        loadout.apply_to_weapon(&mut sim.world.weapon[si]);

        // Enemy dummy in range, lots of HP so it survives the window (we measure divergence, not a
        // race to a kill that could mask it).
        let enemy = sim.world.spawn();
        let ei = enemy.index as usize;
        sim.world.pos[ei] = Vec2::new(fx(10), Fixed::ZERO);
        sim.world.faction[ei] = Faction::Enemy;
        sim.world.health[ei] = Health::full(fx(100_000));
        sim.world.weapon[ei] = Weapon::default(); // unarmed dummy
        sim.world.stance[ei] = Stance::HoldFire;
        (sim, shooter)
    }

    /// Two peers running the SAME loadout step bit-identically — the per-tick checksum stream
    /// matches every tick (invariant #7). This is the 2-peer agreement property for the loadout.
    #[test]
    fn same_loadout_two_peers_stay_bit_identical() {
        let loadout = Loadout {
            optic: Optic::Marksman,
            barrel: Barrel::Heavy,
            magazine: Magazine::Quickdraw,
        };
        let (mut a, _) = fight_with_loadout(0xA11CE, loadout);
        let (mut b, _) = fight_with_loadout(0xA11CE, loadout);
        // The loadout is in the weapon component, so it is folded from the very first checksum.
        assert_eq!(
            a.checksum(),
            b.checksum(),
            "tick 0 (pre-step) must already agree"
        );
        for t in 0..180u32 {
            a.step(&[]);
            b.step(&[]);
            assert_eq!(a.checksum(), b.checksum(), "peers diverged at tick {t}");
        }
    }

    /// Two peers running DIFFERENT loadouts diverge — and the divergence is honest sim state (it
    /// shows up in the checksum, exactly as a real weapon-stat difference must, so the cross-arch
    /// matrix would catch a loadout desync). The control half asserts the divergence is *caused by*
    /// the loadout: same scene + same loadout would have agreed (covered above).
    #[test]
    fn different_loadouts_diverge_in_the_checksum() {
        let marksman = Loadout {
            optic: Optic::Marksman,
            barrel: Barrel::Heavy,
            magazine: Magazine::Extended,
        };
        let runner = Loadout {
            optic: Optic::CloseQuarters,
            barrel: Barrel::Light,
            magazine: Magazine::Quickdraw,
        };
        let (mut a, _) = fight_with_loadout(0xBEEF, marksman);
        let (mut b, _) = fight_with_loadout(0xBEEF, runner);
        // Different weapon stats (range/damage/cooldown/mag/reload/reserve) are all folded, so the
        // streams differ immediately at the pre-step checksum.
        assert_ne!(
            a.checksum(),
            b.checksum(),
            "different loadouts must produce a different sim checksum"
        );
        // …and stay diverged as the fight plays out differently (sanity that it isn't a one-tick
        // coincidence that re-converges).
        let mut diverged_every_tick = true;
        for _ in 0..120u32 {
            a.step(&[]);
            b.step(&[]);
            if a.checksum() == b.checksum() {
                diverged_every_tick = false;
            }
        }
        assert!(
            diverged_every_tick,
            "the loadout difference must persist as sim divergence"
        );
    }

    /// The neutral `STANDARD` loadout is a true no-op on the weapon — applying it leaves a freshly
    /// spawned weapon byte-identical, so an opted-out player's sim is unchanged (and existing scenes
    /// keep their checksums).
    #[test]
    fn standard_loadout_is_a_no_op() {
        let (_, base) = unit_stats(UnitKind::Rifleman);
        let mut w = base;
        Loadout::STANDARD.apply_to_weapon(&mut w);
        assert_eq!(w, base, "the all-Standard loadout must not move any stat");
        assert_eq!(Loadout::STANDARD.total_delta(), StatDelta::ZERO);
        assert_eq!(Loadout::default(), Loadout::STANDARD);
    }

    /// A drive over a Sim with a Standard loadout produces the IDENTICAL checksum stream to the same
    /// Sim with no loadout call at all — i.e. opting in to the neutral loadout costs the checksum
    /// nothing (the byte-neutral guarantee existing scenes rely on).
    #[test]
    fn standard_loadout_matches_no_loadout_stream() {
        let (mut with_std, _) = fight_with_loadout(0x1234, Loadout::STANDARD);
        // Same scene, but never call apply_to_weapon at all.
        let mut without = {
            let mut sim = Sim::new(0x1234);
            let shooter = sim.world.spawn();
            let si = shooter.index as usize;
            let (sh, sw) = unit_stats(UnitKind::Rifleman);
            sim.world.pos[si] = Vec2::ZERO;
            sim.world.faction[si] = Faction::Player;
            sim.world.health[si] = sh;
            sim.world.weapon[si] = sw;
            sim.world.stance[si] = Stance::FireAtWill;
            let enemy = sim.world.spawn();
            let ei = enemy.index as usize;
            sim.world.pos[ei] = Vec2::new(fx(10), Fixed::ZERO);
            sim.world.faction[ei] = Faction::Enemy;
            sim.world.health[ei] = Health::full(fx(100_000));
            sim.world.weapon[ei] = Weapon::default();
            sim.world.stance[ei] = Stance::HoldFire;
            sim
        };
        for t in 0..120u32 {
            with_std.step(&[]);
            without.step(&[]);
            assert_eq!(
                with_std.checksum(),
                without.checksum(),
                "diverged at tick {t}"
            );
        }
    }

    // ---- factions WS-E: per-faction gunsmith pools ------------------------------------------------

    /// The three armies whose pools WS-E ships.
    const ARMIES: [Army; 3] = [Army::Neutral, Army::Us, Army::Fr];

    /// Every build in a given army's pool (3 slots × 3 options = 27), as summed [`StatDelta`]s.
    fn pool_builds(army: Army) -> Vec<StatDelta> {
        all_loadouts()
            .iter()
            .map(|l| l.total_delta_for(army))
            .collect()
    }

    /// THE WS-E fairness invariant, **per pool**: inside each army's gunsmith pool, no attachment
    /// build strictly dominates another (the D60 no-dominant-build rule, applied per `Army`). A
    /// per-faction pool is identity, never power — proven exhaustively over each army's full
    /// 27-build space. A future re-tune that makes one army's build a flat upgrade trips this.
    #[test]
    fn no_pool_build_strictly_dominates_another() {
        for army in ARMIES {
            let builds = pool_builds(army);
            for (i, a) in builds.iter().enumerate() {
                for (j, b) in builds.iter().enumerate() {
                    if i == j {
                        continue;
                    }
                    assert!(
                        !a.strictly_dominates(b),
                        "{army:?} pool: build {i} ({a:?}) strictly dominates build {j} ({b:?})",
                    );
                }
            }
        }
    }

    /// Within every pool, each non-`Standard` option is still a genuine **pure trade** versus the
    /// neutral `Standard` (strictly better on one axis, strictly worse on another). This is the
    /// structural reason the per-pool no-domination property above holds for `Us`/`Fr` too — they
    /// only scale the magnitudes, never turn a trade into a free upgrade.
    #[test]
    fn every_pool_option_is_a_pure_trade() {
        for army in ARMIES {
            let pool = pool_for(army);
            let opts = [
                ("marksman", pool.marksman),
                ("close_quarters", pool.close_quarters),
                ("heavy", pool.heavy),
                ("light", pool.light),
                ("extended", pool.extended),
                ("quickdraw", pool.quickdraw),
            ];
            for (name, d) in opts {
                assert!(
                    d.some_axis_better(&StatDelta::ZERO),
                    "{army:?}/{name} must improve at least one axis: {d:?}"
                );
                assert!(
                    StatDelta::ZERO.some_axis_better(&d),
                    "{army:?}/{name} must cost at least one axis: {d:?}"
                );
            }
        }
    }

    /// Each pool preserves the **disjoint slot/axis** structure the no-domination proof rests on:
    /// the marksman/close-quarters pair touches only {range, cooldown}, heavy/light only
    /// {damage, reserve}, extended/quickdraw only {mag_size, reload}. Scaling magnitudes per army
    /// must never spill a trade onto another slot's axis (which could break disjointness and thus
    /// the proof).
    #[test]
    fn pool_options_keep_disjoint_axis_pairs() {
        for army in ARMIES {
            let pool = pool_for(army);
            for d in [pool.marksman, pool.close_quarters] {
                assert_eq!(d.damage, Fixed::ZERO);
                assert_eq!((d.mag_size, d.reload_ticks, d.reserve), (0, 0, 0));
            }
            for d in [pool.heavy, pool.light] {
                assert_eq!(d.range, Fixed::ZERO);
                assert_eq!((d.cooldown_ticks, d.mag_size, d.reload_ticks), (0, 0, 0));
            }
            for d in [pool.extended, pool.quickdraw] {
                assert_eq!((d.range, d.damage), (Fixed::ZERO, Fixed::ZERO));
                assert_eq!((d.cooldown_ticks, d.reserve), (0, 0));
            }
        }
    }

    /// The [`Army::Neutral`] pool reproduces the slot enums' own deltas byte-for-byte, so a loadout
    /// on a legacy / non-aligned scene behaves and checksums exactly as it did before WS-E. In
    /// particular [`Loadout::total_delta`] equals [`Loadout::total_delta_for(Army::Neutral)`].
    #[test]
    fn neutral_pool_equals_the_baseline() {
        let pool = pool_for(Army::Neutral);
        assert_eq!(pool.marksman, Optic::Marksman.delta());
        assert_eq!(pool.close_quarters, Optic::CloseQuarters.delta());
        assert_eq!(pool.heavy, Barrel::Heavy.delta());
        assert_eq!(pool.light, Barrel::Light.delta());
        assert_eq!(pool.extended, Magazine::Extended.delta());
        assert_eq!(pool.quickdraw, Magazine::Quickdraw.delta());
        for l in all_loadouts() {
            assert_eq!(
                l.total_delta(),
                l.total_delta_for(Army::Neutral),
                "total_delta must equal the Neutral-pool delta for {l:?}"
            );
        }
    }

    /// The pools are genuinely **distinct** — WS-E is identity, so US, FR, and Neutral must not be
    /// the same table. For at least one slot option the three armies' deltas differ.
    #[test]
    fn pools_differ_between_armies() {
        let n = pool_for(Army::Neutral);
        let us = pool_for(Army::Us);
        let fr = pool_for(Army::Fr);
        assert_ne!(us, n, "US pool must differ from the baseline");
        assert_ne!(fr, n, "FR pool must differ from the baseline");
        assert_ne!(us, fr, "US and FR pools must differ from each other");
    }

    /// Build a deterministic fight where the shooter's Rifleman carries `loadout` drawn from
    /// `army`'s pool (via [`Loadout::apply_to_weapon_for`]) — the per-army analogue of
    /// [`fight_with_loadout`].
    fn fight_with_army_loadout(seed: u64, army: Army, loadout: Loadout) -> Sim {
        let mut sim = Sim::new(seed);
        let shooter = sim.world.spawn();
        let si = shooter.index as usize;
        let (sh, sw) = unit_stats(UnitKind::Rifleman);
        sim.world.pos[si] = Vec2::ZERO;
        sim.world.faction[si] = Faction::Player;
        sim.world.health[si] = sh;
        sim.world.weapon[si] = sw;
        sim.world.stance[si] = Stance::FireAtWill;
        loadout.apply_to_weapon_for(army, &mut sim.world.weapon[si]);

        let enemy = sim.world.spawn();
        let ei = enemy.index as usize;
        sim.world.pos[ei] = Vec2::new(fx(10), Fixed::ZERO);
        sim.world.faction[ei] = Faction::Enemy;
        sim.world.health[ei] = Health::full(fx(100_000));
        sim.world.weapon[ei] = Weapon::default();
        sim.world.stance[ei] = Stance::HoldFire;
        sim
    }

    /// **Checksum parity (invariant #7).** Two peers that pick the SAME army and the SAME loadout
    /// step bit-identically — the per-tick checksum stream agrees every tick, for every army. The
    /// per-army loadout rides the existing weapon fold with no new fold surface.
    #[test]
    fn same_army_same_loadout_two_peers_stay_bit_identical() {
        let loadout = Loadout {
            optic: Optic::Marksman,
            barrel: Barrel::Heavy,
            magazine: Magazine::Quickdraw,
        };
        for army in ARMIES {
            let mut a = fight_with_army_loadout(0xA11CE, army, loadout);
            let mut b = fight_with_army_loadout(0xA11CE, army, loadout);
            assert_eq!(
                a.checksum(),
                b.checksum(),
                "{army:?}: tick 0 (pre-step) must already agree"
            );
            for t in 0..180u32 {
                a.step(&[]);
                b.step(&[]);
                assert_eq!(a.checksum(), b.checksum(), "{army:?}: diverged at tick {t}");
            }
        }
    }

    /// Two peers that pick DIFFERENT armies but the same loadout diverge — and the divergence is
    /// honest sim state (it shows up in the checksum, because the per-army pool produced different
    /// real weapon stats). This is the identity-without-power-creep effect made observable: a
    /// US-pool Marksman/Heavy/Extended Rifleman is a measurably different weapon from the FR-pool
    /// one, and a cross-arch matrix would catch a per-army-pool desync like any other.
    #[test]
    fn different_armies_same_loadout_diverge_in_the_checksum() {
        let loadout = Loadout {
            optic: Optic::Marksman,
            barrel: Barrel::Heavy,
            magazine: Magazine::Extended,
        };
        let mut us = fight_with_army_loadout(0xBEEF, Army::Us, loadout);
        let mut fr = fight_with_army_loadout(0xBEEF, Army::Fr, loadout);
        assert_ne!(
            us.checksum(),
            fr.checksum(),
            "US and FR pools must apply different weapon stats for the same loadout"
        );
        for _ in 0..120u32 {
            us.step(&[]);
            fr.step(&[]);
        }
        assert_ne!(
            us.checksum(),
            fr.checksum(),
            "the per-army-pool difference must persist as sim divergence"
        );
    }

    /// Applying a `Us`-pool loadout moves the Rifleman by the US pool's magnitudes (not the
    /// baseline's), and never trips a floor — the per-army apply path is wired correctly.
    #[test]
    fn apply_us_pool_uses_us_magnitudes() {
        let (_, base) = unit_stats(UnitKind::Rifleman);
        let lo = Loadout {
            optic: Optic::Marksman,       // US: range +3, cooldown +7
            barrel: Barrel::Heavy,        // US: damage +8, reserve -80
            magazine: Magazine::Extended, // US: mag +14, reload +40
        };
        let mut w = base;
        lo.apply_to_weapon_for(Army::Us, &mut w);
        assert_eq!(w.range, base.range + fx(3));
        assert_eq!(w.damage, base.damage + fx(8));
        assert_eq!(w.cooldown_ticks, base.cooldown_ticks + 7);
        assert_eq!(w.mag_size, base.mag_size + 14);
        assert_eq!(w.reload_ticks, base.reload_ticks + 40);
        assert_eq!(w.reserve, base.reserve - 80);
        // …and differs from the baseline apply on the same loadout (identity is observable).
        let mut wn = base;
        lo.apply_to_weapon(&mut wn);
        assert_ne!(w, wn, "US pool must differ from the Neutral baseline apply");
    }
}
