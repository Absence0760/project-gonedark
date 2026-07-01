//! The desktop **app shell** — native out-of-match chrome (D32) for Windows/Linux, drawn with
//! **egui** (D36: the desktop-toolkit call D32 left open). It is host-side, holds no game/sim logic,
//! and drives the shared engine only through the `core::shell` seam — the desktop counterpart of the
//! Android Jetpack-Compose shell (D35).
//!
//! Two layers, kept apart exactly like the Android shell:
//!  - a tiny **pure seam** ([`resolve_title_action`], [`build_stamp`]/[`build_channel`]) — the
//!    testable decision/formatting logic, unit-tested below with no GPU or window;
//!  - the **egui glue** ([`EguiShell`]) — device-gated chrome (an egui context + the winit input
//!    bridge + the wgpu renderer) that draws the title screen and reports the clicked action. The
//!    glue is exempt from unit tests (CLAUDE.md: thin, un-constructible-in-test platform glue), so
//!    the real logic is pushed down into the pure seam where it *is* tested.

use gonedark_core::campaign::{
    Campaign, Difficulty, MissionSelectEntry, NodeId, NodeProgress,
};
use gonedark_engine::loadout_ui::{LoadoutEditor, LoadoutSlot};
use gonedark_pal_desktop::DesktopRenderSurface;
use gonedark_render::title_backdrop::TitleBackdrop;
use winit::window::Window;

// ---- The pure seam (unit-tested) ----------------------------------------------------------------

/// A top-level action the player can pick on the title screen. The three play modes all open the
/// gunsmith→match flow today; their divergence is future work (see [`resolve_title_action`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TitleAction {
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
pub enum AboutReturn {
    /// Opened from the title screen — BACK returns to the title.
    Title,
    /// Opened from Settings — BACK returns to Settings (the original desktop path).
    Settings,
}

/// What the host does in response to a title action — the decision table the run loop switches on.
/// Separated from [`TitleAction`] so it is unit-testable without a window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HostTransition {
    /// Switch the host to the pre-match gunsmith / loadout screen. Start now lands here first; the
    /// screen's **Deploy** is what subsequently creates the `Game` (carrying the chosen loadout).
    OpenLoadout,
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
    /// campaign-tier the **clear** is recorded against on a win — *not* the commander-AI tier (that
    /// stays the mission's authored tier; the 4→3 mapping is open question Q21).
    LaunchMission { node: NodeId, difficulty: Difficulty },
    /// Lazily create `engine::Game` and switch the host to the in-match screen.
    EnterMatch,
    /// Switch the host to the Settings screen (audio / video / controls preferences).
    OpenSettings,
    /// Switch the host to the player Profile screen (callsign, faction preference, lifetime record).
    OpenProfile,
    /// Switch the host to the About / controls-reference screen, remembering where BACK returns to
    /// ([`AboutReturn`]) — reachable from both the title and Settings.
    OpenAbout(AboutReturn),
    /// Toggle borderless fullscreen and stay on the current screen — the Settings video toggle. The
    /// window mode lives on the host (`App::fullscreen`), so this defers the actual flip to the run
    /// loop rather than carrying a second source of truth into the settings model.
    ToggleFullscreen,
    /// Tear down and exit the app.
    Exit,
    /// Leave the current match and return to the title screen — the post-match summary's DISMISS,
    /// and any other in-match "give up the match without quitting the app" path. Drops the `Game`.
    ExitToTitle,
}

/// Map a title action to the host transition it triggers (the pure run-loop decision).
pub fn resolve_title_action(action: TitleAction) -> HostTransition {
    match action {
        // CAMPAIGN now opens the Operations-hub mission-select (the PvE pillar, D58) — the player
        // picks a node, reads its briefing, and launches it (still through the gunsmith). PvE/PvP
        // keep the direct gunsmith→match flow: there is no PvP lobby or standalone-skirmish picker
        // yet, so each still folds straight to the loadout screen (their mode divergence is future
        // work, exactly as before).
        TitleAction::Campaign => HostTransition::OpenMissionSelect,
        TitleAction::Pve | TitleAction::Pvp => HostTransition::OpenLoadout,
        TitleAction::Settings => HostTransition::OpenSettings,
        TitleAction::Profile => HostTransition::OpenProfile,
        // The FIELD MANUAL button opens About and returns to the title on BACK (Android parity).
        TitleAction::About => HostTransition::OpenAbout(AboutReturn::Title),
        TitleAction::Quit => HostTransition::Exit,
    }
}

// ---- The gunsmith / loadout screen — pure seam (unit-tested) -------------------------------------

/// An action the pre-match gunsmith / loadout screen can emit in a frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoadoutAction {
    /// Cycle the slot at on-screen index `slot_index` forward (`true`) or back (`false`) — an edit.
    Cycle { slot_index: usize, forward: bool },
    /// Reset every slot to the neutral all-`Standard` baseline.
    Reset,
    /// Deploy with the current loadout — leave the gunsmith and enter the match.
    Deploy,
    /// Abandon the gunsmith and return to the title screen (no match started).
    Back,
}

/// The screen-level outcome of a [`LoadoutAction`] once applied to the editor — what the host run
/// loop switches on. Separated from the egui glue so it is unit-testable without a window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoadoutStep {
    /// Stay on the gunsmith (an edit was applied, or nothing happened this frame).
    Stay,
    /// Enter the match, fielding the editor's current loadout.
    Deploy,
    /// Return to the title screen without starting a match.
    Back,
}

/// Apply a [`LoadoutAction`] to the player's [`LoadoutEditor`] and report the resulting screen step.
/// Edits (`Cycle`/`Reset`) mutate the editor and keep us on the gunsmith; `Deploy`/`Back` are screen
/// transitions the run loop acts on. Pure (no egui/window) — the gunsmith's testable decision seam,
/// mirroring [`resolve_title_action`]. The actual loadout *model* (validation + the sidegrade-fairness
/// proof) lives in `core::gunsmith` and is consumed through the editor read-only; this never touches
/// the sim.
pub fn apply_loadout_action(action: LoadoutAction, editor: &mut LoadoutEditor) -> LoadoutStep {
    match action {
        LoadoutAction::Cycle {
            slot_index,
            forward,
        } => {
            // An out-of-range index is a harmless no-op (the editor tolerates stray slot values).
            editor.apply_input(slot_index, forward);
            LoadoutStep::Stay
        }
        LoadoutAction::Reset => {
            editor.reset();
            LoadoutStep::Stay
        }
        LoadoutAction::Deploy => LoadoutStep::Deploy,
        LoadoutAction::Back => LoadoutStep::Back,
    }
}

/// A short, one-line description of the *axis pair* a gunsmith slot trades — the readout that makes
/// the sidegrade nature legible (every option spends one of these axes to buy the other). Pure and
/// static, so it is unit-tested; the numeric per-axis deltas live in `core::gunsmith` and are not
/// surfaced here (they need fixed-point formatting and add nothing to "which way does this trade").
/// ASCII only so it can never tofu in egui's default font.
pub fn slot_trade_hint(slot: LoadoutSlot) -> &'static str {
    match slot {
        LoadoutSlot::Optic => "range <-> fire-rate",
        LoadoutSlot::Barrel => "damage <-> reserve",
        LoadoutSlot::Magazine => "capacity <-> handling",
    }
}

/// Format the title screen's build stamp — e.g. `build dev · v0.0.0`.
pub fn build_stamp(channel: &str, version: &str) -> String {
    format!(
        "build {} · v{}",
        channel.trim().to_ascii_lowercase(),
        version.trim()
    )
}

/// The build channel from cargo's debug-assertions flag: a debug build is "dev", a release "release".
pub fn build_channel(debug_assertions: bool) -> &'static str {
    if debug_assertions {
        "dev"
    } else {
        "release"
    }
}

/// Convert an egui pointer position (logical points, origin top-left, y down) into the title
/// backdrop's NDC ([-1, 1] on both axes, origin centre, **y up**) given the surface size in the same
/// logical points. Pure arithmetic — extracted from the [`EguiShell`] glue exactly so the cursor
/// mapping the 3D backdrop reacts to is unit-tested (the wgpu compositing around it is exempt). This
/// is host presentation math, not sim — the f32s here never touch `core` (invariant #1 is about the
/// sim, not the renderer's float boundary).
pub fn pointer_to_ndc(pos: [f32; 2], size_points: [f32; 2]) -> [f32; 2] {
    // Guard a zero/negative extent (a not-yet-sized surface) so we never divide by zero.
    let w = if size_points[0] > 0.0 { size_points[0] } else { 1.0 };
    let h = if size_points[1] > 0.0 { size_points[1] } else { 1.0 };
    [(pos[0] / w) * 2.0 - 1.0, 1.0 - (pos[1] / h) * 2.0]
}

// ---- The Settings screen — pure seam (unit-tested) ----------------------------------------------

/// The render-quality preference exposed on the Settings screen. `Auto` lets the in-match tier
/// controller (`render::tiers`) pick from thermals; the explicit tiers pin it. A small cycler enum so
/// the screen needs no slider for a discrete choice. Pure data — no GPU.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum QualityChoice {
    /// Let the thermal/perf tier controller choose (the shipped default).
    #[default]
    Auto,
    Low,
    Medium,
    High,
}

impl QualityChoice {
    /// Cycle order for the `<`/`>` style toggle.
    pub const ALL: [QualityChoice; 4] = [
        QualityChoice::Auto,
        QualityChoice::Low,
        QualityChoice::Medium,
        QualityChoice::High,
    ];

    /// The on-screen label.
    pub fn label(self) -> &'static str {
        match self {
            QualityChoice::Auto => "Auto",
            QualityChoice::Low => "Low",
            QualityChoice::Medium => "Medium",
            QualityChoice::High => "High",
        }
    }

    /// The next choice in [`Self::ALL`], wrapping — what the cycler advances to.
    pub fn next(self) -> QualityChoice {
        let i = Self::ALL.iter().position(|&c| c == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// Host-side player preferences edited on the Settings screen. **Presentation only** — none of these
/// reach the deterministic sim (invariant #1 is about the sim's fixed-point state, not the host's
/// float prefs). Fullscreen is deliberately **not** here — the window mode's single source of truth
/// is `App::fullscreen` (the Settings checkbox reflects it and emits
/// [`SettingsAction::ToggleFullscreen`]).
///
/// Wiring status: **`master_volume` + `sfx_volume`** drive the desktop audio sink (the host pushes
/// them via `DesktopAudio::set_gains` each match frame) and **`mouse_sensitivity` + `invert_look_y`**
/// shape the desktop look input (`DesktopInput::set_look_prefs`). **`music_volume`** is a dormant
/// stored pref — there is no music cue to scale yet — and **`quality`** is not yet wired into
/// `render::tiers`. Both survive across screens for the session.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SettingsState {
    /// Master output gain, `0.0..=1.0`.
    pub master_volume: f32,
    /// SFX bus gain, `0.0..=1.0`.
    pub sfx_volume: f32,
    /// Music bus gain, `0.0..=1.0`.
    pub music_volume: f32,
    /// Mouse-look sensitivity multiplier, [`Self::SENS_MIN`]`..=`[`Self::SENS_MAX`].
    pub mouse_sensitivity: f32,
    /// Invert the embodied vertical look axis.
    pub invert_look_y: bool,
    /// Render-quality preference (see [`QualityChoice`]).
    pub quality: QualityChoice,
}

impl Default for SettingsState {
    fn default() -> Self {
        SettingsState {
            master_volume: 0.8,
            sfx_volume: 0.8,
            music_volume: 0.6,
            mouse_sensitivity: 1.0,
            invert_look_y: false,
            quality: QualityChoice::Auto,
        }
    }
}

impl SettingsState {
    /// Sensitivity slider bounds (a multiplier around 1.0).
    pub const SENS_MIN: f32 = 0.1;
    pub const SENS_MAX: f32 = 3.0;

    /// Clamp every field back into its valid range — called after the egui sliders write, so a future
    /// non-slider edit path (config import, keybind) can never leave an out-of-range value. Pure.
    pub fn clamp(&mut self) {
        for v in [
            &mut self.master_volume,
            &mut self.sfx_volume,
            &mut self.music_volume,
        ] {
            *v = v.clamp(0.0, 1.0);
        }
        self.mouse_sensitivity = self.mouse_sensitivity.clamp(Self::SENS_MIN, Self::SENS_MAX);
    }

    /// Restore the shipped defaults — the Settings RESET button.
    pub fn reset(&mut self) {
        *self = SettingsState::default();
    }
}

/// An action the Settings screen can emit in a frame. Slider/checkbox edits mutate [`SettingsState`]
/// in place (no action — they're the "Stay" case); only these discrete controls are actions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsAction {
    /// Flip borderless fullscreen (the window mode lives on the host).
    ToggleFullscreen,
    /// Restore the shipped defaults.
    ResetDefaults,
    /// Open the About / controls-reference screen.
    About,
    /// Return to the title screen.
    Back,
}

