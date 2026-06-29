//! Command-view **touch button bar** — the mobile affordance for the RTS half (build / train /
//! upgrade). The desktop drives those intents off the B/R/H/U keys; on a touchscreen there is no
//! keyboard, so the `InputFrame` command intents (`train_slot` / `upgrade_pressed` / `building_slot`)
//! had no way in. This is the missing on-screen surface: a row of labelled buttons along the bottom
//! of the command view, hit-tested per tap by the engine's `command_touch` seam, that arm exactly
//! those intents.
//!
//! PRESENTATION ONLY (invariant #2/#4): the engine fills [`CommandBarView`] from its pixel layout
//! (the hit shapes), converting to NDC so the drawn shapes can never drift from the hit shapes; this
//! module only turns that view into overlay quads + text labels and feeds the **same** overlay-quad
//! and W4 text pipelines `command_panel` / `objective_hud` use — no new shader, no sim touch. Pure +
//! GPU-free, so it is host-unit-tested below.

use crate::command_panel::PanelLabel;
use crate::overlay::{OverlayQuad, QuadRole};
use crate::text::Anchor;

/// Label text size in the text pass's NDC-fraction units (NOT pixels — matches `command_panel`'s
/// ~0.04–0.05 row/title sizes). A hair bigger than a panel row so the buttons read at a glance.
const LABEL_SIZE: f32 = 0.044;
const FILL_ALPHA: f32 = 0.82;
const RIM_ALPHA: f32 = 0.9;
/// Resting fill / rim colors (RGB). Match the `command_panel` palette family so the bar reads as the
/// same chrome.
const FILL: [f32; 3] = [0.10, 0.12, 0.17];
const RIM: [f32; 3] = [0.30, 0.34, 0.44];
const LABEL_COLOR: [f32; 3] = [0.92, 0.94, 0.98];
/// NDC rim thickness added around each button's fill (a crisp border, like the panels' rim).
const RIM_PAD: f32 = 0.006;

/// One drawable command button: its center + half-extents in **NDC** (filled by the engine from its
/// pixel hit rect) plus the label.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandBarButton {
    pub ndc_x: f32,
    pub ndc_y: f32,
    pub half_x: f32,
    pub half_y: f32,
    pub label: String,
}

/// The whole command bar to draw this frame — zero or more buttons. Empty ⇒ nothing drawn (e.g. the
/// embodied view, where the bar is suppressed entirely).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandBarView {
    pub buttons: Vec<CommandBarButton>,
}

impl CommandBarView {
    /// Nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.buttons.is_empty()
    }
}

/// The bar's background quads — a rim then a fill per button (the rim drawn first, behind), through
/// the shared overlay quad pipeline. Empty view ⇒ no quads. Pure + GPU-free → unit-tested.
pub fn command_bar_quads(view: &CommandBarView) -> Vec<OverlayQuad> {
    let mut out = Vec::with_capacity(view.buttons.len() * 2);
    for b in &view.buttons {
        // Rim (behind), slightly larger.
        out.push(OverlayQuad {
            cx: b.ndc_x,
            cy: b.ndc_y,
            hw: b.half_x + RIM_PAD,
            hh: b.half_y + RIM_PAD,
            r: RIM[0],
            g: RIM[1],
            b: RIM[2],
            alpha: RIM_ALPHA,
            role: QuadRole::PanelRim,
        });
        // Fill on top.
        out.push(OverlayQuad {
            cx: b.ndc_x,
            cy: b.ndc_y,
            hw: b.half_x,
            hh: b.half_y,
            r: FILL[0],
            g: FILL[1],
            b: FILL[2],
            alpha: FILL_ALPHA,
            role: QuadRole::Panel,
        });
    }
    out
}

/// The bar's text labels — one centered in each button. Empty view ⇒ no labels. Pure + GPU-free →
/// unit-tested.
pub fn command_bar_labels(view: &CommandBarView) -> Vec<PanelLabel> {
    view.buttons
        .iter()
        .map(|b| PanelLabel {
            text: b.label.clone(),
            pos: [b.ndc_x, b.ndc_y],
            px_size: LABEL_SIZE,
            anchor: Anchor::Center,
            color: LABEL_COLOR,
            alpha: 1.0,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn btn(label: &str) -> CommandBarButton {
        CommandBarButton {
            ndc_x: 0.0,
            ndc_y: -0.8,
            half_x: 0.1,
            half_y: 0.05,
            label: label.to_string(),
        }
    }

    #[test]
    fn empty_view_draws_nothing() {
        let v = CommandBarView::default();
        assert!(v.is_empty());
        assert!(command_bar_quads(&v).is_empty());
        assert!(command_bar_labels(&v).is_empty());
    }

    #[test]
    fn each_button_yields_a_rim_then_a_fill_quad() {
        let v = CommandBarView {
            buttons: vec![btn("RIFLE"), btn("UPGRADE")],
        };
        let q = command_bar_quads(&v);
        assert_eq!(q.len(), 4, "two buttons → rim+fill each");
        assert_eq!(q[0].role, QuadRole::PanelRim, "rim first (behind)");
        assert_eq!(q[1].role, QuadRole::Panel, "fill on top");
        // The rim is larger than the fill it backs.
        assert!(q[0].hw > q[1].hw && q[0].hh > q[1].hh);
    }

    #[test]
    fn one_centered_label_per_button_at_its_ndc_center() {
        let v = CommandBarView {
            buttons: vec![btn("RIFLE"), btn("HEAVY"), btn("UPGRADE")],
        };
        let labels = command_bar_labels(&v);
        assert_eq!(labels.len(), 3);
        assert_eq!(labels[0].text, "RIFLE");
        assert_eq!(labels[0].anchor, Anchor::Center);
        assert_eq!(labels[0].pos, [v.buttons[0].ndc_x, v.buttons[0].ndc_y]);
    }
}
