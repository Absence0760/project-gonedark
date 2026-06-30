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
/// Panel half-width bounds. The box auto-sizes its width to the widest row it must hold (see
/// [`box_geom`]) so text never spills past the right screen edge — a short troops summary stays
/// compact and a long build row still fits — then clamps into `[MIN_HALF_W, MAX_HALF_W]` so a tiny
/// panel still reads as a card and a very long row can't run off the left edge.
const MIN_HALF_W: f32 = 0.20;
const MAX_HALF_W: f32 = 0.46;
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

const BG_COLOR: [f32; 3] = crate::theme::PANEL;
const BG_ALPHA: f32 = 0.84;
const RIM_COLOR: [f32; 3] = crate::theme::RIM;
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

/// The box center + half-extents (NDC) for `view`, plus the inner-left `x` text starts from and the
/// inner-top `y` the title starts at. Shared by [`command_panel_quads`] and [`command_panel_labels`]
/// so the box and its text always agree.
///
/// The width auto-sizes to the widest line the panel must hold — the title (at [`TITLE_SIZE`]) or any
/// body row (at [`ROW_SIZE`]) — plus inner padding, clamped to `[MIN_HALF_W, MAX_HALF_W]`. Widths are
/// measured at the live `aspect` (width / height) so the box hugs the *on-screen* text footprint: the
/// glyphs are aspect-corrected by the text pass, so on a wide window the rows are narrower and the box
/// shrinks to match instead of leaving dead right-padding — while on any window the row still fits
/// inside its box (no off-screen clipping, the bug that made the panel look broken). The right + top
/// edges stay pinned (`RIGHT`/`TOP`), so the panel hugs the corner and grows leftward/downward.
fn box_geom(view: &CommandPanelView, aspect: f32) -> (f32, f32, f32, f32, f32, f32) {
    let widest = std::iter::once(crate::text::measure(&view.title, TITLE_SIZE, aspect).0)
        .chain(
            view.lines
                .iter()
                .map(|l| crate::text::measure(&l.text, ROW_SIZE, aspect).0),
        )
        .fold(0.0_f32, f32::max);
    let hw = ((widest + 2.0 * PAD) * 0.5).clamp(MIN_HALF_W, MAX_HALF_W);
    let inner_h = TITLE_SIZE + TITLE_GAP + view.lines.len() as f32 * ROW_STEP;
    let hh = (inner_h + 2.0 * PAD) * 0.5;
    let cx = RIGHT - hw;
    let cy = TOP - hh;
    let left = RIGHT - 2.0 * hw + PAD;
    let top_inner = TOP - PAD;
    (cx, cy, hw, hh, left, top_inner)
}