/// The screen-level outcome of a [`SettingsAction`] once applied — what the run loop switches on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsStep {
    /// Stay on Settings (a pref edit, a reset, or nothing this frame).
    Stay,
    /// Toggle fullscreen and stay (the host flips the window mode).
    ToggleFullscreen,
    /// Leave for the About screen.
    About,
    /// Return to the title screen.
    Back,
}

/// Apply a [`SettingsAction`] to the preferences and report the resulting screen step. `ResetDefaults`
/// mutates state and stays; the rest are screen/host transitions. Pure (no egui/window) — the
/// Settings testable decision seam, mirroring [`apply_loadout_action`].
pub fn apply_settings_action(action: SettingsAction, state: &mut SettingsState) -> SettingsStep {
    match action {
        SettingsAction::ResetDefaults => {
            state.reset();
            SettingsStep::Stay
        }
        SettingsAction::ToggleFullscreen => SettingsStep::ToggleFullscreen,
        SettingsAction::About => SettingsStep::About,
        SettingsAction::Back => SettingsStep::Back,
    }
}

// ---- The Profile screen — pure seam (unit-tested) -----------------------------------------------

/// The player's preferred faction (the real-army roster, `docs/factions.md`). A cosmetic/pre-match
/// preference only — it never constrains fairness (the roster is fairness-bounded). Pure data.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FactionPref {
    #[default]
    UsArmy,
    FrenchArmy,
}

impl FactionPref {
    /// The on-screen label.
    pub fn label(self) -> &'static str {
        match self {
            FactionPref::UsArmy => "US Army",
            FactionPref::FrenchArmy => "French Army",
        }
    }

    /// The next faction, wrapping — what the cycler advances to.
    pub fn next(self) -> FactionPref {
        match self {
            FactionPref::UsArmy => FactionPref::FrenchArmy,
            FactionPref::FrenchArmy => FactionPref::UsArmy,
        }
    }
}

/// Host-side player identity / record shown on the Profile screen. Presentation only — never touches
/// the sim. The lifetime record is a real counter the host *will* bump at match end (placeholder
/// zeroes today; the post-match summary is the natural writer). Persists across matches like the
/// loadout.
#[derive(Clone, PartialEq, Debug)]
pub struct ProfileState {
    /// The player's chosen callsign (display name). Sanitised by [`sanitize_callsign`] on commit.
    pub callsign: String,
    /// Preferred faction (see [`FactionPref`]).
    pub faction: FactionPref,
    /// Lifetime matches played.
    pub matches_played: u32,
    /// Lifetime wins (`<= matches_played`).
    pub wins: u32,
}

impl Default for ProfileState {
    fn default() -> Self {
        ProfileState {
            callsign: DEFAULT_CALLSIGN.to_string(),
            faction: FactionPref::UsArmy,
            matches_played: 0,
            wins: 0,
        }
    }
}

/// The fallback callsign when the field is left empty.
pub const DEFAULT_CALLSIGN: &str = "Commander";
/// Maximum callsign length (chars) — keeps it fitting the field and the in-match nameplate.
pub const CALLSIGN_MAX: usize = 18;

/// Normalise a raw callsign: trim surrounding whitespace, truncate to [`CALLSIGN_MAX`] characters,
/// and fall back to [`DEFAULT_CALLSIGN`] when the result is empty. Pure — the Profile screen's one bit
/// of real input validation, so it is unit-tested. Char-based truncation (not byte) so a multi-byte
/// name can't be split mid-codepoint.
pub fn sanitize_callsign(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return DEFAULT_CALLSIGN.to_string();
    }
    trimmed.chars().take(CALLSIGN_MAX).collect()
}

/// Win-rate percentage (`0..=100`), or `None` when no matches have been played (a clean "--" readout
/// instead of a divide-by-zero). Integer math, rounded down. Pure — unit-tested.
pub fn win_rate_pct(wins: u32, played: u32) -> Option<u32> {
    if played == 0 {
        None
    } else {
        // u64 to avoid overflow on `wins * 100` for large lifetime counts.
        Some(((wins as u64 * 100) / played as u64) as u32)
    }
}

/// An action the Profile screen can emit. The callsign `TextEdit` mutates [`ProfileState::callsign`]
/// in place (the "Stay" case); these are the discrete controls.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProfileAction {
    /// Cycle the preferred faction.
    CycleFaction,
    /// Zero the lifetime record.
    ResetStats,
    /// Return to the title screen (sanitises the callsign on the way out).
    Back,
}

/// The screen-level outcome of a [`ProfileAction`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProfileStep {
    /// Stay on Profile.
    Stay,
    /// Return to the title screen.
    Back,
}

/// Apply a [`ProfileAction`] to the profile and report the resulting screen step. `Back` sanitises the
/// callsign (so an empty/over-long field commits a clean value). Pure — the Profile decision seam.
pub fn apply_profile_action(action: ProfileAction, profile: &mut ProfileState) -> ProfileStep {
    match action {
        ProfileAction::CycleFaction => {
            profile.faction = profile.faction.next();
            ProfileStep::Stay
        }
        ProfileAction::ResetStats => {
            profile.matches_played = 0;
            profile.wins = 0;
            ProfileStep::Stay
        }
        ProfileAction::Back => {
            profile.callsign = sanitize_callsign(&profile.callsign);
            ProfileStep::Back
        }
    }
}

// ---- The About / controls-reference screen — pure seam (unit-tested) ----------------------------

/// One control-reference row: the input and what it does, grouped by layer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ControlRow {
    /// The layer this binding belongs to ("COMMAND", "EMBODIED", "GLOBAL").
    pub group: &'static str,
    /// The key/mouse input (ASCII only — it renders in egui's default font).
    pub keys: &'static str,
    /// What the input does.
    pub action: &'static str,
}

/// The one-paragraph "what is this game" pitch shown atop the About / field-manual screen — the
/// canonical blurb, kept **verbatim** in step with Android's `FIELD_MANUAL_BLURB`
/// (`FieldManual.kt`) so both shells read identically (A2 parity). Android's fuller three-sentence
/// copy is the source of truth; this must match it byte-for-byte. Pure `&'static str`, so it's
/// unit-tested (non-empty, ASCII — never a tofu glyph in egui's default font).
pub const FIELD_MANUAL_BLURB: &str =
    "Command and grow your camps from above, then possess a single soldier and fight it in first \
     person while the strategic map goes dark. One commander does both jobs; the tension is your \
     divided attention. Stay embodied too long and the map you left behind moves without you.";

/// The desktop controls reference shown on the About screen — the **real** default keymap (kept in
/// sync with `pal-desktop`'s `DesktopInput` doc + `app`'s host keys). Static data, so it's unit-tested
/// for shape (every group present, no empty cells). ASCII only — never a tofu glyph.
///
/// The list is **prefixed by a non-keybinding "GOING DARK" concept section** (A1 parity with
/// Android's `fieldManualSections`): those rows reuse the `ControlRow` shape with the concept name in
/// the `keys` column and its one-line framing in `action`, so the grouped `about_ui` renderer draws
/// them ahead of the COMMAND/EMBODIED/GLOBAL keymap groups with no special case. Content is Android's
/// verbatim, with its "Going dark" em-dash rendered as ASCII `--` (the file's default-font/no-tofu
/// rule is the one deviation).
pub fn controls_reference() -> &'static [ControlRow] {
    const fn row(group: &'static str, keys: &'static str, action: &'static str) -> ControlRow {
        ControlRow {
            group,
            keys,
            action,
        }
    }
    // A `static` (not a returned temporary) so the slice is genuinely `'static`.
    static ROWS: &[ControlRow] = &[
        // The "GOING DARK" concept block — the game's framing ahead of the keymap (mirrors Android's
        // `fieldManualSections` leading section). Not keybindings: the `keys` cell is the concept
        // name, `action` its one-line explanation.
        row("GOING DARK", "Embodiment", "Possess one unit and fight it in first person"),
        row("GOING DARK", "Going dark", "Embodying blacks out the strategic map -- alerts, not intel"),
        row("GOING DARK", "Surface", "Eject back to command; death also ejects you (no respawn)"),
        row("GOING DARK", "Stay fair", "While dark you get a directional flash + audio, never a map reveal"),
        // Command layer (RTS) — pal-desktop keymap (D42 classic-RTS split).
        row("COMMAND", "Left-click", "Select / band-select"),
        row("COMMAND", "Right-click", "Move or attack-move the selection"),
        row("COMMAND", "B", "Place a Camp at the cursor"),
        row("COMMAND", "R / H", "Queue a Rifleman / Heavy at the camp"),
        row("COMMAND", "U", "Upgrade the active camp"),
        row("COMMAND", "1 - 0", "Order / stance vocabulary slots"),
        // Embodiment layer (FPS).
        row("EMBODIED", "E", "Embody the targeted unit"),
        row("EMBODIED", "Q", "Surface (eject back to command)"),
        row("EMBODIED", "W A S D", "Move"),
        row("EMBODIED", "Mouse", "Look"),
        row("EMBODIED", "Left-click / Space", "Fire"),
        // Global host keys (app/src/main.rs).
        row("GLOBAL", "Esc", "Pause / resume"),
        row("GLOBAL", "Left Alt", "Free the cursor (hold)"),
        row("GLOBAL", "F11", "Toggle fullscreen"),
        row("GLOBAL", "F3", "Toggle the debug overlay"),
    ];
    ROWS
}

// ---- The Operations-hub mission-select screen — pure seam (unit-tested) -------------------------

/// An action the mission-select (Operations-hub) screen can emit in a frame. The hub reads the
/// campaign through [`Campaign::mission_select`] (host-side, never the sim — invariants #1/#7); the
/// only outcomes are launching a node's briefing or backing out to the title.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MissionSelectAction {
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
pub fn playable_node(entry: &MissionSelectEntry) -> Option<NodeId> {
    entry.progress.is_playable().then_some(entry.node)
}

