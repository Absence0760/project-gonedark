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
use gonedark_core::components::Faction;

/// Label text size in the text pass's NDC-fraction units (NOT pixels — matches `command_panel`'s
/// ~0.04–0.05 row/title sizes). Sized so the longest label ("UPGRADE", 7 chars) still fits inside a
/// button's width at the worst-case **portrait** aspect (~0.46), where the buttons are widest in NDC
/// (`half_x ≈ 0.20`, from `command_touch`'s `0.20·min(w,h)`) but the glyphs are also widest per the
/// aspect correction. `0.044` overflowed "UPGRADE" past its button in portrait; `0.040` fits with a
/// small margin. (See `upgrade_label_fits_its_button_in_portrait`.)
// Rides the shared type scale's body step (`theme`, = 0.040) so the bar's labels read at the same
// size as the command-panel rows (WS-C). `0.044` overflowed "UPGRADE" past its button in portrait;
// the body step fits with a small margin (see `upgrade_label_fits_its_button_in_portrait`).
const LABEL_SIZE: f32 = crate::theme::TYPE_BODY;
const FILL_ALPHA: f32 = 0.82;
const RIM_ALPHA: f32 = 0.9;
/// Resting fill / rim colors (RGB) — the shared `theme` raised-surface + rim, so the bar wears the
/// SAME chrome as the command panel and readout cards (WS-C: one designed set, no ad-hoc literals).
const FILL: [f32; 3] = crate::theme::PANEL_RAISED;
const RIM: [f32; 3] = crate::theme::RIM;
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
/// labels from `command_touch::CommandButton::label` (today "RIFLE" / "HEAVY" / "UPGRADE"); an
/// unrecognised label gets no icon (returns `None`), so the bar degrades gracefully. The mapping
/// covers the WHOLE unit/action command vocabulary — every unit kind (`token_icons`'
/// `icon_for_unit_kind` and the command-panel rows use the same [`IconKind`]s) plus build/upgrade —
/// so a button always carries the same glyph the rest of the HUD uses for that thing (WS-C: one icon
/// language across the bar, menus, and radial). Unit-type buttons take the **player-faction** colour
/// resolved from the ACTIVE `palette` (`faction_color_in(Player, palette)`) so a colourblind ramp
/// (WS-D) swaps it in lockstep with the world-unit body colours; the build + upgrade action buttons
/// take the amber signal accent, which is NOT a faction colour and stays literal. Pure + GPU-free →
/// unit-tested.
fn icon_for_label(label: &str, palette: &crate::theme::Palette) -> Option<(IconKind, [f32; 3])> {
    let player = crate::faction_color_in(Faction::Player, palette);
    match label {
        "RIFLE" | "RIFLEMAN" | "INFANTRY" => Some((IconKind::Infantry, player)),
        "HEAVY" | "TANK" | "ARMOR" => Some((IconKind::Armor, player)),
        "MEDIC" => Some((IconKind::Medic, player)),
        "ANTI-TANK" | "ANTITANK" | "AT" => Some((IconKind::AntiTank, player)),
        "BUILD" => Some((IconKind::Build, crate::theme::AMBER)),
        "UPGRADE" => Some((IconKind::Upgrade, crate::theme::AMBER)),
        _ => None,
    }
}

