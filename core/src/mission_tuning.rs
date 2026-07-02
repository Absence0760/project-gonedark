//! WS-E — difficulty tiers, scenario modifiers, and light per-node briefing framing for the
//! PvE Operations campaign ([`docs/plans/pve-campaign-plan.md`], [D58]/[D60]).
//!
//! Three small, **deterministic, float-free** value types (invariant #1):
//!
//! - [`Difficulty`] → [`DifficultyParams`]: a tier that scales the **seeded** enemy
//!   [`commander`](crate::commander) planner — reserve, unit-mix, re-plan cadence, and
//!   production aggression. It is threaded in through [`CommanderConfig`](crate::commander)
//!   and reads **nothing** about the player's embodiment / fog state. A harder tier issues
//!   orders sooner and spends more freely; it never *learns* more. That structural bound is
//!   the point: difficulty makes the honest AI a **better commander**, never an **omniscient**
//!   one (invariant #6 / `game-design.md` §9, the "AI honest, never omniscient" guardrail).
//!
//! - [`ScenarioModifiers`]: reshape the **situation** through scenario-local levers only —
//!   force size, reinforcement cadence (income period), fog rules ([`TellMode`]), and a match
//!   time limit. They **never** touch the locked [D30] balance constants
//!   ([`economy`](crate::economy)): a Rifleman costs the same and hits as hard at every tier,
//!   so the measured balance baseline and cross-arch determinism (invariant #7) hold. Modifiers
//!   change the *board*, not the *pieces*.
//!
//! - [`Briefing`]: minimal narrative framing per campaign node ([Q16] keeps depth deferred) —
//!   pure static text plus the tuning that node runs at. No sim state, no logic.
//!
//! - [`ScenarioModifiers::for_rotation`] (CP-8): the live-ops rotation bridge —
//!   `server::liveops` ships a plain `(period, track)` pair over the wire, and this is the *one*
//!   place that pair turns into a real [`ScenarioModifiers`]. Same bound as above: it can only
//!   ever select among the fixed, authored presets, so a live-ops payload structurally cannot
//!   reach a balance number or grant power.
//!
//! Determinism: every knob is an integer (or an [`Option<u64>`] tick count). The
//! [`Difficulty::default()`] tier ([`Veteran`](Difficulty::Veteran)) reproduces the commander's
//! original constants **byte-for-byte**, so adding the tier perturbs no default/golden-checksum
//! stream — exactly the discipline [`CommanderConfig`](crate::commander)'s other knobs follow.

use crate::detection::TellMode;
use crate::economy;
use crate::sim::{Sim, TICK_HZ};

/// How aggressively the **seeded** enemy commander plays — a deterministic difficulty tier
/// (invariant #1). It scales the planner's *choices* (how deep it queues production, how big a
/// cushion it keeps before splurging on a Heavy, how often it re-tasks its army), never its
/// *knowledge*: a tier is just a different set of integer thresholds fed to the same honest
/// survey of already-checksummed state. It reads nothing about the player going dark — invariant
/// #6 is structural here, not a discipline.
///
/// [`Veteran`](Self::Veteran) is the **default** and reproduces the commander's original constants
/// exactly, so the default scenes' lockstep/checksum streams are untouched.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Difficulty {
    /// Easiest: shallow production backlog, a fat reserve (rarely splurges on a Heavy), and a
    /// sluggish re-plan cadence (reconsiders its army's orders half as often). A forgiving first
    /// contact with going-dark — punishes overstaying *honestly*, never cruelly.
    Recruit,
    /// The baseline tier — **byte-for-byte** the commander's pre-difficulty behavior (backlog 2,
    /// Heavy reserve = one Rifleman, re-plan every cycle). The golden-checksum-stable default.
    #[default]
    Veteran,
    /// Hardest: a deeper production backlog (presses its economic edge harder), no Heavy reserve
    /// (buys the bruiser the moment it can afford one), and a re-plan every cycle. A *sharper
    /// commander*, still bounded to what it can honestly see.
    Elite,
}