// ---- The briefing screen — pure seam (unit-tested) ----------------------------------------------

/// An action the briefing screen can emit in a frame. `CycleDifficulty` advances the host-side
/// replay-tier selector (a stay-on-screen edit); `Deploy`/`Back` are screen transitions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BriefingAction {
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
pub enum BriefingOutcome {
    /// Stay on the briefing (a difficulty edit, or nothing this frame).
    Stay,
    /// Launch the mission at the selected campaign `difficulty` (recorded against the **clear** on a
    /// win; the commander-AI tier stays the mission's authored one — Q21).
    Launch { difficulty: Difficulty },
    /// Return to the mission-select hub.
    Back,
}

/// The next campaign [`Difficulty`] tier, wrapping through [`Difficulty::ALL`]
/// (`Recruit → Regular → Veteran → Elite → Recruit`). Pure helper for the briefing's difficulty
/// cycler — `core::campaign::Difficulty` derives `Ord` but ships no `next`, so the shell owns the
/// cycle order here (and tests it). ASCII-free of any sim concern; this is presentation only.
pub fn next_difficulty(d: Difficulty) -> Difficulty {
    let all = Difficulty::ALL;
    let i = all.iter().position(|&x| x == d).unwrap_or(0);
    all[(i + 1) % all.len()]
}

/// The human-readable label for a campaign [`Difficulty`] tier (the briefing's difficulty cycler
/// readout). `core::campaign::Difficulty::id` returns a stable key (`"recruit"`…) for localization;
/// the shell owns the display string. ASCII only so it can never tofu in egui's default font. Pure —
/// unit-tested.
pub fn difficulty_label(d: Difficulty) -> &'static str {
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
/// launch tier (the host records it against `Campaign::clear` on a win); the enemy commander's
/// aggression is **not** taken from here — it stays the mission's authored tier (the 4-tier campaign
/// → 3-tier commander mapping is open question Q21).
pub fn apply_briefing_action(action: BriefingAction, selected: &mut Difficulty) -> BriefingOutcome {
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

// ---- The "going-dark" palette + theme -----------------------------------------------------------

// A near-black field, dim chrome, one amber alert accent (the game's directional-alert colour).
// These five base values (INK/PANEL/BONE/ASH/AMBER) are kept **bit-identical to the canonical
// renderer palette** documented in `render/src/theme.rs` (gonedark_render::theme) so the out-of-match
// egui chrome and the in-match wgpu HUD read as one art-directed identity. (`app` now *does* depend
// on `gonedark-render` — for the 3D title backdrop, see [`EguiShell`] — but we still mirror the hex
// here rather than pull the colour table through that dep: egui wants `Color32`, render wants linear
// `[f32; 4]`, and this egui chrome predates the dep. The two palettes must still move together; see
// the doc-hex annotations in theme.rs.)
const INK: egui::Color32 = egui::Color32::from_rgb(0x07, 0x09, 0x0C);
const PANEL: egui::Color32 = egui::Color32::from_rgb(0x12, 0x18, 0x20);
const BONE: egui::Color32 = egui::Color32::from_rgb(0xE7, 0xEC, 0xEF);
const ASH: egui::Color32 = egui::Color32::from_rgb(0x8A, 0x94, 0x9C);
const AMBER: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x79, 0x1F);
// One step lighter than PANEL for raised/hovered/active surfaces; a hairline RIM lifts a card off
// the ink; MUTED is the dimmest legible text (mirrors theme.rs PANEL_RAISED/RIM/MUTED).
const PANEL_RAISED: egui::Color32 = egui::Color32::from_rgb(0x1B, 0x25, 0x31);
const RIM: egui::Color32 = egui::Color32::from_rgb(0x29, 0x30, 0x42);
const MUTED: egui::Color32 = egui::Color32::from_rgb(0x61, 0x68, 0x75);
// A semi-opaque PANEL for chrome floated over the live 3D title backdrop: the PANEL hue at ~88%
// alpha (224/255) so the moving sky reads faintly behind a card without costing text legibility.
// Only the title screen (which has a backdrop behind it) uses it; the loadout screen keeps the
// opaque PANEL. `Color32` stores PREMULTIPLIED alpha and only `from_rgba_premultiplied` is `const`,
// so the channels here are PANEL (0x12/0x18/0x20) already multiplied by 224/255 (→ 16/21/28); this
// is the const-fn equivalent of `from_rgba_unmultiplied(0x12, 0x18, 0x20, 224)`.
const PANEL_GLASS: egui::Color32 = egui::Color32::from_rgba_premultiplied(16, 21, 28, 224);

// The desktop-shell type scale (egui point sizes). One small, fixed ramp so every screen shares a
// heading/body/caption hierarchy instead of each call site picking an ad-hoc glyph size — the
// pixel-space analogue of theme.rs's NDC `TYPE_*` scale. `DISPLAY` is the title hero; `HEADING` the
// per-screen banner (GUNSMITH); `BUTTON`/`BODY`/`CAPTION` the rest.
const TYPE_DISPLAY: f32 = 52.0;
const TYPE_HEADING: f32 = 30.0;
const TYPE_SUBHEAD: f32 = 16.0;
const TYPE_BUTTON: f32 = 16.0;
const TYPE_BODY: f32 = 14.0;
const TYPE_CAPTION: f32 = 12.0;

/// Build the shell's cohesive dark [`egui::Style`] — the single source of truth for the title /
/// gunsmith / settings chrome's look (fills, widget ramp, corner radii, spacing, and the
/// heading->caption type scale). Pure data: `egui::Style`/`Visuals` are plain structs with no GPU or
/// window, so this is unit-tested below (unlike the [`EguiShell`] glue that *applies* it). Keeping it
/// pure also means a retune is one function, asserted by tests, rather than scattered `set_*` calls.
fn shell_style() -> egui::Style {
    use egui::{CornerRadius, FontFamily, FontId, Stroke, TextStyle};

    let mut style = egui::Style::default();
    let mut v = egui::Visuals::dark();

    // Surfaces: ink behind everything, panel for cards, amber as the lone signal accent.
    v.panel_fill = INK;
    v.window_fill = PANEL;
    v.window_stroke = Stroke::new(1.0, RIM);
    v.window_corner_radius = CornerRadius::same(10);
    v.faint_bg_color = PANEL;
    v.extreme_bg_color = INK;
    v.hyperlink_color = AMBER;
    v.selection.bg_fill = egui::Color32::from_rgba_unmultiplied(0xE0, 0x79, 0x1F, 96);
    v.selection.stroke = Stroke::new(1.0, AMBER);

    // The widget interaction ramp: a button at rest sits on PANEL with a RIM hairline; hover/active
    // lift it to PANEL_RAISED, ring it in amber, and nudge it out by a pixel for tactile feedback.
    // Secondary buttons (no explicit fill) ride this ramp directly, so their fill changes on hover;
    // the primary (amber-filled) button keeps its fill but still gains the amber rim + expansion.
    let radius = CornerRadius::same(6);
    let w = &mut v.widgets;

    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, RIM);
    w.noninteractive.fg_stroke = Stroke::new(1.0, BONE);
    w.noninteractive.corner_radius = radius;

    w.inactive.bg_fill = PANEL;
    w.inactive.weak_bg_fill = PANEL;
    w.inactive.bg_stroke = Stroke::new(1.0, RIM);
    w.inactive.fg_stroke = Stroke::new(1.0, BONE);
    w.inactive.corner_radius = radius;
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = PANEL_RAISED;
    w.hovered.weak_bg_fill = PANEL_RAISED;
    w.hovered.bg_stroke = Stroke::new(1.0, AMBER);
    w.hovered.fg_stroke = Stroke::new(1.5, BONE);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 1.0;

    w.active.bg_fill = PANEL_RAISED;
    w.active.weak_bg_fill = PANEL_RAISED;
    w.active.bg_stroke = Stroke::new(1.5, AMBER);
    w.active.fg_stroke = Stroke::new(1.5, BONE);
    w.active.corner_radius = radius;
    w.active.expansion = 1.0;

    // Open menus/combos mirror the pressed look (WidgetVisuals is Copy).
    w.open = w.active;

    style.visuals = v;

    // Generous, even spacing so rows and buttons breathe.
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(16.0, 9.0);

    // The default text styles follow the scale. Per-widget `RichText::size`/`color` still override
    // these where a screen wants the title hero or an amber readout, but unstyled text is consistent.
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(TYPE_HEADING, FontFamily::Proportional),
        ),
        (
            TextStyle::Body,
            FontId::new(TYPE_BODY, FontFamily::Proportional),
        ),
        (
            TextStyle::Button,
            FontId::new(TYPE_BUTTON, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(TYPE_CAPTION, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(TYPE_BODY, FontFamily::Monospace),
        ),
    ]
    .into();

    style
}

// ---- The egui glue (device-gated chrome; exempt from unit tests) --------------------------------

/// The egui-backed title screen: an egui context, the winit→egui input bridge, and the egui-wgpu
/// renderer that paints into the same surface the engine uses. Owns no game state.
pub struct EguiShell {
    ctx: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    stamp: String,
    /// The live 3D title backdrop (`render` crate). Painted behind the title-screen egui pass (which
    /// then composites with `LoadOp::Load`). `Option` so a future fallible build could degrade to a
    /// flat-clear title without panicking the shell — the pinned `new` is infallible today, so it is
    /// always `Some`. Only the title screen uses it; the loadout screen clears its own ink panel.
    backdrop: Option<TitleBackdrop>,
}