/// The bar's icons — one small glyph tucked into each button's left inset, beside its centered label.
/// A button whose label has no mapped icon ([`icon_for_label`]) simply contributes none. Empty view ⇒
/// no icons. The icon center sits `ICON_CENTER_FRAC` of the half-width left of the button center; the
/// icon pass aspect-corrects the width so it stays square in pixels. Pure + GPU-free → unit-tested.
pub fn command_bar_icons(view: &CommandBarView, palette: &crate::theme::Palette) -> Vec<IconItem> {
    view.buttons
        .iter()
        .filter_map(|b| {
            let (kind, tint) = icon_for_label(&b.label, palette)?;
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

    /// A representative portrait phone aspect (width / height, e.g. 1080×2340 ≈ 0.46) — the
    /// worst-case for label-vs-button fit, where glyphs are widest per the text pass's aspect
    /// correction. Layout that stays on-screen here stays on-screen on any wider window.
    const PORTRAIT_ASPECT: f32 = 0.46;

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
        assert!(command_bar_icons(&v, &crate::theme::Palette::DEFAULT).is_empty());
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
        let icons = command_bar_icons(&v, &crate::theme::Palette::DEFAULT);
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
        let icons = command_bar_icons(&v, &crate::theme::Palette::DEFAULT);
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
    fn icon_vocabulary_covers_every_unit_kind_and_action() {
        // WS-C: the bar can glyph the whole command vocabulary, so a future Medic / Anti-Tank /
        // Build button reads with the SAME icon the token glyphs + panel rows use, not bare text.
        let v = CommandBarView {
            buttons: vec![
                btn("RIFLE"),
                btn("HEAVY"),
                btn("MEDIC"),
                btn("ANTI-TANK"),
                btn("BUILD"),
                btn("UPGRADE"),
            ],
        };
        let icons = command_bar_icons(&v, &crate::theme::Palette::DEFAULT);
        assert_eq!(icons.len(), 6, "every vocabulary button gets an icon");
        let kinds: Vec<IconKind> = icons.iter().map(|i| i.kind).collect();
        assert_eq!(
            kinds,
            vec![
                IconKind::Infantry,
                IconKind::Armor,
                IconKind::Medic,
                IconKind::AntiTank,
                IconKind::Build,
                IconKind::Upgrade,
            ]
        );
        // Unit tokens take faction blue; the build + upgrade actions take the amber accent.
        assert_eq!(icons[2].tint, crate::theme::PLAYER, "medic → faction blue");
        assert_eq!(icons[3].tint, crate::theme::PLAYER, "anti-tank → faction blue");
        assert_eq!(icons[4].tint, crate::theme::AMBER, "build → amber action accent");
        assert_eq!(icons[5].tint, crate::theme::AMBER, "upgrade → amber action accent");
    }

    #[test]
    fn bar_chrome_is_sourced_from_the_shared_theme() {
        // The drawn rim + fill + label are `theme` colours (one designed set with the panel/readout
        // cards) — asserted on the actual emitted quads/labels so the wiring is covered.
        let v = CommandBarView { buttons: vec![btn("RIFLE")] };
        let q = command_bar_quads(&v);
        let (rim, fill) = (&q[0], &q[1]);
        assert_eq!([rim.r, rim.g, rim.b], crate::theme::RIM, "rim is theme::RIM");
        assert_eq!([fill.r, fill.g, fill.b], crate::theme::PANEL_RAISED, "fill is theme::PANEL_RAISED");
        assert_eq!(command_bar_labels(&v)[0].color, crate::theme::BONE, "label is theme::BONE");
    }

    #[test]
    fn upgrade_label_fits_its_button_in_portrait() {
        // Regression (portrait overflow): the longest label ("UPGRADE") must measure no wider than
        // its button at the worst-case portrait aspect. A command_touch button is `0.20·min(w,h)`
        // wide → half_x ≈ 0.20 (full width 0.40 NDC) on a portrait phone. The 0.044 size overflowed;
        // 0.040 fits. Every command-bar label ("RIFLE"/"HEAVY"/"UPGRADE") is at most 7 chars.
        let half_x = 0.20_f32;
        let button_w = 2.0 * half_x;
        for label in ["RIFLE", "HEAVY", "UPGRADE"] {
            let w = crate::text::measure(label, LABEL_SIZE, PORTRAIT_ASPECT).0;
            assert!(
                w <= button_w,
                "label {label:?} measures {w} NDC, wider than its {button_w} NDC button in portrait"
            );
        }
    }

    #[test]
    fn unit_icon_tint_follows_the_active_colourblind_palette() {
        // WS-D: a unit button's tint is the ACTIVE palette's player colour, so switching to a
        // colourblind ramp swaps it in lockstep with the world units. The amber action accent is not
        // a faction colour, so it stays put across modes.
        let v = CommandBarView {
            buttons: vec![btn("RIFLE"), btn("UPGRADE")],
        };
        let cvd = crate::theme::palette(crate::theme::PaletteMode::Deuteranopia);
        let icons = command_bar_icons(&v, &cvd);
        assert_eq!(
            icons[0].tint,
            crate::faction_color_in(Faction::Player, &cvd),
            "unit tint tracks the active palette's player colour"
        );
        assert_ne!(
            icons[0].tint,
            crate::theme::PLAYER,
            "the CVD ramp actually moved the player tint off the default"
        );
        assert_eq!(icons[1].tint, crate::theme::AMBER, "amber action accent is palette-independent");
    }

    #[test]
    fn unknown_labels_contribute_no_icon() {
        // A label outside the mapped vocabulary degrades to no icon (the bar still draws its box+label).
        let v = CommandBarView {
            buttons: vec![btn("RIFLE"), btn("MYSTERY"), btn("UPGRADE")],
        };
        let icons = command_bar_icons(&v, &crate::theme::Palette::DEFAULT);
        assert_eq!(icons.len(), 2, "only the two recognised labels get icons");
        assert_eq!(icons[0].kind, IconKind::Infantry);
        assert_eq!(icons[1].kind, IconKind::Upgrade);
    }
}