/// The resolved integer knobs a [`Difficulty`] feeds the commander. A plain table — the commander
/// reads these instead of its old hard-coded constants, so swapping the tier swaps the numbers
/// with zero new branches in the planner.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DifficultyParams {
    /// Max production backlog the commander will let stack at one camp (the **aggression** lever).
    /// Deeper ⇒ it converts banked resources into bodies faster. Veteran = 2 (the original
    /// `MAX_QUEUE_DEPTH`).
    pub max_queue_depth: usize,
    /// Resource cushion kept free before splurging on a Heavy — the **reserve / unit-mix** lever.
    /// Larger ⇒ buys Heavies later and rarer (a lighter, rifle-heavy mix); zero ⇒ buys a Heavy the
    /// instant it is affordable. Veteran = one Rifleman (the original `RESERVE`). Never negative.
    pub heavy_reserve: i64,
    /// Re-plan **cadence** stride, in commander cycles: the army-tasking + posture pass runs only
    /// on cycles where `cycle % stride == 0` (`cycle = tick / COMMANDER_PERIOD`). `1` ⇒ every cycle
    /// (Veteran, the original behavior); `2` ⇒ every other cycle, so an easier commander reacts
    /// more slowly. Reinforcement is *not* strided — only the order-reconsideration cadence is.
    /// Always `>= 1`.
    pub command_stride: u64,
}

impl Difficulty {
    /// Every tier, easiest → hardest, in a fixed order (a stable space for menus / tests).
    pub const ALL: [Difficulty; 3] = [Difficulty::Recruit, Difficulty::Veteran, Difficulty::Elite];

    /// The integer knobs this tier feeds the commander. `const` and float-free (invariant #1).
    ///
    /// [`Veteran`](Self::Veteran) returns *exactly* the commander's original constants
    /// (`max_queue_depth = 2`, `heavy_reserve = RIFLEMAN_COST`, `command_stride = 1`), so the
    /// default tier perturbs no golden-checksum stream.
    pub const fn params(self) -> DifficultyParams {
        match self {
            Difficulty::Recruit => DifficultyParams {
                max_queue_depth: 1,
                // Two Riflemen of cushion → splurges on a Heavy only when genuinely flush.
                heavy_reserve: 2 * economy::RIFLEMAN_COST,
                command_stride: 2,
            },
            Difficulty::Veteran => DifficultyParams {
                max_queue_depth: 2,
                heavy_reserve: economy::RIFLEMAN_COST,
                command_stride: 1,
            },
            Difficulty::Elite => DifficultyParams {
                max_queue_depth: 3,
                heavy_reserve: 0,
                command_stride: 1,
            },
        }
    }
}

/// Scenario-parameter modifiers — reshape the **situation**, never the [D30] balance numbers.
///
/// Each field maps to a scenario-local lever the sim/host already exposes (income period, the
/// detection [`TellMode`], the starting force count, the host-side timeout) — *not* to a per-unit
/// stat or cost. So a node can be made harder by giving the enemy a bigger garrison, a faster
/// reinforcement drip, a louder/quieter fog regime, or a tighter clock, while every unit fights
/// to the **same** measured baseline on every arch (invariants #1/#7). [`Default`] is fully
/// neutral: it changes nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScenarioModifiers {
    /// Starting-force size as an integer **percent** of the scenario's base (`100` = unchanged).
    /// Applied by a scenario when it spawns its opening force — *more bodies, identical per-unit
    /// stats*. Integer-scaled (see [`scaled_force`](Self::scaled_force)), so it is deterministic.
    pub force_scale_pct: u32,
    /// Reinforcement cadence: an override for the income-accrual period (ticks), or `None` to keep
    /// the scenario's own. Faster drip ⇒ the enemy reinforces sooner — a situation lever
    /// ([`Sim::set_income_period`]), not a change to how much a held point is worth.
    pub reinforcement_period: Option<u32>,
    /// Fog rule for the going-dark tell — the node's **intel regime** ([`TellMode`]). `Hidden`
    /// makes the player's blindness total (pure inference); `Marked` is the most forgiving. This is
    /// the *situation's* fairness dial, still fully honest (invariant #6).
    pub fog: TellMode,
    /// Optional match time limit in ticks (host-side, evaluated outside the checksum fold), or
    /// `None` for no clock. A tighter clock raises pressure without touching a single balance number.
    pub time_limit_ticks: Option<u64>,
}

