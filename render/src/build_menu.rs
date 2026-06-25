//! Command-view **build palette** (roadmap Phase 2 — "place/queue structures from the command
//! view"). The top-down view could grow camps in the sim but offered no on-screen way to start one;
//! this module lays out the palette of placeable structures — for each one a label, its cost, and an
//! **affordability flag** so unaffordable entries can be greyed — as screen-space chrome drawn via
//! the W4 [`text`](crate::text) pass.
//!
//! ## Why a render-side layout (and the seam reasoning)
//!
//! This mirrors [`readout`](crate::readout): it is a pure layout function over plain inputs, not a
//! sim read. The renderer is the float boundary and **never calls back into `core`** for live state.
//! The only `core` touch here is the *const* cost table ([`economy::build_cost`] /
//! [`economy::CAMP_BUILD_COST`]) — compile-time constants, not a per-frame sim read — so the palette
//! shows the true, single-source-of-truth cost without coupling the renderer to the sim economy.
//!
//! Affordability is computed from a **host-supplied plain number** (the player's current resource
//! balance), exactly the placeholder-seam pattern `readout`'s resource line uses: a host that plumbs
//! the balance into the render call gets greyed-out unaffordable entries for free; the layout itself
//! reaches into nothing. The *authoritative* spend check still lives in the deterministic sim
//! (`economy::build` refuses a build it can't afford) — this flag is presentation only.
//!
//! ## Fairness (invariant #6)
//!
//! The entries are screen-space NDC chrome (the W4 text pass), carry no world position, and belong
//! only to the command view — never the dark embodied frame. They surface the player's own cost /
//! affordability, never any opponent intel.
//!
//! ## The pure seam
//!
//! [`build_menu_entries`] is a free fn, unit-testable without a GPU — the `readout_labels` pattern.
//! The host turns each [`BuildMenuEntry`] into a [`text::TextRenderer::queue`] call (and, when it
//! wires input, the entry index doubles as the `build_ui` slot the player picked).

use crate::text::Anchor;
use gonedark_core::components::BuildingKind;
use gonedark_core::economy;

/// One laid-out build-palette entry ready to hand to the W4 [`text`](crate::text) pass: the label
/// string, the structure's cost, whether the player can currently afford it, plus the NDC anchor +
/// color the host draws it with. Pure data — the host loops these into
/// [`crate::text::TextRenderer::queue`] calls. The entry's position in the returned `Vec` is also
/// its build-palette **slot** (index 0 = the first structure), matching `engine::build_ui`'s slot
/// table, so a host wiring input can map a tapped entry straight to that slot.
#[derive(Clone, PartialEq, Debug)]
pub struct BuildMenuEntry {
    /// Display label, e.g. `"Camp  250"` (structure name + cost).
    pub text: String,
    /// The structure this entry places (the kind the matching `build_ui` slot would build).
    pub kind: BuildingKind,
    /// Build cost from the `core` const table — the single source of truth the sim also spends.
    pub cost: i64,
    /// Whether the host-supplied resource balance covers [`cost`](Self::cost). Presentation only —
    /// the sim still does the authoritative spend check; this just lets the host grey the entry.
    pub affordable: bool,
    /// NDC position ([-1,1], +y up), anchored by [`anchor`](Self::anchor).
    pub pos: [f32; 2],
    pub px_size: f32,
    pub anchor: Anchor,
    /// Draw color — the affordable tint when the player can afford it, a dimmed grey when not.
    pub color: [f32; 3],
    pub alpha: f32,
}

/// Label glyph height in NDC (matches `readout`'s label size).
const LABEL_SIZE: f32 = 0.05;
/// Inset from the screen edge for the bottom-left palette stack, in NDC.
const MARGIN: f32 = 0.04;
/// Vertical step between stacked palette lines, in NDC (a touch more than the glyph height).
const LINE_STEP: f32 = 0.075;

/// Color for an affordable entry (a readable off-white, like `readout`'s neutral label).
const AFFORDABLE_COLOR: [f32; 3] = [0.85, 0.85, 0.9];
/// Dimmed grey for an entry the player can't currently afford (greyed out).
const UNAFFORDABLE_COLOR: [f32; 3] = [0.45, 0.45, 0.48];

/// The placeable structures, in palette/slot order. Index = the `engine::build_ui` slot. Currently
/// just the Camp (the only [`BuildingKind`]); adding a structure here adds a palette entry, and the
/// matching `build_ui::slot_kind` arm makes it placeable.
const PALETTE: [(&str, BuildingKind); 1] = [("Camp", BuildingKind::Camp)];