/// The panel's background + rim quads (drawn through the overlay quad pipeline), auto-sized to the
/// view's content at the live viewport `aspect` (width / height). Empty view ⇒ no quads. Pure +
/// GPU-free → unit-tested.
pub fn command_panel_quads(view: &CommandPanelView, aspect: f32) -> Vec<OverlayQuad> {
    if view.is_empty() {
        return Vec::new();
    }
    let (cx, cy, hw, hh, _, _) = box_geom(view, aspect);
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
/// the inner top, placed against the box sized at the live viewport `aspect`. Empty view ⇒ no labels.
/// Pure + GPU-free → unit-tested.
pub fn command_panel_labels(view: &CommandPanelView, aspect: f32) -> Vec<PanelLabel> {
    if view.is_empty() {
        return Vec::new();
    }
    let (_, _, _, _, left, top_inner) = box_geom(view, aspect);
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
        assert!(command_panel_quads(&v, 1.0).is_empty());
        assert!(command_panel_labels(&v, 1.0).is_empty());
    }

    #[test]
    fn quads_are_a_rim_behind_a_fill_in_the_top_right() {
        let v = view("CAMP — TIER 1", &[("Resources 300", LineStyle::Normal)]);
        let q = command_panel_quads(&v, 1.0);
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
        let short = command_panel_quads(&view("T", &[("a", LineStyle::Normal)]), 1.0);
        let tall = command_panel_quads(
            &view(
                "T",
                &[
                    ("a", LineStyle::Normal),
                    ("b", LineStyle::Normal),
                    ("c", LineStyle::Normal),
                    ("d", LineStyle::Normal),
                ],
            ),
            1.0,
        );
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
        let ls = command_panel_labels(&v, 1.0);
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
    fn box_auto_widens_for_long_rows_but_stays_on_screen() {
        // A long row grows the box wider than a short one (auto-width), yet the panel never runs off
        // the left edge — the bug that made it look broken. Both keep the right edge pinned at RIGHT.
        let short = command_panel_quads(&view("SEL", &[("3x RIFLE", LineStyle::Normal)]), 1.0);
        let long = command_panel_quads(
            &view("SELECTED — 3", &[("STANCE: FIRE AT WILL", LineStyle::Normal)]),
            1.0,
        );
        assert!(long[1].hw > short[1].hw, "a longer row makes a wider box");
        for q in long.iter().chain(short.iter()) {
            // Right edge pinned; left edge stays inside the screen (> -1.0) so nothing clips off.
            assert!((q.cx + q.hw - RIGHT).abs() < 1e-6 || (q.cx + q.hw - (RIGHT + RIM_PAD)).abs() < 1e-6);
            assert!(q.cx - q.hw > -1.0, "left edge stays on screen (no off-screen clip)");
        }
    }

    #[test]
    fn rows_fit_inside_the_auto_sized_box() {
        // The whole point of auto-width: every row's measured footprint fits within the box's inner
        // width, so text can't spill past the panel edge on a square (worst-case) viewport.
        let v = view(
            "SELECTED — 3",
            &[
                ("3X RIFLEMAN", LineStyle::Normal),
                ("STANCE: FIRE AT WILL", LineStyle::Normal),
                ("E  EMBODY", LineStyle::Dim),
            ],
        );
        let q = command_panel_quads(&v, 1.0);
        let fill = &q[1];
        let inner_w = 2.0 * fill.hw - 2.0 * PAD;
        for line in &v.lines {
            let w = crate::text::measure(&line.text, ROW_SIZE, 1.0).0;
            assert!(w <= inner_w + 1e-6, "row {:?} ({w}) fits inner width {inner_w}", line.text);
        }
    }

    #[test]
    fn box_tightens_on_a_wide_viewport_without_clipping() {
        // A wide window narrows the on-screen text, so the auto-width box shrinks to hug it (no dead
        // right-padding) — yet every row still fits inside the (now tighter) inner width.
        let v = view(
            "SELECTED — 3",
            &[("STANCE: FIRE AT WILL", LineStyle::Normal), ("E  EMBODY", LineStyle::Dim)],
        );
        let aspect = 16.0 / 9.0;
        let sq = command_panel_quads(&v, 1.0);
        let wide = command_panel_quads(&v, aspect);
        // Long content isn't clamped, so the wide box is strictly narrower than the square one.
        assert!(wide[1].hw < sq[1].hw, "box hugs the narrower wide-screen text");
        let inner_w = 2.0 * wide[1].hw - 2.0 * PAD;
        for line in &v.lines {
            let w = crate::text::measure(&line.text, ROW_SIZE, aspect).0;
            assert!(w <= inner_w + 1e-6, "row still fits the tightened box");
        }
        // Right edge stays pinned; left edge stays on screen.
        assert!((wide[1].cx + wide[1].hw - RIGHT).abs() < 1e-6);
        assert!(wide[1].cx - wide[1].hw > -1.0);
    }

    #[test]
    fn good_and_bad_rows_take_distinct_colors() {
        let v = view(
            "CAMP",
            &[("Upgrade 200", LineStyle::Good), ("Upgrade 200", LineStyle::Bad)],
        );
        let ls = command_panel_labels(&v, 1.0);
        assert_eq!(ls[1].color, LineStyle::Good.color());
        assert_eq!(ls[2].color, LineStyle::Bad.color());
        assert_ne!(ls[1].color, ls[2].color);
    }
}
