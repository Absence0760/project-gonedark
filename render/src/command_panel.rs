//! Contextual command panel (command view) — a boxed top-right panel whose contents change with the
//! current selection (CoH-style): select a camp and it shows that camp's train / upgrade / resources;
//! select troops and it shows their composition + stance; select nothing and it shows the build
//! palette + economy. The host derives *which* context and the exact rows from the (checksummed) sim
//! + selection, then hands this presentation description in; this module is pure layout.
//!
//! It composites in two passes the host drives: the **box** (a background + rim quad, reusing the
//! shared [`crate::overlay`] quad pipeline via [`crate::overlay::OverlayRenderer::draw_quads`]) and
//! the **text** (title + rows through [`crate::text`]). Screen-space chrome, command-view-only — it
//! carries no world position and reveals no intel the command frame doesn't already show (invariant
//! #6). All geometry is NDC (`[-1, 1]`, `+y` up); the box auto-sizes to the row count so a short
//! troops summary and a full camp panel both read cleanly.

use crate::overlay::{OverlayQuad, QuadRole};
use crate::text::Anchor;

// --- box geometry (NDC) ---------------------------------------------------------------------------
/// Right edge of the panel (a small margin in from the screen edge).
const RIGHT: f32 = 0.97;
/// Top edge of the panel.
const TOP: f32 = 0.93;
/// Panel half-width; the box spans `[RIGHT - 2·HALF_W, RIGHT]`.
const HALF_W: f32 = 0.27;
/// Inner padding between the box edge and its content.
const PAD: f32 = 0.022;
/// Title text height.
const TITLE_SIZE: f32 = 0.050;
/// Body row text height.
const ROW_SIZE: f32 = 0.040;
/// Vertical step between body row tops.
const ROW_STEP: f32 = 0.058;
/// Gap between the title and the first body row.
const TITLE_GAP: f32 = 0.028;
/// The rim quad extends this far past the panel on each side to draw a thin border.
const RIM_PAD: f32 = 0.010;

const BG_COLOR: [f32; 3] = [0.05, 0.06, 0.09];
const BG_ALPHA: f32 = 0.84;
const RIM_COLOR: [f32; 3] = [0.16, 0.18, 0.24];
const RIM_ALPHA: f32 = 0.92;

/// How a body row reads — drives its tint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineStyle {
    /// A section heading inside the panel ("TRAIN", "UPGRADE", "BUILD").
    Header,
    /// Ordinary informational text.
    Normal,
    /// De-emphasised / unavailable text.
    Dim,
    /// A positive / affordable value (green).
    Good,
    /// A negative / unaffordable value (red).
    Bad,
}

impl LineStyle {
    fn color(self) -> [f32; 3] {
        match self {
            LineStyle::Header => [0.80, 0.86, 1.0],
            LineStyle::Normal => [0.82, 0.84, 0.90],
            LineStyle::Dim => [0.50, 0.50, 0.56],
            LineStyle::Good => [0.55, 0.85, 0.50],
            LineStyle::Bad => [0.88, 0.48, 0.42],
        }
    }
}

/// One body row of the panel: its text and how it reads.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelLine {
    pub text: String,
    pub style: LineStyle,
}

impl PanelLine {
    pub fn new(text: impl Into<String>, style: LineStyle) -> Self {
        PanelLine {
            text: text.into(),
            style,
        }
    }
}

/// The presentation description of the contextual command panel: a title + body rows. The host
/// rebuilds it each command frame from the selection + sim. Empty (`title` empty AND no `lines`) ⇒
/// nothing is drawn.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandPanelView {
    pub title: String,
    pub lines: Vec<PanelLine>,
}

impl CommandPanelView {
    /// True when there is nothing worth drawing (no title and no rows).
    pub fn is_empty(&self) -> bool {
        self.title.is_empty() && self.lines.is_empty()
    }
}

/// One laid-out label for the text pass: text + NDC placement + tint.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelLabel {
    pub text: String,
    pub pos: [f32; 2],
    pub px_size: f32,
    pub anchor: Anchor,
    pub color: [f32; 3],
    pub alpha: f32,
}

/// The box center + half-extents (NDC) for `n_lines` body rows, plus the inner-left `x` text starts
/// from and the inner-top `y` the title starts at. Shared by [`command_panel_quads`] and
/// [`command_panel_labels`] so the box and its text always agree.
fn box_geom(n_lines: usize) -> (f32, f32, f32, f32, f32, f32) {
    let inner_h = TITLE_SIZE + TITLE_GAP + n_lines as f32 * ROW_STEP;
    let hh = (inner_h + 2.0 * PAD) * 0.5;
    let cx = RIGHT - HALF_W;
    let cy = TOP - hh;
    let left = RIGHT - 2.0 * HALF_W + PAD;
    let top_inner = TOP - PAD;
    (cx, cy, HALF_W, hh, left, top_inner)
}

