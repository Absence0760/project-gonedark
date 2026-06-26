//! Command-view **upgrade panel** — the readable tier display for the selected camp (roadmap:
//! upgrade trees — "a readable tier display", the "growth" half of "command and grow your camps").
//!
//! When the player selects one of their camps, this lays out a small screen-space panel that tells
//! the truth about *growth*: the camp's current tier, what the **next** tier costs
//! ([`economy::upgrade_cost`]), what it actually *improves* (faster production —
//! [`economy::LEVEL_PROD_SPEEDUP`] off the per-unit build time, down to [`economy::PROD_TICKS_FLOOR`]),
//! and whether the player can afford it right now. It is the visual companion to the engine's
//! `upgrade_ui::upgrade_commands` intent: the panel shows the cost/effect, the button issues the
//! command.
//!
//! ## Pure data seam (the `readout` / `tiers` pattern)
//!
//! [`upgrade_view`] derives the numbers — current tier, next-tier cost, and the production-speed
//! effect — as a free fn, unit-testable without a GPU. The contextual
//! [`command_panel`](crate::command_panel) formats these into its camp-panel rows; this module owns
//! only the numbers, not the layout. It never reads or mutates sim state (invariant #4) — it is a
//! pure function of two host-supplied numbers (the camp's level and the faction's resource purse).
//!
//! ## Inputs come from the host, not a sim read
//!
//! The renderer never calls back into `core` ([`crate::Renderer`] only reads a snapshot). So this
//! panel takes the camp **level** (`u8`) and the **resources** (`i64`) as plain inputs the host
//! plumbs from the sim — the same seam pattern as `readout`'s `resources: Option<u32>`. It then
//! consults the `economy` cost/speedup **consts** (pure `const fn` / `const` reads, no sim state) so
//! the displayed cost and effect are byte-for-byte what the sim will actually charge and apply.
//!
//! ## Scope today: linear leveling. The prereq *tree* is a `core` follow-up.
//!
//! The sim models **linear camp-tier leveling only** today: one `BuildingKind::Camp`, a single
//! `level`, `upgrade_cost(level) = 200 * (level + 1)`, and the sole per-tier benefit is production
//! speedup. So this panel is a faithful, single-track "current tier → next tier" display — it does
//! **not** invent a multi-branch tech tree the sim can't honor.
//!
//! A richer **per-structure / per-unit prerequisite tree** (several upgrades to pick from, gated on
//! prerequisites, each with its own effect) is a **`core` follow-up**, not a render change: it needs
//! new sim state (an upgrade id/enum + a per-building owned-upgrade set), a new `Command` variant,
//! and prerequisite/cost logic in `core::economy` — all checksum-folded. Once that lands, this module
//! grows from one "next tier" view to a list of branch rows (id, cost, prereq-met, effect) the
//! [`command_panel`](crate::command_panel) renders.

use gonedark_core::components::UnitKind;
use gonedark_core::economy;

/// The derived, truthful numbers for the selected camp's upgrade state. Pure data — no GPU, no sim
/// read (computed from the host-supplied `level`/`resources` plus the `economy` cost/speedup consts).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UpgradeView {
    /// Current camp tier (the sim's `Building::level`). Tier 0 is a freshly-built camp.
    pub level: u8,
    /// Cost the sim will charge to reach the next tier: [`economy::upgrade_cost`]`(level)`.
    pub next_cost: i64,
    /// Whether the host-supplied resource purse currently covers [`Self::next_cost`].
    pub affordable: bool,
    /// Per-unit production time (ticks) at the **current** tier, for a [`UnitKind::Rifleman`]
    /// (the bread-and-butter body) — the "before" of the next-tier improvement.
    pub prod_ticks_now: u16,
    /// Per-unit production time (ticks) at the **next** tier — the "after". Equal to
    /// [`Self::prod_ticks_now`] only once the camp has bottomed out at [`economy::PROD_TICKS_FLOOR`]
    /// (a maxed camp: the next tier costs more but buys no further speedup).
    pub prod_ticks_next: u16,
}

