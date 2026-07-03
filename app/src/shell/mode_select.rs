//! The Pve/Pvp mode/map-select screen (D81) — its action enum plus the egui builders that
//! present the standing battle scenes as tiles. Picking a mode deploys straight into its scene
//! with the persisted loadout; BACK returns to the title. The pure launch decision lives in the
//! `engine`-tested [`GameMode::scene`] seam; these builders only report the pick.

use crate::shell::theme::*;
use crate::shell::widgets::*;
use gonedark_engine::shell_modes::{GameMode, SHELL_GAME_MODES};

/// An action the Pve/Pvp mode/map-select screen (D81) can emit in a frame. Picking a mode deploys
/// straight into its scene with the persisted loadout (no gunsmith); BACK returns to the title. The
/// mode table itself is the static [`SHELL_GAME_MODES`] (tested in `gonedark_engine::shell_modes`);
/// the picked mode's scene resolution is [`GameMode::scene`] — both live in `engine` so the
/// scene-token guard is unit-tested there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ModeSelectAction {
    /// Deploy the picked mode — the host resolves its [`GameMode::scene`] and boots the match.
    Pick(GameMode),
    /// Return to the title screen.
    Back,
}

/// One mode/map tile: the mode name over its one-line blurb, as a full-width button; clicking it
/// deploys that mode. Mirrors Android's `ModeTile`. Glue (needs a live `Ui`) — the launch decision is
/// the pure [`GameMode::scene`] seam the host resolves; this only reports the pick. ASCII only.
pub(crate) fn mode_tile(ui: &mut egui::Ui, mode: &GameMode) -> Option<ModeSelectAction> {
    use egui::RichText;
    let title = mode.name.to_uppercase();
    // One full-width selectable card row (name over blurb) — no more per-tile shrink-wrapped card
    // that drew each mode at a different width and double-framed it inside the screen card.
    let clicked = selectable_row(ui, ("mode_tile", mode.id), true, |ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(title).color(BONE).size(TYPE_SUBHEAD).strong());
            ui.label(RichText::new(mode.blurb).color(ASH).size(TYPE_CAPTION));
        });
    });
    clicked.then_some(ModeSelectAction::Pick(*mode))
}

/// The immediate-mode Pve/Pvp mode/map-select screen (D81): the standing battle scenes as tiles in a
/// card over the backdrop, then BACK. Reads the static [`SHELL_GAME_MODES`] (host presentation, never
/// the sim); each pick routes through the `engine`-tested [`GameMode::scene`] seam at the host. Glue.
pub(crate) fn mode_select_ui(ui: &mut egui::Ui) -> Option<ModeSelectAction> {
    use egui::RichText;
    let mut action = None;

    over_backdrop_screen(ui, 0.07, |ui| {
        screen_banner(ui, "SELECT MODE", 130.0);
        ui.label(
            RichText::new(
                "Pick a battle to deploy into. Your loadout is set in the gunsmith, under Settings.",
            )
            .color(ASH)
            .size(TYPE_BODY),
        );
        ui.add_space(18.0);

        for (i, mode) in SHELL_GAME_MODES.iter().enumerate() {
            if let Some(act) = mode_tile(ui, mode) {
                action = Some(act);
            }
            if i + 1 < SHELL_GAME_MODES.len() {
                ui.add_space(12.0);
            }
        }

        ui.add_space(FOOTER_GAP);
        // BACK is the only exit on this screen — Secondary, not Tertiary, so it isn't the dimmest
        // control on a screen where it's the sole way out.
        if menu_button(ui, "BACK", Emphasis::Secondary) {
            action = Some(ModeSelectAction::Back);
        }
    });

    action
}