impl EguiShell {
    /// Build the shell against the desktop surface's device/format and the window (for input/DPI).
    /// `stamp` is the already-formatted build/version line (see [`build_stamp`]).
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        window: &Window,
        stamp: String,
    ) -> Self {
        let ctx = egui::Context::default();
        // Install the art-directed shell theme once on the context (the pure [`shell_style`] is the
        // single source of truth; this is the one place the glue applies it). egui 0.35 keeps a style
        // per theme, so pin the preference to Dark and write our style into every theme slot — the
        // shell is dark-only, never tracking the system light/dark setting.
        ctx.set_theme(egui::ThemePreference::Dark);
        ctx.all_styles_mut(|style| *style = shell_style());

        let state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            None, // native pixels-per-point — let egui read the window scale factor
            None, // system theme
            None, // max texture side
        );
        let renderer = egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default());

        // Build the live 3D title backdrop against the same device/format the egui pass and the
        // engine share. Infallible per the pinned API, so always `Some` today.
        let backdrop = Some(TitleBackdrop::new(device, format));

        EguiShell {
            ctx,
            state,
            renderer,
            stamp,
            backdrop,
        }
    }

    /// Feed one winit window event to egui (pointer/keys). Returns whether egui consumed it.
    pub fn on_window_event(&mut self, window: &Window, event: &winit::event::WindowEvent) -> bool {
        self.state.on_window_event(window, event).consumed
    }

    /// Draw the title screen for one frame and return a clicked [`TitleAction`], if any. Pure
    /// presentation — it never touches sim state.
    pub fn draw_title(&mut self, surface: &mut DesktopRenderSurface) -> Option<TitleAction> {
        // Clone the stamp so the immediate-mode closure doesn't alias the `&mut self` borrow
        // `run_and_paint` takes.
        let stamp = self.stamp.clone();
        // `with_backdrop = true`: paint the live 3D backdrop into the frame first, then composite the
        // title HUD over it (`LoadOp::Load`).
        self.run_and_paint(surface, true, |ui| title_ui(ui, &stamp))
    }

    /// Draw the pre-match gunsmith / loadout screen for one frame and return the [`LoadoutAction`]
    /// whose control was used, if any. `editor` is the host-side pre-match selection state (read-only
    /// here — it never reaches the sim). Pure presentation, same paint path as the title screen.
    pub fn draw_loadout(
        &mut self,
        surface: &mut DesktopRenderSurface,
        editor: &LoadoutEditor,
    ) -> Option<LoadoutAction> {
        // `with_backdrop = false`: the gunsmith keeps its opaque ink panel (it has no 3D backdrop),
        // so the egui pass clears as before — no regression to `draw_loadout`.
        self.run_and_paint(surface, false, |ui| loadout_ui(ui, editor))
    }

    /// Draw the Settings screen for one frame and return the [`SettingsAction`] whose control was
    /// used, if any. `state` is the host-side preference model (edited in place by the sliders);
    /// `fullscreen` is the host's current window mode (reflected by the video checkbox). Drawn over the
    /// live 3D backdrop so the out-of-match shell stays cohesive. Pure presentation — never the sim.
    pub fn draw_settings(
        &mut self,
        surface: &mut DesktopRenderSurface,
        state: &mut SettingsState,
        fullscreen: bool,
    ) -> Option<SettingsAction> {
        self.run_and_paint(surface, true, |ui| settings_ui(ui, state, fullscreen))
    }

    /// Draw the player Profile screen for one frame and return the [`ProfileAction`] used, if any.
    /// `profile` is the host-side identity/record (the callsign field edits it in place). Over the
    /// backdrop, same as Settings. Pure presentation.
    pub fn draw_profile(
        &mut self,
        surface: &mut DesktopRenderSurface,
        profile: &mut ProfileState,
    ) -> Option<ProfileAction> {
        self.run_and_paint(surface, true, |ui| profile_ui(ui, profile))
    }

    /// Draw the About / controls-reference screen for one frame. Returns `true` on BACK (the only
    /// control), so the run loop returns to Settings. Static content over the backdrop. Pure.
    pub fn draw_about(&mut self, surface: &mut DesktopRenderSurface) -> bool {
        let stamp = self.stamp.clone();
        self.run_and_paint(surface, true, |ui| about_ui(ui, &stamp).then_some(()))
            .is_some()
    }

    /// Draw the Operations-hub **mission-select** screen for one frame and return the
    /// [`MissionSelectAction`] used, if any. `campaign` is the host-side campaign model (read-only
    /// here — it is never sim state, never checksummed). Over the live 3D backdrop, same as the
    /// other out-of-match screens. Pure presentation — the tile-launchable gate lives in the pure
    /// [`playable_node`] seam, this is the device-gated glue.
    pub fn draw_mission_select(
        &mut self,
        surface: &mut DesktopRenderSurface,
        campaign: &Campaign,
    ) -> Option<MissionSelectAction> {
        self.run_and_paint(surface, true, |ui| mission_select_ui(ui, campaign))
    }

    /// Draw the **briefing** screen for `node` for one frame and return the [`BriefingAction`] used,
    /// if any. Reads the node's briefing through [`Campaign::briefing`]; `selected` is the host-side
    /// replay-tier selector the difficulty cycler edits in place. Over the backdrop. Pure
    /// presentation — the decision logic is the pure [`apply_briefing_action`] seam.
    pub fn draw_briefing(
        &mut self,
        surface: &mut DesktopRenderSurface,
        campaign: &Campaign,
        node: NodeId,
        selected: Difficulty,
    ) -> Option<BriefingAction> {
        self.run_and_paint(surface, true, |ui| briefing_ui(ui, campaign, node, selected))
    }

    /// Run one egui frame (`build` lays out the UI and returns this frame's action) and paint the
    /// tessellated output into a freshly-acquired surface frame. The shared paint path behind both
    /// [`draw_title`](Self::draw_title) and [`draw_loadout`](Self::draw_loadout) — device-gated glue,
    /// exempt from unit tests; the per-screen *logic* it drives lives in the pure `*_ui` builders and
    /// the pure action seams above.
    ///
    /// When `with_backdrop` is set (the title screen), the live 3D
    /// [`gonedark_render::title_backdrop::TitleBackdrop`] is painted into the acquired view FIRST
    /// (it clears the view to its sky and submits its own encoder), and the egui pass then composites
    /// over it with `LoadOp::Load`. Otherwise (the gunsmith) the egui pass clears the view itself —
    /// the original opaque behaviour, unchanged. The animation clock + cursor handed to the backdrop
    /// come from this just-run frame's egui input (a one-frame lag is fine), with the pixel→NDC
    /// conversion living in the pure [`pointer_to_ndc`] seam.
    fn run_and_paint<T>(
        &mut self,
        surface: &mut DesktopRenderSurface,
        with_backdrop: bool,
        // `egui::Context::run_ui` takes an `FnMut` (it may run the UI more than once for a sizing
        // pass), so the per-screen builder is `FnMut` too.
        mut build: impl FnMut(&mut egui::Ui) -> Option<T>,
    ) -> Option<T> {
        let ctx = self.ctx.clone();

        // Run egui (needs the window for input gather + platform output).
        let raw_input = self.state.take_egui_input(surface.window());
        let mut action = None;
        let full_output = ctx.run_ui(raw_input, |ui| {
            action = build(ui);
        });
        self.state
            .handle_platform_output(surface.window(), full_output.platform_output);

        let ppp = full_output.pixels_per_point;
        let paint_jobs = ctx.tessellate(full_output.shapes, ppp);
        let (w, h) = surface.size();

        // Pull the backdrop's animation clock + cursor from this frame's egui input. `i.time` is a
        // monotonic seconds clock; the latest pointer is in egui logical points (origin top-left),
        // mapped to NDC against the surface size in the same logical points (physical / ppp).
        let time = ctx.input(|i| i.time) as f32;
        let cursor = ctx.input(|i| i.pointer.latest_pos()).map(|p| {
            let size_points = [w as f32 / ppp, h as f32 / ppp];
            pointer_to_ndc([p.x, p.y], size_points)
        });

        // Acquire the frame (owned — the `&mut` surface borrow ends as this returns).
        let Some((frame, view)) = surface.acquire() else {
            return action;
        };

        let device = surface.device();
        let queue = surface.queue();

        // Paint the 3D backdrop into the view BEFORE egui (it clears + submits its own encoder), so
        // the egui pass below loads over it. `self.backdrop`/`self.renderer` are disjoint fields, so
        // this split borrow is fine.
        if with_backdrop {
            if let Some(bd) = self.backdrop.as_mut() {
                bd.render(device, queue, &view, (w, h), time, cursor);
            }
        }

        for (id, delta) in &full_output.textures_delta.set {
            self.renderer.update_texture(device, queue, *id, delta);
        }
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [w, h],
            pixels_per_point: ppp,
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.shell.egui"),
        });
        let user_cmds =
            self.renderer
                .update_buffers(device, queue, &mut encoder, &paint_jobs, &screen);
        {
            // Title: LOAD over the backdrop the pass above painted. Gunsmith: CLEAR to ink (no
            // backdrop), preserving the original opaque look.
            let load = if with_backdrop {
                wgpu::LoadOp::Load
            } else {
                wgpu::LoadOp::Clear(wgpu::Color {
                    r: 0.007,
                    g: 0.009,
                    b: 0.013,
                    a: 1.0,
                })
            };
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.shell.egui_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            // egui-wgpu's `render` wants a `RenderPass<'static>`; the pass borrows only owned locals
            // here, so forgetting the lifetime is sound for the duration of the call.
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, &paint_jobs, &screen);
        }
        queue.submit(user_cmds.into_iter().chain(std::iter::once(encoder.finish())));
        surface.present(frame);
        for id in &full_output.textures_delta.free {
            self.renderer.free_texture(id);
        }
        action
    }
}

/// The shared "menu button" width — every primary/secondary action button is this wide so the action
/// stacks line up into a clean column.
const MENU_BUTTON_W: f32 = 256.0;

/// How a [`menu_button`] reads in the visual hierarchy: the one amber call-to-action, a neutral
/// secondary, or a de-emphasised tertiary (e.g. QUIT / BACK).
#[derive(Clone, Copy)]
enum Emphasis {
    /// Filled amber, ink text — the single primary action on a screen.
    Primary,
    /// Panel-filled, bone text — a normal secondary action (rides the hover/active fill ramp).
    Secondary,
    /// Panel-filled, ash text — a quieter, lower-stakes action.
    Tertiary,
}

/// Draw one full-width menu button in the shell style and report whether it was clicked. Glue (it
/// needs a live `Ui`), so it's exempt from unit tests — the click→action mapping it feeds is what the
/// pure [`resolve_title_action`] / [`apply_loadout_action`] seams cover. Only the primary button sets
/// an explicit fill; secondary/tertiary leave the fill to the widget ramp in [`shell_style`] so they
/// visibly lift on hover.
fn menu_button(ui: &mut egui::Ui, text: &str, emphasis: Emphasis) -> bool {
    use egui::{Button, RichText};
    let fg = match emphasis {
        Emphasis::Primary => INK,
        Emphasis::Secondary => BONE,
        Emphasis::Tertiary => ASH,
    };
    let mut button =
        Button::new(RichText::new(text).color(fg).size(TYPE_BUTTON)).min_size([MENU_BUTTON_W, 46.0].into());
    if matches!(emphasis, Emphasis::Primary) {
        button = button.fill(AMBER);
    }
    ui.add(button).clicked()
}

/// A short amber accent rule, centred under a heading — the one bit of "brand" line work that ties
/// the title and gunsmith screens together. Pure presentation glue (needs a `Ui`/painter).
fn accent_rule(ui: &mut egui::Ui, width: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 2.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, egui::CornerRadius::same(1), AMBER);
}

/// The framed "card" the menu/content column sits in — a PANEL fill with a RIM hairline, rounded,
/// with comfortable inner padding. It shrinks to its content, so inside a `vertical_centered` column
/// it renders as a centred panel rather than a full-bleed band. Glue (returns an egui builder).
fn card_frame() -> egui::Frame {
    egui::Frame::default()
        .fill(PANEL)
        .stroke(egui::Stroke::new(1.0, RIM))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(22))
}

/// The title screen's framed card — [`card_frame`] refilled with the translucent [`PANEL_GLASS`] so
/// the live 3D backdrop bleeds faintly through it. Glue (returns an egui builder).
fn glass_card_frame() -> egui::Frame {
    card_frame().fill(PANEL_GLASS)
}

/// A compact secondary "chip" button for the title screen's top-right utility cluster
/// (SETTINGS / PROFILE) — smaller than the full-width [`menu_button`] so it reads as utility chrome
/// rather than a primary action. Rides the [`shell_style`] widget ramp (lifts to PANEL_RAISED + an
/// amber rim on hover). Glue (needs a live `Ui`); the click→action mapping it feeds is what the pure
/// [`resolve_title_action`] seam covers. Text-only, uppercase ASCII — never a risky glyph (the
/// file's tofu caution: only default-font glyphs like U+00B7 are trusted).
fn chip_button(ui: &mut egui::Ui, text: &str) -> bool {
    use egui::{Button, RichText};
    ui.add(
        Button::new(RichText::new(text).color(BONE).size(TYPE_BODY))
            .min_size([104.0, 32.0].into()),
    )
    .clicked()
}

