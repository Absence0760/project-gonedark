//! Command-view **troop-training panel** (roadmap Phase 2: "Troop-training UI — pick a unit type,
//! see cost + queue + ETA, set a rally point"). This is the render-side counterpart to the
//! `engine::train_ui` intent seam: `train_ui` emits the
//! [`QueueProduction`](gonedark_core::sim::Command::QueueProduction) command; this module
//! lays out what the player reads before pressing — per unit type a **cost** and a **production
//! ETA**, plus the camp's current **production queue** with each item's remaining time.
//!
//! It is the [`readout`](crate::readout) pattern: pure free fns returning plain data ([`TrainOption`]
//! / [`QueueRow`]) and laid-out labels ([`TrainPanelLabel`]) with NDC anchors — unit-testable
//! without a GPU. The host loops each [`TrainPanelLabel`] into a
//! [`text::TextRenderer::queue`](crate::text::TextRenderer::queue) call, exactly as it does the
//! [`readout`](crate::readout) labels.
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

use crate::text::Anchor;
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

/// One entry of the camp's live production queue as the panel presents it: the [`UnitKind`], its
/// label, and the ticks/seconds remaining on it. Built from host-supplied `(kind, ticks_remaining)`
/// pairs (the renderer never reads the sim queue itself).
#[derive(Clone, PartialEq, Debug)]
pub struct QueueRow {
    pub kind: UnitKind,
    pub label: &'static str,
    pub ticks_remaining: u16,
    pub seconds_remaining: f32,
}

/// Lay out the camp's production queue rows from host-supplied `(kind, ticks_remaining)` pairs (the
/// front of the slice is the item currently in production). Pure (no GPU, no sim read).
pub fn queue_rows(queue: &[(UnitKind, u16)]) -> Vec<QueueRow> {
    queue
        .iter()
        .map(|&(kind, ticks_remaining)| QueueRow {
            kind,
            label: kind_label(kind),
            ticks_remaining,
            seconds_remaining: eta_seconds(ticks_remaining),
        })
        .collect()
}

/// One laid-out training-panel label ready for the W4 [`text`](crate::text) pass — the same plain
/// shape as [`readout::ReadoutLabel`](crate::readout::ReadoutLabel). The host loops these into
/// [`text::TextRenderer::queue`](crate::text::TextRenderer::queue) calls.
#[derive(Clone, PartialEq, Debug)]
pub struct TrainPanelLabel {
    pub text: String,
    pub pos: [f32; 2],
    pub px_size: f32,
    pub anchor: Anchor,
    pub color: [f32; 3],
    pub alpha: f32,
}

/// Label glyph height in NDC (matches [`readout`](crate::readout)'s label size).
const LABEL_SIZE: f32 = 0.05;
/// Inset from the top edge for the panel stack, in NDC.
const TOP_MARGIN: f32 = 0.04;
/// Left edge of the panel, in NDC. Placed on the RIGHT half so it does not collide with the
/// top-left [`readout`](crate::readout) tally stack.
const PANEL_LEFT: f32 = 0.40;
/// Vertical step between stacked rows, in NDC (a touch more than the glyph height).
const LINE_STEP: f32 = 0.075;

/// Section-header color (neutral white).
const HEADER_COLOR: [f32; 3] = [0.85, 0.85, 0.9];
/// An affordable training option (bright).
const AFFORDABLE_COLOR: [f32; 3] = [0.78, 0.92, 0.78];
/// An unaffordable training option (dim red — the cost is out of reach).
const UNAFFORDABLE_COLOR: [f32; 3] = [0.85, 0.45, 0.42];
/// Queue-row color (neutral, slightly dim — informational).
const QUEUE_COLOR: [f32; 3] = [0.72, 0.78, 0.88];

