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
use crate::icon::{IconItem, IconKind};
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
const LABEL_COLOR: [f32; 3] = crate::theme::BONE;
/// NDC rim thickness added around each button's fill (a crisp border, like the panels' rim).
const RIM_PAD: f32 = 0.006;

/// Icon cell height in NDC — a touch larger than [`LABEL_SIZE`] so the glyph reads as an icon, not a
/// letter. The icon pass keeps it square in pixels (aspect-corrected at draw time).
const ICON_SIZE: f32 = 0.060;
/// Where the icon sits horizontally inside a button: a fraction of the button's half-width left of
/// center, so it tucks into the left inset and clears the centered label (even the longest, "UPGRADE").
const ICON_CENTER_FRAC: f32 = 0.78;

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

/// Map a button label to the tactical icon that belongs beside it (and its tint). The engine fills
/// labels from `command_touch::CommandButton::label` ("RIFLE" / "HEAVY" / "UPGRADE"); an unrecognised
/// label gets no icon (returns `None`), so the bar degrades gracefully if the vocabulary grows. Unit-
/// type buttons take the player-faction blue; the upgrade button takes the amber signal accent — both
/// from `theme`, so the icon language matches the rest of the HUD. Pure + GPU-free → unit-tested.
fn icon_for_label(label: &str) -> Option<(IconKind, [f32; 3])> {
    match label {
        "RIFLE" => Some((IconKind::Infantry, crate::theme::PLAYER)),
        "HEAVY" => Some((IconKind::Armor, crate::theme::PLAYER)),
        "UPGRADE" => Some((IconKind::Upgrade, crate::theme::AMBER)),
        _ => None,
    }
}

/// The bar's icons — one small glyph tucked into each button's left inset, beside its centered label.
/// A button whose label has no mapped icon ([`icon_for_label`]) simply contributes none. Empty view ⇒
/// no icons. The icon center sits `ICON_CENTER_FRAC` of the half-width left of the button center; the
/// icon pass aspect-corrects the width so it stays square in pixels. Pure + GPU-free → unit-tested.
pub fn command_bar_icons(view: &CommandBarView) -> Vec<IconItem> {
    view.buttons
        .iter()
        .filter_map(|b| {
            let (kind, tint) = icon_for_label(&b.label)?;
            Some(IconItem {
                kind,
                pos: [b.ndc_x - b.half_x * ICON_CENTER_FRAC, b.ndc_y],
                size: ICON_SIZE,
                tint,
                alpha: 1.0,
            })
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
        assert!(command_bar_icons(&v).is_empty());
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

    #[test]
    fn each_known_button_gets_its_icon_tucked_left() {
        let v = CommandBarView {
            buttons: vec![btn("RIFLE"), btn("HEAVY"), btn("UPGRADE")],
        };
        let icons = command_bar_icons(&v);
        assert_eq!(icons.len(), 3, "every mapped label gets one icon");
        assert_eq!(icons[0].kind, IconKind::Infantry);
        assert_eq!(icons[1].kind, IconKind::Armor);
        assert_eq!(icons[2].kind, IconKind::Upgrade);
        for (icon, b) in icons.iter().zip(&v.buttons) {
            assert!(icon.pos[0] < b.ndc_x, "icon sits left of the button center");
            assert!(
                icon.pos[0] > b.ndc_x - b.half_x,
                "icon stays inside the button's left edge"
            );
            assert_eq!(
                icon.pos[1], b.ndc_y,
                "icon is vertically centered in the button"
            );
            assert!(icon.size > 0.0);
            assert_eq!(icon.alpha, 1.0);
        }
    }

    #[test]
    fn unit_icons_take_player_tint_upgrade_takes_amber() {
        let v = CommandBarView {
            buttons: vec![btn("RIFLE"), btn("UPGRADE")],
        };
        let icons = command_bar_icons(&v);
        assert_eq!(
            icons[0].tint,
            crate::theme::PLAYER,
            "unit button → faction blue"
        );
        assert_eq!(
            icons[1].tint,
            crate::theme::AMBER,
            "upgrade button → amber accent"
        );
    }

    #[test]
    fn unknown_labels_contribute_no_icon() {
        // A label outside the mapped vocabulary degrades to no icon (the bar still draws its box+label).
        let v = CommandBarView {
            buttons: vec![btn("RIFLE"), btn("MYSTERY"), btn("UPGRADE")],
        };
        let icons = command_bar_icons(&v);
        assert_eq!(icons.len(), 2, "only the two recognised labels get icons");
        assert_eq!(icons[0].kind, IconKind::Infantry);
        assert_eq!(icons[1].kind, IconKind::Upgrade);
    }
}