/// The immediate-mode title-screen UI — a real HUD-anchored landing screen drawn over the live 3D
/// [`gonedark_render::title_backdrop::TitleBackdrop`]. Returns the action whose control was clicked
/// this frame.
///
/// Layout (four floating [`egui::Area`]s anchored to the corners over the backdrop, so the central
/// field stays transparent and the 3D shows through — there is deliberately **no** opaque
/// CentralPanel fill here):
///  - **top-left**  — the brand: GOING DARK hero + amber rule + the COMMAND · EMBODY tagline;
///  - **top-right** — a compact SETTINGS / PROFILE utility chip row;
///  - **bottom-left** — the DEPLOY cluster: CAMPAIGN (the lone amber CTA), PvE, PvP, then QUIT,
///    in a translucent [`glass_card_frame`] so it reads as a deliberate panel;
///  - **bottom-right** — the muted build stamp, the quiet corner opposite the play cluster.
fn title_ui(ui: &mut egui::Ui, stamp: &str) -> Option<TitleAction> {
    use egui::{Align2, Area, Id, RichText};
    let mut action = None;
    // Areas attach to the context, not the parent `Ui`, so they float over the (transparent) root
    // and composite over the backdrop. Clone the ctx so each `.show` is independent.
    let ctx = ui.ctx().clone();

    // ---- Brand, top-left -------------------------------------------------------------------------
    Area::new(Id::new("title.brand"))
        .anchor(Align2::LEFT_TOP, egui::vec2(40.0, 44.0))
        .show(&ctx, |ui| {
            ui.label(
                RichText::new("GOING DARK")
                    .color(BONE)
                    .size(TYPE_DISPLAY)
                    .strong(),
            );
            ui.add_space(10.0);
            accent_rule(ui, 150.0);
            ui.add_space(10.0);
            ui.label(
                // U+00B7 middle dot (the same glyph the build stamp uses) — proven to render in
                // egui's default font, so the tagline can never tofu.
                RichText::new("COMMAND \u{00B7} EMBODY")
                    .color(ASH)
                    .size(TYPE_SUBHEAD),
            );
        });

    // ---- Utility chips, top-right ----------------------------------------------------------------
    Area::new(Id::new("title.utility"))
        .anchor(Align2::RIGHT_TOP, egui::vec2(-32.0, 32.0))
        .show(&ctx, |ui| {
            ui.horizontal(|ui| {
                if chip_button(ui, "SETTINGS") {
                    action = Some(TitleAction::Settings);
                }
                if chip_button(ui, "PROFILE") {
                    action = Some(TitleAction::Profile);
                }
                // The field manual (About) — reachable straight from the title, mirroring Android's
                // title About entry (it is also reachable from Settings).
                if chip_button(ui, "MANUAL") {
                    action = Some(TitleAction::About);
                }
            });
        });

    // ---- Deploy cluster, bottom-left -------------------------------------------------------------
    Area::new(Id::new("title.deploy"))
        .anchor(Align2::LEFT_BOTTOM, egui::vec2(40.0, -40.0))
        .show(&ctx, |ui| {
            glass_card_frame().show(ui, |ui| {
                ui.label(
                    RichText::new("DEPLOY")
                        .color(ASH)
                        .size(TYPE_SUBHEAD)
                        .strong(),
                );
                ui.add_space(6.0);
                accent_rule(ui, 72.0);
                ui.add_space(14.0);
                // One amber call-to-action (CAMPAIGN); the other modes are neutral secondaries; QUIT
                // is the quiet tertiary at the foot.
                if menu_button(ui, "CAMPAIGN", Emphasis::Primary) {
                    action = Some(TitleAction::Campaign);
                }
                ui.add_space(10.0);
                if menu_button(ui, "PvE", Emphasis::Secondary) {
                    action = Some(TitleAction::Pve);
                }
                ui.add_space(10.0);
                if menu_button(ui, "PvP", Emphasis::Secondary) {
                    action = Some(TitleAction::Pvp);
                }
                ui.add_space(14.0);
                if menu_button(ui, "QUIT", Emphasis::Tertiary) {
                    action = Some(TitleAction::Quit);
                }
            });
        });

    // ---- Build stamp, bottom-right (the quiet corner opposite the play cluster) -------------------
    Area::new(Id::new("title.stamp"))
        .anchor(Align2::RIGHT_BOTTOM, egui::vec2(-28.0, -24.0))
        .show(&ctx, |ui| {
            ui.label(RichText::new(stamp).color(MUTED).size(TYPE_CAPTION));
        });

    action
}

/// The immediate-mode gunsmith / loadout screen, drawn into the root [`egui::Ui`] for the frame.
/// Reads the current selection from `editor` (host-side pre-match state — never the sim) and returns
/// the action whose control was used this frame. Layout: a centered column of the three attachment
/// slots — each a `<` / `>` cycler over its current option plus the slot's trade-axis hint — the
/// sidegrade explainer, then DEPLOY / RESET / BACK. All the decision logic is in the pure seam
/// ([`apply_loadout_action`], [`slot_trade_hint`], and the `core::gunsmith`-backed editor); this fn
/// is just the egui glue.
fn loadout_ui(ui: &mut egui::Ui, editor: &LoadoutEditor) -> Option<LoadoutAction> {
    use egui::{Button, Label, RichText};
    let mut action = None;

    egui::CentralPanel::default().show(ui, |ui| {
        let h = ui.available_height();
        ui.vertical_centered(|ui| {
            ui.add_space(h * 0.09);
            // Screen banner + amber rule, mirroring the title hero treatment.
            ui.label(
                RichText::new("GUNSMITH")
                    .color(BONE)
                    .size(TYPE_HEADING)
                    .strong(),
            );
            ui.add_space(8.0);
            accent_rule(ui, 100.0);
            ui.add_space(10.0);
            ui.label(
                RichText::new(
                    "Every attachment is a sidegrade -- it spends one stat to buy another. \
                     No build is strictly better than any other.",
                )
                .color(ASH)
                .size(TYPE_BODY),
            );
            ui.add_space(24.0);

            // The three attachment slots live in a framed card so the cyclers read as a panel.
            card_frame().show(ui, |ui| {
                // One aligned row per attachment slot. The on-screen index `i` is exactly the index
                // the editor's `apply_input` routes on (`LoadoutSlot::from_index`), so the cycler maps
                // 1:1.
                for (i, &slot) in LoadoutSlot::ALL.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [104.0, 32.0],
                            Label::new(
                                RichText::new(slot.label()).color(BONE).size(TYPE_SUBHEAD).strong(),
                            ),
                        );
                        if ui
                            .add_sized([34.0, 32.0], Button::new(RichText::new("<").color(BONE)))
                            .clicked()
                        {
                            action = Some(LoadoutAction::Cycle {
                                slot_index: i,
                                forward: false,
                            });
                        }
                        ui.add_sized(
                            [150.0, 32.0],
                            Label::new(
                                RichText::new(editor.option_label(slot))
                                    .color(AMBER)
                                    .size(TYPE_BODY)
                                    .strong(),
                            ),
                        );
                        if ui
                            .add_sized([34.0, 32.0], Button::new(RichText::new(">").color(BONE)))
                            .clicked()
                        {
                            action = Some(LoadoutAction::Cycle {
                                slot_index: i,
                                forward: true,
                            });
                        }
                        ui.add_sized(
                            [172.0, 32.0],
                            Label::new(
                                RichText::new(slot_trade_hint(slot)).color(MUTED).size(TYPE_CAPTION),
                            ),
                        );
                    });
                    if i + 1 < LoadoutSlot::ALL.len() {
                        ui.add_space(8.0);
                    }
                }
            });

            ui.add_space(22.0);
            if menu_button(ui, "DEPLOY", Emphasis::Primary) {
                action = Some(LoadoutAction::Deploy);
            }
            ui.add_space(10.0);
            if menu_button(ui, "RESET", Emphasis::Secondary) {
                action = Some(LoadoutAction::Reset);
            }
            ui.add_space(10.0);
            if menu_button(ui, "BACK", Emphasis::Tertiary) {
                action = Some(LoadoutAction::Back);
            }
        });
    });

    action
}

/// A centred screen banner — the heading + amber rule treatment the gunsmith/settings/profile/about
/// screens share, so they read as one family. Glue (needs a `Ui`).
fn screen_banner(ui: &mut egui::Ui, title: &str, rule_w: f32) {
    use egui::RichText;
    ui.label(
        RichText::new(title)
            .color(BONE)
            .size(TYPE_HEADING)
            .strong(),
    );
    ui.add_space(8.0);
    accent_rule(ui, rule_w);
    ui.add_space(16.0);
}

/// A left-aligned section sub-heading inside a screen card (e.g. "AUDIO", "CONTROLS"). Glue.
fn section_label(ui: &mut egui::Ui, text: &str) {
    use egui::RichText;
    ui.add_space(6.0);
    ui.label(RichText::new(text).color(ASH).size(TYPE_CAPTION).strong());
    ui.add_space(6.0);
}

/// The transparent full-screen panel the over-backdrop screens (settings/profile/about) sit in, with
/// their content centred in a translucent [`glass_card_frame`]. The central panel paints **no** fill
/// (`Frame::NONE`) so the live 3D backdrop shows through around the card. `build` lays out the card's
/// interior; the whole screen returns whatever `build` produced. Glue.
fn over_backdrop_screen<T>(
    ui: &mut egui::Ui,
    top_frac: f32,
    build: impl FnOnce(&mut egui::Ui) -> T,
) -> T {
    let mut out = None;
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            let h = ui.available_height();
            ui.vertical_centered(|ui| {
                ui.add_space(h * top_frac);
                glass_card_frame().show(ui, |ui| {
                    out = Some(build(ui));
                });
            });
        });
    out.expect("over_backdrop_screen build ran")
}

