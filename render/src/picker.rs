//! Embody-unit picker (command view) — a small on-screen list of the currently-selected units so
//! the player chooses **which one to possess** instead of the engine silently taking the first
//! (tank embodiment follow-up). When a mixed troops-and-tanks band is selected, pressing embody
//! opens this list; a number key (`1`..`9`) or a tap on a row commits `Command::Embody` for that
//! unit. Tapping off the list, or pressing embody again, cancels.
//!
//! This is **screen-space chrome drawn through the shared overlay-quad + text passes** — exactly
//! like the build / train / upgrade panels (a PANEL fill + RIM backing card behind text rows), so it
//! needs no GPU pipeline of its own. The host owns the picker state (the live selected entities) and
//! rebuilds this presentation description each frame; `picker_row_at` is the matching hit-test the
//! host runs against a tap. It is command-view-only and carries no world position, so it reveals no
//! map intel (invariant #6).
//!
//! All geometry is in NDC (`[-1, 1]`, `+y` up), matching the rest of the command-view text chrome.
//! [`picker_labels`], [`picker_quads`], and [`picker_row_at`] derive every position from the SAME
//! constants below, so the drawn card, its rows, and their hit bands line up 1:1.
//!
//! ## `ui_scale` and the hit contract
//!
//! The host hit-tests taps with the (unscaled) [`picker_row_at`], so the header/row CENTERS are the
//! hit contract and never move with `ui_scale`. The glyphs themselves scale (the shared text pass
//! draws every label at `px * ui_scale`, `Anchor::Center` keeps them on their row centers), and only
//! the backing card ([`picker_quads_scaled`]) grows to wrap the scaled text — the same reconciliation
//! note `command_bar` carries.

use crate::overlay::{OverlayQuad, QuadRole};
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
/// Half the horizontal extent (NDC) a tap may land from center and still hit a row. Also the backing
/// card's content half-width, so the drawn card IS the tappable band (what you see is what you tap).
const HALF_WIDTH: f32 = 0.45;
/// Text heights — the shared type scale's section-title step (`theme`). The header is the panel's
/// title; the rows deliberately ride the SAME title step rather than `TYPE_BODY` — each row is a
/// primary touch target and must stay big (the old ad-hoc 0.050 / 0.055 mapped here).
const HEADER_SIZE: f32 = crate::theme::TYPE_TITLE;
const ROW_SIZE: f32 = crate::theme::TYPE_TITLE;

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

/// The picker's backing card — the same PANEL fill + RIM the command/train panels wear
/// ([`crate::theme`] panel spec), so the list reads as a designed card instead of text floating over
/// the world. Wraps the header and every row vertically; horizontally it spans the tappable band
/// ([`HALF_WIDTH`]) plus padding, so the visible card and the hit extent agree. Empty picker ⇒ no
/// quads. Pure + GPU-free → unit-tested.
pub fn picker_quads(picker: &EmbodyPicker) -> Vec<OverlayQuad> {
    picker_quads_scaled(picker, 1.0)
}

