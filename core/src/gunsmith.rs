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

use crate::components::Weapon;
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

    /// The summed [`StatDelta`] this loadout applies — the per-slot deltas added across the three
    /// slots. This is the value the no-strict-domination property is proven on (it is a pure
    /// function of the *selection*, independent of any base weapon, so the fairness guarantee holds
    /// regardless of which weapon the loadout is bolted onto).
    #[inline]
    pub fn total_delta(self) -> StatDelta {
        self.optic
            .delta()
            .add(self.barrel.delta())
            .add(self.magazine.delta())
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
        // A disarmed weapon is not a gunsmith target — leave it exactly as it was.
        if w.range <= Fixed::ZERO {
            return;
        }
        let d = self.total_delta();

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
}