impl Default for ScenarioModifiers {
    /// Neutral: force unchanged (100%), scenario's own reinforcement cadence, the `Subtle` fog
    /// baseline ([D33]), no time limit. Applying the default modifies nothing.
    fn default() -> Self {
        ScenarioModifiers {
            force_scale_pct: 100,
            reinforcement_period: None,
            fog: TellMode::Subtle,
            time_limit_ticks: None,
        }
    }
}

impl ScenarioModifiers {
    /// Scale a base force **count** by [`force_scale_pct`](Self::force_scale_pct), integer and
    /// saturating (no float, no overflow). A non-empty base never scales to zero — a node always
    /// fields *someone* — so `scaled_force(n>=1)` is clamped to at least `1`.
    pub const fn scaled_force(&self, base: u32) -> u32 {
        // u64 intermediate so `base * pct` can't overflow u32; integer division truncates.
        let scaled = (base as u64 * self.force_scale_pct as u64) / 100;
        let scaled = if scaled > u32::MAX as u64 {
            u32::MAX
        } else {
            scaled as u32
        };
        if base >= 1 && scaled == 0 {
            1
        } else {
            scaled
        }
    }

    /// Apply the one scenario lever `core` itself owns — the reinforcement cadence — onto `sim`.
    /// The force size and time limit are the scenario seeder's / host's to consume
    /// ([`scaled_force`](Self::scaled_force) / [`time_limit_ticks`](Self::time_limit_ticks)); the
    /// fog regime is the detection channel's. With the default (`None`) period this is a no-op, so
    /// it never perturbs a scenario's baseline checksum.
    pub fn apply_to_sim(&self, sim: &mut Sim) {
        if let Some(period) = self.reinforcement_period {
            sim.set_income_period(period);
        }
    }

    /// Build a time limit from whole seconds at the locked [`TICK_HZ`] (60). `secs == 0` ⇒ `None`
    /// (no clock). Convenience so briefings can read in seconds while the host counts ticks.
    pub const fn time_limit_from_secs(secs: u64) -> Option<u64> {
        if secs == 0 {
            None
        } else {
            Some(secs * TICK_HZ as u64)
        }
    }
}

// ===========================================================================
// CP-8 — the live-ops rotation seam
// ===========================================================================
//
// `server::liveops` (server-side, no `core`/determinism concern) ships only plain scalars over
// the wire: a rotation **period** (`u64`, "which week/cycle is active") and a rotation **track**
// (`u32`, "which authored catalog" — the wire index a consent-gated experiment can override). It
// never invents a `ScenarioModifiers` value itself — it can't, it doesn't depend on `core`. The
// host (the platform-specific `app`/`engine` glue that already fetches the live-ops config) reads
// those two plain integers off the response and calls [`ScenarioModifiers::for_rotation`] here,
// which is the ONLY place a live-ops payload turns into scenario tuning. Because the function can
// only ever return one of the fixed, authored [`ScenarioModifiers`] values below, it is
// structurally impossible for a live-ops payload to reach a D30 balance constant or grant power —
// the same bound [`ScenarioModifiers`] already enforces for authored difficulty tiers.

/// One rotation's worth of pre-authored [`ScenarioModifiers`] presets — the **"standard"** track
/// every client gets absent a personalized override. Four presets so a week's pick is never the
/// same as its immediate neighbor by construction (`len` divides evenly into few enough weeks that
/// authors can reason about the cycle). Every entry is a genuine situation-lever combination
/// (force size / reinforcement cadence / fog / clock) — never a balance number; the type itself
/// makes that impossible to violate.
const STANDARD_ROTATION: [ScenarioModifiers; 4] = [
    ScenarioModifiers {
        force_scale_pct: 100,
        reinforcement_period: None,
        fog: TellMode::Subtle,
        time_limit_ticks: None,
    },
    ScenarioModifiers {
        force_scale_pct: 120,
        reinforcement_period: Some(480),
        fog: TellMode::Subtle,
        time_limit_ticks: None,
    },
    ScenarioModifiers {
        force_scale_pct: 100,
        reinforcement_period: None,
        fog: TellMode::Marked,
        time_limit_ticks: Some(600 * TICK_HZ as u64),
    },
    ScenarioModifiers {
        force_scale_pct: 140,
        reinforcement_period: Some(300),
        fog: TellMode::Hidden,
        time_limit_ticks: None,
    },
];