/// Lay out the full troop-training panel as a stack of [`TrainPanelLabel`]s, hugging the top of the
/// screen's right half. Pure (no GPU, no sim read) — the testable layout seam. The stack is:
///
/// - `TRAIN` — section header (neutral).
/// - one row per trainable unit: `"<unit>  <cost>  <eta>s"` — affordable rows bright, unaffordable
///   rows dim-red (so the purse reads at a glance). Cost from [`economy::unit_cost`], ETA from
///   [`economy::prod_time`] at `camp_level`, converted to seconds at [`TICK_HZ`].
/// - `QUEUE` — section header (neutral), only when the queue is non-empty.
/// - one row per queued item: `"<unit>  <secs>s"` — the live countdown the host plumbs in.
///
/// All positions are NDC ([-1,1], +y up) anchored [`Anchor::TopLeft`]. Screen-space chrome only
/// (invariant #6) — no world position.
pub fn train_panel_labels(
    trainable: &[UnitKind],
    camp_level: u8,
    resources: i64,
    queue: &[(UnitKind, u16)],
) -> Vec<TrainPanelLabel> {
    let top = 1.0 - TOP_MARGIN;
    let mut out = Vec::new();
    let mut row = 0;
    let mut push = |text: String, color: [f32; 3], row: &mut i32| {
        out.push(TrainPanelLabel {
            text,
            pos: [PANEL_LEFT, top - (*row as f32) * LINE_STEP],
            px_size: LABEL_SIZE,
            anchor: Anchor::TopLeft,
            color,
            alpha: 1.0,
        });
        *row += 1;
    };

    push("TRAIN".to_string(), HEADER_COLOR, &mut row);
    for opt in train_options(trainable, camp_level, resources) {
        let color = if opt.affordable {
            AFFORDABLE_COLOR
        } else {
            UNAFFORDABLE_COLOR
        };
        push(
            format!("{}  {}  {:.1}s", opt.label, opt.cost, opt.eta_seconds),
            color,
            &mut row,
        );
    }

    let rows = queue_rows(queue);
    if !rows.is_empty() {
        push("QUEUE".to_string(), HEADER_COLOR, &mut row);
        for q in rows {
            push(
                format!("{}  {:.1}s", q.label, q.seconds_remaining),
                QUEUE_COLOR,
                &mut row,
            );
        }
    }

    out
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

    // ---- queue_rows ----

    #[test]
    fn queue_rows_convert_remaining_ticks_to_seconds() {
        let q = [(UnitKind::Rifleman, 150u16), (UnitKind::Heavy, 660)];
        let rows = queue_rows(&q);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, UnitKind::Rifleman);
        assert_eq!(rows[0].label, "Rifleman");
        assert_eq!(rows[0].ticks_remaining, 150);
        assert_eq!(rows[0].seconds_remaining, 2.5, "150 ticks / 60 = 2.5s");
        assert_eq!(rows[1].seconds_remaining, 11.0);
    }

    #[test]
    fn empty_queue_yields_no_rows() {
        assert!(queue_rows(&[]).is_empty());
    }

    // ---- train_panel_labels ----

    #[test]
    fn panel_has_train_header_and_one_row_per_option() {
        let labels = train_panel_labels(&ALL, 0, 1_000, &[]);
        // Header + 2 options, no queue section (empty queue).
        assert_eq!(labels.len(), 3);
        assert_eq!(labels[0].text, "TRAIN");
        assert!(labels[1].text.starts_with("Rifleman"));
        assert!(labels[2].text.starts_with("Heavy"));
        // Each option row shows its cost and ETA seconds.
        assert!(labels[1].text.contains(&RIFLEMAN_COST.to_string()));
        assert!(labels[1].text.contains("5.0s"));
        assert!(labels[2].text.contains(&HEAVY_COST.to_string()));
        assert!(labels[2].text.contains("11.0s"));
    }

    #[test]
    fn unaffordable_rows_are_dimmed_distinctly_from_affordable_ones() {
        // Purse buys a rifleman but not a heavy → the two rows must color differently.
        let labels = train_panel_labels(&ALL, 0, RIFLEMAN_COST, &[]);
        assert_eq!(labels[1].color, AFFORDABLE_COLOR, "rifleman affordable");
        assert_eq!(labels[2].color, UNAFFORDABLE_COLOR, "heavy out of reach");
        assert_ne!(labels[1].color, labels[2].color);
    }

    #[test]
    fn queue_section_appears_only_with_a_non_empty_queue() {
        let none = train_panel_labels(&ALL, 0, 1_000, &[]);
        assert!(
            !none.iter().any(|l| l.text == "QUEUE"),
            "no QUEUE header for an empty queue"
        );

        let q = [(UnitKind::Rifleman, 150u16), (UnitKind::Heavy, 660)];
        let with = train_panel_labels(&ALL, 0, 1_000, &q);
        let queue_hdr = with.iter().position(|l| l.text == "QUEUE");
        assert!(queue_hdr.is_some(), "QUEUE header present");
        let h = queue_hdr.unwrap();
        // The header is followed by one row per queued item, in order, with its countdown.
        assert!(with[h + 1].text.starts_with("Rifleman"));
        assert!(with[h + 1].text.contains("2.5s"));
        assert!(with[h + 2].text.starts_with("Heavy"));
        assert!(with[h + 2].text.contains("11.0s"));
    }

    #[test]
    fn labels_stack_down_the_top_right_and_stay_in_ndc() {
        let q = [(UnitKind::Rifleman, 150u16)];
        let labels = train_panel_labels(&ALL, 0, 1_000, &q);
        for w in labels.windows(2) {
            assert_eq!(w[0].pos[0], w[1].pos[0], "same left x");
            assert!(w[1].pos[1] < w[0].pos[1], "next line is lower");
            assert_eq!(w[0].anchor, Anchor::TopLeft);
        }
        // Fairness guard (invariant #6): every label is NDC chrome, never a world position. The
        // panel sits in the right half (left edge > 0) so it clears the left-corner readout.
        for l in &labels {
            assert!(l.pos[0] >= -1.0 && l.pos[0] <= 1.0, "x in NDC");
            assert!(l.pos[1] >= -1.0 && l.pos[1] <= 1.0, "y in NDC");
            assert!(l.px_size > 0.0 && l.alpha > 0.0);
        }
        assert!(labels[0].pos[0] > 0.0, "panel hugs the right half");
        assert!(labels[0].pos[1] > 0.0 && labels[0].pos[1] < 1.0, "near the top");
    }
}