/// The immediate-mode Settings screen: audio/controls/video preferences in a centred card over the
/// backdrop. Sliders/checkboxes edit `state` in place (then [`SettingsState::clamp`] re-bounds it);
/// the discrete controls return a [`SettingsAction`] the pure [`apply_settings_action`] seam resolves.
/// `fullscreen` is the host's current window mode (reflected by the video checkbox). Glue.
fn settings_ui(
    ui: &mut egui::Ui,
    state: &mut SettingsState,
    fullscreen: bool,
) -> Option<SettingsAction> {
    use egui::{RichText, Slider};
    let mut action = None;

    over_backdrop_screen(ui, 0.10, |ui| {
        ui.set_min_width(420.0);
        screen_banner(ui, "SETTINGS", 96.0);

        section_label(ui, "AUDIO");
        ui.add(Slider::new(&mut state.master_volume, 0.0..=1.0).text("Master"));
        ui.add(Slider::new(&mut state.sfx_volume, 0.0..=1.0).text("SFX"));
        ui.add(Slider::new(&mut state.music_volume, 0.0..=1.0).text("Music"));

        section_label(ui, "CONTROLS");
        ui.add(
            Slider::new(
                &mut state.mouse_sensitivity,
                SettingsState::SENS_MIN..=SettingsState::SENS_MAX,
            )
            .text("Look sensitivity"),
        );
        ui.checkbox(&mut state.invert_look_y, "Invert look Y");

        section_label(ui, "VIDEO");
        // The window-mode source of truth is the host: reflect it, and emit the toggle action rather
        // than editing a second copy here.
        let mut fs = fullscreen;
        if ui.checkbox(&mut fs, "Fullscreen").clicked() {
            action = Some(SettingsAction::ToggleFullscreen);
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Quality").color(BONE).size(TYPE_BODY));
            ui.add_space(8.0);
            // A single cycling button over the discrete tiers (a direct edit — no action needed).
            if ui
                .button(RichText::new(state.quality.label()).color(AMBER).size(TYPE_BODY))
                .clicked()
            {
                state.quality = state.quality.next();
            }
        });

        // Defensive re-clamp after the slider writes (sliders already bound, but a future edit path
        // might not).
        state.clamp();

        ui.add_space(18.0);
        if menu_button(ui, "BACK", Emphasis::Primary) {
            action = Some(SettingsAction::Back);
        }
        ui.add_space(10.0);
        if menu_button(ui, "CONTROLS / ABOUT", Emphasis::Secondary) {
            action = Some(SettingsAction::About);
        }
        ui.add_space(10.0);
        if menu_button(ui, "RESET DEFAULTS", Emphasis::Tertiary) {
            action = Some(SettingsAction::ResetDefaults);
        }
    });

    action
}

/// The immediate-mode Profile screen: callsign, faction preference, and the lifetime record, centred
/// over the backdrop. The callsign `TextEdit` edits `profile` in place (length-capped to
/// [`CALLSIGN_MAX`]); the discrete controls return a [`ProfileAction`] the pure [`apply_profile_action`]
/// seam resolves (BACK sanitises the callsign). Glue.
fn profile_ui(ui: &mut egui::Ui, profile: &mut ProfileState) -> Option<ProfileAction> {
    use egui::{RichText, TextEdit};
    let mut action = None;

    over_backdrop_screen(ui, 0.12, |ui| {
        ui.set_min_width(420.0);
        screen_banner(ui, "PROFILE", 84.0);

        section_label(ui, "IDENTITY");
        ui.horizontal(|ui| {
            ui.add_sized(
                [96.0, 28.0],
                egui::Label::new(RichText::new("Callsign").color(BONE).size(TYPE_BODY)),
            );
            ui.add_sized(
                [220.0, 28.0],
                TextEdit::singleline(&mut profile.callsign).char_limit(CALLSIGN_MAX),
            );
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_sized(
                [96.0, 28.0],
                egui::Label::new(RichText::new("Faction").color(BONE).size(TYPE_BODY)),
            );
            if ui
                .add_sized(
                    [220.0, 28.0],
                    egui::Button::new(
                        RichText::new(profile.faction.label()).color(AMBER).size(TYPE_BODY),
                    ),
                )
                .clicked()
            {
                action = Some(ProfileAction::CycleFaction);
            }
        });

        section_label(ui, "RECORD");
        let rate = match win_rate_pct(profile.wins, profile.matches_played) {
            Some(p) => format!("{p}%"),
            None => "--".to_string(),
        };
        ui.label(
            RichText::new(format!(
                "Matches {}   ·   Wins {}   ·   Win rate {}",
                profile.matches_played, profile.wins, rate
            ))
            .color(ASH)
            .size(TYPE_BODY),
        );

        ui.add_space(18.0);
        if menu_button(ui, "BACK", Emphasis::Primary) {
            action = Some(ProfileAction::Back);
        }
        ui.add_space(10.0);
        if menu_button(ui, "RESET RECORD", Emphasis::Tertiary) {
            action = Some(ProfileAction::ResetStats);
        }
    });

    action
}

/// The immediate-mode About / field-manual screen: the one-line pitch, the real default keymap
/// (grouped), and the build stamp, centred over the backdrop. Returns `true` on BACK. Static content
/// from the pure [`controls_reference`] seam. Glue.
fn about_ui(ui: &mut egui::Ui, stamp: &str) -> bool {
    use egui::{Grid, RichText, ScrollArea};
    let mut back = false;

    over_backdrop_screen(ui, 0.06, |ui| {
        ui.set_min_width(460.0);
        screen_banner(ui, "FIELD MANUAL", 120.0);
        ui.label(RichText::new(FIELD_MANUAL_BLURB).color(ASH).size(TYPE_BODY));
        ui.add_space(14.0);

        // The keymap, grouped by layer. A bounded ScrollArea keeps the card sane on a short window.
        ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            let mut current_group = "";
            for row in controls_reference() {
                if row.group != current_group {
                    section_label(ui, row.group);
                    current_group = row.group;
                }
                Grid::new(("about.controls", row.group, row.keys))
                    .num_columns(2)
                    .min_col_width(150.0)
                    .show(ui, |ui| {
                        ui.label(RichText::new(row.keys).color(AMBER).size(TYPE_BODY).strong());
                        ui.label(RichText::new(row.action).color(BONE).size(TYPE_BODY));
                        ui.end_row();
                    });
            }
        });

        ui.add_space(14.0);
        ui.label(RichText::new(stamp).color(MUTED).size(TYPE_CAPTION));
        ui.add_space(12.0);
        if menu_button(ui, "BACK", Emphasis::Primary) {
            back = true;
        }
    });

    back
}

/// One mission-select tile: a status pill (Locked/Available/Cleared, colour-coded) beside the node
/// title as a full-width button. A **playable** node (Available or already-Cleared/replayable) is an
/// enabled button that emits [`MissionSelectAction::OpenNode`]; a **Locked** node renders disabled and
/// cannot be clicked. The launchable decision is the pure [`playable_node`] seam (double-guarded on
/// the click), so this is the exempt egui glue. Returns the action on a click. ASCII status text only.
fn mission_tile(ui: &mut egui::Ui, entry: &MissionSelectEntry) -> Option<MissionSelectAction> {
    use egui::{Button, Label, RichText};
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
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.add_sized(
            [150.0, 36.0],
            Label::new(RichText::new(status).color(status_color).size(TYPE_CAPTION).strong()),
        );
        let button = Button::new(
            RichText::new(entry.title.clone()).color(title_color).size(TYPE_BODY).strong(),
        )
        .min_size([280.0, 36.0].into());
        clicked = ui.add_enabled(playable, button).clicked();
    });
    clicked
        .then(|| playable_node(entry).map(MissionSelectAction::OpenNode))
        .flatten()
}

/// The immediate-mode Operations-hub mission-select screen: the campaign's nodes as
/// status-coded tiles in a card over the backdrop, then BACK. Reads
/// [`Campaign::mission_select`] (host-side, never the sim); each tile's launchability + the click
/// routing go through the pure [`playable_node`] seam. Glue.
fn mission_select_ui(ui: &mut egui::Ui, campaign: &Campaign) -> Option<MissionSelectAction> {
    use egui::RichText;
    let mut action = None;

    over_backdrop_screen(ui, 0.07, |ui| {
        ui.set_min_width(500.0);
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

        card_frame().show(ui, |ui| {
            let entries = campaign.mission_select();
            for (i, entry) in entries.iter().enumerate() {
                if let Some(act) = mission_tile(ui, entry) {
                    action = Some(act);
                }
                if i + 1 < entries.len() {
                    ui.add_space(8.0);
                }
            }
        });

        ui.add_space(20.0);
        if menu_button(ui, "BACK", Emphasis::Tertiary) {
            action = Some(MissionSelectAction::Back);
        }
    });

    action
}