/// The **"variant A"** rotation track — a second authored catalog an analytics-derived experiment
/// cohort can be pointed at instead of [`STANDARD_ROTATION`] (consent-gated: see
/// `server::liveops::PersonalizedConfig::modifier_track_override`). Same length and the same
/// "situation levers only" bound as the standard track; deliberately a *different* sequence so an
/// A/B rotation test is actually distinguishable, never a stronger/weaker one.
const VARIANT_A_ROTATION: [ScenarioModifiers; 4] = [
    ScenarioModifiers {
        force_scale_pct: 110,
        reinforcement_period: None,
        fog: TellMode::Subtle,
        time_limit_ticks: None,
    },
    ScenarioModifiers {
        force_scale_pct: 100,
        reinforcement_period: None,
        fog: TellMode::Subtle,
        time_limit_ticks: None,
    },
    ScenarioModifiers {
        force_scale_pct: 130,
        reinforcement_period: Some(420),
        fog: TellMode::Marked,
        time_limit_ticks: None,
    },
    ScenarioModifiers {
        force_scale_pct: 100,
        reinforcement_period: Some(240),
        fog: TellMode::Hidden,
        time_limit_ticks: Some(480 * TICK_HZ as u64),
    },
];

impl ScenarioModifiers {
    /// The CP-8 live-ops bridge: resolve a wire-level `(period, track)` pair — exactly what
    /// `server::liveops::LiveOpsConfig` carries — into the [`ScenarioModifiers`] to apply for that
    /// rotation. Pure and total over all `u64`/`u32` inputs (no panics, no I/O, no clock read): the
    /// **same** `(period, track)` always yields the **same** modifiers, on every platform, forever
    /// (invariant #1/#7) — the live-ops server only ever *names* an index into an authored,
    /// client-shipped catalog, it never hands the client a bespoke value.
    ///
    /// - `period` selects the entry by wrapping (`period % catalog.len()`), so the rotation simply
    ///   repeats — the server can hand out an ever-increasing week counter forever without `core`
    ///   growing a table.
    /// - `track` selects the catalog: `1` ⇒ [`VARIANT_A_ROTATION`] (the consent-gated experiment
    ///   catalog); anything else (including the `0` baseline and any unrecognized/future wire
    ///   value) ⇒ [`STANDARD_ROTATION`] — an unknown track degrades to the safe public baseline,
    ///   never to a guess (mirrors [`ConsentGate::guard`](crate)'s "absent/unknown ⇒ the safe
    ///   default" posture).
    pub fn for_rotation(period: u64, track: u32) -> ScenarioModifiers {
        let catalog: &[ScenarioModifiers] = if track == 1 {
            &VARIANT_A_ROTATION
        } else {
            &STANDARD_ROTATION
        };
        catalog[(period % catalog.len() as u64) as usize]
    }
}

/// Light per-node briefing framing ([Q16]: campaign-narrative depth is deferred — this is the
/// minimal seam). Pure static text plus the [`Difficulty`] and [`ScenarioModifiers`] the node runs
/// at; no sim state and no logic, so it carries zero determinism surface. The shell renders these
/// strings; the host applies the tuning.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Briefing {
    /// Node title, e.g. `"Seize the Outpost"`.
    pub title: &'static str,
    /// One or two lines of situation framing — *why* you are here.
    pub situation: &'static str,
    /// The objective, in the player's words, e.g. `"Take the enemy camp. Don't lose all ten."`
    pub objective_line: &'static str,
    /// The tier this node is briefed at.
    pub difficulty: Difficulty,
    /// The situation modifiers in force for this node.
    pub modifiers: ScenarioModifiers,
}

/// The first campaign node's briefing — the WS-A "Seize" mission ("10 troops, take the base"),
/// framed at the easiest tier with neutral modifiers. A concrete, minimal example of the seam; the
/// campaign graph (WS-B) owns the full node set.
pub const MISSION_ONE_BRIEFING: Briefing = Briefing {
    title: "Seize the Outpost",
    situation: "Ten of yours against a dug-in garrison. Command them — or go dark and fight one \
                yourself. Just don't stay blind too long.",
    objective_line: "Take the enemy camp. Don't lose all ten.",
    difficulty: Difficulty::Recruit,
    modifiers: ScenarioModifiers {
        force_scale_pct: 100,
        reinforcement_period: None,
        fog: TellMode::Subtle,
        time_limit_ticks: None,
    },
};

