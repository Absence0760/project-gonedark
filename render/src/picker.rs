//! Embody-unit picker (command view) — a small on-screen list of the currently-selected units so
//! the player chooses **which one to possess** instead of the engine silently taking the first
//! (tank embodiment follow-up). When a mixed troops-and-tanks band is selected, pressing embody
//! opens this list; a number key (`1`..`9`) or a tap on a row commits `Command::Embody` for that
//! unit. Tapping off the list, or pressing embody again, cancels.
//!
//! This is **screen-space chrome drawn entirely through the shared text pass** ([`crate::text`]) —
//! exactly like the build / train / upgrade panels — so it needs no GPU pipeline of its own. The
//! host owns the picker state (the live selected entities) and rebuilds this presentation
//! description each frame; `picker_row_at` is the matching hit-test the host runs against a tap. It
//! is command-view-only and carries no world position, so it reveals no map intel (invariant #6).
//!
//! All geometry is in NDC (`[-1, 1]`, `+y` up), matching the rest of the command-view text chrome.
//! `picker_labels` and `picker_row_at` derive every position from the SAME constants below, so a
//! drawn row and its hit band line up 1:1.

use crate::text::Anchor;

/// NDC `y` of the header line (drawn above the rows; not selectable).
const HEADER_Y: f32 = 0.40;
/// NDC `y` of the first (top) row's center.
const FIRST_ROW_Y: f32 = 0.28;
/// NDC vertical spacing between adjacent row centers — this is also the hit band's full height (a tap
/// lands on a row within ±`ROW_STEP/2` of its center). Sized so the tappable band clears the ~44 dp
/// touch-target floor on a portrait phone: `0.13` NDC ≈ `0.13·height/2` px (e.g. ~152 px ≈ 51 dp at
/// 3× on a 2340-tall panel). The old `0.10` gave a ~39 dp band, under the floor and easy to mis-tap.
const ROW_STEP: f32 = 0.13;
/// Half the horizontal extent (NDC) a tap may land from center and still hit a row.
const HALF_WIDTH: f32 = 0.45;
/// Text heights (NDC), in the same scale the build/train panels use (~0.05).
const HEADER_SIZE: f32 = 0.050;
const ROW_SIZE: f32 = 0.055;

/// Header tint — a neutral bright label.
const HEADER_COLOR: [f32; 3] = crate::theme::BONE;
/// An embodiable row — warm amber, echoing the possessed-avatar color so "this is what you'd
/// become" reads at a glance.
const ROW_COLOR: [f32; 3] = crate::theme::AVATAR;
/// A non-embodiable row — dimmed (reserved for future unit kinds that can't be possessed).
const ROW_DIM: [f32; 3] = crate::theme::MUTED;

/// The NDC center `y` of row `i` (0 = top). Shared by [`picker_labels`] and [`picker_row_at`].
#[inline]
fn row_center_y(i: usize) -> f32 {
    FIRST_ROW_Y - i as f32 * ROW_STEP
}

/// One selectable row: a label (e.g. `"Tank"`) and whether the unit can actually be possessed.
#[derive(Clone, Debug, PartialEq)]
pub struct PickerRow {
    pub label: String,
    /// `true` if pressing this row would embody the unit (every unit today; the flag lets a future
    /// non-embodiable kind render greyed without changing the hit-test).
    pub embodiable: bool,
}

/// The presentation description of the open picker: the rows to list, in selection order. The host
/// rebuilds this each frame from its live selected entities. Empty ⇒ nothing to draw.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmbodyPicker {
    pub rows: Vec<PickerRow>,
}

/// One laid-out label for the text pass: text + NDC placement + tint. Mirrors the build/train panel
/// label shape so the host queues it through the same [`crate::text::TextRenderer`].
#[derive(Clone, Debug, PartialEq)]
pub struct PickerLabel {
    pub text: String,
    pub pos: [f32; 2],
    pub px_size: f32,
    pub anchor: Anchor,
    pub color: [f32; 3],
    pub alpha: f32,
}

/// Lay the picker out as text labels: a header, then one numbered row per selected unit
/// (`"[1]  Tank"`, `"[2]  Rifleman"`, …). Embodiable rows read amber, others dimmed. Centered on
/// `x = 0`, stacked downward from [`FIRST_ROW_Y`]. Pure + GPU-free, so it is unit-tested without a
/// device. An empty picker yields no labels.
pub fn picker_labels(picker: &EmbodyPicker) -> Vec<PickerLabel> {
    if picker.rows.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(picker.rows.len() + 1);
    out.push(PickerLabel {
        text: "EMBODY WHICH UNIT?  (1-9 / tap)".to_string(),
        pos: [0.0, HEADER_Y],
        px_size: HEADER_SIZE,
        anchor: Anchor::Center,
        color: HEADER_COLOR,
        alpha: 1.0,
    });
    for (i, row) in picker.rows.iter().enumerate() {
        out.push(PickerLabel {
            text: format!("[{}]  {}", i + 1, row.label),
            pos: [0.0, row_center_y(i)],
            px_size: ROW_SIZE,
            anchor: Anchor::Center,
            color: if row.embodiable { ROW_COLOR } else { ROW_DIM },
            alpha: 1.0,
        });
    }
    out
}