impl UpgradeView {
    /// Ticks the next tier shaves off per-unit production (`0` once at the speed floor).
    #[inline]
    pub fn prod_ticks_saved(&self) -> u16 {
        self.prod_ticks_now.saturating_sub(self.prod_ticks_next)
    }

    /// Whether the next tier still buys a production speedup (vs. only costing more at the floor).
    #[inline]
    pub fn next_tier_improves_speed(&self) -> bool {
        self.prod_ticks_saved() > 0
    }
}

/// Derive the [`UpgradeView`] for a selected camp from the host-supplied current `level` and the
/// faction's `resources`. Pure: reads only the `economy` cost/speedup consts (the same single source
/// of truth the sim charges against), never sim state. The production-time sample uses
/// [`UnitKind::Rifleman`] so the "faster production" effect is shown against the unit the player
/// makes most.
pub fn upgrade_view(level: u8, resources: i64) -> UpgradeView {
    let next_cost = economy::upgrade_cost(level);
    let next_level = level.saturating_add(1);
    UpgradeView {
        level,
        next_cost,
        affordable: resources >= next_cost,
        prod_ticks_now: economy::prod_time(UnitKind::Rifleman, level),
        prod_ticks_next: economy::prod_time(UnitKind::Rifleman, next_level),
    }
}


#[cfg(test)]
mod tests {
    //! `render` is the float boundary, so f32 layout math is fair game. The pure view + layout
    //! seams are tested here without a GPU. Costs/speedups are asserted against the `economy`
    //! consts so the panel can never drift from what the sim actually charges/applies.

    use super::*;

    // ---- upgrade_view ----

    #[test]
    fn view_next_cost_matches_economy_upgrade_cost() {
        for level in [0u8, 1, 2, 5] {
            let v = upgrade_view(level, 0);
            assert_eq!(
                v.next_cost,
                economy::upgrade_cost(level),
                "panel cost must equal the sim's upgrade_cost(level)"
            );
        }
        // Concrete sanity: 200 * (level + 1).
        assert_eq!(upgrade_view(0, 0).next_cost, 200);
        assert_eq!(upgrade_view(1, 0).next_cost, 400);
        assert_eq!(upgrade_view(2, 0).next_cost, 600);
    }

    #[test]
    fn view_affordability_tracks_the_purse() {
        let cost = economy::upgrade_cost(0); // 200
        assert!(!upgrade_view(0, cost - 1).affordable, "one short → not affordable");
        assert!(upgrade_view(0, cost).affordable, "exact balance → affordable");
        assert!(upgrade_view(0, cost + 1000).affordable, "surplus → affordable");
    }

    #[test]
    fn view_next_tier_improves_production_speed() {
        // Tier 0 → 1 shaves exactly LEVEL_PROD_SPEEDUP off the per-unit time (well above the floor).
        let v = upgrade_view(0, 10_000);
        assert_eq!(v.prod_ticks_now, economy::prod_time(UnitKind::Rifleman, 0));
        assert_eq!(v.prod_ticks_next, economy::prod_time(UnitKind::Rifleman, 1));
        assert_eq!(
            v.prod_ticks_saved(),
            economy::LEVEL_PROD_SPEEDUP,
            "one tier saves one LEVEL_PROD_SPEEDUP of production time"
        );
        assert!(v.next_tier_improves_speed());
    }

    #[test]
    fn view_at_speed_floor_reports_no_further_speedup() {
        // A maxed-out level: production has bottomed out at the floor, so the next tier buys no
        // speed (it still costs more — the cost keeps climbing).
        let v = upgrade_view(254, i64::MAX / 2);
        assert_eq!(v.prod_ticks_now, economy::PROD_TICKS_FLOOR);
        assert_eq!(v.prod_ticks_next, economy::PROD_TICKS_FLOOR);
        assert_eq!(v.prod_ticks_saved(), 0);
        assert!(!v.next_tier_improves_speed());
        // Cost still grows even at the speed floor.
        assert_eq!(v.next_cost, economy::upgrade_cost(254));
    }

}
