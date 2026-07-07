//! Army-select screen — the host-side real-army roster pick (US / French, factions-plan WS-D, D68).
//!
//! The player-selectable armies, their labels/flavour blurbs, the persisted [`ArmySelectState`], the
//! pure [`apply_army_select_action`] decision seam, and the immediate-mode army-select UI. The pick is
//! match-setup config (never checksummed — invariant #7); the sim only sees it via the `core::shell`
//! SelectArmy seam (`Game::select_army`) at match start.

use crate::shell::theme::*;
use crate::shell::widgets::*;
use gonedark_core::components::Army;

/// The player-selectable armies on the army-select screen, in the fixed cycle/display order. Only the
/// **combatant** rosters (US, French) are offered — [`Army::Neutral`] is the non-aligned default and
/// is never a player pick (a commander always fields a real army; factions-plan WS-A). Pure data.
pub(crate) const SELECTABLE_ARMIES: [Army; 2] = [Army::Us, Army::Fr];

/// The on-screen name for an [`Army`] (the army-select card + title readout). ASCII only so it can
/// never tofu in egui's default font. Pure — unit-tested.
pub(crate) fn army_label(army: Army) -> &'static str {
    match army {
        Army::Us => "US Army",
        Army::Fr => "French Army",
        // WW2 cost-vs-power armies (D120) — not offered on this modern army-select screen (they field
        // in the WW2 campaign), but labelled for any HUD/post-match readout.
        Army::UsWw2 => "US Army (WW2)",
        Army::Germany => "German Army (WW2)",
        Army::Neutral => "Non-aligned",
    }
}

/// A one-line identity/flavour blurb for an [`Army`] — the real-platform anchors from
/// `docs/factions.md` §4, so the two cards read as distinct forces. Flavour only: asymmetry is of
/// feel, never of power (the fairness bound, factions.md §2 / pillar 4). ASCII only. Pure —
/// unit-tested.
pub(crate) fn army_flavor(army: Army) -> &'static str {
    match army {
        Army::Us => "M4 carbines, M1 Abrams armour, combat medics -- the US Army roster.",
        Army::Fr => {
            "FAMAS rifles, Leclerc armour, auxiliaires sanitaires -- the French Army roster."
        }
        // WW2 cost-vs-power doctrines (D120): cheap mass vs pricey elite armour.
        Army::UsWw2 => "Cheap, mass-produced Shermans -- field MORE, each one lighter (WW2).",
        Army::Germany => "Pricey Panther/Tiger armour -- field FEWER, each one a wall (WW2).",
        Army::Neutral => "No real-army identity -- the non-aligned default.",
    }
}

/// Host-side army-select state — which real-army roster the player fields. **Match-setup config**, not
/// sim state: it is routed to the sim through the `core::shell` SelectArmy seam at match start (never
/// checksummed itself — invariant #7). Persists across launches like the loadout / faction preference.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ArmySelectState {
    /// The currently-selected army. Defaults to [`Army::Us`] (a real combatant roster — never the
    /// non-aligned [`Army::Neutral`], which is not a player pick).
    pub selected: Army,
}

impl Default for ArmySelectState {
    fn default() -> Self {
        ArmySelectState { selected: Army::Us }
    }
}

/// An action the army-select screen can emit in a frame. Choosing an army is an in-place edit (stays
/// on-screen so the player can compare the two identities); CONFIRM commits and returns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ArmySelectAction {
    /// Select an army (an in-place edit — stays on the screen).
    Choose(Army),
    /// Confirm the current selection and return to the title (the pick persists + is fielded next
    /// match).
    Confirm,
}

/// The screen-level outcome of an [`ArmySelectAction`] once applied — what the run loop switches on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ArmySelectStep {
    /// Stay on army-select (a selection changed, or nothing happened this frame).
    Stay,
    /// Confirm the pick and return to the title.
    Confirm,
}

/// Apply an [`ArmySelectAction`] to the army-select state and report the resulting screen step.
/// `Choose` records the selection and stays; `Confirm` is the screen transition the run loop acts on.
/// Pure (no egui/window) — the army-select testable decision seam, mirroring `apply_profile_action`.
/// It never touches the sim; the sim only sees the pick via the `core::shell` SelectArmy seam that the
/// host resolves at match start (`Game::select_army`).
pub(crate) fn apply_army_select_action(
    action: ArmySelectAction,
    state: &mut ArmySelectState,
) -> ArmySelectStep {
    match action {
        ArmySelectAction::Choose(army) => {
            state.selected = army;
            ArmySelectStep::Stay
        }
        ArmySelectAction::Confirm => ArmySelectStep::Confirm,
    }
}

