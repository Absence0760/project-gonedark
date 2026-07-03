//! The Operations-hub mission-select screen — the campaign's nodes as status-coded tiles.
//!
//! A pure launchability seam ([`playable_node`]) gates every tile click, plus the egui glue
//! ([`mission_tile`], [`mission_select_ui`]) that renders the hub over the backdrop. Reads the
//! campaign through [`Campaign::mission_select`] (host-side, never the sim — invariants #1/#7).

use crate::shell::theme::*;
use crate::shell::widgets::*;
use crate::shell::briefing::difficulty_label;
use gonedark_core::campaign::{Campaign, MissionSelectEntry, NodeId, NodeProgress};

/// An action the mission-select (Operations-hub) screen can emit in a frame. The hub reads the
/// campaign through [`Campaign::mission_select`] (host-side, never the sim — invariants #1/#7); the
/// only outcomes are launching a node's briefing or backing out to the title.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MissionSelectAction {
    /// Open the briefing for the clicked node (only ever a *playable* node — see [`playable_node`]).
    OpenNode(NodeId),
    /// Return to the title screen.
    Back,
}

/// The node a mission-select **tile** click resolves to — `Some(node)` only when the tile is
/// playable ([`NodeProgress::is_playable`]: Available **or** already-Cleared/replayable), `None` for
/// a Locked tile. This is the single gate the egui builder routes every tile click through, so a
/// locked tile can never launch even if it somehow reports a click. Pure — unit-tested without a GPU
/// (the rendering of the tile is the exempt glue; this *decision* is what's tested).
pub(crate) fn playable_node(entry: &MissionSelectEntry) -> Option<NodeId> {
    entry.progress.is_playable().then_some(entry.node)
}

/// One mission-select tile: a status pill (Locked/Available/Cleared, colour-coded) beside the node
/// title as a full-width button. A **playable** node (Available or already-Cleared/replayable) is an
/// enabled button that emits [`MissionSelectAction::OpenNode`]; a **Locked** node renders disabled and
/// cannot be clicked. The launchable decision is the pure [`playable_node`] seam (double-guarded on
/// the click), so this is the exempt egui glue. Returns the action on a click. ASCII status text only.
pub(crate) fn mission_tile(ui: &mut egui::Ui, entry: &MissionSelectEntry) -> Option<MissionSelectAction> {
    use egui::RichText;
    let playable = playable_node(entry).is_some();
    let (status, status_color) = match entry.progress {
        NodeProgress::Locked => ("LOCKED".to_string(), MUTED),
        NodeProgress::Available => ("AVAILABLE".to_string(), AMBER),
        NodeProgress::Cleared { best } => {
            // U+00B7 middle dot — the one non-ASCII glyph proven to render in egui's default font.
            (format!("CLEARED \u{00B7} {}", difficulty_label(best)), ASH)
        }
    };
    let title_color = if playable { BONE } else { MUTED };
    // Title first (primary), status as a right-aligned chip — the whole row is one selectable card.
    let clicked = selectable_row(ui, ("mission_tile", entry.node), playable, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(entry.title.clone()).color(title_color).size(TYPE_SUBHEAD).strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                status_chip(ui, &status, status_color);
            });
        });
    });
    clicked
        .then(|| playable_node(entry).map(MissionSelectAction::OpenNode))
        .flatten()
}

/// The immediate-mode Operations-hub mission-select screen: the campaign's nodes as
/// status-coded tiles in a card over the backdrop, then BACK. Reads
/// [`Campaign::mission_select`] (host-side, never the sim); each tile's launchability + the click
/// routing go through the pure [`playable_node`] seam. Glue.
pub(crate) fn mission_select_ui(ui: &mut egui::Ui, campaign: &Campaign) -> Option<MissionSelectAction> {
    use egui::RichText;
    let mut action = None;

    over_backdrop_screen(ui, 0.07, |ui| {
        screen_banner(ui, "OPERATIONS", 130.0);
        ui.label(
            RichText::new(
                "Clear an operation to open the next. A cleared operation can be replayed at a \
                 higher tier.",
            )
            .color(ASH)
            .size(TYPE_BODY),
        );
        ui.add_space(16.0);

        // Each mission is its own selectable card, so they sit directly in the screen card (no
        // second enclosing frame). The list has its own bounded scroll so the banner and BACK stay
        // pinned as the campaign grows; a short list shows no scrollbar.
        let entries = campaign.mission_select();
        egui::ScrollArea::vertical()
            .max_height(5.0 * 72.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (i, entry) in entries.iter().enumerate() {
                    if let Some(act) = mission_tile(ui, entry) {
                        action = Some(act);
                    }
                    if i + 1 < entries.len() {
                        ui.add_space(8.0);
                    }
                }
            });

        ui.add_space(FOOTER_GAP);
        // Sole exit on this screen — Secondary, not the dimmest Tertiary. (Briefing keeps BACK
        // Tertiary because DEPLOY is the genuine primary action there.)
        if menu_button(ui, "BACK", Emphasis::Secondary) {
            action = Some(MissionSelectAction::Back);
        }
    });

    action
}
