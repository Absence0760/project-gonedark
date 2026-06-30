//! Command-view **troop-training data** (roadmap Phase 2: "Troop-training UI — pick a unit type,
//! see cost + queue + ETA"). This is the render-side counterpart to the `engine::train_ui` intent
//! seam: `train_ui` emits the
//! [`QueueProduction`](gonedark_core::sim::Command::QueueProduction) command; this module supplies
//! the numbers the player reads before pressing — per unit type a **cost** and a **production ETA**.
//!
//! It is the [`readout`](crate::readout) pattern: pure free fns returning plain data ([`TrainOption`],
//! [`eta_seconds`]) — unit-testable without a GPU. The contextual [`command_panel`](crate::command_panel)
//! formats these into its camp panel rows; this module owns only the numbers, not the layout.
//!
//! ## Where the numbers come from (and what stays a host input)
//!
//! Costs and per-level production times are *static design tables*, so this module reads them
//! straight from the deterministic [`economy`] const helpers ([`economy::unit_cost`],
//! [`economy::prod_time`]) — the only `core` calls it makes, and both are pure `const fn`s that
//! cannot touch sim state. Everything *dynamic* — the camp's current level, the player's resource
//! purse, and the live production queue — is **passed in as plain data** by the host (the renderer
//! is the float boundary and never reads the sim itself, exactly like [`readout`](crate::readout)'s
//! resource seam). The ETA is converted ticks → seconds at the locked [`TICK_HZ`] (D21).
//!
//! ## Fairness (invariant #6)
//!
//! The labels are screen-space NDC chrome (the W4 text pass), carry no world position, and are
//! emitted only for the command view — never over the dark embodied frame. They surface only the
//! player's own economy/queue (numbers the commander owns), so they leak no enemy intel.

use gonedark_core::components::UnitKind;
use gonedark_core::economy;
use gonedark_core::sim::TICK_HZ;

/// Convert a production duration in sim ticks to seconds at the locked global tick rate
/// ([`TICK_HZ`], D21). The renderer is the float boundary, so this f32 division is fair here
/// (it never feeds back into the sim — invariant #1/#4).
#[inline]
pub fn eta_seconds(ticks: u16) -> f32 {
    ticks as f32 / TICK_HZ as f32
}

/// Display name for a producible unit kind (the training-button / queue-row text).
fn kind_label(kind: UnitKind) -> &'static str {
    match kind {
        UnitKind::Rifleman => "Rifleman",
        UnitKind::Heavy => "Heavy",
        UnitKind::Tank => "Tank",
        UnitKind::Medic => "Medic",
        UnitKind::AntiTank => "Anti-Tank", // D73
    }
}

/// One trainable unit type as the panel presents it: its [`UnitKind`], display label, resource
/// `cost`, production ETA (both ticks and the seconds the player reads), and whether the player can
/// currently afford it. Pure data derived from the static [`economy`] tables + the host-supplied
/// camp level and resource purse.
#[derive(Clone, PartialEq, Debug)]
pub struct TrainOption {
    pub kind: UnitKind,
    pub label: &'static str,
    pub cost: i64,
    pub eta_ticks: u16,
    pub eta_seconds: f32,
    /// `resources >= cost` for the purse handed in — drives the affordable/dimmed coloring.
    pub affordable: bool,
}

/// Build the per-unit-type training options for a camp of `camp_level`, given the player's current
/// `resources`. Pure (no GPU, no sim read) — the testable seam. `cost` and `eta_ticks` come from the
/// static [`economy`] tables ([`economy::unit_cost`] / [`economy::prod_time`], the latter applying
/// the camp's level speed-up); `affordable` is `resources >= cost`. `trainable` is the host's list
/// of offered kinds, in display order (e.g. `[Rifleman, Heavy]`).
pub fn train_options(trainable: &[UnitKind], camp_level: u8, resources: i64) -> Vec<TrainOption> {
    trainable
        .iter()
        .map(|&kind| {
            let cost = economy::unit_cost(kind);
            let eta_ticks = economy::prod_time(kind, camp_level);
            TrainOption {
                kind,
                label: kind_label(kind),
                cost,
                eta_ticks,
                eta_seconds: eta_seconds(eta_ticks),
                affordable: resources >= cost,
            }
        })
        .collect()
}


#[cfg(test)]
mod tests {
    //! `render` is the float boundary, so the f32 ETA/layout math is fair game. The pure option /
    //! queue / layout seams are tested here without a GPU.

    use super::*;
    use gonedark_core::economy::{HEAVY_COST, RIFLEMAN_COST};

    const ALL: [UnitKind; 2] = [UnitKind::Rifleman, UnitKind::Heavy];

    // ---- eta_seconds ----

    #[test]
    fn eta_converts_ticks_to_seconds_at_tick_hz() {
        // 300 ticks at 60 Hz is exactly 5 s; 660 ticks is 11 s (the D30 baselines).
        assert_eq!(eta_seconds(TICK_HZ as u16), 1.0, "one second of ticks = 1.0s");
        assert_eq!(eta_seconds(300), 5.0);
        assert_eq!(eta_seconds(660), 11.0);
    }

    // ---- train_options ----

    #[test]
    fn options_carry_cost_and_eta_from_the_economy_tables() {
        let opts = train_options(&ALL, 0, 1_000);
        assert_eq!(opts.len(), 2);

        assert_eq!(opts[0].kind, UnitKind::Rifleman);
        assert_eq!(opts[0].label, "Rifleman");
        assert_eq!(opts[0].cost, RIFLEMAN_COST);
        assert_eq!(opts[0].eta_ticks, economy::prod_time(UnitKind::Rifleman, 0));
        assert_eq!(opts[0].eta_seconds, eta_seconds(opts[0].eta_ticks));

        assert_eq!(opts[1].kind, UnitKind::Heavy);
        assert_eq!(opts[1].cost, HEAVY_COST);
        assert_eq!(opts[1].eta_ticks, economy::prod_time(UnitKind::Heavy, 0));
    }

    #[test]
    fn higher_camp_level_shortens_the_eta() {
        let lvl0 = train_options(&ALL, 0, 1_000);
        let lvl3 = train_options(&ALL, 3, 1_000);
        for (a, b) in lvl0.iter().zip(&lvl3) {
            assert!(
                b.eta_ticks < a.eta_ticks,
                "a higher-level camp produces {} faster",
                a.label
            );
            assert!(b.eta_seconds < a.eta_seconds);
        }
    }

    #[test]
    fn affordability_tracks_the_resource_purse() {
        // Exactly a Rifleman's worth: rifle affordable, heavy not.
        let opts = train_options(&ALL, 0, RIFLEMAN_COST);
        assert!(opts[0].affordable, "exact-cost purse affords the rifleman");
        assert!(!opts[1].affordable, "but not the pricier heavy");

        // One short of a rifleman: nothing affordable.
        let broke = train_options(&ALL, 0, RIFLEMAN_COST - 1);
        assert!(!broke[0].affordable && !broke[1].affordable);

        // Flush: everything affordable.
        let rich = train_options(&ALL, 0, 10_000);
        assert!(rich.iter().all(|o| o.affordable));
    }

    #[test]
    fn empty_trainable_yields_no_options() {
        assert!(train_options(&[], 0, 1_000).is_empty());
    }

}
