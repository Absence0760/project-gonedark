//! In-match **objective HUD** (PvE WS-A) — a thin top-LEFT presentation surface telling the player
//! their current mission objective + progress, drawn through the shared W4 [`text`](crate::text)
//! pass and the [`overlay`](crate::overlay) quad pipeline (the same two passes the contextual
//! [`command_panel`](crate::command_panel) uses, just anchored to the opposite corner).
//!
//! Like every HUD module here, it is **pure layout**: the host (`engine`) derives *which* objective
//! is current + its progress from the host-side `ObjectiveSet` (which OBSERVES the sim and never
//! mutates it — objectives are not sim state, so this never folds into the checksum, invariant
//! #1/#7) and hands in this [`ObjectiveHudView`]; this module turns it into NDC quads + labels.
//!
//! Fairness (invariant #6): the objective text is **command-layer information** — it is a screen-
//! space NDC label with no world position, and the host gates it to the command view, so it never
//! draws over the dark embodied frame. The free fns [`objective_hud_quads`] / [`objective_hud_labels`]
//! are GPU-free and unit-tested (the `command_panel` / `readout` pattern).

use crate::overlay::{OverlayQuad, QuadRole};
use crate::text::Anchor;

// --- box geometry (NDC, top-LEFT corner) -------------------------------------------------------
/// Left edge of the panel (a small margin in from the screen edge).
const LEFT: f32 = -0.97;
/// Top edge of the panel.
const TOP: f32 = 0.93;
/// Panel half-width; the box spans `[LEFT, LEFT + 2·HALF_W]`.
const HALF_W: f32 = 0.30;
/// Inner padding between the box edge and its content.
const PAD: f32 = 0.022;
/// Title ("OBJECTIVE") text height.
const TITLE_SIZE: f32 = 0.044;
/// Objective + progress row text height.
const ROW_SIZE: f32 = 0.040;
/// Vertical step between row tops.
const ROW_STEP: f32 = 0.058;
/// Gap between the title and the first row.
const TITLE_GAP: f32 = 0.026;
/// The rim quad extends this far past the panel on each side to draw a thin border.
const RIM_PAD: f32 = 0.010;

const BG_COLOR: [f32; 3] = [0.05, 0.06, 0.09];
const BG_ALPHA: f32 = 0.84;
const RIM_COLOR: [f32; 3] = [0.16, 0.18, 0.24];
const RIM_ALPHA: f32 = 0.92;

const TITLE_COLOR: [f32; 3] = [0.80, 0.86, 1.0];

/// How the current objective reads — drives the objective line's tint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectiveStateView {
    /// Still in progress.
    Active,
    /// Achieved (green).
    Completed,
    /// Failed (red).
    Failed,
}

impl ObjectiveStateView {
    fn color(self) -> [f32; 3] {
        match self {
            ObjectiveStateView::Active => [0.82, 0.84, 0.90],
            ObjectiveStateView::Completed => [0.55, 0.85, 0.50],
            ObjectiveStateView::Failed => [0.88, 0.48, 0.42],
        }
    }
}

/// The presentation description of the in-match objective HUD: the current objective's label, its
/// lifecycle state, and an optional `(current, goal)` progress pair. The host rebuilds it each
/// command frame from the `ObjectiveSet`. An empty `objective` ⇒ nothing is drawn (no active
/// mission, e.g. the skirmish/sandbox scenes).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObjectiveHudView {
    /// The current objective's short label ("Take the enemy base"). Empty ⇒ draw nothing.
    pub objective: String,
    /// Its lifecycle state, tinting the line. `None` for an empty view.
    pub state: Option<ObjectiveStateView>,
    /// Optional progress `(current, goal)`. `None` (or `goal == 0`) ⇒ a binary objective with no
    /// numeric bar; `Some((c, g))` renders a "c / g" row.
    pub progress: Option<(u32, u32)>,
}

impl ObjectiveHudView {
    /// True when there is no active objective worth drawing.
    pub fn is_empty(&self) -> bool {
        self.objective.is_empty()
    }

    /// How many body rows this view lays out (the objective line, plus a progress line when one is
    /// shown). Drives the box height so it auto-sizes.
    fn n_rows(&self) -> usize {
        1 + usize::from(matches!(self.progress, Some((_, g)) if g > 0))
    }
}

/// One laid-out label for the text pass: text + NDC placement + tint.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectiveLabel {
    pub text: String,
    pub pos: [f32; 2],
    pub px_size: f32,
    pub anchor: Anchor,
    pub color: [f32; 3],
    pub alpha: f32,
}

/// The box center + half-extents (NDC) for `n_rows` body rows, plus the inner-left `x` text starts
/// from and the inner-top `y` the title starts at. Shared by [`objective_hud_quads`] and
/// [`objective_hud_labels`] so the box and its text always agree.
fn box_geom(n_rows: usize) -> (f32, f32, f32, f32, f32, f32) {
    let inner_h = TITLE_SIZE + TITLE_GAP + n_rows as f32 * ROW_STEP;
    let hh = (inner_h + 2.0 * PAD) * 0.5;
    let cx = LEFT + HALF_W;
    let cy = TOP - hh;
    let left = LEFT + PAD;
    let top_inner = TOP - PAD;
    (cx, cy, HALF_W, hh, left, top_inner)
}