/// One army card: the army name over its one-line identity blurb, in a framed card whose name is
/// clickable to select it. The currently-selected army reads amber with a SELECTED marker (legible
/// beyond colour alone); clicking a card emits [`ArmySelectAction::Choose`]. Mirrors `mode_tile`.
/// Glue (needs a live `Ui`) — the decision seam is the pure [`apply_army_select_action`]. ASCII only.
pub(crate) fn army_card(ui: &mut egui::Ui, army: Army, selected: bool) -> Option<ArmySelectAction> {
    use egui::{Button, RichText};
    // The selected card reads amber (the lone accent); the others stay bone.
    let name_color = if selected { AMBER } else { BONE };
    let label = RichText::new(army_label(army).to_uppercase())
        .color(name_color)
        .size(TYPE_SUBHEAD)
        .strong();
    let mut clicked = false;
    // The selected card rings amber (the shell's active-state convention); the others keep the
    // neutral RIM hairline, so the pick is legible from the card, not just the small text.
    let frame = if selected {
        card_frame().stroke(egui::Stroke::new(1.5, AMBER))
    } else {
        card_frame()
    };
    frame.show(ui, |ui| {
        // Fill the card's width instead of shrink-wrapping to the button column, so both rosters
        // share one column edge with the intro paragraph above them.
        let w = ui.available_width();
        ui.set_width(w);
        let resp = ui.add(Button::new(label).frame(false).min_size([w, 28.0].into()));
        ui.label(
            RichText::new(army_flavor(army))
                .color(ASH)
                .size(TYPE_CAPTION),
        );
        // Always reserve the marker row (an empty label still takes one line height) so selecting a
        // roster never changes the card's height — the column no longer jumps as you compare rosters.
        let marker = if selected { "SELECTED" } else { "" };
        ui.label(
            RichText::new(marker)
                .color(AMBER)
                .size(TYPE_CAPTION)
                .strong(),
        );
        clicked = resp.clicked();
    });
    clicked.then_some(ArmySelectAction::Choose(army))
}

/// The immediate-mode army-select screen (factions-plan WS-D, D68): the US / French rosters as
/// selectable cards in a column over the backdrop, then CONFIRM. Reads the host-side
/// [`ArmySelectState`] to highlight the current pick; each card's click routes through the pure
/// [`apply_army_select_action`] seam, and the confirmed pick reaches the sim via the `core::shell`
/// SelectArmy seam (`Game::select_army`) at match start. Glue.
pub(crate) fn army_select_ui(
    ui: &mut egui::Ui,
    state: &ArmySelectState,
) -> Option<ArmySelectAction> {
    use egui::RichText;
    let mut action = None;

    over_backdrop_screen(ui, "army", |ui| {
        screen_banner(ui, "SELECT ARMY", 130.0);
        ui.label(
            RichText::new(
                "Choose the real-army roster you deploy as. Asymmetry is of flavour and feel, \
                 never of power -- no army is stronger than the other.",
            )
            .color(ASH)
            .size(TYPE_BODY),
        );
        ui.add_space(12.0);
        section_label(ui, "ROSTER");

        for (i, &army) in SELECTABLE_ARMIES.iter().enumerate() {
            if let Some(act) = army_card(ui, army, state.selected == army) {
                action = Some(act);
            }
            if i + 1 < SELECTABLE_ARMIES.len() {
                ui.add_space(12.0);
            }
        }

        ui.add_space(22.0);
        // Picking a card applies the army in place immediately (no staged draft), so this button
        // only leaves the screen — it's a BACK, not a "commit". Labeling it CONFIRM implied a
        // commit-vs-cancel choice that doesn't exist. (Action stays `Confirm`: a transition that
        // leaves the already-applied selection alone.) Secondary per the shell emphasis policy: a
        // back-out is never the amber CTA.
        if footer_button(ui, "BACK", Emphasis::Secondary) {
            action = Some(ArmySelectAction::Confirm);
        }
    });

    action
}