/// The second campaign node's briefing — Mission 2, *Hold the Line*
/// ([`crate::scenario::seed_hold_mission`], a Survive/defense archetype). Framed a step up from the
/// opening mission's `Recruit` (a `Veteran` commander), with neutral modifiers. The situation reframes
/// the going-dark cost from the *defender's* side — the flank you can't see is the one that caves.
pub const MISSION_TWO_BRIEFING: Briefing = Briefing {
    title: "Hold the Line",
    situation: "They're coming for your dug-in line. Fight it from cover, or embody one rifle and \
                hold by hand — but go dark and the line you can't see is the one that breaks.",
    objective_line: "Hold the position. Don't let them overrun you.",
    difficulty: Difficulty::Veteran,
    modifiers: ScenarioModifiers {
        force_scale_pct: 100,
        reinforcement_period: None,
        fog: TellMode::Subtle,
        time_limit_ticks: None,
    },
};

#[cfg(test)]
mod tests {
    use super::*;

    /// The default tier is `Veteran` and its knobs reproduce the commander's original constants
    /// **exactly** — the property that keeps the default scenes' golden checksums untouched.
    #[test]
    fn veteran_default_reproduces_original_commander_constants() {
        assert_eq!(Difficulty::default(), Difficulty::Veteran);
        let p = Difficulty::Veteran.params();
        assert_eq!(p.max_queue_depth, 2, "original MAX_QUEUE_DEPTH");
        assert_eq!(p.heavy_reserve, economy::RIFLEMAN_COST, "original RESERVE");
        assert_eq!(p.command_stride, 1, "original: re-plan every cycle");
    }

    /// The tiers scale monotonically in the intended direction: harder ⇒ deeper backlog, smaller
    /// reserve, tighter (smaller) cadence stride. This is the "honest, better commander" curve —
    /// all knobs, no new knowledge.
    #[test]
    fn tiers_scale_monotonically_easy_to_hard() {
        let r = Difficulty::Recruit.params();
        let v = Difficulty::Veteran.params();
        let e = Difficulty::Elite.params();

        // Aggression: backlog deepens easiest → hardest.
        assert!(r.max_queue_depth <= v.max_queue_depth);
        assert!(v.max_queue_depth <= e.max_queue_depth);
        assert!(r.max_queue_depth < e.max_queue_depth, "the curve has real spread");

        // Reserve / unit-mix: cushion shrinks easiest → hardest (Elite buys Heavies freely).
        assert!(r.heavy_reserve >= v.heavy_reserve);
        assert!(v.heavy_reserve >= e.heavy_reserve);
        assert_eq!(e.heavy_reserve, 0, "Elite keeps no reserve");

        // Cadence: stride tightens (gets smaller) easiest → hardest; never below 1.
        assert!(r.command_stride >= v.command_stride);
        assert!(v.command_stride >= e.command_stride);
        for d in Difficulty::ALL {
            assert!(d.params().command_stride >= 1, "stride must never be 0 (no div issues)");
            assert!(d.params().heavy_reserve >= 0, "reserve is never negative");
        }
    }

    /// Force scaling is integer, saturating, and never empties a non-empty base.
    #[test]
    fn scaled_force_is_integer_and_clamped() {
        let m = |pct| ScenarioModifiers {
            force_scale_pct: pct,
            ..ScenarioModifiers::default()
        };
        assert_eq!(m(100).scaled_force(10), 10, "100% is identity");
        assert_eq!(m(200).scaled_force(10), 20, "doubling");
        assert_eq!(m(50).scaled_force(10), 5, "halving");
        assert_eq!(m(33).scaled_force(10), 3, "integer truncation, not rounding");
        assert_eq!(m(0).scaled_force(10), 1, "a non-empty base never goes to zero");
        assert_eq!(m(0).scaled_force(0), 0, "an empty base stays empty");
        // No overflow at the extreme.
        assert_eq!(m(u32::MAX).scaled_force(u32::MAX), u32::MAX);
    }

