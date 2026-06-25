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
//! ## Pure layout seam (the `readout` / `tiers` pattern)
//!
//! [`upgrade_view`] (derive the numbers) and [`upgrade_labels`] (lay out label strings + NDC
//! anchors) are free fns, unit-testable without a GPU — exactly like [`crate::readout`]. The host
//! turns each [`UpgradeLabel`] into a [`crate::text::TextRenderer::queue`] call. `render` is the
//! float boundary (invariant #1), so the f32 NDC layout math here is fair game; this module never
//! reads or mutates sim state (invariant #4) — it is a pure function of two host-supplied numbers
//! (the camp's level and the faction's resource purse).
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
//! grows from one "next tier" row to a list of branch rows (id, cost, prereq-met, effect), reusing
//! the exact same [`UpgradeLabel`] layout primitive.

use crate::text::Anchor;
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

/// One laid-out upgrade-panel label ready for the [`text`](crate::text) pass: the string, its NDC
/// anchor position + [`Anchor`], a size, and a color. Pure data — the host loops these into
/// [`crate::text::TextRenderer::queue`] calls. Mirrors [`crate::readout::ReadoutLabel`].
#[derive(Clone, PartialEq, Debug)]
pub struct UpgradeLabel {
    pub text: String,
    pub pos: [f32; 2],
    pub px_size: f32,
    pub anchor: Anchor,
    pub color: [f32; 3],
    pub alpha: f32,
}

/// Label glyph height in NDC (cf. `text`/`readout`: practical label sizes ~0.03–0.08).
const LABEL_SIZE: f32 = 0.045;
/// Inset from the screen edge for the bottom-right panel stack, in NDC.
const MARGIN: f32 = 0.04;
/// Vertical step between stacked panel lines, in NDC.
const LINE_STEP: f32 = 0.065;

/// Title / heading color (neutral, reads as a panel header).
const TITLE_COLOR: [f32; 3] = [0.9, 0.9, 0.95];
/// Body line color (effect/description text).
const BODY_COLOR: [f32; 3] = [0.82, 0.82, 0.88];
/// Cost color when the next tier is affordable (player-blue/green-leaning "go").
const AFFORDABLE_COLOR: [f32; 3] = [0.55, 0.92, 0.62];
/// Cost color when the next tier is NOT affordable (red-leaning "can't yet").
const UNAFFORDABLE_COLOR: [f32; 3] = [1.0, 0.55, 0.48];