/// Lay out the command-view build palette from a host-supplied current resource balance. Pure (no
/// GPU, no sim) — the testable layout seam. One [`BuildMenuEntry`] per placeable structure, stacked
/// up the **bottom-left** corner (last structure hugs the bottom margin), each labelled `"<Name>
/// <cost>"`, with [`affordable`](BuildMenuEntry::affordable) = `resources >= cost` and a dimmed
/// color when not.
///
/// `resources` is a plain number (the player's current balance) supplied by the host — the renderer
/// has no sim economy read of its own (see the module docs / `readout`'s seam). All positions are
/// NDC anchored [`Anchor::TopLeft`]; screen-space chrome only (invariant #6) — no world position.
pub fn build_menu_entries(resources: i64) -> Vec<BuildMenuEntry> {
    let left = -1.0 + MARGIN;
    let bottom = -1.0 + MARGIN;
    let total = PALETTE.len();
    PALETTE
        .iter()
        .enumerate()
        .map(|(i, &(name, kind))| {
            let cost = economy::build_cost(kind);
            let affordable = resources >= cost;
            // Stack up from the bottom margin: the last entry sits at `bottom`, earlier ones above
            // it, so the palette is pinned to the bottom-left corner regardless of how many there
            // are.
            let y = bottom + ((total - 1 - i) as f32) * LINE_STEP;
            BuildMenuEntry {
                text: format!("{name}  {cost}"),
                kind,
                cost,
                affordable,
                pos: [left, y],
                px_size: LABEL_SIZE,
                anchor: Anchor::TopLeft,
                color: if affordable {
                    AFFORDABLE_COLOR
                } else {
                    UNAFFORDABLE_COLOR
                },
                alpha: 1.0,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary, so f32 layout math is fair game. The pure layout seam is
    //! tested here without a GPU.

    use super::*;
    use gonedark_core::economy::CAMP_BUILD_COST;

    #[test]
    fn lists_every_placeable_structure_with_its_const_cost() {
        let entries = build_menu_entries(10_000);
        assert_eq!(entries.len(), PALETTE.len(), "one entry per placeable structure");
        // Camp is the first (and only) palette slot, carrying the core const cost.
        assert_eq!(entries[0].kind, BuildingKind::Camp);
        assert_eq!(entries[0].cost, CAMP_BUILD_COST, "cost from the core const table");
        assert!(entries[0].text.starts_with("Camp"), "label names the structure");
        assert!(
            entries[0].text.contains(&CAMP_BUILD_COST.to_string()),
            "label shows the cost"
        );
    }

    #[test]
    fn affordability_flag_tracks_the_supplied_balance() {
        // Exactly affordable (balance == cost), comfortably affordable, and too poor.
        let exact = build_menu_entries(CAMP_BUILD_COST);
        assert!(exact[0].affordable, "balance == cost is affordable (no debt needed)");

        let rich = build_menu_entries(CAMP_BUILD_COST + 1);
        assert!(rich[0].affordable);

        let poor = build_menu_entries(CAMP_BUILD_COST - 1);
        assert!(!poor[0].affordable, "one short → not affordable");
    }

    #[test]
    fn unaffordable_entries_are_greyed() {
        let poor = build_menu_entries(0);
        assert_eq!(poor[0].color, UNAFFORDABLE_COLOR, "can't afford → dimmed");
        let rich = build_menu_entries(CAMP_BUILD_COST);
        assert_eq!(rich[0].color, AFFORDABLE_COLOR, "can afford → normal tint");
        // The dimmed color is strictly darker so it reads as greyed-out.
        for c in 0..3 {
            assert!(
                UNAFFORDABLE_COLOR[c] < AFFORDABLE_COLOR[c],
                "unaffordable channel {c} should be dimmer"
            );
        }
    }

    #[test]
    fn entries_stack_up_the_bottom_left_corner() {
        // With a single structure there's nothing to compare, so assert the corner placement and
        // the NDC bounds that hold for any count.
        let entries = build_menu_entries(500);
        for w in entries.windows(2) {
            // Same left x; each later entry steps DOWN (toward the bottom margin).
            assert_eq!(w[0].pos[0], w[1].pos[0], "same left x");
            assert!(w[1].pos[1] < w[0].pos[1], "later entry is lower");
            assert_eq!(w[0].anchor, Anchor::TopLeft);
        }
        // The last entry hugs the bottom-left corner (inside the screen).
        let last = entries.last().unwrap();
        assert!(last.pos[0] < 0.0 && last.pos[0] > -1.0, "left side, on screen");
        assert!(last.pos[1] < 0.0 && last.pos[1] > -1.0, "bottom region, on screen");
    }

    #[test]
    fn entries_are_screen_space_chrome() {
        // Fairness guard (invariant #6): every entry is NDC chrome, never a world position.
        for e in build_menu_entries(250) {
            assert!(e.pos[0] >= -1.0 && e.pos[0] <= 1.0, "x in NDC");
            assert!(e.pos[1] >= -1.0 && e.pos[1] <= 1.0, "y in NDC");
            assert!(e.px_size > 0.0 && e.alpha > 0.0);
        }
    }

    #[test]
    fn slot_index_matches_build_ui_palette_order() {
        // The entry index is the build_ui slot: index 0 must be the Camp (build_ui slot 0 = Camp).
        let entries = build_menu_entries(0);
        assert_eq!(entries[0].kind, BuildingKind::Camp, "slot 0 is the Camp");
    }
}