    /// The default modifiers are fully neutral — applying them changes nothing.
    #[test]
    fn default_modifiers_are_neutral() {
        let d = ScenarioModifiers::default();
        assert_eq!(d.force_scale_pct, 100);
        assert_eq!(d.reinforcement_period, None);
        assert_eq!(d.fog, TellMode::Subtle);
        assert_eq!(d.time_limit_ticks, None);
        assert_eq!(d.scaled_force(7), 7, "neutral force scale is identity");
    }

    /// A modifier reshapes the situation through the scenario lever (income cadence) **only**, and
    /// does so deterministically. The cadence is a *stepping* parameter, so its effect shows up in
    /// the sim's evolved state: drive the same modified `Sim` the same number of ticks and the
    /// checksum is a pure function of (scenario, modifier) — identical for the same period, and
    /// distinct for distinct periods (the faster drip banks more by a given tick). The D30 balance
    /// constants are untouched throughout — the board changes, never the pieces.
    #[test]
    fn modifier_reshapes_scenario_param_deterministically_not_balance() {
        // Balance constants are constants — a modifier can never reach them (asserted for the record).
        let costs_before = (economy::RIFLEMAN_COST, economy::HEAVY_COST, economy::CAMP_BUILD_COST);

        // Apply the period modifier, drive a fixed span, return the evolved checksum.
        let run = |period: Option<u32>| -> u64 {
            let mut sim = Sim::new(0xA11CE);
            ScenarioModifiers {
                reinforcement_period: period,
                ..ScenarioModifiers::default()
            }
            .apply_to_sim(&mut sim);
            for _ in 0..120 {
                sim.step(&[]);
            }
            sim.checksum()
        };
        // The scenario's own baseline (default income period = 1), driven the same span.
        let baseline = {
            let mut sim = Sim::new(0xA11CE);
            for _ in 0..120 {
                sim.step(&[]);
            }
            sim.checksum()
        };

        // Deterministic: same modifier → same evolved checksum, twice over.
        assert_eq!(run(Some(18)), run(Some(18)));
        // The neutral (None) modifier leaves the scenario's own cadence untouched.
        assert_eq!(run(None), baseline, "neutral modifier perturbs nothing");
        // A slower drip banks less by tick 120 → a different (but still deterministic) situation.
        assert_ne!(run(Some(60)), baseline, "the cadence lever actually bit");
        assert_ne!(run(Some(6)), run(Some(60)), "distinct periods diverge");

        // ...and the modifier never reached a balance number.
        assert_eq!(
            costs_before,
            (economy::RIFLEMAN_COST, economy::HEAVY_COST, economy::CAMP_BUILD_COST),
            "modifiers must never touch the D30 balance constants"
        );
    }

    /// `apply_to_sim` actually sets the period the sim reports back (the scenario-local lever wired).
    #[test]
    fn apply_to_sim_sets_the_income_period() {
        let mut sim = Sim::new(1);
        ScenarioModifiers {
            reinforcement_period: Some(24),
            ..ScenarioModifiers::default()
        }
        .apply_to_sim(&mut sim);
        assert_eq!(sim.income_period(), 24);
    }

    /// The seconds→ticks helper is integer and treats 0 as "no clock".
    #[test]
    fn time_limit_from_secs_is_tick_exact() {
        assert_eq!(ScenarioModifiers::time_limit_from_secs(0), None);
        assert_eq!(ScenarioModifiers::time_limit_from_secs(1), Some(TICK_HZ as u64));
        assert_eq!(ScenarioModifiers::time_limit_from_secs(120), Some(120 * TICK_HZ as u64));
    }

    /// CP-8: the same `(period, track)` pair always resolves to the same modifiers — the property
    /// the whole live-ops bridge rests on (a client that fetches the config twice, or two peers
    /// fetching independently, must land on an identical rotation).
    #[test]
    fn for_rotation_is_deterministic_for_the_same_period_and_track() {
        for track in [0u32, 1, 2, 999] {
            for period in [0u64, 1, 3, 400, u64::MAX] {
                assert_eq!(
                    ScenarioModifiers::for_rotation(period, track),
                    ScenarioModifiers::for_rotation(period, track),
                    "period={period} track={track} must be stable"
                );
            }
        }
    }