/// Hit-test a tap (NDC) against the `row_count` drawn rows, returning the row index it lands on, or
/// `None` if it missed every row (the band between rows, or outside the list — the host reads `None`
/// as "cancel"). Geometry mirrors [`picker_labels`] exactly, so a tap on a visible row resolves to
/// that row. Pure + testable.
pub fn picker_row_at(row_count: usize, ndc_x: f32, ndc_y: f32) -> Option<usize> {
    if ndc_x.abs() > HALF_WIDTH {
        return None;
    }
    let half = ROW_STEP * 0.5;
    (0..row_count).find(|&i| (ndc_y - row_center_y(i)).abs() <= half)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker(labels: &[(&str, bool)]) -> EmbodyPicker {
        EmbodyPicker {
            rows: labels
                .iter()
                .map(|&(l, e)| PickerRow {
                    label: l.to_string(),
                    embodiable: e,
                })
                .collect(),
        }
    }

    #[test]
    fn labels_have_header_plus_numbered_rows() {
        let p = picker(&[("Tank", true), ("Rifleman", true)]);
        let ls = picker_labels(&p);
        assert_eq!(ls.len(), 3, "header + 2 rows");
        assert!(ls[0].text.starts_with("EMBODY"), "first label is the header");
        assert_eq!(ls[1].text, "[1]  Tank");
        assert_eq!(ls[2].text, "[2]  Rifleman");
        // Rows are centered on x = 0 and stack downward.
        assert_eq!(ls[1].pos[0], 0.0);
        assert!(ls[2].pos[1] < ls[1].pos[1], "row 2 sits below row 1");
    }

    #[test]
    fn embodiable_rows_are_amber_others_dimmed() {
        let p = picker(&[("Tank", true), ("Drone", false)]);
        let ls = picker_labels(&p);
        assert_eq!(ls[1].color, ROW_COLOR, "embodiable row is amber");
        assert_eq!(ls[2].color, ROW_DIM, "non-embodiable row is dimmed");
    }

    #[test]
    fn empty_picker_has_no_labels() {
        assert!(picker_labels(&EmbodyPicker::default()).is_empty());
    }

    #[test]
    fn row_hit_test_lands_on_drawn_rows() {
        // A tap on each row's center resolves to that row.
        assert_eq!(picker_row_at(3, 0.0, row_center_y(0)), Some(0));
        assert_eq!(picker_row_at(3, 0.0, row_center_y(1)), Some(1));
        assert_eq!(picker_row_at(3, 0.0, row_center_y(2)), Some(2));
        // A small horizontal offset within the panel still hits.
        assert_eq!(picker_row_at(3, 0.3, row_center_y(1)), Some(1));
    }

    #[test]
    fn row_band_clears_the_touch_target_floor() {
        // The tappable band is ROW_STEP tall in NDC; on a portrait phone the vertical axis spans the
        // full height, so band_px = ROW_STEP · height / 2. It must clear the ~44 dp touch floor at
        // common phone densities (the old 0.10 gave a ~39 dp band). Checked on representative panels.
        for (height_px, density) in [(2340.0_f32, 3.0_f32), (3200.0, 3.5)] {
            let band_px = ROW_STEP * height_px / 2.0;
            let band_dp = band_px / density;
            assert!(
                band_dp >= 44.0,
                "{height_px}px @{density}x: row band {band_dp} dp is below the 44 dp touch floor"
            );
        }
    }

    #[test]
    fn row_hit_test_misses_outside_and_between() {
        // Outside the horizontal extent → miss (cancel).
        assert_eq!(picker_row_at(3, 0.9, row_center_y(0)), None);
        // Far above the first row / below the last → miss.
        assert_eq!(picker_row_at(3, 0.0, FIRST_ROW_Y + ROW_STEP), None);
        assert_eq!(picker_row_at(3, 0.0, row_center_y(2) - ROW_STEP), None);
        // A row index beyond the count is never hit.
        assert_eq!(picker_row_at(2, 0.0, row_center_y(2)), None);
    }
}