/// The HUD's background + rim quads (drawn through the overlay quad pipeline), auto-sized to the
/// view's row count. Empty view ⇒ no quads. Pure + GPU-free → unit-tested.
pub fn objective_hud_quads(view: &ObjectiveHudView) -> Vec<OverlayQuad> {
    if view.is_empty() {
        return Vec::new();
    }
    let (cx, cy, hw, hh, _, _) = box_geom(view.n_rows());
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

/// The HUD's text labels — the "OBJECTIVE" title, the objective line (tinted by state), and a
/// progress line when a numeric `(current, goal)` is present. All left-aligned and stacked from the
/// inner top. Empty view ⇒ no labels. Pure + GPU-free → unit-tested.
pub fn objective_hud_labels(view: &ObjectiveHudView) -> Vec<ObjectiveLabel> {
    if view.is_empty() {
        return Vec::new();
    }
    let (_, _, _, _, left, top_inner) = box_geom(view.n_rows());
    let state_color = view
        .state
        .unwrap_or(ObjectiveStateView::Active)
        .color();

    let mut out = Vec::with_capacity(3);
    out.push(ObjectiveLabel {
        text: "OBJECTIVE".to_string(),
        pos: [left, top_inner],
        px_size: TITLE_SIZE,
        anchor: Anchor::TopLeft,
        color: TITLE_COLOR,
        alpha: 1.0,
    });
    let rows_top = top_inner - TITLE_SIZE - TITLE_GAP;
    out.push(ObjectiveLabel {
        text: view.objective.clone(),
        pos: [left, rows_top],
        px_size: ROW_SIZE,
        anchor: Anchor::TopLeft,
        color: state_color,
        alpha: 1.0,
    });
    if let Some((current, goal)) = view.progress {
        if goal > 0 {
            out.push(ObjectiveLabel {
                text: format!("{current} / {goal}"),
                pos: [left, rows_top - ROW_STEP],
                px_size: ROW_SIZE,
                anchor: Anchor::TopLeft,
                color: state_color,
                alpha: 1.0,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(objective: &str, state: ObjectiveStateView, progress: Option<(u32, u32)>) -> ObjectiveHudView {
        ObjectiveHudView {
            objective: objective.to_string(),
            state: Some(state),
            progress,
        }
    }

    #[test]
    fn empty_view_draws_nothing() {
        let v = ObjectiveHudView::default();
        assert!(v.is_empty());
        assert!(objective_hud_quads(&v).is_empty());
        assert!(objective_hud_labels(&v).is_empty());
    }

    #[test]
    fn box_hugs_the_top_left_corner() {
        let v = view("Take the enemy base", ObjectiveStateView::Active, Some((0, 5)));
        let q = objective_hud_quads(&v);
        assert_eq!(q.len(), 2, "rim + fill");
        let (rim, fill) = (&q[0], &q[1]);
        assert!(rim.hw > fill.hw && rim.hh > fill.hh, "rim is larger than the fill");
        assert!((fill.cx - fill.hw - LEFT).abs() < 1e-6, "left edge at LEFT");
        assert!((fill.cy + fill.hh - TOP).abs() < 1e-6, "top edge at TOP");
        assert!(fill.cx < 0.0 && fill.cy > 0.0, "sits in the top-left quadrant");
    }

    #[test]
    fn labels_are_title_objective_then_progress_top_down() {
        let v = view("Take the enemy base", ObjectiveStateView::Active, Some((2, 5)));
        let ls = objective_hud_labels(&v);
        assert_eq!(ls.len(), 3, "title + objective + progress");
        assert_eq!(ls[0].text, "OBJECTIVE");
        assert_eq!(ls[1].text, "Take the enemy base");
        assert_eq!(ls[2].text, "2 / 5");
        // Stack downward.
        assert!(ls[1].pos[1] < ls[0].pos[1]);
        assert!(ls[2].pos[1] < ls[1].pos[1]);
        // Share the left edge.
        assert!(ls.iter().all(|l| (l.pos[0] - ls[0].pos[0]).abs() < 1e-6));
    }

    #[test]
    fn binary_objective_has_no_progress_row() {
        // goal == 0 ⇒ a binary objective: no numeric "c / g" row, and a shorter box.
        let v = view("Reach the LZ", ObjectiveStateView::Active, None);
        let ls = objective_hud_labels(&v);
        assert_eq!(ls.len(), 2, "title + objective only");
        let zero_goal = view("Reach the LZ", ObjectiveStateView::Active, Some((0, 0)));
        assert_eq!(objective_hud_labels(&zero_goal).len(), 2, "goal 0 is also binary");
        // The binary box is shorter than a progress box.
        let with_progress = objective_hud_quads(&view("X", ObjectiveStateView::Active, Some((1, 3))));
        let binary = objective_hud_quads(&v);
        assert!(binary[1].hh < with_progress[1].hh, "no progress row → shorter box");
    }

    #[test]
    fn completed_and_failed_tint_the_objective_line() {
        let done = objective_hud_labels(&view("Win", ObjectiveStateView::Completed, None));
        assert_eq!(done[1].color, ObjectiveStateView::Completed.color());
        let lost = objective_hud_labels(&view("Lose", ObjectiveStateView::Failed, None));
        assert_eq!(lost[1].color, ObjectiveStateView::Failed.color());
        assert_ne!(done[1].color, lost[1].color);
        // The title stays the neutral header tint regardless of state.
        assert_eq!(done[0].color, TITLE_COLOR);
    }
}