/// The immediate-mode briefing screen for one campaign node: the title, the briefing copy, a replay
/// **difficulty** cycler (the host-side `selected` tier), the clear status, then DEPLOY / BACK.
/// Reads the node through [`Campaign::briefing`]; the discrete controls return a [`BriefingAction`]
/// the pure [`apply_briefing_action`] seam resolves. An out-of-range node degrades to a BACK-only
/// card. Glue.
fn briefing_ui(
    ui: &mut egui::Ui,
    campaign: &Campaign,
    node: NodeId,
    selected: Difficulty,
) -> Option<BriefingAction> {
    use egui::{Button, Label, RichText};
    let mut action = None;

    over_backdrop_screen(ui, 0.07, |ui| {
        ui.set_min_width(500.0);
        let Some(b) = campaign.briefing(node) else {
            // The hub only opens playable, in-range nodes, so this is purely defensive.
            screen_banner(ui, "BRIEFING", 110.0);
            ui.label(RichText::new("No such operation.").color(ASH).size(TYPE_BODY));
            ui.add_space(16.0);
            if menu_button(ui, "BACK", Emphasis::Primary) {
                action = Some(BriefingAction::Back);
            }
            return;
        };

        screen_banner(ui, &b.title.to_uppercase(), 130.0);
        ui.label(RichText::new(b.briefing.clone()).color(ASH).size(TYPE_BODY));
        ui.add_space(16.0);

        card_frame().show(ui, |ui| {
            // Difficulty cycler — the replay tier the CLEAR is recorded against on a win. (The enemy
            // commander's aggression is NOT taken from here; it stays the mission's authored tier —
            // the 4-tier campaign → 3-tier commander mapping is open question Q21.)
            ui.horizontal(|ui| {
                ui.add_sized(
                    [120.0, 32.0],
                    Label::new(RichText::new("Difficulty").color(BONE).size(TYPE_BODY)),
                );
                if ui
                    .add_sized(
                        [200.0, 32.0],
                        Button::new(
                            RichText::new(difficulty_label(selected)).color(AMBER).size(TYPE_BODY).strong(),
                        ),
                    )
                    .clicked()
                {
                    action = Some(BriefingAction::CycleDifficulty);
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

        ui.add_space(20.0);
        if menu_button(ui, "DEPLOY", Emphasis::Primary) {
            action = Some(BriefingAction::Deploy);
        }
        ui.add_space(10.0);
        if menu_button(ui, "BACK", Emphasis::Tertiary) {
            action = Some(BriefingAction::Back);
        }
    });

    action
}

#[cfg(test)]
mod tests {
    //! The pure seam only — the egui glue (`EguiShell`/`title_ui`/`loadout_ui`/`run_and_paint`) needs
    //! a GPU + window and is the exempt device-gated chrome (D32 / CLAUDE.md testing rule).
    use super::*;
    // Re-imported explicitly: the parent's `use` of these is private, so it isn't pulled in by the
    // `super::*` glob above.
    use gonedark_engine::loadout_ui::{LoadoutEditor, LoadoutSlot};

    #[test]
    fn debug_build_is_the_dev_channel() {
        assert_eq!(build_channel(true), "dev");
    }

    #[test]
    fn release_build_is_the_release_channel() {
        assert_eq!(build_channel(false), "release");
    }

    #[test]
    fn stamp_formats_channel_and_version() {
        assert_eq!(build_stamp("dev", "0.0.0"), "build dev · v0.0.0");
    }

    #[test]
    fn stamp_normalises_case_and_trims_whitespace() {
        assert_eq!(build_stamp("  RELEASE ", " 1.2.3 "), "build release · v1.2.3");
    }

    #[test]
    fn campaign_opens_the_operations_hub() {
        // CAMPAIGN now routes to the Operations-hub mission-select (the PvE pillar, D58), not
        // straight to the gunsmith.
        assert_eq!(
            resolve_title_action(TitleAction::Campaign),
            HostTransition::OpenMissionSelect
        );
    }

    #[test]
    fn pve_and_pvp_still_open_the_gunsmith() {
        // PvE/PvP keep the direct gunsmith→match flow (no lobby / skirmish picker yet) — each routes
        // through the loadout screen first, and Deploy from there creates the `Game`.
        for mode in [TitleAction::Pve, TitleAction::Pvp] {
            assert_eq!(
                resolve_title_action(mode),
                HostTransition::OpenLoadout,
                "{mode:?} must open the gunsmith"
            );
        }
    }

    #[test]
    fn settings_opens_settings() {
        assert_eq!(
            resolve_title_action(TitleAction::Settings),
            HostTransition::OpenSettings
        );
    }

    #[test]
    fn profile_opens_profile() {
        assert_eq!(
            resolve_title_action(TitleAction::Profile),
            HostTransition::OpenProfile
        );
    }

    #[test]
    fn title_about_opens_the_field_manual_returning_to_the_title() {
        // T2 parity: the title's FIELD MANUAL button opens About and BACK returns to the title
        // (the Settings entry — tested via the run loop — returns to Settings instead).
        assert_eq!(
            resolve_title_action(TitleAction::About),
            HostTransition::OpenAbout(AboutReturn::Title)
        );
    }

    #[test]
    fn quit_exits() {
        assert_eq!(resolve_title_action(TitleAction::Quit), HostTransition::Exit);
    }

    #[test]
    fn pointer_maps_to_centre_corners_and_flips_y() {
        // A 800x600-point surface. The centre is the NDC origin; corners map to ±1 with y up.
        let size = [800.0, 600.0];
        let approx = |a: [f32; 2], b: [f32; 2]| {
            (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5
        };
        assert!(approx(pointer_to_ndc([400.0, 300.0], size), [0.0, 0.0]));
        // Top-left pixel (0,0) → NDC (-1, +1): y is flipped (egui y-down → NDC y-up).
        assert!(approx(pointer_to_ndc([0.0, 0.0], size), [-1.0, 1.0]));
        // Bottom-right pixel → NDC (+1, -1).
        assert!(approx(pointer_to_ndc([800.0, 600.0], size), [1.0, -1.0]));
    }

    #[test]
    fn pointer_to_ndc_guards_a_zero_size_surface() {
        // A not-yet-sized surface (0x0) must not divide by zero — it degrades to a finite result.
        let ndc = pointer_to_ndc([10.0, 10.0], [0.0, 0.0]);
        assert!(ndc[0].is_finite() && ndc[1].is_finite());
    }

    #[test]
    fn stamp_matches_the_path_main_uses() {
        // The exact composition `resumed()` performs: channel from debug flag, then format.
        assert_eq!(build_stamp(build_channel(true), "0.0.0"), "build dev · v0.0.0");
        assert_eq!(
            build_stamp(build_channel(false), "0.0.0"),
            "build release · v0.0.0"
        );
    }

    // ---- The gunsmith / loadout pure seam --------------------------------------------------------

    #[test]
    fn cycle_action_edits_the_routed_slot_and_stays() {
        let mut ed = LoadoutEditor::new();
        assert_eq!(ed.option_label(LoadoutSlot::Optic), "Standard");
        // Index 0 is the Optic slot (LoadoutSlot::from_index order); cycling forward advances it.
        let step = apply_loadout_action(
            LoadoutAction::Cycle {
                slot_index: 0,
                forward: true,
            },
            &mut ed,
        );
        assert_eq!(step, LoadoutStep::Stay);
        assert_eq!(ed.option_label(LoadoutSlot::Optic), "Marksman");
        // The other slots are untouched by an Optic cycle.
        assert_eq!(ed.option_label(LoadoutSlot::Barrel), "Standard");
        assert_eq!(ed.option_label(LoadoutSlot::Magazine), "Standard");
    }

    #[test]
    fn cycle_forward_then_back_round_trips() {
        let mut ed = LoadoutEditor::new();
        apply_loadout_action(
            LoadoutAction::Cycle {
                slot_index: 1,
                forward: true,
            },
            &mut ed,
        );
        apply_loadout_action(
            LoadoutAction::Cycle {
                slot_index: 1,
                forward: false,
            },
            &mut ed,
        );
        assert_eq!(ed.current(), LoadoutEditor::new().current());
    }

    #[test]
    fn out_of_range_cycle_is_a_harmless_stay_noop() {
        let mut ed = LoadoutEditor::new();
        let before = ed.current();
        let step = apply_loadout_action(
            LoadoutAction::Cycle {
                slot_index: 99,
                forward: true,
            },
            &mut ed,
        );
        assert_eq!(step, LoadoutStep::Stay);
        assert_eq!(ed.current(), before, "a stray slot index changes nothing");
    }

    #[test]
    fn reset_action_returns_to_baseline_and_stays() {
        let mut ed = LoadoutEditor::new();
        apply_loadout_action(
            LoadoutAction::Cycle {
                slot_index: 0,
                forward: true,
            },
            &mut ed,
        );
        assert_ne!(ed.current(), LoadoutEditor::new().current());
        let step = apply_loadout_action(LoadoutAction::Reset, &mut ed);
        assert_eq!(step, LoadoutStep::Stay);
        assert_eq!(ed.current(), LoadoutEditor::new().current());
    }

    #[test]
    fn deploy_and_back_are_screen_transitions_that_leave_the_editor_alone() {
        let mut ed = LoadoutEditor::new();
        apply_loadout_action(
            LoadoutAction::Cycle {
                slot_index: 2,
                forward: true,
            },
            &mut ed,
        );
        let chosen = ed.current();
        // Deploy/Back report a screen step but never mutate the chosen loadout.
        assert_eq!(apply_loadout_action(LoadoutAction::Deploy, &mut ed), LoadoutStep::Deploy);
        assert_eq!(ed.current(), chosen, "Deploy carries the chosen loadout unchanged");
        assert_eq!(apply_loadout_action(LoadoutAction::Back, &mut ed), LoadoutStep::Back);
        assert_eq!(ed.current(), chosen, "Back doesn't alter the editor either");
    }

    // ---- The shell theme (pure egui::Style data — no GPU/window, so it IS testable) --------------

    #[test]
    fn shell_style_paints_the_going_dark_surfaces() {
        let style = shell_style();
        let v = &style.visuals;
        // The base surfaces are the palette: ink behind everything, panel for windows/cards.
        assert_eq!(v.panel_fill, INK);
        assert_eq!(v.window_fill, PANEL);
        assert_eq!(v.extreme_bg_color, INK);
        // Amber is the lone accent.
        assert_eq!(v.hyperlink_color, AMBER);
        assert_eq!(v.selection.stroke.color, AMBER);
    }

    #[test]
    fn shell_style_widget_ramp_lifts_on_hover_and_rings_in_amber() {
        let w = &shell_style().visuals.widgets;
        // A button at rest sits on PANEL; hover/active lift it to the raised surface.
        assert_eq!(w.inactive.weak_bg_fill, PANEL);
        assert_eq!(w.hovered.weak_bg_fill, PANEL_RAISED);
        assert_eq!(w.active.weak_bg_fill, PANEL_RAISED);
        assert_ne!(
            w.inactive.weak_bg_fill, w.hovered.weak_bg_fill,
            "secondary buttons must visibly change fill on hover"
        );
        // The focus ring is amber, and hover nudges the widget outward for tactile feedback.
        assert_eq!(w.hovered.bg_stroke.color, AMBER);
        assert!(w.hovered.expansion > w.inactive.expansion);
        // Open menus mirror the pressed look.
        assert_eq!(w.open.weak_bg_fill, w.active.weak_bg_fill);
    }

    #[test]
    fn shell_style_type_scale_matches_the_named_ramp_and_descends() {
        use egui::TextStyle;
        let style = shell_style();
        let size = |s: TextStyle| style.text_styles.get(&s).map(|f| f.size).unwrap();
        assert_eq!(size(TextStyle::Heading), TYPE_HEADING);
        assert_eq!(size(TextStyle::Button), TYPE_BUTTON);
        assert_eq!(size(TextStyle::Body), TYPE_BODY);
        assert_eq!(size(TextStyle::Small), TYPE_CAPTION);
        // The hierarchy is strictly descending (a guard against a future edit inverting two sizes).
        assert!(TYPE_DISPLAY > TYPE_HEADING);
        assert!(TYPE_HEADING > TYPE_SUBHEAD);
        assert!(TYPE_SUBHEAD >= TYPE_BUTTON);
        assert!(TYPE_BUTTON > TYPE_BODY);
        assert!(TYPE_BODY > TYPE_CAPTION);
    }

    #[test]
    fn each_slot_advertises_its_own_trade_axis_pair() {
        // Every slot trades a distinct, disjoint axis pair (the source of the no-strict-domination
        // proof in core::gunsmith); the hints must reflect that and stay ASCII (no tofu).
        assert_eq!(slot_trade_hint(LoadoutSlot::Optic), "range <-> fire-rate");
        assert_eq!(slot_trade_hint(LoadoutSlot::Barrel), "damage <-> reserve");
        assert_eq!(slot_trade_hint(LoadoutSlot::Magazine), "capacity <-> handling");
        // All three are distinct — no slot duplicates another's pitch.
        let hints = [
            slot_trade_hint(LoadoutSlot::Optic),
            slot_trade_hint(LoadoutSlot::Barrel),
            slot_trade_hint(LoadoutSlot::Magazine),
        ];
        assert!(hints[0] != hints[1] && hints[1] != hints[2] && hints[0] != hints[2]);
        assert!(
            hints.iter().all(|h| h.is_ascii()),
            "trade hints must be ASCII to render in egui's default font"
        );
    }

    // ---- The Settings pure seam ------------------------------------------------------------------

    #[test]
    fn settings_defaults_are_in_range() {
        let s = SettingsState::default();
        for v in [s.master_volume, s.sfx_volume, s.music_volume] {
            assert!((0.0..=1.0).contains(&v));
        }
        assert!((SettingsState::SENS_MIN..=SettingsState::SENS_MAX).contains(&s.mouse_sensitivity));
        assert_eq!(s.quality, QualityChoice::Auto);
        assert!(!s.invert_look_y);
    }

    #[test]
    fn settings_clamp_rebounds_every_out_of_range_field() {
        let mut s = SettingsState {
            master_volume: 5.0,
            sfx_volume: -2.0,
            music_volume: 0.5,
            mouse_sensitivity: 99.0,
            invert_look_y: true,
            quality: QualityChoice::High,
        };
        s.clamp();
        assert_eq!(s.master_volume, 1.0);
        assert_eq!(s.sfx_volume, 0.0);
        assert_eq!(s.music_volume, 0.5);
        assert_eq!(s.mouse_sensitivity, SettingsState::SENS_MAX);
        // Non-numeric fields are untouched by clamp.
        assert!(s.invert_look_y);
        assert_eq!(s.quality, QualityChoice::High);
    }

    #[test]
    fn settings_reset_restores_defaults_and_stays() {
        let mut s = SettingsState::default();
        s.master_volume = 0.0;
        s.invert_look_y = true;
        s.quality = QualityChoice::Low;
        let step = apply_settings_action(SettingsAction::ResetDefaults, &mut s);
        assert_eq!(step, SettingsStep::Stay);
        assert_eq!(s, SettingsState::default());
    }

    #[test]
    fn settings_discrete_actions_map_to_their_steps() {
        let mut s = SettingsState::default();
        assert_eq!(
            apply_settings_action(SettingsAction::ToggleFullscreen, &mut s),
            SettingsStep::ToggleFullscreen
        );
        assert_eq!(
            apply_settings_action(SettingsAction::About, &mut s),
            SettingsStep::About
        );
        assert_eq!(
            apply_settings_action(SettingsAction::Back, &mut s),
            SettingsStep::Back
        );
        // None of those non-reset actions mutate the prefs.
        assert_eq!(s, SettingsState::default());
    }

    #[test]
    fn quality_cycles_through_all_choices_and_wraps() {
        let mut q = QualityChoice::Auto;
        let mut seen = Vec::new();
        for _ in 0..QualityChoice::ALL.len() {
            seen.push(q);
            q = q.next();
        }
        // Visited every distinct tier exactly once...
        for choice in QualityChoice::ALL {
            assert!(seen.contains(&choice), "{choice:?} must appear in the cycle");
        }
        // ...and wrapped back to the start.
        assert_eq!(q, QualityChoice::Auto);
    }

    // ---- The Profile pure seam -------------------------------------------------------------------

    #[test]
    fn sanitize_callsign_trims_truncates_and_falls_back() {
        assert_eq!(sanitize_callsign("  Reaper  "), "Reaper");
        // Empty / whitespace-only → the default.
        assert_eq!(sanitize_callsign("   "), DEFAULT_CALLSIGN);
        assert_eq!(sanitize_callsign(""), DEFAULT_CALLSIGN);
        // Over-long names truncate to CALLSIGN_MAX chars.
        let long = "X".repeat(CALLSIGN_MAX + 10);
        assert_eq!(sanitize_callsign(&long).chars().count(), CALLSIGN_MAX);
    }

    #[test]
    fn sanitize_callsign_truncates_on_char_boundaries() {
        // A multi-byte name must never split mid-codepoint (char-based take, not byte slice).
        let name = "é".repeat(CALLSIGN_MAX + 5);
        let out = sanitize_callsign(&name);
        assert_eq!(out.chars().count(), CALLSIGN_MAX);
        assert!(out.chars().all(|c| c == 'é'));
    }

    #[test]
    fn win_rate_guards_zero_and_computes_a_floor_percentage() {
        assert_eq!(win_rate_pct(0, 0), None, "no matches → no rate (clean '--')");
        assert_eq!(win_rate_pct(0, 4), Some(0));
        assert_eq!(win_rate_pct(2, 4), Some(50));
        assert_eq!(win_rate_pct(4, 4), Some(100));
        // Floors (1/3 = 33.3% → 33).
        assert_eq!(win_rate_pct(1, 3), Some(33));
        // No overflow on a large lifetime record.
        assert_eq!(win_rate_pct(1_000_000, 2_000_000), Some(50));
    }

    #[test]
    fn faction_pref_cycles_and_wraps() {
        assert_eq!(FactionPref::UsArmy.next(), FactionPref::FrenchArmy);
        assert_eq!(FactionPref::FrenchArmy.next(), FactionPref::UsArmy);
    }

    #[test]
    fn profile_actions_apply_and_transition() {
        let mut p = ProfileState {
            callsign: "  Ghost  ".to_string(),
            faction: FactionPref::UsArmy,
            matches_played: 9,
            wins: 3,
        };
        // Cycle faction stays on-screen.
        assert_eq!(
            apply_profile_action(ProfileAction::CycleFaction, &mut p),
            ProfileStep::Stay
        );
        assert_eq!(p.faction, FactionPref::FrenchArmy);
        // Reset stats zeroes the record and stays.
        assert_eq!(
            apply_profile_action(ProfileAction::ResetStats, &mut p),
            ProfileStep::Stay
        );
        assert_eq!((p.matches_played, p.wins), (0, 0));
        // Back sanitises the callsign and leaves.
        assert_eq!(
            apply_profile_action(ProfileAction::Back, &mut p),
            ProfileStep::Back
        );
        assert_eq!(p.callsign, "Ghost");
    }

    // ---- The About controls reference ------------------------------------------------------------

    #[test]
    fn field_manual_blurb_is_the_canonical_three_sentence_copy() {
        // A2 parity: the desktop blurb converges on Android's fuller `FIELD_MANUAL_BLURB` verbatim.
        // Guard the exact canonical string so a future one-side edit re-opens the drift the sync
        // closed, and keep it ASCII (default-font, no tofu).
        assert_eq!(
            FIELD_MANUAL_BLURB,
            "Command and grow your camps from above, then possess a single soldier and fight it in \
             first person while the strategic map goes dark. One commander does both jobs; the \
             tension is your divided attention. Stay embodied too long and the map you left behind \
             moves without you."
        );
        assert!(FIELD_MANUAL_BLURB.is_ascii(), "the blurb must render in egui's default font");
        // Three sentences (the "richer" copy the sync adopted), not the old one-liner.
        assert_eq!(FIELD_MANUAL_BLURB.matches(". ").count() + 1, 3);
    }

    #[test]
    fn controls_reference_is_well_formed_and_covers_every_layer() {
        let rows = controls_reference();
        assert!(!rows.is_empty());
        // No empty cells, and every label stays ASCII so it can't tofu.
        for r in rows {
            assert!(!r.group.is_empty() && !r.keys.is_empty() && !r.action.is_empty());
            assert!(r.keys.is_ascii() && r.action.is_ascii() && r.group.is_ascii());
        }
        // All three layers are documented.
        for layer in ["COMMAND", "EMBODIED", "GLOBAL"] {
            assert!(
                rows.iter().any(|r| r.group == layer),
                "the {layer} layer must have at least one binding"
            );
        }
    }

    #[test]
    fn controls_reference_leads_with_the_going_dark_concept_section() {
        // A1 parity: the field manual prepends a GOING DARK concept block (mirrors Android's
        // `fieldManualSections`) ahead of the keymap groups, so the first rows are that section.
        let rows = controls_reference();
        assert_eq!(rows[0].group, "GOING DARK", "the concept section must lead the manual");
        // The four concept rows, in order and verbatim (em-dash rendered ASCII per the no-tofu rule).
        let concept: Vec<(&str, &str)> = rows
            .iter()
            .filter(|r| r.group == "GOING DARK")
            .map(|r| (r.keys, r.action))
            .collect();
        assert_eq!(
            concept,
            vec![
                ("Embodiment", "Possess one unit and fight it in first person"),
                ("Going dark", "Embodying blacks out the strategic map -- alerts, not intel"),
                ("Surface", "Eject back to command; death also ejects you (no respawn)"),
                ("Stay fair", "While dark you get a directional flash + audio, never a map reveal"),
            ]
        );
        // The concept block sits entirely before the first keymap group (no interleaving).
        let last_concept = rows.iter().rposition(|r| r.group == "GOING DARK").unwrap();
        let first_keymap = rows.iter().position(|r| r.group != "GOING DARK").unwrap();
        assert!(last_concept < first_keymap, "the concept section is not interleaved with the keymap");
    }

    // ---- The Operations-hub mission-select + briefing pure seams ---------------------------------

    use gonedark_core::campaign::{MissionId, OperationNode};

    /// A small A -> B chain campaign: A is a root (Available), B is gated behind A (Locked).
    fn chain_campaign() -> Campaign {
        Campaign::new(vec![
            OperationNode::new(NodeId(0), MissionId(1), "Alpha", "take the outpost"),
            OperationNode::new(NodeId(1), MissionId(2), "Bravo", "hold the ridge")
                .requires([NodeId(0)]),
        ])
    }

    #[test]
    fn campaign_routes_through_the_mission_select_then_briefing() {
        // The full title -> hub -> briefing wiring at the seam level: CAMPAIGN opens the hub, a hub
        // tile opens a briefing for that node.
        assert_eq!(
            resolve_title_action(TitleAction::Campaign),
            HostTransition::OpenMissionSelect
        );
    }

    #[test]
    fn only_playable_tiles_resolve_to_a_node() {
        let campaign = chain_campaign();
        let entries = campaign.mission_select();
        // Node A is Available → playable → resolves to its own NodeId.
        assert_eq!(entries[0].progress, NodeProgress::Available);
        assert_eq!(playable_node(&entries[0]), Some(NodeId(0)));
        // Node B is Locked → not playable → a click resolves to nothing (can't launch what you can't
        // play), even though the tile exists.
        assert_eq!(entries[1].progress, NodeProgress::Locked);
        assert_eq!(playable_node(&entries[1]), None);
    }

    #[test]
    fn cleared_tiles_stay_playable_for_replay() {
        let mut campaign = chain_campaign();
        // Clear A → it becomes Cleared (replayable) and B unlocks (Available). Both are now playable.
        campaign.clear(NodeId(0), Difficulty::Regular).unwrap();
        let entries = campaign.mission_select();
        assert!(matches!(entries[0].progress, NodeProgress::Cleared { .. }));
        assert_eq!(playable_node(&entries[0]), Some(NodeId(0)), "a cleared node replays");
        assert_eq!(entries[1].progress, NodeProgress::Available);
        assert_eq!(playable_node(&entries[1]), Some(NodeId(1)));
    }

    #[test]
    fn difficulty_cycles_through_all_four_tiers_and_wraps() {
        // The briefing's cycler walks every campaign tier exactly once, then wraps.
        let mut d = Difficulty::Recruit;
        let mut seen = Vec::new();
        for _ in 0..Difficulty::ALL.len() {
            seen.push(d);
            d = next_difficulty(d);
        }
        for tier in Difficulty::ALL {
            assert!(seen.contains(&tier), "{tier:?} must appear in the cycle");
        }
        assert_eq!(d, Difficulty::Recruit, "the cycle wraps back to the start");
    }

    #[test]
    fn difficulty_labels_are_distinct_ascii() {
        let labels: Vec<&str> = Difficulty::ALL.iter().map(|&d| difficulty_label(d)).collect();
        assert!(labels.iter().all(|l| l.is_ascii() && !l.is_empty()));
        let mut sorted = labels.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), labels.len(), "every tier has a distinct label");
    }

    #[test]
    fn briefing_cycle_advances_the_selection_and_stays() {
        let mut selected = Difficulty::Recruit;
        let step = apply_briefing_action(BriefingAction::CycleDifficulty, &mut selected);
        assert_eq!(step, BriefingOutcome::Stay);
        assert_eq!(selected, Difficulty::Regular, "cycle advances the live selection");
    }

    #[test]
    fn briefing_deploy_carries_the_selected_tier_and_back_leaves() {
        let mut selected = Difficulty::Veteran;
        // Deploy reports the launch with the current selection (recorded against the clear on a win);
        // it does NOT mutate the selection.
        assert_eq!(
            apply_briefing_action(BriefingAction::Deploy, &mut selected),
            BriefingOutcome::Launch { difficulty: Difficulty::Veteran }
        );
        assert_eq!(selected, Difficulty::Veteran);
        // Back is a screen transition that leaves the selection alone.
        assert_eq!(
            apply_briefing_action(BriefingAction::Back, &mut selected),
            BriefingOutcome::Back
        );
        assert_eq!(selected, Difficulty::Veteran);
    }
}