/// [`picker_quads`] with an explicit physical `ui_scale` (DPI/point-per-NDC correction). The header/
/// row CENTERS are the hit contract ([`picker_row_at`] is unscaled) and stay put; only the card's
/// glyph half-heights and paddings scale, so it keeps wrapping the text pass's `px * ui_scale`
/// glyphs. `ui_scale == 1.0` is byte-identical to [`picker_quads`].
pub fn picker_quads_scaled(picker: &EmbodyPicker, ui_scale: f32) -> Vec<OverlayQuad> {
    if picker.rows.is_empty() {
        return Vec::new();
    }
    let pad = crate::theme::PANEL_PAD * ui_scale;
    // Center-anchored lines: each glyph box extends half its (scaled) cell height either side.
    let top = HEADER_Y + HEADER_SIZE * 0.5 * ui_scale + pad;
    let bottom = row_center_y(picker.rows.len() - 1) - ROW_SIZE * 0.5 * ui_scale - pad;
    let cy = (top + bottom) * 0.5;
    let hh = (top - bottom) * 0.5;
    let hw = HALF_WIDTH + pad;
    vec![
        // Rim first (behind), then the panel fill on top — a crisp border (the panels' pattern).
        OverlayQuad {
            cx: 0.0,
            cy,
            hw: hw + crate::theme::PANEL_RIM_PAD * ui_scale,
            hh: hh + crate::theme::PANEL_RIM_PAD * ui_scale,
            r: crate::theme::RIM[0],
            g: crate::theme::RIM[1],
            b: crate::theme::RIM[2],
            alpha: crate::theme::PANEL_RIM_ALPHA,
            role: QuadRole::PanelRim,
        },
        OverlayQuad {
            cx: 0.0,
            cy,
            hw,
            hh,
            r: crate::theme::PANEL[0],
            g: crate::theme::PANEL[1],
            b: crate::theme::PANEL[2],
            alpha: crate::theme::PANEL_BG_ALPHA,
            role: QuadRole::Panel,
        },
    ]
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
        assert!(picker_quads(&EmbodyPicker::default()).is_empty(), "no card either");
    }

    #[test]
    fn text_rides_the_shared_type_scale() {
        // The header and rows sit on the theme's title step (no more ad-hoc 0.050 / 0.055) —
        // asserted on the laid-out labels so the wiring is covered.
        let ls = picker_labels(&picker(&[("Tank", true)]));
        assert_eq!(ls[0].px_size, crate::theme::TYPE_TITLE, "header on the type scale");
        assert_eq!(ls[1].px_size, crate::theme::TYPE_TITLE, "rows on the type scale");
    }

    #[test]
    fn card_is_a_rim_behind_a_panel_wrapping_every_line() {
        // The picker wears the same PANEL+RIM card the command/train panels do (theme panel spec),
        // wrapping the header and all rows, spanning the tappable band horizontally.
        let p = picker(&[("Tank", true), ("Rifleman", true), ("Drone", false)]);
        let q = picker_quads(&p);
        assert_eq!(q.len(), 2, "rim + fill");
        let (rim, fill) = (&q[0], &q[1]);
        assert_eq!(rim.role, QuadRole::PanelRim);
        assert_eq!(fill.role, QuadRole::Panel);
        assert!(rim.hw > fill.hw && rim.hh > fill.hh, "rim is larger than the fill");
        assert_eq!([fill.r, fill.g, fill.b], crate::theme::PANEL, "fill is theme::PANEL");
        assert_eq!([rim.r, rim.g, rim.b], crate::theme::RIM, "rim is theme::RIM");
        assert_eq!(fill.alpha, crate::theme::PANEL_BG_ALPHA);
        assert_eq!(rim.alpha, crate::theme::PANEL_RIM_ALPHA);
        // Every laid-out line (header + rows) sits inside the fill, and the fill spans the hit band.
        for l in picker_labels(&p) {
            assert!(l.pos[1] + l.px_size * 0.5 <= fill.cy + fill.hh + 1e-6, "line top inside");
            assert!(l.pos[1] - l.px_size * 0.5 >= fill.cy - fill.hh - 1e-6, "line bottom inside");
        }
        assert!(fill.hw >= HALF_WIDTH, "card spans the tappable band (see-what-you-tap)");
        // Centered on x = 0, like the rows.
        assert_eq!(fill.cx, 0.0);
    }

    #[test]
    fn more_rows_grow_the_card_downward() {
        let short = picker_quads(&picker(&[("Tank", true)]));
        let tall = picker_quads(&picker(&[("Tank", true), ("Rifleman", true)]));
        assert!(tall[1].hh > short[1].hh, "more rows → taller card");
        // The top edge stays put (the header is fixed); the card grows downward.
        assert!(
            ((short[1].cy + short[1].hh) - (tall[1].cy + tall[1].hh)).abs() < 1e-6,
            "top edge pinned under the header"
        );
    }

    #[test]
    fn ui_scale_grows_the_card_but_never_moves_the_hit_centers() {
        // The hit contract: `picker_row_at` is unscaled, so the row centers the labels draw at must
        // not move with ui_scale — only the card wraps the (text-pass-scaled) glyphs.
        let p = picker(&[("Tank", true), ("Rifleman", true)]);
        assert_eq!(picker_quads(&p), picker_quads_scaled(&p, 1.0), "1.0 is byte-identical");
        let base = picker_quads_scaled(&p, 1.0);
        let scaled = picker_quads_scaled(&p, 2.0);
        assert!(scaled[1].hh > base[1].hh && scaled[1].hw > base[1].hw, "card grows at 2x");
        // Labels (the hit-band centers) are ui_scale-independent by construction.
        for (i, _) in p.rows.iter().enumerate() {
            assert_eq!(picker_row_at(p.rows.len(), 0.0, row_center_y(i)), Some(i));
        }
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