    /// The rotation wraps: a period `catalog.len()` ticks later repeats the same entry.
    #[test]
    fn for_rotation_wraps_around_the_catalog() {
        assert_eq!(
            ScenarioModifiers::for_rotation(0, 0),
            ScenarioModifiers::for_rotation(4, 0),
            "standard catalog has 4 entries; period 4 must repeat period 0"
        );
        assert_eq!(
            ScenarioModifiers::for_rotation(1, 1),
            ScenarioModifiers::for_rotation(5, 1),
            "variant-A catalog has 4 entries too"
        );
    }

    /// Track `1` selects a genuinely different catalog from track `0` — otherwise the A/B
    /// rotation experiment would be a no-op.
    #[test]
    fn for_rotation_track_one_differs_from_standard() {
        let mut any_diff = false;
        for period in 0..4u64 {
            if ScenarioModifiers::for_rotation(period, 0) != ScenarioModifiers::for_rotation(period, 1) {
                any_diff = true;
            }
        }
        assert!(any_diff, "variant-A track must diverge from standard somewhere in the cycle");
    }

    /// Any unrecognized track (not just `0`) safely degrades to the standard catalog — an unknown
    /// wire value is never guessed into a new behavior, mirroring the consent gate's
    /// absent/unknown ⇒ safe-default posture.
    #[test]
    fn for_rotation_unknown_track_falls_back_to_standard() {
        for period in 0..4u64 {
            assert_eq!(
                ScenarioModifiers::for_rotation(period, 0),
                ScenarioModifiers::for_rotation(period, 7),
                "unrecognized track must degrade to the standard catalog"
            );
            assert_eq!(
                ScenarioModifiers::for_rotation(period, 0),
                ScenarioModifiers::for_rotation(period, u32::MAX),
            );
        }
    }

    /// The full bridge proof, mirroring `modifier_reshapes_scenario_param_deterministically_not_balance`:
    /// resolving a live-ops `(period, track)` into modifiers, applying them, and stepping the sim
    /// stays checksum-deterministic — and the D30 balance constants are never reachable through this
    /// path, exactly like every other `ScenarioModifiers` entry point.
    #[test]
    fn for_rotation_selected_modifiers_are_checksum_deterministic_and_never_touch_balance() {
        let costs_before = (economy::RIFLEMAN_COST, economy::HEAVY_COST, economy::CAMP_BUILD_COST);

        let run = |period: u64, track: u32| -> u64 {
            let mut sim = Sim::new(0xA11CE);
            ScenarioModifiers::for_rotation(period, track).apply_to_sim(&mut sim);
            for _ in 0..120 {
                sim.step(&[]);
            }
            sim.checksum()
        };

        // Same (period, track) twice ⇒ identical evolved checksum.
        assert_eq!(run(2, 0), run(2, 0));
        assert_eq!(run(1, 1), run(1, 1));
        // Distinct rotations (different track, same period) diverge — the lever actually bit.
        assert_ne!(run(2, 0), run(2, 1));

        assert_eq!(
            costs_before,
            (economy::RIFLEMAN_COST, economy::HEAVY_COST, economy::CAMP_BUILD_COST),
            "the live-ops rotation bridge must never touch the D30 balance constants"
        );
    }

    /// The example briefing is wired to a real tier + neutral modifiers (the minimal seam works).
    #[test]
    fn mission_one_briefing_is_framed() {
        assert_eq!(MISSION_ONE_BRIEFING.difficulty, Difficulty::Recruit);
        assert_eq!(MISSION_ONE_BRIEFING.modifiers, ScenarioModifiers::default());
        assert!(!MISSION_ONE_BRIEFING.title.is_empty());
        assert!(!MISSION_ONE_BRIEFING.objective_line.is_empty());
    }

    /// Mission 2 (*Hold the Line*) is briefed a step up (`Veteran`) with neutral modifiers, and is a
    /// distinct, fully-populated node from Mission 1.
    #[test]
    fn mission_two_briefing_is_framed() {
        assert_eq!(MISSION_TWO_BRIEFING.difficulty, Difficulty::Veteran);
        assert_eq!(MISSION_TWO_BRIEFING.modifiers, ScenarioModifiers::default());
        assert!(!MISSION_TWO_BRIEFING.title.is_empty());
        assert!(!MISSION_TWO_BRIEFING.objective_line.is_empty());
        assert_ne!(MISSION_TWO_BRIEFING.title, MISSION_ONE_BRIEFING.title, "a distinct mission");
    }
}