/// Lay out the selected camp's upgrade panel from an [`UpgradeView`]. Pure (no GPU, no sim) — the
/// testable layout seam. The lines stack **up** the bottom-right corner so the panel reads as a
/// grouped block anchored to that corner (newest/topmost line is the title):
///
/// - `CAMP — TIER <n>`            — the current tier (title, neutral).
/// - `NEXT TIER: <cost>`          — the next-tier cost from [`economy::upgrade_cost`], colored by
///   affordability (green = affordable, red = too poor).
/// - `+ PRODUCTION SPEED`         — the truthful effect line: faster unit production (the only thing a
///   camp tier improves today), with the per-unit ticks saved; OR `MAX TIER (no speedup)` once the
///   camp has hit the [`economy::PROD_TICKS_FLOOR`] and the next tier buys no further speed.
///
/// All positions are NDC ([-1,1], +y up) anchored [`Anchor::TopLeft`] (so each line's box grows
/// right/down from a known point); they sit in the bottom-right region. Screen-space chrome only
/// (invariant #6) — no world position, command-view-only.
pub fn upgrade_labels(view: &UpgradeView) -> Vec<UpgradeLabel> {
    // Bottom-right anchor. We lay 3 lines from a top y that leaves room to stack downward while
    // hugging the bottom edge, left-aligned at a right-side x.
    let right_x = 0.45; // left edge of the (right-side) panel column, in NDC
    let bottom = -1.0 + MARGIN; // bottom edge, inset
    let n_lines = 3;
    let top = bottom + (n_lines as f32 - 1.0) * LINE_STEP; // top line y so the last sits at `bottom`

    let mut out = Vec::with_capacity(n_lines as usize);
    let mut row = 0;
    let mut push = |text: String, color: [f32; 3], size: f32, row: &mut i32| {
        out.push(UpgradeLabel {
            text,
            pos: [right_x, top - (*row as f32) * LINE_STEP],
            px_size: size,
            anchor: Anchor::TopLeft,
            color,
            alpha: 1.0,
        });
        *row += 1;
    };

    // Title: current tier.
    push(
        format!("CAMP — TIER {}", view.level),
        TITLE_COLOR,
        LABEL_SIZE,
        &mut row,
    );

    // Next-tier cost, colored by affordability.
    let cost_color = if view.affordable {
        AFFORDABLE_COLOR
    } else {
        UNAFFORDABLE_COLOR
    };
    push(
        format!("NEXT TIER: {}", view.next_cost),
        cost_color,
        LABEL_SIZE,
        &mut row,
    );

    // Truthful effect line: what the next tier actually buys.
    let effect = if view.next_tier_improves_speed() {
        format!("+ PRODUCTION SPEED (-{} ticks/unit)", view.prod_ticks_saved())
    } else {
        "MAX TIER (no further speedup)".to_string()
    };
    push(effect, BODY_COLOR, LABEL_SIZE * 0.9, &mut row);

    out
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

    // ---- upgrade_labels ----

    #[test]
    fn labels_report_tier_cost_and_effect() {
        let v = upgrade_view(2, 10_000);
        let labels = upgrade_labels(&v);
        assert_eq!(labels.len(), 3, "title + cost + effect");
        assert!(labels[0].text.starts_with("CAMP"), "title line");
        assert!(labels[0].text.contains('2'), "shows current tier 2");
        assert!(labels[1].text.starts_with("NEXT TIER"), "cost line");
        assert!(
            labels[1].text.contains(&economy::upgrade_cost(2).to_string()),
            "cost line shows upgrade_cost(2) = {}",
            economy::upgrade_cost(2)
        );
        assert!(
            labels[2].text.contains("PRODUCTION"),
            "effect line names the truthful benefit (faster production)"
        );
        assert!(
            labels[2].text.contains(&economy::LEVEL_PROD_SPEEDUP.to_string()),
            "effect line quantifies the ticks saved"
        );
    }

    #[test]
    fn cost_line_is_green_when_affordable_red_when_not() {
        let cost = economy::upgrade_cost(0);
        let rich = upgrade_labels(&upgrade_view(0, cost));
        let poor = upgrade_labels(&upgrade_view(0, cost - 1));
        // Affordable → green-leaning (green channel dominates).
        assert!(
            rich[1].color[1] > rich[1].color[0] && rich[1].color[1] > rich[1].color[2],
            "affordable cost label leans green, got {:?}",
            rich[1].color
        );
        // Unaffordable → red-leaning (red channel dominates).
        assert!(
            poor[1].color[0] > poor[1].color[2],
            "unaffordable cost label leans red, got {:?}",
            poor[1].color
        );
    }

    #[test]
    fn maxed_camp_effect_line_says_no_further_speedup() {
        let labels = upgrade_labels(&upgrade_view(254, i64::MAX / 2));
        assert!(
            labels[2].text.contains("MAX TIER"),
            "at the floor the effect line is honest: no more speed, got {:?}",
            labels[2].text
        );
    }

    #[test]
    fn labels_stack_up_the_bottom_right_corner() {
        let labels = upgrade_labels(&upgrade_view(0, 0));
        for w in labels.windows(2) {
            // Same left x; each later line steps DOWN (smaller y).
            assert_eq!(w[0].pos[0], w[1].pos[0], "same left x");
            assert!(w[1].pos[1] < w[0].pos[1], "next line is lower");
            assert_eq!(w[0].anchor, Anchor::TopLeft);
        }
        // The block sits in the bottom-right region (right of center, below center).
        let last = labels.last().unwrap();
        assert!(labels[0].pos[0] > 0.0, "panel is on the right");
        assert!(last.pos[1] < 0.0, "panel hugs the bottom");
    }

    #[test]
    fn labels_are_screen_space_chrome() {
        // Fairness guard (invariant #6): every label is NDC chrome, never a world position.
        for v in [upgrade_view(0, 0), upgrade_view(9, 99_999), upgrade_view(254, 0)] {
            for l in upgrade_labels(&v) {
                assert!(l.pos[0] >= -1.0 && l.pos[0] <= 1.0, "x in NDC");
                assert!(l.pos[1] >= -1.0 && l.pos[1] <= 1.0, "y in NDC");
                assert!(l.px_size > 0.0 && l.alpha > 0.0);
            }
        }
    }
}
