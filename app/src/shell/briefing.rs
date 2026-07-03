//! The briefing screen — pure seam (unit-tested) plus its immediate-mode egui glue.
//!
//! One campaign node's briefing: the title, the briefing copy, a replay **difficulty** cycler
//! (the host-side `selected` tier), the clear status, then DEPLOY / BACK. The discrete controls
//! return a [`BriefingAction`] the pure [`apply_briefing_action`] seam resolves into a
//! [`BriefingOutcome`].

use crate::shell::theme::*;
use crate::shell::widgets::*;
use gonedark_core::campaign::{Campaign, Difficulty, NodeId, NodeProgress};

/// An action the briefing screen can emit in a frame. `CycleDifficulty` advances the host-side
/// replay-tier selector (a stay-on-screen edit); `Deploy`/`Back` are screen transitions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BriefingAction {
    /// Advance the selected replay difficulty to the next tier (wrapping).
    CycleDifficulty,
    /// Launch this mission with the currently-selected difficulty (routes through the gunsmith).
    Deploy,
    /// Return to the mission-select hub.
    Back,
}

/// The screen-level outcome of a [`BriefingAction`] once applied — what the host run loop switches
/// on. Separated from the egui glue so it is unit-testable without a window, mirroring
/// [`LoadoutStep`] / [`SettingsStep`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BriefingOutcome {
    /// Stay on the briefing (a difficulty edit, or nothing this frame).
    Stay,
    /// Launch the mission at the selected campaign `difficulty` — the replay tier that drives the
    /// fight (D83: commander band + situation modifiers) and is recorded against the **clear** on a win.
    Launch { difficulty: Difficulty },
    /// Return to the mission-select hub.
    Back,
}

/// The next campaign [`Difficulty`] tier, wrapping through [`Difficulty::ALL`]
/// (`Recruit → Regular → Veteran → Elite → Recruit`). Pure helper for the briefing's difficulty
/// cycler — `core::campaign::Difficulty` derives `Ord` but ships no `next`, so the shell owns the
/// cycle order here (and tests it). ASCII-free of any sim concern; this is presentation only.
pub(crate) fn next_difficulty(d: Difficulty) -> Difficulty {
    let all = Difficulty::ALL;
    let i = all.iter().position(|&x| x == d).unwrap_or(0);
    all[(i + 1) % all.len()]
}

/// The human-readable label for a campaign [`Difficulty`] tier (the briefing's difficulty cycler
/// readout). `core::campaign::Difficulty::id` returns a stable key (`"recruit"`…) for localization;
/// the shell owns the display string. ASCII only so it can never tofu in egui's default font. Pure —
/// unit-tested.
pub(crate) fn difficulty_label(d: Difficulty) -> &'static str {
    match d {
        Difficulty::Recruit => "Recruit",
        Difficulty::Regular => "Regular",
        Difficulty::Veteran => "Veteran",
        Difficulty::Elite => "Elite",
    }
}

/// Apply a [`BriefingAction`], advancing the host-side `selected` replay tier in place on a cycle and
/// reporting the resulting screen step. Pure (no egui/window) — the briefing's testable decision
/// seam, mirroring [`apply_loadout_action`]. `Deploy` carries the *current* selection out as the
/// launch tier: the host applies its combat tuning (D83: the 4→3 enemy-commander band + the scenario
/// situation modifiers, via `Game::apply_campaign_tuning`) and records it against `Campaign::clear`
/// on a win.
pub(crate) fn apply_briefing_action(action: BriefingAction, selected: &mut Difficulty) -> BriefingOutcome {
    match action {
        BriefingAction::CycleDifficulty => {
            *selected = next_difficulty(*selected);
            BriefingOutcome::Stay
        }
        BriefingAction::Deploy => BriefingOutcome::Launch {
            difficulty: *selected,
        },
        BriefingAction::Back => BriefingOutcome::Back,
    }
}

/// The immediate-mode briefing screen for one campaign node: the title, the briefing copy, a replay
/// **difficulty** cycler (the host-side `selected` tier), the clear status, then DEPLOY / BACK.
/// Reads the node through [`Campaign::briefing`]; the discrete controls return a [`BriefingAction`]
/// the pure [`apply_briefing_action`] seam resolves. An out-of-range node degrades to a BACK-only
/// card. Glue.
pub(crate) fn briefing_ui(
    ui: &mut egui::Ui,
    campaign: &Campaign,
    node: NodeId,
    selected: Difficulty,
) -> Option<BriefingAction> {
    use egui::{Button, RichText};
    let mut action = None;

    over_backdrop_screen(ui, "briefing", |ui| {
        let Some(b) = campaign.briefing(node) else {
            // The hub only opens playable, in-range nodes, so this is purely defensive.
            screen_banner(ui, "BRIEFING", 110.0);
            ui.label(RichText::new("No such operation.").color(ASH).size(TYPE_BODY));
            ui.add_space(16.0);
            if footer_button(ui, "BACK", Emphasis::Secondary) {
                action = Some(BriefingAction::Back);
            }
            return;
        };

        screen_banner(ui, &b.title.to_uppercase(), 130.0);
        ui.label(RichText::new(b.briefing.clone()).color(ASH).size(TYPE_BODY));
        ui.add_space(16.0);

        card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            // Difficulty cycler — the replay tier that drives the fight (D83: the 4→3 enemy-commander
            // band + the scenario situation modifiers) and the tier the CLEAR is recorded against on a
            // win. Label flush-left, cycle button flush-right so the row spans the card.
            ui.horizontal(|ui| {
                ui.label(RichText::new("Difficulty").color(BONE).size(TYPE_BODY));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_sized(
                            [200.0, 32.0],
                            Button::new(
                                RichText::new(difficulty_label(selected))
                                    .color(AMBER)
                                    .size(TYPE_BODY)
                                    .strong(),
                            ),
                        )
                        .clicked()
                    {
                        action = Some(BriefingAction::CycleDifficulty);
                    }
                });
            });
            ui.add_space(6.0);
            // A 4-pip ladder (Recruit -> Elite) showing where the selected tier sits — a fixed cycle
            // button otherwise gives no sense of "1 of 4". Presentation only; `selected` is threaded.
            ui.horizontal(|ui| {
                for d in Difficulty::ALL {
                    let filled = d <= selected;
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 4.0), egui::Sense::hover());
                    ui.painter().rect_filled(
                        rect,
                        egui::CornerRadius::same(2),
                        if filled { AMBER } else { RIM },
                    );
                    ui.add_space(4.0);
                }
            });
            ui.add_space(8.0);
            // Clear status — `replayable` once cleared, with the best tier so far.
            let status = match b.progress {
                NodeProgress::Cleared { best } => {
                    format!("Cleared at {} -- replay to raise your best.", difficulty_label(best))
                }
                NodeProgress::Available => "Not yet cleared.".to_string(),
                NodeProgress::Locked => "Locked.".to_string(),
            };
            ui.label(RichText::new(status).color(MUTED).size(TYPE_CAPTION));
        });

        ui.add_space(FOOTER_GAP);
        if footer_button(ui, "DEPLOY", Emphasis::Primary) {
            action = Some(BriefingAction::Deploy);
        }
        ui.add_space(10.0);
        if footer_button(ui, "BACK", Emphasis::Tertiary) {
            action = Some(BriefingAction::Back);
        }
    });

    action
}
