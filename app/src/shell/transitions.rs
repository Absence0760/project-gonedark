//! The title-screen action vocabulary and the host state-machine transitions it resolves to — the
//! pure run-loop decision surface, unit-tested with no window. [`EguiShell`](super::egui_shell)
//! reports a [`TitleAction`]; the run loop switches on the [`HostTransition`] it maps to.

use gonedark_core::campaign::{Difficulty, NodeId};

/// A top-level action the player can pick on the title screen. The three play modes all open the
/// gunsmith→match flow today; their divergence is future work (see [`resolve_title_action`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TitleAction {
    /// The PvE story campaign — the first shippable pillar (`docs/pve-campaign.md`, D58).
    Campaign,
    /// A standalone PvE skirmish against the scripted enemy commander.
    Pve,
    /// Player-vs-player — the lockstep-netcode match.
    Pvp,
    /// Open settings (a placeholder until the Settings surface lands).
    Settings,
    /// Open the player profile / progression surface (a no-op placeholder until it lands).
    Profile,
    /// Open the **army-select** screen — pick which real-army roster (US vs French, `docs/factions.md`,
    /// D68) the player deploys as. A pre-deploy pick that routes through the `core::shell` SelectArmy
    /// seam and is fielded at every match start.
    Army,
    /// Open the About / field-manual (controls-reference) screen straight from the title. Mirrors
    /// Android's `TitleAction.About` — on desktop About is *also* reachable from Settings, so its
    /// return target is carried through [`AboutReturn`] rather than fixed.
    About,
    /// Quit the app.
    Quit,
}

/// Where the About / field-manual screen returns on BACK — the entry point it was opened from. About
/// is reachable from **both** the title (Android parity) and Settings (the pre-existing desktop path),
/// so BACK must land back where the player came from rather than a fixed screen. Pure data.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AboutReturn {
    /// Opened from the title screen — BACK returns to the title.
    Title,
    /// Opened from Settings — BACK returns to Settings (the original desktop path).
    Settings,
}

/// What the host does in response to a title action — the decision table the run loop switches on.
/// Separated from [`TitleAction`] so it is unit-testable without a window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HostTransition {
    /// Switch the host to the pre-match gunsmith / loadout screen. Start now lands here first; the
    /// screen's **Deploy** is what subsequently creates the `Game` (carrying the chosen loadout).
    OpenLoadout,
    /// Switch the host to the Pve/Pvp **mode / map select** screen (D81). Reached from the title's
    /// PvE / PvP buttons; picking a mode deploys straight into that scene with the persisted loadout
    /// (the gunsmith no longer gates play — it moved behind Settings). No data: the mode table is the
    /// static [`gonedark_engine::shell_modes::SHELL_GAME_MODES`].
    OpenModeSelect,
    /// Switch the host to the **Operations-hub mission-select** screen — the PvE campaign entry
    /// (`docs/pve-campaign.md`, D58). Reached from the title's CAMPAIGN button; the player picks a
    /// node tile there, which opens its [`OpenBriefing`](HostTransition::OpenBriefing).
    OpenMissionSelect,
    /// Switch the host to the **briefing** screen for a campaign node (the "launch this mission"
    /// surface). Carries the [`NodeId`] the mission-select tile resolved to.
    OpenBriefing(NodeId),
    /// Queue the campaign mission for `node` at the chosen replay `difficulty`, then route through
    /// the gunsmith (the player still picks a loadout) before the match starts. The host stashes the
    /// pending launch and switches to the loadout screen; the gunsmith's **Deploy** then creates the
    /// `Game` for this node (see [`EnterMatch`](HostTransition::EnterMatch)). The `difficulty` is the
    /// chosen replay tier: it drives the launched fight on both D83 axes (the 4→3 enemy-commander band
    /// + the scenario situation modifiers, via `Game::apply_campaign_tuning`) **and** is the tier the
    /// **clear** is recorded against on a win.
    LaunchMission { node: NodeId, difficulty: Difficulty },
    /// Lazily create `engine::Game` and switch the host to the in-match screen.
    EnterMatch,
    /// Switch the host to the Settings screen (audio / video / controls preferences).
    OpenSettings,
    /// Switch the host to the player Profile screen (callsign, faction preference, lifetime record).
    OpenProfile,
    /// Switch the host to the **army-select** screen — choose the US/FR roster the player fields
    /// (factions-plan WS-D, D68). Reached from the title; the confirmed pick persists and is fielded
    /// at every subsequent match start (both the PvE/PvP mode-select and the campaign deploy paths).
    OpenArmySelect,
    /// Switch the host to the About / controls-reference screen, remembering where BACK returns to
    /// ([`AboutReturn`]) — reachable from both the title and Settings.
    OpenAbout(AboutReturn),
    /// Toggle borderless fullscreen and stay on the current screen — the Settings video toggle. The
    /// window mode lives on the host (`App::fullscreen`), so this defers the actual flip to the run
    /// loop rather than carrying a second source of truth into the settings model.
    ToggleFullscreen,
    /// Tear down and exit the app.
    Exit,
    /// Leave the current match and return to the title screen — the post-match summary's HUB button,
    /// and any other in-match "give up the match without quitting the app" path. Drops the `Game`.
    ExitToTitle,
    /// The post-match summary's **REMATCH**: re-seed a fresh match of the same scene/mission with the
    /// same loadout (a new deterministic `Sim`, not a reuse of the ended one — invariant #5). Deferred
    /// to the run-loop dispatch because it re-enters the match while the ended `Game` is still borrowed.
    Rematch,
}

/// Map a title action to the host transition it triggers (the pure run-loop decision).
pub(crate) fn resolve_title_action(action: TitleAction) -> HostTransition {
    match action {
        // CAMPAIGN opens the Operations-hub mission-select (the PvE pillar, D58) — the player picks a
        // node, reads its briefing, and launches it. PvE/PvP open the mode/map select (D81); the
        // gunsmith is customization-only behind Settings, no longer a play gate.
        TitleAction::Campaign => HostTransition::OpenMissionSelect,
        // PvE/PvP open the mode/map select (D81) — the deploy gate that boots the chosen scene with
        // the persisted loadout. The gunsmith no longer gates play (it moved behind Settings). PvE
        // and PvP share the picker until PvP match-setup lands (Q5).
        TitleAction::Pve | TitleAction::Pvp => HostTransition::OpenModeSelect,
        TitleAction::Settings => HostTransition::OpenSettings,
        TitleAction::Profile => HostTransition::OpenProfile,
        // The ARMY chip opens the army-select screen (US vs FR); the confirmed pick routes through
        // the SelectArmy seam at match start.
        TitleAction::Army => HostTransition::OpenArmySelect,
        // The FIELD MANUAL button opens About and returns to the title on BACK (Android parity).
        TitleAction::About => HostTransition::OpenAbout(AboutReturn::Title),
        TitleAction::Quit => HostTransition::Exit,
    }
}