/// The panel's background + rim quads (drawn through the overlay quad pipeline), auto-sized to the
/// view's row count. Empty view ⇒ no quads. Pure + GPU-free → unit-tested.
pub fn command_panel_quads(view: &CommandPanelView) -> Vec<OverlayQuad> {
    if view.is_empty() {
        return Vec::new();
    }
    let (cx, cy, hw, hh, _, _) = box_geom(view.lines.len());
    vec![
        // Rim first (behind), then the panel fill on top — a crisp border.
        OverlayQuad {
            cx,
            cy,
            hw: hw + RIM_PAD,
            hh: hh + RIM_PAD,
            r: RIM_COLOR[0],
            g: RIM_COLOR[1],
            b: RIM_COLOR[2],
            alpha: RIM_ALPHA,
            role: QuadRole::PanelRim,
        },
        OverlayQuad {
            cx,
            cy,
            hw,
            hh,
            r: BG_COLOR[0],
            g: BG_COLOR[1],
            b: BG_COLOR[2],
            alpha: BG_ALPHA,
            role: QuadRole::Panel,
        },
    ]
}

/// The panel's text labels — the title, then one label per body row, left-aligned and stacked from
/// the inner top. Empty view ⇒ no labels. Pure + GPU-free → unit-tested.
pub fn command_panel_labels(view: &CommandPanelView) -> Vec<PanelLabel> {
    if view.is_empty() {
        return Vec::new();
    }
    let (_, _, _, _, left, top_inner) = box_geom(view.lines.len());
    let mut out = Vec::with_capacity(view.lines.len() + 1);
    out.push(PanelLabel {
        text: view.title.clone(),
        pos: [left, top_inner],
        px_size: TITLE_SIZE,
        anchor: Anchor::TopLeft,
        color: LineStyle::Header.color(),
        alpha: 1.0,
    });
    let rows_top = top_inner - TITLE_SIZE - TITLE_GAP;
    for (i, line) in view.lines.iter().enumerate() {
        out.push(PanelLabel {
            text: line.text.clone(),
            pos: [left, rows_top - i as f32 * ROW_STEP],
            px_size: ROW_SIZE,
            anchor: Anchor::TopLeft,
            color: line.style.color(),
            alpha: 1.0,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(title: &str, lines: &[(&str, LineStyle)]) -> CommandPanelView {
        CommandPanelView {
            title: title.to_string(),
            lines: lines
                .iter()
                .map(|&(t, s)| PanelLine::new(t, s))
                .collect(),
        }
    }

    #[test]
    fn empty_view_draws_nothing() {
        let v = CommandPanelView::default();
        assert!(v.is_empty());
        assert!(command_panel_quads(&v).is_empty());
        assert!(command_panel_labels(&v).is_empty());
    }

    #[test]
    fn quads_are_a_rim_behind_a_fill_in_the_top_right() {
        let v = view("CAMP — TIER 1", &[("Resources 300", LineStyle::Normal)]);
        let q = command_panel_quads(&v);
        assert_eq!(q.len(), 2, "rim + fill");
        let (rim, fill) = (&q[0], &q[1]);
        assert!(rim.hw > fill.hw && rim.hh > fill.hh, "rim is larger than the fill");
        // Box hugs the top-right corner.
        assert!((fill.cx + fill.hw - RIGHT).abs() < 1e-6, "right edge at RIGHT");
        assert!((fill.cy + fill.hh - TOP).abs() < 1e-6, "top edge at TOP");
        assert!(fill.cx > 0.0 && fill.cy > 0.0, "sits in the top-right quadrant");
    }

    #[test]
    fn taller_content_grows_the_box_downward() {
        let short = command_panel_quads(&view("T", &[("a", LineStyle::Normal)]));
        let tall = command_panel_quads(&view(
            "T",
            &[
                ("a", LineStyle::Normal),
                ("b", LineStyle::Normal),
                ("c", LineStyle::Normal),
                ("d", LineStyle::Normal),
            ],
        ));
        assert!(tall[1].hh > short[1].hh, "more rows → taller box");
        // Both keep their top edge pinned at TOP (they grow downward).
        assert!((short[1].cy + short[1].hh - TOP).abs() < 1e-6);
        assert!((tall[1].cy + tall[1].hh - TOP).abs() < 1e-6);
    }

    #[test]
    fn labels_are_title_then_rows_top_down_with_style_colors() {
        let v = view(
            "SELECTED — 3",
            &[
                ("2x Rifleman", LineStyle::Normal),
                ("1x Tank", LineStyle::Normal),
                ("Stance: Hold", LineStyle::Dim),
            ],
        );
        let ls = command_panel_labels(&v);
        assert_eq!(ls.len(), 4, "title + 3 rows");
        assert_eq!(ls[0].text, "SELECTED — 3");
        assert_eq!(ls[0].color, LineStyle::Header.color(), "title reads as a header");
        assert_eq!(ls[3].color, LineStyle::Dim.color(), "dim row is dimmed");
        // Rows stack downward and sit below the title.
        assert!(ls[1].pos[1] < ls[0].pos[1]);
        assert!(ls[2].pos[1] < ls[1].pos[1]);
        assert!(ls[3].pos[1] < ls[2].pos[1]);
        // Everything shares the same left edge.
        assert!(ls.iter().all(|l| (l.pos[0] - ls[0].pos[0]).abs() < 1e-6));
    }

    #[test]
    fn good_and_bad_rows_take_distinct_colors() {
        let v = view(
            "CAMP",
            &[("Upgrade 200", LineStyle::Good), ("Upgrade 200", LineStyle::Bad)],
        );
        let ls = command_panel_labels(&v);
        assert_eq!(ls[1].color, LineStyle::Good.color());
        assert_eq!(ls[2].color, LineStyle::Bad.color());
        assert_ne!(ls[1].color, ls[2].color);
    }
}
