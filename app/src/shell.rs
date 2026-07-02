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
use gonedark_core::components::Army;
use gonedark_core::fixed::Fixed;
use gonedark_core::gunsmith::{Barrel, Loadout, Magazine, Muzzle, Optic, StatDelta, Stock};
use gonedark_engine::keybind::{GameAction, KeyId, KeybindMap, RebindOutcome};
use gonedark_engine::loadout_ui::{LoadoutEditor, LoadoutSlot};
use gonedark_engine::shell_modes::{GameMode, SHELL_GAME_MODES};
use gonedark_engine::AlertCueMode;
use gonedark_pal_desktop::DesktopRenderSurface;
use gonedark_render::theme::PaletteMode;
use gonedark_render::tiers::QualityTier;
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
pub fn resolve_title_action(action: TitleAction) -> HostTransition {
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

// ---- The gunsmith / loadout screen — pure seam (unit-tested) -------------------------------------

/// An action the gunsmith / loadout screen can emit in a frame. **D81: the gunsmith is
/// customization-only** — reached from Settings, it edits the persisted loadout and never starts a
/// match (the mode/mission-select screens are the deploy gates). So it has no Deploy: only edits
/// (`Cycle`/`Reset`) and DONE ([`LoadoutAction::Done`], which returns to Settings).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoadoutAction {
    /// Cycle the slot at on-screen index `slot_index` forward (`true`) or back (`false`) — an edit.
    Cycle { slot_index: usize, forward: bool },
    /// Reset every slot to the neutral all-`Standard` baseline.
    Reset,
    /// Finish customizing — leave the gunsmith and return to Settings (the edits persist).
    Done,
}

/// The screen-level outcome of a [`LoadoutAction`] once applied to the editor — what the host run
/// loop switches on. Separated from the egui glue so it is unit-testable without a window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoadoutStep {
    /// Stay on the gunsmith (an edit was applied, or nothing happened this frame).
    Stay,
    /// Finished customizing — return to Settings (the gunsmith's entry point, D81).
    Done,
}

/// Apply a [`LoadoutAction`] to the player's [`LoadoutEditor`] and report the resulting screen step.
/// Edits (`Cycle`/`Reset`) mutate the editor and keep us on the gunsmith; `Done` is the screen
/// transition the run loop acts on (back to Settings — the gunsmith is customization-only under D81).
/// Pure (no egui/window) — the gunsmith's testable decision seam, mirroring [`resolve_title_action`].
/// The actual loadout *model* (validation + the sidegrade-fairness proof) lives in `core::gunsmith`
/// and is consumed through the editor read-only; this never touches the sim.
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
        LoadoutAction::Done => LoadoutStep::Done,
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
        LoadoutSlot::Stock => "mobility <-> steadiness",
        LoadoutSlot::Muzzle => "suppression <-> downrange retention",
        // Grip is cosmetic/feel-only (D85): no sim trade, just recoil/hipfire feel.
        LoadoutSlot::Grip => "grip feel (cosmetic)",
    }
}

/// Format a [`Fixed`] axis value as a signed whole-unit decimal (e.g. `+2.00`, `-0.03`) for the
/// gunsmith readout. Scales by the type's own whole unit (`Fixed::from_int(1)`), so it is correct
/// regardless of the fixed-point Q-format and uses integer math only. Presentation-side (app crate):
/// floats would be fine here, but there is no need — the sim stays fixed-point (invariant #1).
fn fixed_signed(f: Fixed) -> String {
    let unit = Fixed::from_int(1).to_bits() as i64; // one whole unit, in bits
    let hundredths = f.to_bits() as i64 * 100 / unit;
    let sign = if hundredths < 0 { "-" } else { "+" };
    let mag = hundredths.abs();
    format!("{sign}{}.{:02}", mag / 100, mag % 100)
}

/// One integer axis token (`+10 mag`), or `None` when the axis is unchanged.
fn axis_i(v: i32, unit: &str) -> Option<String> {
    (v != 0).then(|| format!("{v:+} {unit}"))
}

/// One fixed-point axis token (`+2.00 rng`), or `None` when the axis is unchanged.
fn axis_f(f: Fixed, unit: &str) -> Option<String> {
    (f != Fixed::ZERO).then(|| format!("{} {unit}", fixed_signed(f)))
}

/// A compact, ASCII, signed readout of the REAL per-axis numbers a [`StatDelta`] moves — the
/// gunsmith's "what does this option actually cost and buy" line. Lists only the axes an option
/// touches (each slot's trade is disjoint, so a single option shows exactly its two poles), e.g.
/// `+6.00 dmg  -60 res` for a Heavy barrel. Empty (all-zero, e.g. a `Standard` option or the
/// cosmetic Grip) reads `no change`. Pure + static → unit-tested (mirrors [`slot_trade_hint`]); the
/// numeric deltas come from `core::gunsmith`, so this is the single legible surface for them.
pub fn stat_delta_summary(d: &StatDelta) -> String {
    let parts: Vec<String> = [
        axis_f(d.range, "rng"),
        axis_f(d.damage, "dmg"),
        axis_i(d.cooldown_ticks, "cd"),
        axis_i(d.mag_size, "mag"),
        axis_i(d.reload_ticks, "rld"),
        axis_i(d.reserve, "res"),
        axis_f(d.move_speed_delta, "spd"),
        axis_f(d.cone_cos_delta, "aim"),
        axis_f(d.supp_out_delta, "supp"),
        axis_f(d.falloff_delta, "fall"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        "no change".to_string()
    } else {
        parts.join("  ")
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

    /// This choice's stable index in [`Self::ALL`] — the persisted ordinal (mirrors the Android
    /// codec storing enums by ordinal so a renamed variant can't silently invalidate a saved blob).
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&c| c == self).unwrap_or(0)
    }

    /// The choice at persisted index `i`, or the default ([`QualityChoice::Auto`]) for an
    /// out-of-range ordinal — the tolerant decode side of [`Self::index`].
    pub fn from_index(i: usize) -> QualityChoice {
        Self::ALL.get(i).copied().unwrap_or(QualityChoice::Auto)
    }

    /// Resolve this Settings choice to a concrete render [`QualityTier`] — the seam that drives
    /// `render::tiers` through `Game::set_tier` (D75 follow-up). `Auto` yields the caller's
    /// `device_default` (there is no per-device auto-detect yet; desktop is the D22 flagship class,
    /// so the host passes [`QualityTier::High`]); the explicit picks pin `Low`/`Mid`/`High`. Pure —
    /// no GPU, no sim (a tier is a RENDER choice, invariant #1/#4), so it is host-tested.
    pub fn to_tier(self, device_default: QualityTier) -> QualityTier {
        match self {
            QualityChoice::Auto => device_default,
            QualityChoice::Low => QualityTier::Low,
            QualityChoice::Medium => QualityTier::Mid,
            QualityChoice::High => QualityTier::High,
        }
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
/// shape the desktop look input (`DesktopInput::set_look_prefs`). **`quality`** now drives
/// `render::tiers`: the host resolves it via [`QualityChoice::to_tier`] and pushes it through
/// `Game::set_tier` each match frame (D75 follow-up). **`music_volume`** is **dormant-but-wired**:
/// the host composes its effective bus gain each frame via `gonedark_engine::music_gain` and carries
/// it to the audio host, but there is no music *source* to scale yet (every `SoundId` is SFX), so it
/// has no audible effect until a music track lands. All survive across screens for the session.
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
    /// Accessibility — **Colorblind (CVD) cues**. When on, the embodied alert HUD labels each ring
    /// marker (FIRE/LOST/BASE/TERR) so the four alert kinds read without relying on hue (invariant #6
    /// fairness). Fed to the engine via `Game::set_accessibility_prefs`. Presentation only.
    pub colorblind_cues: bool,
    /// Accessibility — **Visual sound cues**. When on, the audio-only signals the coarse 4-kind alert
    /// HUD never draws get a visual echo (a production-ready "+" and a dimmed distant-capture ring), so
    /// a hard-of-hearing player has parity with the primary embodied-audio channel (invariant #6).
    /// Presentation only.
    pub visual_sound_cues: bool,
    /// Accessibility — **Colorblind palette** ([`PaletteMode`]). When not `Off`, the renderer swaps
    /// the faction colour ramp for a colourblind-safe alternate (blue/orange for red-green
    /// deficiencies, a red-green-axis ramp for tritanopia) so unit identity does not rest on hue
    /// alone (WS-D, invariant #6 fairness). Fed to the engine via `Game::set_accessibility_prefs` →
    /// `Renderer::set_palette_mode`. Presentation only.
    pub cvd_palette: PaletteMode,
    /// Accessibility — **Alert cues** ([`AlertCueMode`]). Selects the NON-visual equivalent(s) of the
    /// embodied directional flash — a bearing-panned audio ping and/or a directional haptic pulse — so
    /// a player who can't read the colour flash still gets the going-dark alert (WS-D, invariant #6).
    /// Still an *alert, not intel* (bearing + kind only). Fed to the engine via `Game::set_alert_cue_mode`.
    /// Presentation only.
    pub alert_cue_mode: AlertCueMode,
    /// The desktop key-rebind map (D75 follow-up "the rebind editor"): which physical key fires each
    /// rebindable host action (pause / fullscreen / debug overlay). The pure model lives in
    /// `gonedark_engine::keybind` (winit-free, invariant #2); `main.rs` maps `winit::KeyCode` ↔ `KeyId`
    /// at its boundary and routes each press through this map. Persisted by stable ordinal alongside
    /// the other prefs. Presentation only — a keybind never reaches the sim (invariants #1/#4).
    pub keybinds: KeybindMap,
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
            // Accessibility cues default OFF — the base alert channel already carries shape +
            // luminance-spread CVD redundancy and the primary audio channel, so these are opt-in
            // intensifiers, not always-on chrome.
            colorblind_cues: false,
            visual_sound_cues: false,
            // The shipped hue palette; a CVD-safe alternate is opt-in per player.
            cvd_palette: PaletteMode::Off,
            // The base flash + positioned audio is the shipped fair channel; the audio ping / haptic
            // pulse cross-modal equivalents are opt-in.
            alert_cue_mode: AlertCueMode::Off,
            // The shipped desktop bindings (Esc pause / F11 fullscreen / F3 debug) — the historical
            // hardcoded keys, now data the rebind editor can change.
            keybinds: KeybindMap::default(),
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
    /// Open the gunsmith / loadout customization screen (D81: the gunsmith lives under Settings now,
    /// as customization-only — not a play gate).
    OpenLoadout,
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
    /// Leave for the gunsmith / loadout customization screen (D81).
    OpenLoadout,
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
        SettingsAction::OpenLoadout => SettingsStep::OpenLoadout,
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
    /// Every faction, in a fixed order (the persisted-ordinal order and the cycle order).
    pub const ALL: [FactionPref; 2] = [FactionPref::UsArmy, FactionPref::FrenchArmy];

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

    /// This faction's stable index in [`Self::ALL`] — the persisted ordinal.
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&f| f == self).unwrap_or(0)
    }

    /// The faction at persisted index `i`, or the default ([`FactionPref::UsArmy`]) for an
    /// out-of-range ordinal — the tolerant decode side of [`Self::index`].
    pub fn from_index(i: usize) -> FactionPref {
        Self::ALL.get(i).copied().unwrap_or(FactionPref::UsArmy)
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

// ---- The army-select screen — pure seam (unit-tested) -------------------------------------------

/// The player-selectable armies on the army-select screen, in the fixed cycle/display order. Only the
/// **combatant** rosters (US, French) are offered — [`Army::Neutral`] is the non-aligned default and
/// is never a player pick (a commander always fields a real army; factions-plan WS-A). Pure data.
pub const SELECTABLE_ARMIES: [Army; 2] = [Army::Us, Army::Fr];

/// The on-screen name for an [`Army`] (the army-select card + title readout). ASCII only so it can
/// never tofu in egui's default font. Pure — unit-tested.
pub fn army_label(army: Army) -> &'static str {
    match army {
        Army::Us => "US Army",
        Army::Fr => "French Army",
        Army::Neutral => "Non-aligned",
    }
}

/// A one-line identity/flavour blurb for an [`Army`] — the real-platform anchors from
/// `docs/factions.md` §4, so the two cards read as distinct forces. Flavour only: asymmetry is of
/// feel, never of power (the fairness bound, factions.md §2 / pillar 4). ASCII only. Pure —
/// unit-tested.
pub fn army_flavor(army: Army) -> &'static str {
    match army {
        Army::Us => "M4 carbines, M1 Abrams armour, combat medics -- the US Army roster.",
        Army::Fr => "FAMAS rifles, Leclerc armour, auxiliaires sanitaires -- the French Army roster.",
        Army::Neutral => "No real-army identity -- the non-aligned default.",
    }
}

/// Host-side army-select state — which real-army roster the player fields. **Match-setup config**, not
/// sim state: it is routed to the sim through the `core::shell` SelectArmy seam at match start (never
/// checksummed itself — invariant #7). Persists across launches like the loadout / faction preference.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ArmySelectState {
    /// The currently-selected army. Defaults to [`Army::Us`] (a real combatant roster — never the
    /// non-aligned [`Army::Neutral`], which is not a player pick).
    pub selected: Army,
}

impl Default for ArmySelectState {
    fn default() -> Self {
        ArmySelectState {
            selected: Army::Us,
        }
    }
}

/// An action the army-select screen can emit in a frame. Choosing an army is an in-place edit (stays
/// on-screen so the player can compare the two identities); CONFIRM commits and returns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArmySelectAction {
    /// Select an army (an in-place edit — stays on the screen).
    Choose(Army),
    /// Confirm the current selection and return to the title (the pick persists + is fielded next
    /// match).
    Confirm,
}

/// The screen-level outcome of an [`ArmySelectAction`] once applied — what the run loop switches on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArmySelectStep {
    /// Stay on army-select (a selection changed, or nothing happened this frame).
    Stay,
    /// Confirm the pick and return to the title.
    Confirm,
}

/// Apply an [`ArmySelectAction`] to the army-select state and report the resulting screen step.
/// `Choose` records the selection and stays; `Confirm` is the screen transition the run loop acts on.
/// Pure (no egui/window) — the army-select testable decision seam, mirroring [`apply_profile_action`].
/// It never touches the sim; the sim only sees the pick via the `core::shell` SelectArmy seam that the
/// host resolves at match start (`Game::select_army`).
pub fn apply_army_select_action(
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

// ---- Shell-prefs persistence codec — pure seam (unit-tested) ------------------------------------

/// The blob format version tag (the first line). Bumped only on an incompatible layout change; a
/// mismatched or missing tag is tolerated — decode still reads whatever keys it recognises.
const SHELL_PREFS_VERSION: &str = "gonedark-shell 1";

/// This [`Optic`]'s stable index in [`Optic::ALL`] — the persisted ordinal (an unknown ordinal
/// decodes to the slot's `Default`, so a reordered table can't inject an invalid selection).
fn optic_index(o: Optic) -> usize {
    Optic::ALL.iter().position(|&x| x == o).unwrap_or(0)
}
/// This [`Barrel`]'s stable index in [`Barrel::ALL`].
fn barrel_index(b: Barrel) -> usize {
    Barrel::ALL.iter().position(|&x| x == b).unwrap_or(0)
}
/// This [`Magazine`]'s stable index in [`Magazine::ALL`].
fn magazine_index(m: Magazine) -> usize {
    Magazine::ALL.iter().position(|&x| x == m).unwrap_or(0)
}

/// Serialize the three player-owned shell state objects — [`SettingsState`] (audio/look/video),
/// [`ProfileState`] (callsign/faction/record), and the gunsmith [`LoadoutEditor`] — to a flat,
/// line-based `key=value` blob for the host to persist across launches. The Rust counterpart of the
/// Android `ShellPrefsCodec.encode` (a desktop-appropriate blob, **not** the Kotlin wire format — the
/// format is not required to match, only the pattern). Every field is written in its canonical,
/// already-clamped / sanitized form (settings are clamped, the callsign sanitized, enums stored by
/// stable ordinal via [`QualityChoice::index`]/[`FactionPref::index`]), so a save→load round-trip is
/// stable. Pure (no fs/env) — the file I/O around it is the exempt host glue (in `main.rs`).
///
/// **Presentation only** — none of this is sim state (invariant #1 is about the sim's fixed-point
/// state, not host prefs), so it is never checksummed and can't desync anything.
pub fn encode_shell_prefs(
    settings: &SettingsState,
    profile: &ProfileState,
    loadout: &LoadoutEditor,
    army: &ArmySelectState,
) -> String {
    let mut s = *settings;
    s.clamp();
    let l = loadout.current();
    // Strip any newline from the free-text callsign so it can't break the line-based format (the one
    // value that isn't a number/ordinal). `sanitize_callsign` handles trim/truncate/empty-fallback.
    let callsign = sanitize_callsign(&profile.callsign).replace(['\n', '\r'], " ");
    format!(
        "{SHELL_PREFS_VERSION}\n\
         master={}\nsfx={}\nmusic={}\nsens={}\ninverty={}\nquality={}\n\
         cvdcues={}\nsoundcues={}\ncvdpal={}\nalertcue={}\n\
         callsign={}\nfaction={}\nmatches={}\nwins={}\n\
         optic={}\nbarrel={}\nmagazine={}\n\
         army={}\nkeybinds={}\n",
        s.master_volume,
        s.sfx_volume,
        s.music_volume,
        s.mouse_sensitivity,
        s.invert_look_y as u8,
        s.quality.index(),
        s.colorblind_cues as u8,
        s.visual_sound_cues as u8,
        s.cvd_palette.index(),
        s.alert_cue_mode.index(),
        callsign,
        profile.faction.index(),
        profile.matches_played,
        profile.wins,
        optic_index(l.optic),
        barrel_index(l.barrel),
        magazine_index(l.magazine),
        // The selected army as its stable `Army::index` ordinal (the same tag order the sim/wire
        // codecs use), tolerant-decoded back by [`decode_army`].
        army.selected.index(),
        // The rebind map as its own compact ordinal blob (`KeybindMap::encode`), tolerant-decoded
        // back by `KeybindMap::decode` — a missing/garbage value falls back to the shipped bindings.
        s.keybinds.encode(),
    )
}

/// Tolerantly decode a [`encode_shell_prefs`] blob back to the three state objects. Any missing,
/// unparseable, or out-of-range value falls back to that field's default — this **never** panics
/// (mirroring the Android codec's forward-compat + corruption-safety contract). An empty/garbage blob
/// therefore decodes to the shipped defaults. Settings are re-clamped and the callsign re-sanitized on
/// the way out, so the result is always valid. Pure — unit-tested without touching the filesystem.
pub fn decode_shell_prefs(
    blob: &str,
) -> (SettingsState, ProfileState, LoadoutEditor, ArmySelectState) {
    use std::collections::HashMap;

    // Parse `key=value` lines (split on the FIRST '=', so a value may itself contain '='). The
    // version tag and any unrecognised line are simply ignored.
    let map: HashMap<&str, &str> = blob
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect();

    let ds = SettingsState::default();
    let mut settings = SettingsState {
        master_volume: parse_or(map.get("master"), ds.master_volume),
        sfx_volume: parse_or(map.get("sfx"), ds.sfx_volume),
        music_volume: parse_or(map.get("music"), ds.music_volume),
        mouse_sensitivity: parse_or(map.get("sens"), ds.mouse_sensitivity),
        invert_look_y: parse_bool(map.get("inverty"), ds.invert_look_y),
        quality: QualityChoice::from_index(parse_or::<usize>(map.get("quality"), 0)),
        colorblind_cues: parse_bool(map.get("cvdcues"), ds.colorblind_cues),
        visual_sound_cues: parse_bool(map.get("soundcues"), ds.visual_sound_cues),
        // Tolerant ordinal decode (an unknown/missing ordinal → `Off`), the `quality` pattern.
        cvd_palette: PaletteMode::from_index(parse_or::<usize>(map.get("cvdpal"), 0)),
        alert_cue_mode: AlertCueMode::from_index(parse_or::<usize>(map.get("alertcue"), 0)),
        // The rebind map from its compact ordinal blob. `KeybindMap::decode` is total (a missing key
        // → `""` → the shipped default bindings; a corrupt/duplicate blob → defaults), never panics.
        keybinds: KeybindMap::decode(map.get("keybinds").copied().unwrap_or("")),
    };
    // The clamp guards a stored-but-out-of-range numeric (e.g. a hand-edited blob) exactly as the
    // Settings sliders do.
    settings.clamp();

    let dp = ProfileState::default();
    let profile = ProfileState {
        // `sanitize_callsign("")` yields the default callsign, so a missing key is handled here.
        callsign: sanitize_callsign(map.get("callsign").copied().unwrap_or("")),
        faction: FactionPref::from_index(parse_or::<usize>(map.get("faction"), 0)),
        matches_played: parse_or(map.get("matches"), dp.matches_played),
        wins: parse_or(map.get("wins"), dp.wins),
    };

    let loadout = LoadoutEditor::with_loadout(Loadout {
        optic: Optic::ALL
            .get(parse_or::<usize>(map.get("optic"), 0))
            .copied()
            .unwrap_or_default(),
        barrel: Barrel::ALL
            .get(parse_or::<usize>(map.get("barrel"), 0))
            .copied()
            .unwrap_or_default(),
        magazine: Magazine::ALL
            .get(parse_or::<usize>(map.get("magazine"), 0))
            .copied()
            .unwrap_or_default(),
        // Gunsmith breadth (D85): decode the two new sim slots the same way. A missing key defaults
        // to Standard (a pre-D85 save has no stock/muzzle key), so old saves round-trip unchanged.
        stock: Stock::ALL
            .get(parse_or::<usize>(map.get("stock"), 0))
            .copied()
            .unwrap_or_default(),
        muzzle: Muzzle::ALL
            .get(parse_or::<usize>(map.get("muzzle"), 0))
            .copied()
            .unwrap_or_default(),
    });

    let army = ArmySelectState {
        selected: decode_army(map.get("army")),
    };

    (settings, profile, loadout, army)
}

/// Decode a stored [`Army`] ordinal to a **player-selectable** army, defaulting to the shipped
/// [`ArmySelectState::default`] (US) for a missing, unparseable, out-of-range, or non-combatant
/// value. [`Army::Neutral`] is never a valid player pick (factions-plan WS-A), so a stored `0`
/// (Neutral) decodes to the default just like garbage would — the tolerant, corruption-safe read
/// mirroring the enum-ordinal fields above.
fn decode_army(value: Option<&&str>) -> Army {
    let default = ArmySelectState::default().selected;
    match value
        .and_then(|s| s.parse::<usize>().ok())
        .and_then(|i| Army::ALL.get(i).copied())
    {
        Some(Army::Neutral) | None => default,
        Some(a) => a,
    }
}

/// Parse a stored value to `T`, falling back to `fallback` on a missing key or a parse failure. The
/// numeric decode primitive the codec leans on (mirrors the Android codec's tolerant field reads).
fn parse_or<T: std::str::FromStr>(value: Option<&&str>, fallback: T) -> T {
    value.and_then(|s| s.parse::<T>().ok()).unwrap_or(fallback)
}

/// Decode a stored boolean: `"1"`/`"true"` → true, `"0"`/`"false"` → false, anything else → fallback.
fn parse_bool(value: Option<&&str>, fallback: bool) -> bool {
    match value.map(|s| *s) {
        Some("1") | Some("true") => true,
        Some("0") | Some("false") => false,
        _ => fallback,
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

// ---- The Pve/Pvp mode-select screen — pure seam (unit-tested) -----------------------------------

/// An action the Pve/Pvp mode/map-select screen (D81) can emit in a frame. Picking a mode deploys
/// straight into its scene with the persisted loadout (no gunsmith); BACK returns to the title. The
/// mode table itself is the static [`SHELL_GAME_MODES`] (tested in `gonedark_engine::shell_modes`);
/// the picked mode's scene resolution is [`GameMode::scene`] — both live in `engine` so the
/// scene-token guard is unit-tested there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModeSelectAction {
    /// Deploy the picked mode — the host resolves its [`GameMode::scene`] and boots the match.
    Pick(GameMode),
    /// Return to the title screen.
    Back,
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
/// launch tier: the host applies its combat tuning (D83: the 4→3 enemy-commander band + the scenario
/// situation modifiers, via `Game::apply_campaign_tuning`) and records it against `Campaign::clear`
/// on a win.
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

// A near-black field, dim chrome, one amber alert accent (the game's directional-alert colour). The
// **shared-identity** ramp (ink / text / hairline) is now DERIVED from the canonical renderer palette
// (`gonedark_render::theme`, `render/src/theme.rs`) through [`rgb8`], so there is ONE source of truth
// instead of a duplicated hex table that could silently drift (closing the D75 "the two palettes must
// still move together" hazard — it is now enforced by [`shell_palette_shares_the_render_theme`]).
//
// egui wants `Color32` (8-bit sRGB) and render wants `[f32; 3]` in [0,1]; `rgb8` is the one bridge.
// The **in-match-tuned** variants (PANEL / PANEL_RAISED / AMBER) are DELIBERATELY *not* derived: the
// renderer's `theme::{PANEL, PANEL_RAISED, AMBER}` are nudged deeper/warmer for the in-match HUD (see
// their doc-hex in theme.rs), while the out-of-match shell keeps the lighter/cooler card + amber it
// shipped with. Those stay explicit here and are pinned by tests so a retune is a conscious edit.

/// Convert a `gonedark_render::theme` sRGB colour (`[0,1]` components) to an egui `Color32` (8-bit
/// sRGB) — the single bridge that lets the shell chrome share the renderer's palette source of truth.
/// Round-to-nearest (`+0.5` then truncate; components are non-negative). `const` so it seeds the
/// palette consts below.
const fn rgb8(c: gonedark_render::theme::Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(
        (c[0] * 255.0 + 0.5) as u8,
        (c[1] * 255.0 + 0.5) as u8,
        (c[2] * 255.0 + 0.5) as u8,
    )
}

// Shared-identity ramp — derived straight from the renderer theme.
const INK: egui::Color32 = rgb8(gonedark_render::theme::INK);
const BONE: egui::Color32 = rgb8(gonedark_render::theme::BONE);
const ASH: egui::Color32 = rgb8(gonedark_render::theme::ASH);
const RIM: egui::Color32 = rgb8(gonedark_render::theme::RIM);
// In-match-tuned variants — deliberately the SHELL values (the renderer nudges these deeper/warmer
// for the in-match HUD); kept explicit and pinned by tests. PANEL is the card fill; PANEL_RAISED the
// raised/hover/active surface; AMBER the lone signal accent; MUTED the dimmest legible text.
const PANEL: egui::Color32 = egui::Color32::from_rgb(0x12, 0x18, 0x20);
const AMBER: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x79, 0x1F);
const PANEL_RAISED: egui::Color32 = egui::Color32::from_rgb(0x1B, 0x25, 0x31);
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
    // The selection fill is AMBER at ~38% alpha — derived from the const, not a re-typed hex, so it
    // tracks any AMBER retune (was a duplicated 0xE0/0x79/0x1F literal).
    v.selection.bg_fill =
        egui::Color32::from_rgba_unmultiplied(AMBER.r(), AMBER.g(), AMBER.b(), 96);
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
    /// Transient Settings **rebind-editor** state (D75 follow-up): the action currently capturing a
    /// key (`Some` while the row shows "press a key…"), and the last conflict `(action, owner)` to
    /// surface as feedback. Ephemeral UI interaction state — not a persisted pref (the map itself
    /// lives on `SettingsState::keybinds`), so it rides the device-gated glue, not the pure model.
    rebinding: Option<GameAction>,
    rebind_conflict: Option<(GameAction, GameAction)>,
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
            rebinding: None,
            rebind_conflict: None,
        }
    }

    /// Feed one winit window event to egui (pointer/keys). Returns whether egui consumed it.
    pub fn on_window_event(&mut self, window: &Window, event: &winit::event::WindowEvent) -> bool {
        self.state.on_window_event(window, event).consumed
    }

    /// Whether the Settings rebind editor is mid-capture (a row is armed, waiting for a key). The
    /// host reads this to suppress the global F11 fullscreen hotkey during capture — otherwise
    /// pressing F11 to *bind* it to an action would also silently flip the window into fullscreen.
    pub fn is_capturing_rebind(&self) -> bool {
        self.rebinding.is_some()
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
        // Copy the transient rebind-editor state into locals so the paint closure doesn't alias the
        // `&mut self` borrow `run_and_paint` takes (the `stamp` clone pattern), then write it back.
        let mut rebinding = self.rebinding;
        let mut conflict = self.rebind_conflict;
        let action = self.run_and_paint(surface, true, |ui| {
            settings_ui(ui, state, fullscreen, &mut rebinding, &mut conflict)
        });
        self.rebinding = rebinding;
        self.rebind_conflict = conflict;
        action
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

    /// Draw the **army-select** screen for one frame and return the [`ArmySelectAction`] used, if any.
    /// `state` is the host-side army pick (read here to highlight the current card). Over the live 3D
    /// backdrop, same as the other out-of-match screens. Pure presentation — the decision logic is the
    /// pure [`apply_army_select_action`] seam and the sim routing is the `core::shell` SelectArmy seam.
    pub fn draw_army_select(
        &mut self,
        surface: &mut DesktopRenderSurface,
        state: &ArmySelectState,
    ) -> Option<ArmySelectAction> {
        self.run_and_paint(surface, true, |ui| army_select_ui(ui, state))
    }

    /// Draw the About / controls-reference screen for one frame. Returns `true` on BACK (the only
    /// control), so the run loop returns to Settings. Static content over the backdrop. Pure.
    pub fn draw_about(&mut self, surface: &mut DesktopRenderSurface) -> bool {
        let stamp = self.stamp.clone();
        self.run_and_paint(surface, true, |ui| about_ui(ui, &stamp).then_some(()))
            .is_some()
    }

    /// Draw the Pve/Pvp **mode / map select** screen for one frame and return the
    /// [`ModeSelectAction`] used, if any (D81). The mode table is the static [`SHELL_GAME_MODES`];
    /// this holds no host state. Over the live 3D backdrop, same as the other out-of-match screens.
    /// Pure presentation — the picked mode's scene resolution is the `engine`-tested
    /// [`GameMode::scene`] seam, this is the device-gated glue.
    pub fn draw_mode_select(
        &mut self,
        surface: &mut DesktopRenderSurface,
    ) -> Option<ModeSelectAction> {
        self.run_and_paint(surface, true, mode_select_ui)
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

/// The pure state transition for a two-click confirm gate on a destructive button. Given whether
/// the button is currently *armed* (already clicked once), a click returns `(new_armed, fired)`:
/// the first click arms it (`(true, false)`) and relabels to a confirm prompt; a click while armed
/// fires the action and disarms (`(false, true)`). Pure → unit-tested; the egui glue
/// [`confirm_menu_button`] carries the transient armed bit.
fn confirm_click(armed: bool) -> (bool, bool) {
    if armed {
        (false, true)
    } else {
        (true, false)
    }
}

/// A destructive-action button that requires two clicks: the first arms it (relabeling to
/// `confirm_label` in the primary/amber emphasis), the second fires. The armed bit lives in egui's
/// transient memory keyed by `id_salt`, so no host state threading is needed and it clears itself
/// when the screen stops drawing the button. Returns `true` only on the confirming click. Guards the
/// three one-click-wipe actions (gunsmith RESET, Settings RESET DEFAULTS, Profile RESET RECORD) that
/// previously destroyed state with no undo. Glue (needs a `Ui`); the decision is [`confirm_click`].
fn confirm_menu_button(
    ui: &mut egui::Ui,
    id_salt: &str,
    label: &str,
    confirm_label: &str,
    emphasis: Emphasis,
) -> bool {
    let id = ui.make_persistent_id(id_salt);
    let armed = ui.data(|d| d.get_temp::<bool>(id).unwrap_or(false));
    // Armed → show the confirm prompt in amber so the escalated state is unmistakable.
    let shown = if armed { confirm_label } else { label };
    let shown_emphasis = if armed { Emphasis::Primary } else { emphasis };
    let mut fired = false;
    if menu_button(ui, shown, shown_emphasis) {
        let (new_armed, did_fire) = confirm_click(armed);
        ui.data_mut(|d| d.insert_temp(id, new_armed));
        fired = did_fire;
    }
    fired
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
                // The army-select entry (US vs FR) — a pre-deploy pick fielded at every match start.
                if chip_button(ui, "ARMY") {
                    action = Some(TitleAction::Army);
                }
                // The field manual (About) — reachable straight from the title, mirroring Android's
                // title About entry (it is also reachable from Settings).
                if chip_button(ui, "FIELD MANUAL") {
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
/// sidegrade explainer, then DONE / RESET (D81: customization-only, no Deploy). All the decision
/// logic is in the pure seam
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
                        // The REAL per-option trade numbers (D60/M3): they change as the slot cycles,
                        // so the sidegrade is legible ("+6.00 dmg  -60 res"), not just an axis pair.
                        ui.add_sized(
                            [200.0, 32.0],
                            Label::new(
                                RichText::new(stat_delta_summary(&editor.option_delta(slot)))
                                    .color(ASH)
                                    .size(TYPE_CAPTION),
                            ),
                        );
                    });
                    if i + 1 < LoadoutSlot::ALL.len() {
                        ui.add_space(8.0);
                    }
                }
            });

            ui.add_space(14.0);
            // Build-wide net delta (the sum of the sim slots' trades). By the sidegrade rule it is
            // never a flat upgrade over the baseline — surfacing it makes that legible at a glance.
            ui.label(
                RichText::new(format!("NET  {}", stat_delta_summary(&editor.net_delta())))
                    .color(AMBER)
                    .size(TYPE_CAPTION)
                    .strong(),
            );
            ui.add_space(14.0);
            // D81: customization-only — DONE returns to Settings (the entry point), RESET clears to
            // baseline. There is no Deploy here: the mode/mission-select screens start matches.
            if menu_button(ui, "DONE", Emphasis::Primary) {
                action = Some(LoadoutAction::Done);
            }
            ui.add_space(10.0);
            // RESET wipes every attachment back to Standard and sits right next to DONE — a real
            // misclick target — so it takes two clicks (arm, then confirm) with no undo otherwise.
            if confirm_menu_button(ui, "loadout.reset", "RESET", "RESET? CLICK AGAIN", Emphasis::Secondary) {
                action = Some(LoadoutAction::Reset);
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
            // Bound the card to the viewport and let its content scroll if it overflows — so a
            // shrunk window (down to the min inner size) or a growing list (the campaign
            // mission-select) can never push BACK / footer controls off-screen with no way to
            // reach them. A ScrollArea that fits its content shows no scrollbar, so short screens
            // look identical to before.
            let max_card_h = (h * (1.0 - top_frac) - 24.0).max(120.0);
            ui.vertical_centered(|ui| {
                ui.add_space(h * top_frac);
                glass_card_frame().show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(max_card_h)
                        .show(ui, |ui| {
                            out = Some(build(ui));
                        });
                });
            });
        });
    out.expect("over_backdrop_screen build ran")
}

/// Map an egui [`egui::Key`] to the engine's platform-neutral [`KeyId`], or `None` for a key the
/// rebind vocabulary doesn't cover. This is the Settings **app boundary** for the rebind editor: the
/// engine `keybind` seam is deliberately egui/winit-free (invariant #2), so the conversion of a real
/// key press into a bindable id lives here (its winit twin, `keycode_to_keyid`, lives in `main.rs`).
/// Pure (a total match over plain enums) — unit-tested below without a window.
fn egui_key_to_keyid(key: egui::Key) -> Option<KeyId> {
    use egui::Key;
    Some(match key {
        Key::F1 => KeyId::F1,
        Key::F2 => KeyId::F2,
        Key::F3 => KeyId::F3,
        Key::F4 => KeyId::F4,
        Key::F5 => KeyId::F5,
        Key::F6 => KeyId::F6,
        Key::F7 => KeyId::F7,
        Key::F8 => KeyId::F8,
        Key::F9 => KeyId::F9,
        Key::F10 => KeyId::F10,
        Key::F11 => KeyId::F11,
        Key::F12 => KeyId::F12,
        Key::A => KeyId::A,
        Key::B => KeyId::B,
        Key::C => KeyId::C,
        Key::D => KeyId::D,
        Key::E => KeyId::E,
        Key::F => KeyId::F,
        Key::G => KeyId::G,
        Key::H => KeyId::H,
        Key::I => KeyId::I,
        Key::J => KeyId::J,
        Key::K => KeyId::K,
        Key::L => KeyId::L,
        Key::M => KeyId::M,
        Key::N => KeyId::N,
        Key::O => KeyId::O,
        Key::P => KeyId::P,
        Key::Q => KeyId::Q,
        Key::R => KeyId::R,
        Key::S => KeyId::S,
        Key::T => KeyId::T,
        Key::U => KeyId::U,
        Key::V => KeyId::V,
        Key::W => KeyId::W,
        Key::X => KeyId::X,
        Key::Y => KeyId::Y,
        Key::Z => KeyId::Z,
        Key::Num0 => KeyId::Digit0,
        Key::Num1 => KeyId::Digit1,
        Key::Num2 => KeyId::Digit2,
        Key::Num3 => KeyId::Digit3,
        Key::Num4 => KeyId::Digit4,
        Key::Num5 => KeyId::Digit5,
        Key::Num6 => KeyId::Digit6,
        Key::Num7 => KeyId::Digit7,
        Key::Num8 => KeyId::Digit8,
        Key::Num9 => KeyId::Digit9,
        Key::Escape => KeyId::Escape,
        Key::Tab => KeyId::Tab,
        Key::Space => KeyId::Space,
        Key::Enter => KeyId::Enter,
        Key::Backspace => KeyId::Backspace,
        Key::Insert => KeyId::Insert,
        Key::Delete => KeyId::Delete,
        Key::Home => KeyId::Home,
        Key::End => KeyId::End,
        Key::PageUp => KeyId::PageUp,
        Key::PageDown => KeyId::PageDown,
        Key::ArrowUp => KeyId::Up,
        Key::ArrowDown => KeyId::Down,
        Key::ArrowLeft => KeyId::Left,
        Key::ArrowRight => KeyId::Right,
        Key::Minus => KeyId::Minus,
        Key::Equals => KeyId::Equals,
        Key::Backtick => KeyId::Backquote,
        // Everything else (punctuation, modifiers, media keys, …) is outside the bindable vocabulary.
        _ => return None,
    })
}

/// The immediate-mode Settings screen: audio/controls/video preferences in a centred card over the
/// backdrop. Sliders/checkboxes edit `state` in place (then [`SettingsState::clamp`] re-bounds it);
/// the discrete controls return a [`SettingsAction`] the pure [`apply_settings_action`] seam resolves.
/// `fullscreen` is the host's current window mode (reflected by the video checkbox). The KEY BINDINGS
/// rows drive the rebind editor via the pure `KeybindMap` on `state.keybinds`. Glue.
fn settings_ui(
    ui: &mut egui::Ui,
    state: &mut SettingsState,
    fullscreen: bool,
    rebinding: &mut Option<GameAction>,
    rebind_conflict: &mut Option<(GameAction, GameAction)>,
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

        // The key-rebind editor (D75 follow-up). One row per rebindable host action (pause /
        // fullscreen / debug overlay — the keys `main.rs` owns): its label + a button showing the
        // current binding. Clicking a button arms capture ("press a key…"); the next mappable key
        // press rebinds through the pure `KeybindMap::rebind`, which rejects a key another action
        // already owns and reports the owner for conflict feedback. Direct-mutates `state.keybinds`
        // (like the sliders); persisted with the other prefs and read by `main.rs` each key event.
        section_label(ui, "KEY BINDINGS");
        for act in GameAction::ALL {
            let capturing = *rebinding == Some(act);
            ui.horizontal(|ui| {
                ui.add_sized(
                    [180.0, 28.0],
                    egui::Label::new(RichText::new(act.label()).color(BONE).size(TYPE_BODY)),
                );
                let btn_label = if capturing {
                    "press a key...".to_string()
                } else {
                    state.keybinds.key_for(act).label().to_string()
                };
                // The armed row reads amber (it's the lone accent + signals "waiting for input").
                let color = if capturing { AMBER } else { BONE };
                if ui
                    .add_sized(
                        [120.0, 28.0],
                        egui::Button::new(RichText::new(btn_label).color(color).size(TYPE_BODY)),
                    )
                    .clicked()
                {
                    // Toggle capture for this row (clicking the armed row cancels), clearing any stale
                    // conflict notice.
                    *rebinding = if capturing { None } else { Some(act) };
                    *rebind_conflict = None;
                }
            });
        }
        // While a row is armed, consume the first mappable key press this frame and apply it. The
        // egui `Key` → engine `KeyId` conversion is the app boundary (invariant #2: the engine seam
        // is winit/egui-free); `rebind` upholds the no-shared-keys invariant.
        if let Some(act) = *rebinding {
            let pressed = ui.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        ..
                    } => egui_key_to_keyid(*key),
                    _ => None,
                })
            });
            if let Some(key) = pressed {
                if key == KeyId::Escape {
                    // Escape is the universal "never mind" — cancel the capture instead of binding
                    // Escape itself (which is the shipped Pause key). The row reverts unchanged.
                    *rebinding = None;
                    *rebind_conflict = None;
                } else {
                    match state.keybinds.rebind(act, key) {
                        RebindOutcome::Conflict(owner) => *rebind_conflict = Some((act, owner)),
                        // Bound or Unchanged: the edit took (or was a no-op) — clear any prior notice.
                        _ => *rebind_conflict = None,
                    }
                    *rebinding = None;
                }
            }
        }
        // Conflict feedback: name the action that already owns the key the player tried to bind.
        if let Some((act, owner)) = *rebind_conflict {
            ui.label(
                RichText::new(format!(
                    "That key already runs {} -- couldn't bind it to {}. Rebind {} first.",
                    owner.label(),
                    act.label(),
                    owner.label()
                ))
                .color(AMBER)
                .size(TYPE_CAPTION),
            );
        }
        // Reset only the bindings to the shipped defaults (a direct in-place edit — no action needed);
        // clears any in-flight capture / conflict. The screen's RESET DEFAULTS also covers these.
        if ui
            .button(RichText::new("Reset bindings").color(BONE).size(TYPE_BODY))
            .clicked()
        {
            state.keybinds.reset();
            *rebinding = None;
            *rebind_conflict = None;
        }

        // The going-dark fairness floor (invariant #6): the embodied alert channel is directional
        // flash + positioned audio. These two opt-in cues give colorblind / hard-of-hearing players a
        // non-color / visual equivalent so the core mechanic stays fair. Direct-mutate checkboxes (the
        // `invert_look_y` pattern), fed to the engine each match frame via `set_accessibility_prefs`.
        section_label(ui, "ACCESSIBILITY");
        ui.checkbox(&mut state.colorblind_cues, "Colorblind cues");
        ui.checkbox(&mut state.visual_sound_cues, "Visual sound cues");
        // Colourblind-safe faction palette (WS-D): a cycling button over the modes (a direct edit —
        // no action needed), the `quality` pattern. Swaps the render ramp so "mine / theirs / neutral
        // / possessed" stay separable without hue; presentation only, pushed via
        // `set_accessibility_prefs` → `Renderer::set_palette_mode`.
        ui.horizontal(|ui| {
            ui.label(RichText::new("Colorblind palette").color(BONE).size(TYPE_BODY));
            ui.add_space(8.0);
            if ui
                .button(RichText::new(state.cvd_palette.label()).color(AMBER).size(TYPE_BODY))
                .clicked()
            {
                state.cvd_palette = state.cvd_palette.next();
            }
        });
        // Cross-modal alert cues (WS-D): the NON-visual equivalent(s) of the directional flash — a
        // bearing-panned audio ping and/or a directional haptic pulse — for a player who can't read
        // the colour flash. A cycling button over the modes (a direct edit — the `cvd_palette`
        // pattern), pushed to the engine via `Game::set_alert_cue_mode`; still an alert, not intel.
        ui.horizontal(|ui| {
            ui.label(RichText::new("Alert cues").color(BONE).size(TYPE_BODY));
            ui.add_space(8.0);
            if ui
                .button(RichText::new(state.alert_cue_mode.label()).color(AMBER).size(TYPE_BODY))
                .clicked()
            {
                state.alert_cue_mode = state.alert_cue_mode.next();
            }
        });

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

        // The gunsmith lives here now (D81): customization-only, reached from Settings, not a play
        // gate. Its edits persist for the next match.
        section_label(ui, "LOADOUT");
        if menu_button(ui, "GUNSMITH", Emphasis::Secondary) {
            action = Some(SettingsAction::OpenLoadout);
        }

        ui.add_space(18.0);
        // "FIELD MANUAL" everywhere (matches Android + this screen's own banner) — was "CONTROLS /
        // ABOUT" here and "MANUAL" on the title, three names for one screen.
        if menu_button(ui, "FIELD MANUAL", Emphasis::Secondary) {
            action = Some(SettingsAction::About);
        }
        ui.add_space(10.0);
        // RESET DEFAULTS wipes audio levels, sensitivity, EVERY rebound key, the accessibility/CVD
        // picks, and the quality tier in one click — gate it behind a confirm so it can't happen by
        // accident with no undo.
        if confirm_menu_button(
            ui,
            "settings.reset",
            "RESET DEFAULTS",
            "RESET ALL? CLICK AGAIN",
            Emphasis::Tertiary,
        ) {
            action = Some(SettingsAction::ResetDefaults);
        }
        // BACK anchors the footer (was sandwiched mid-column, hiding the actions below it).
        ui.add_space(18.0);
        if menu_button(ui, "BACK", Emphasis::Primary) {
            action = Some(SettingsAction::Back);
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
        // RESET RECORD zeroes lifetime matches/wins with no recovery — gate it behind a confirm.
        if confirm_menu_button(
            ui,
            "profile.reset",
            "RESET RECORD",
            "ERASE RECORD? CLICK AGAIN",
            Emphasis::Tertiary,
        ) {
            action = Some(ProfileAction::ResetStats);
        }
        ui.add_space(10.0);
        if menu_button(ui, "BACK", Emphasis::Primary) {
            action = Some(ProfileAction::Back);
        }
    });

    action
}

/// One army card: the army name over its one-line identity blurb, in a framed card whose name is
/// clickable to select it. The currently-selected army reads amber with a SELECTED marker (legible
/// beyond colour alone); clicking a card emits [`ArmySelectAction::Choose`]. Mirrors [`mode_tile`].
/// Glue (needs a live `Ui`) — the decision seam is the pure [`apply_army_select_action`]. ASCII only.
fn army_card(ui: &mut egui::Ui, army: Army, selected: bool) -> Option<ArmySelectAction> {
    use egui::{Button, RichText};
    // The selected card reads amber (the lone accent); the others stay bone.
    let name_color = if selected { AMBER } else { BONE };
    let label = RichText::new(army_label(army).to_uppercase())
        .color(name_color)
        .size(TYPE_SUBHEAD)
        .strong();
    let mut clicked = false;
    card_frame().show(ui, |ui| {
        ui.set_min_width(MENU_BUTTON_W);
        let resp = ui.add(Button::new(label).frame(false).min_size([MENU_BUTTON_W, 28.0].into()));
        ui.label(RichText::new(army_flavor(army)).color(ASH).size(TYPE_CAPTION));
        if selected {
            ui.label(RichText::new("SELECTED").color(AMBER).size(TYPE_CAPTION).strong());
        }
        clicked = resp.clicked();
    });
    clicked.then_some(ArmySelectAction::Choose(army))
}

/// The immediate-mode army-select screen (factions-plan WS-D, D68): the US / French rosters as
/// selectable cards in a column over the backdrop, then CONFIRM. Reads the host-side
/// [`ArmySelectState`] to highlight the current pick; each card's click routes through the pure
/// [`apply_army_select_action`] seam, and the confirmed pick reaches the sim via the `core::shell`
/// SelectArmy seam (`Game::select_army`) at match start. Glue.
fn army_select_ui(ui: &mut egui::Ui, state: &ArmySelectState) -> Option<ArmySelectAction> {
    use egui::RichText;
    let mut action = None;

    over_backdrop_screen(ui, 0.08, |ui| {
        ui.set_min_width(460.0);
        screen_banner(ui, "SELECT ARMY", 130.0);
        ui.label(
            RichText::new(
                "Choose the real-army roster you deploy as. Asymmetry is of flavour and feel, \
                 never of power -- no army is stronger than the other.",
            )
            .color(ASH)
            .size(TYPE_BODY),
        );
        ui.add_space(18.0);

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
        // leaves the already-applied selection alone.)
        if menu_button(ui, "BACK", Emphasis::Primary) {
            action = Some(ArmySelectAction::Confirm);
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

/// One mode/map tile: the mode name over its one-line blurb, as a full-width button; clicking it
/// deploys that mode. Mirrors Android's `ModeTile`. Glue (needs a live `Ui`) — the launch decision is
/// the pure [`GameMode::scene`] seam the host resolves; this only reports the pick. ASCII only.
fn mode_tile(ui: &mut egui::Ui, mode: &GameMode) -> Option<ModeSelectAction> {
    use egui::{Button, RichText};
    let label = RichText::new(mode.name.to_uppercase())
        .color(BONE)
        .size(TYPE_SUBHEAD)
        .strong();
    let mut clicked = false;
    card_frame().show(ui, |ui| {
        ui.set_min_width(MENU_BUTTON_W);
        let resp = ui.add(Button::new(label).frame(false).min_size([MENU_BUTTON_W, 28.0].into()));
        ui.label(RichText::new(mode.blurb).color(ASH).size(TYPE_CAPTION));
        clicked = resp.clicked();
    });
    clicked.then_some(ModeSelectAction::Pick(*mode))
}

/// The immediate-mode Pve/Pvp mode/map-select screen (D81): the standing battle scenes as tiles in a
/// card over the backdrop, then BACK. Reads the static [`SHELL_GAME_MODES`] (host presentation, never
/// the sim); each pick routes through the `engine`-tested [`GameMode::scene`] seam at the host. Glue.
fn mode_select_ui(ui: &mut egui::Ui) -> Option<ModeSelectAction> {
    use egui::RichText;
    let mut action = None;

    over_backdrop_screen(ui, 0.08, |ui| {
        ui.set_min_width(460.0);
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

        ui.add_space(22.0);
        // BACK is the only exit on this screen — Secondary, not Tertiary, so it isn't the dimmest
        // control on a screen where it's the sole way out.
        if menu_button(ui, "BACK", Emphasis::Secondary) {
            action = Some(ModeSelectAction::Back);
        }
    });

    action
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
        // Sole exit on this screen — Secondary, not the dimmest Tertiary. (Briefing keeps BACK
        // Tertiary because DEPLOY is the genuine primary action there.)
        if menu_button(ui, "BACK", Emphasis::Secondary) {
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
            // Difficulty cycler — the replay tier that drives the fight (D83: the 4→3 enemy-commander
            // band + the scenario situation modifiers) and the tier the CLEAR is recorded against on a
            // win.
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
    fn pve_and_pvp_open_the_mode_select() {
        // D81: PvE/PvP now open the mode/map select (the deploy gate), not the gunsmith — the
        // gunsmith moved behind Settings as customization-only. PvE and PvP share the picker until
        // PvP match-setup lands (Q5).
        for mode in [TitleAction::Pve, TitleAction::Pvp] {
            assert_eq!(
                resolve_title_action(mode),
                HostTransition::OpenModeSelect,
                "{mode:?} must open the mode select"
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
    fn done_is_a_screen_transition_that_leaves_the_editor_alone() {
        // D81: the gunsmith is customization-only — DONE returns to Settings carrying the edited
        // (persisted) loadout unchanged; there is no Deploy here.
        let mut ed = LoadoutEditor::new();
        apply_loadout_action(
            LoadoutAction::Cycle {
                slot_index: 2,
                forward: true,
            },
            &mut ed,
        );
        let chosen = ed.current();
        assert_eq!(apply_loadout_action(LoadoutAction::Done, &mut ed), LoadoutStep::Done);
        assert_eq!(ed.current(), chosen, "Done doesn't alter the editor");
    }

    // ---- The shell palette (derived from the shared render theme) --------------------------------

    /// `rgb8` rounds an sRGB `[0,1]` colour to 8-bit correctly (the bridge from `render::theme` to
    /// egui `Color32`).
    #[test]
    fn rgb8_rounds_srgb_to_8bit() {
        assert_eq!(rgb8([0.0, 0.5, 1.0]), egui::Color32::from_rgb(0, 128, 255));
        assert_eq!(rgb8([1.0, 1.0, 1.0]), egui::Color32::from_rgb(255, 255, 255));
        // The renderer INK maps to the shell's shipped ink hex.
        assert_eq!(rgb8(gonedark_render::theme::INK), egui::Color32::from_rgb(0x07, 0x09, 0x0C));
    }

    /// The shared-identity ramp is DERIVED from `gonedark_render::theme` — one source of truth, so a
    /// renderer palette retune can't silently drift the out-of-match shell (the D75 hazard, now a
    /// compile-time link + this guard).
    #[test]
    fn shell_shared_ramp_tracks_the_render_theme() {
        use gonedark_render::theme;
        assert_eq!(INK, rgb8(theme::INK));
        assert_eq!(BONE, rgb8(theme::BONE));
        assert_eq!(ASH, rgb8(theme::ASH));
        assert_eq!(RIM, rgb8(theme::RIM));
    }

    /// Switching the shared ramp to derivation did NOT shift the shipped look: the derived colours
    /// still equal the exact hex the shell shipped (INK/BONE/ASH/RIM).
    #[test]
    fn shell_shared_ramp_preserves_the_shipped_hex() {
        assert_eq!(INK, egui::Color32::from_rgb(0x07, 0x09, 0x0C));
        assert_eq!(BONE, egui::Color32::from_rgb(0xE7, 0xEC, 0xEF));
        assert_eq!(ASH, egui::Color32::from_rgb(0x8A, 0x94, 0x9C));
        assert_eq!(RIM, egui::Color32::from_rgb(0x29, 0x30, 0x42));
    }

    /// The in-match-tuned variants are deliberately the SHELL values (the renderer nudges its own
    /// `PANEL`/`PANEL_RAISED`/`AMBER` deeper/warmer for the HUD). Pin them so a retune is a conscious
    /// edit, and assert they genuinely differ from a naive derivation of the renderer consts.
    #[test]
    fn shell_in_match_variants_are_pinned_and_distinct() {
        use gonedark_render::theme;
        assert_eq!(PANEL, egui::Color32::from_rgb(0x12, 0x18, 0x20));
        assert_eq!(PANEL_RAISED, egui::Color32::from_rgb(0x1B, 0x25, 0x31));
        assert_eq!(AMBER, egui::Color32::from_rgb(0xE0, 0x79, 0x1F));
        assert_eq!(MUTED, egui::Color32::from_rgb(0x61, 0x68, 0x75));
        // The shell PANEL/AMBER are intentionally NOT the renderer's in-match variants.
        assert_ne!(PANEL, rgb8(theme::PANEL), "shell PANEL is lighter than the in-match HUD PANEL");
        assert_ne!(AMBER, rgb8(theme::AMBER), "shell AMBER is cooler than the in-match HUD AMBER");
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

    #[test]
    fn per_option_delta_text_changes_with_the_selection() {
        // M3: the gunsmith now surfaces the REAL per-option StatDelta numbers, so the readout differs
        // per selected option (the old static hint read identically for every option). Mirrors the
        // slot_trade_hint tests: pure, ASCII, and asserted on the editor-backed formatter.
        let mut ed = LoadoutEditor::new();
        // Baseline: a Standard option moves nothing.
        let base = stat_delta_summary(&ed.option_delta(LoadoutSlot::Barrel));
        assert_eq!(base, "no change", "the neutral option reads as no change");

        ed.cycle(LoadoutSlot::Barrel, true); // Heavy: +damage, -reserve
        let heavy = stat_delta_summary(&ed.option_delta(LoadoutSlot::Barrel));
        assert_ne!(heavy, base, "cycling changes the surfaced per-option delta");
        assert!(
            heavy.contains("dmg") && heavy.contains("res"),
            "shows the real traded axes with numbers, got {heavy:?}"
        );

        ed.cycle(LoadoutSlot::Barrel, true); // Light: -damage, +reserve (the opposed trade)
        let light = stat_delta_summary(&ed.option_delta(LoadoutSlot::Barrel));
        assert_ne!(light, heavy, "each option reads distinctly");

        // Cosmetic Grip carries no sim delta (D85) → always "no change".
        ed.cycle(LoadoutSlot::Grip, true);
        assert_eq!(stat_delta_summary(&ed.option_delta(LoadoutSlot::Grip)), "no change");

        // The build-wide net readout reflects the chosen build and is nonempty once off baseline.
        assert_ne!(stat_delta_summary(&ed.net_delta()), "no change");

        // ASCII only, so it can never tofu in egui's default font (same rule as the trade hints).
        assert!(heavy.is_ascii() && light.is_ascii());
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
        // Accessibility cues default OFF (opt-in intensifiers over the base fair channel).
        assert!(!s.colorblind_cues);
        assert!(!s.visual_sound_cues);
    }

    #[test]
    fn accessibility_toggles_round_trip_and_default_when_missing() {
        // Both toggles survive an encode→decode round-trip in either state.
        for (cvd, snd) in [(true, false), (false, true), (true, true), (false, false)] {
            let s = SettingsState {
                colorblind_cues: cvd,
                visual_sound_cues: snd,
                ..SettingsState::default()
            };
            let blob = encode_shell_prefs(
                &s,
                &ProfileState::default(),
                &LoadoutEditor::new(),
                &ArmySelectState::default(),
            );
            let (s2, _, _, _) = decode_shell_prefs(&blob);
            assert_eq!(s2.colorblind_cues, cvd, "cvd toggle survives round-trip");
            assert_eq!(s2.visual_sound_cues, snd, "sound toggle survives round-trip");
        }
        // A blob missing the keys (e.g. an older save) decodes them to the OFF default, never panics.
        let (s, _, _, _) = decode_shell_prefs("gonedark-shell 1\nmaster=0.5\n");
        assert!(!s.colorblind_cues, "missing cvdcues → default off");
        assert!(!s.visual_sound_cues, "missing soundcues → default off");
        // An unparseable value also keeps the default.
        let (s2, _, _, _) = decode_shell_prefs("cvdcues=maybe\nsoundcues=\n");
        assert!(!s2.colorblind_cues && !s2.visual_sound_cues);
    }

    #[test]
    fn colorblind_palette_round_trips_every_mode_and_defaults_when_missing() {
        // Every palette mode survives an encode→decode round-trip (WS-D accessibility).
        for &mode in &PaletteMode::ALL {
            let s = SettingsState {
                cvd_palette: mode,
                ..SettingsState::default()
            };
            let blob = encode_shell_prefs(
                &s,
                &ProfileState::default(),
                &LoadoutEditor::new(),
                &ArmySelectState::default(),
            );
            let (s2, _, _, _) = decode_shell_prefs(&blob);
            assert_eq!(s2.cvd_palette, mode, "{mode:?} palette survives round-trip");
        }
        // A blob missing the key (an older save) decodes to Off; a garbage ordinal also falls back.
        let (s, _, _, _) = decode_shell_prefs("gonedark-shell 1\nmaster=0.5\n");
        assert_eq!(s.cvd_palette, PaletteMode::Off, "missing cvdpal → Off");
        let (s2, _, _, _) = decode_shell_prefs("cvdpal=999\n");
        assert_eq!(s2.cvd_palette, PaletteMode::Off, "out-of-range cvdpal → Off");
    }

    #[test]
    fn alert_cue_mode_round_trips_every_mode_and_defaults_when_missing() {
        // Every cross-modal alert-cue mode survives an encode→decode round-trip (WS-D accessibility).
        for &mode in &AlertCueMode::ALL {
            let s = SettingsState {
                alert_cue_mode: mode,
                ..SettingsState::default()
            };
            let blob = encode_shell_prefs(
                &s,
                &ProfileState::default(),
                &LoadoutEditor::new(),
                &ArmySelectState::default(),
            );
            let (s2, _, _, _) = decode_shell_prefs(&blob);
            assert_eq!(s2.alert_cue_mode, mode, "{mode:?} alert-cue mode survives round-trip");
        }
        // A blob missing the key (an older save) decodes to Off; a garbage ordinal also falls back.
        let (s, _, _, _) = decode_shell_prefs("gonedark-shell 1\nmaster=0.5\n");
        assert_eq!(s.alert_cue_mode, AlertCueMode::Off, "missing alertcue → Off");
        let (s2, _, _, _) = decode_shell_prefs("alertcue=999\n");
        assert_eq!(s2.alert_cue_mode, AlertCueMode::Off, "out-of-range alertcue → Off");
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
            colorblind_cues: false,
            visual_sound_cues: false,
            cvd_palette: PaletteMode::Off,
            alert_cue_mode: AlertCueMode::Off,
            keybinds: KeybindMap::default(),
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
        // A remapped binding is also restored by the whole-screen RESET DEFAULTS.
        s.keybinds.rebind(GameAction::Pause, KeyId::P);
        let step = apply_settings_action(SettingsAction::ResetDefaults, &mut s);
        assert_eq!(step, SettingsStep::Stay);
        assert_eq!(s, SettingsState::default());
    }

    #[test]
    fn confirm_gate_requires_two_clicks_to_fire() {
        // First click on a destructive button arms it (relabel to the confirm prompt) but does NOT
        // fire; a click while armed fires and disarms. Guards the three one-click state wipes.
        let (armed_after_first, fired_first) = confirm_click(false);
        assert!(armed_after_first, "first click arms");
        assert!(!fired_first, "first click never fires the destructive action");
        let (armed_after_second, fired_second) = confirm_click(true);
        assert!(!armed_after_second, "confirming click disarms");
        assert!(fired_second, "confirming click fires");
    }

    #[test]
    fn egui_key_maps_to_keyid_at_the_boundary() {
        // The keys the default bindings use map through, plus a representative letter/digit/nav key.
        assert_eq!(egui_key_to_keyid(egui::Key::Escape), Some(KeyId::Escape));
        assert_eq!(egui_key_to_keyid(egui::Key::F11), Some(KeyId::F11));
        assert_eq!(egui_key_to_keyid(egui::Key::F3), Some(KeyId::F3));
        assert_eq!(egui_key_to_keyid(egui::Key::P), Some(KeyId::P));
        assert_eq!(egui_key_to_keyid(egui::Key::Num5), Some(KeyId::Digit5));
        assert_eq!(egui_key_to_keyid(egui::Key::ArrowUp), Some(KeyId::Up));
        assert_eq!(egui_key_to_keyid(egui::Key::Backtick), Some(KeyId::Backquote));
        // A key outside the bindable vocabulary is rejected (so an unmappable press keeps waiting).
        assert_eq!(egui_key_to_keyid(egui::Key::Colon), None);
    }

    #[test]
    fn keybinds_survive_the_shell_prefs_round_trip_and_default_when_missing() {
        // A remapped keybind survives encode→decode alongside the other prefs.
        let mut s = SettingsState::default();
        assert_eq!(s.keybinds.rebind(GameAction::Pause, KeyId::P), RebindOutcome::Bound);
        assert_eq!(
            s.keybinds.rebind(GameAction::ToggleDebugOverlay, KeyId::G),
            RebindOutcome::Bound
        );
        let blob = encode_shell_prefs(
            &s,
            &ProfileState::default(),
            &LoadoutEditor::new(),
            &ArmySelectState::default(),
        );
        let (s2, _, _, _) = decode_shell_prefs(&blob);
        assert_eq!(s2.keybinds, s.keybinds, "keybinds survive the round-trip");
        assert_eq!(s2.keybinds.key_for(GameAction::Pause), KeyId::P);

        // A blob missing the key (an older save) decodes to the shipped default bindings, never panics.
        let (s3, _, _, _) = decode_shell_prefs("gonedark-shell 1\nmaster=0.5\n");
        assert_eq!(s3.keybinds, KeybindMap::default(), "missing keybinds → defaults");
        // A garbage value also falls back to defaults (KeybindMap::decode is total).
        let (s4, _, _, _) = decode_shell_prefs("keybinds=wat,nope\n");
        assert_eq!(s4.keybinds, KeybindMap::default(), "garbage keybinds → defaults");
    }

    #[test]
    fn settings_discrete_actions_map_to_their_steps() {
        let mut s = SettingsState::default();
        assert_eq!(
            apply_settings_action(SettingsAction::ToggleFullscreen, &mut s),
            SettingsStep::ToggleFullscreen
        );
        assert_eq!(
            apply_settings_action(SettingsAction::OpenLoadout, &mut s),
            SettingsStep::OpenLoadout,
            "the gunsmith is reached from Settings (D81)"
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

    #[test]
    fn quality_to_tier_maps_explicit_picks_and_defers_auto() {
        // The explicit picks pin a concrete render tier regardless of the device default...
        assert_eq!(
            QualityChoice::Low.to_tier(QualityTier::High),
            QualityTier::Low
        );
        assert_eq!(
            QualityChoice::Medium.to_tier(QualityTier::High),
            QualityTier::Mid
        );
        assert_eq!(
            QualityChoice::High.to_tier(QualityTier::Low),
            QualityTier::High
        );
        // ...while Auto defers to whatever device default the host passes (so on desktop, High).
        assert_eq!(
            QualityChoice::Auto.to_tier(QualityTier::High),
            QualityTier::High
        );
        assert_eq!(
            QualityChoice::Auto.to_tier(QualityTier::Mid),
            QualityTier::Mid
        );
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

    // ---- The shell-prefs persistence codec -------------------------------------------------------

    use gonedark_core::gunsmith::{Barrel, Loadout, Magazine, Optic};

    /// A non-default state across all four objects, to prove the round-trip carries every field.
    fn sample_state() -> (SettingsState, ProfileState, LoadoutEditor, ArmySelectState) {
        let settings = SettingsState {
            master_volume: 0.35,
            sfx_volume: 0.5,
            music_volume: 0.25,
            mouse_sensitivity: 2.4,
            invert_look_y: true,
            quality: QualityChoice::High,
            colorblind_cues: true,
            visual_sound_cues: true,
            cvd_palette: PaletteMode::Tritanopia,
            alert_cue_mode: AlertCueMode::AudioHaptic,
            // A remapped binding (Pause → P) so the round-trip proves keybinds are carried too.
            keybinds: {
                let mut k = KeybindMap::default();
                k.rebind(GameAction::Pause, KeyId::P);
                k
            },
        };
        let profile = ProfileState {
            callsign: "Reaper".to_string(),
            faction: FactionPref::FrenchArmy,
            matches_played: 12,
            wins: 7,
        };
        let loadout = LoadoutEditor::with_loadout(Loadout {
            optic: Optic::Marksman,
            barrel: Barrel::Heavy,
            magazine: Magazine::Extended,
            ..Loadout::STANDARD
        });
        // A non-default army pick (FR, not the US default) so the round-trip proves the field carries.
        let army = ArmySelectState {
            selected: Army::Fr,
        };
        (settings, profile, loadout, army)
    }

    #[test]
    fn shell_prefs_round_trip_preserves_every_field() {
        let (s, p, l, a) = sample_state();
        let blob = encode_shell_prefs(&s, &p, &l, &a);
        let (s2, p2, l2, a2) = decode_shell_prefs(&blob);
        assert_eq!(s2, s, "settings survive the round-trip");
        assert_eq!(p2, p, "profile survives the round-trip");
        assert_eq!(l2.current(), l.current(), "loadout survives the round-trip");
        assert_eq!(a2, a, "army pick survives the round-trip");
    }

    #[test]
    fn shell_prefs_encode_is_stable_under_re_encode() {
        // A decode→encode of an encoded blob reproduces the same bytes (canonical, already-clamped).
        let (s, p, l, a) = sample_state();
        let blob = encode_shell_prefs(&s, &p, &l, &a);
        let (s2, p2, l2, a2) = decode_shell_prefs(&blob);
        assert_eq!(encode_shell_prefs(&s2, &p2, &l2, &a2), blob);
    }

    #[test]
    fn empty_or_garbage_blob_decodes_to_defaults() {
        for blob in ["", "gonedark-shell 1\n", "total nonsense\n???\n", "master\nsfx=\n"] {
            let (s, p, l, a) = decode_shell_prefs(blob);
            assert_eq!(s, SettingsState::default(), "blob {blob:?} → default settings");
            assert_eq!(p, ProfileState::default(), "blob {blob:?} → default profile");
            assert_eq!(
                l.current(),
                LoadoutEditor::new().current(),
                "blob {blob:?} → default loadout"
            );
            assert_eq!(a, ArmySelectState::default(), "blob {blob:?} → default army pick");
        }
    }

    #[test]
    fn decode_tolerates_out_of_range_and_unparseable_values() {
        // Out-of-range numerics are clamped; out-of-range enum ordinals fall back to the default;
        // an unparseable value keeps the field default. Never panics.
        let blob = "master=9.9\nsfx=-3\nsens=999\ninverty=maybe\nquality=42\n\
                    faction=99\nmatches=notanumber\noptic=7\nbarrel=-1\nmagazine=abc\n\
                    army=42\ncallsign=   \n";
        let (s, p, l, a) = decode_shell_prefs(blob);
        assert_eq!(s.master_volume, 1.0, "over-range gain clamps to 1.0");
        assert_eq!(s.sfx_volume, 0.0, "negative gain clamps to 0.0");
        assert_eq!(s.mouse_sensitivity, SettingsState::SENS_MAX, "over-range sens clamps");
        assert!(!s.invert_look_y, "unparseable bool keeps the default (false)");
        assert_eq!(s.quality, QualityChoice::Auto, "out-of-range quality ordinal → default");
        assert_eq!(p.faction, FactionPref::UsArmy, "out-of-range faction ordinal → default");
        assert_eq!(p.matches_played, 0, "unparseable count keeps the default");
        // A blank callsign sanitises to the default; the out-of-range loadout ordinals default.
        assert_eq!(p.callsign, DEFAULT_CALLSIGN);
        assert_eq!(l.current(), Loadout::STANDARD, "out-of-range slot ordinals → Standard");
        assert_eq!(a, ArmySelectState::default(), "out-of-range army ordinal → default (US)");
    }

    #[test]
    fn decode_sanitizes_and_strips_a_newline_injected_callsign() {
        // A callsign carrying a newline can't corrupt the line-based blob: encode strips it, and the
        // decoded value is the sanitized single-line name.
        let p = ProfileState {
            callsign: "Rea\nper".to_string(),
            ..ProfileState::default()
        };
        let blob = encode_shell_prefs(
            &SettingsState::default(),
            &p,
            &LoadoutEditor::new(),
            &ArmySelectState::default(),
        );
        assert_eq!(blob.lines().filter(|l| l.starts_with("callsign=")).count(), 1);
        let (_, p2, _, _) = decode_shell_prefs(&blob);
        assert!(!p2.callsign.contains('\n'));
        assert_eq!(p2.callsign, "Rea per");
    }

    // ---- The army-select pure seam ---------------------------------------------------------------

    #[test]
    fn army_opens_the_army_select_screen() {
        assert_eq!(
            resolve_title_action(TitleAction::Army),
            HostTransition::OpenArmySelect
        );
    }

    #[test]
    fn army_select_default_is_a_real_combatant_roster() {
        // The player always fields a real army — never the non-aligned Neutral default.
        let a = ArmySelectState::default();
        assert_eq!(a.selected, Army::Us);
        assert_ne!(a.selected, Army::Neutral);
        // Both selectable armies are real combatants, in a stable US-then-FR order, no Neutral.
        assert_eq!(SELECTABLE_ARMIES, [Army::Us, Army::Fr]);
        assert!(!SELECTABLE_ARMIES.contains(&Army::Neutral));
    }

    #[test]
    fn army_choose_edits_the_selection_and_stays() {
        let mut a = ArmySelectState::default();
        assert_eq!(a.selected, Army::Us);
        let step = apply_army_select_action(ArmySelectAction::Choose(Army::Fr), &mut a);
        assert_eq!(step, ArmySelectStep::Stay, "a choice keeps the player on-screen");
        assert_eq!(a.selected, Army::Fr, "the choice is recorded in place");
        // Choosing again switches back (an idempotent in-place edit).
        apply_army_select_action(ArmySelectAction::Choose(Army::Us), &mut a);
        assert_eq!(a.selected, Army::Us);
    }

    #[test]
    fn army_confirm_is_a_transition_that_leaves_the_selection_alone() {
        let mut a = ArmySelectState {
            selected: Army::Fr,
        };
        let step = apply_army_select_action(ArmySelectAction::Confirm, &mut a);
        assert_eq!(step, ArmySelectStep::Confirm);
        assert_eq!(a.selected, Army::Fr, "confirm carries the current pick, unchanged");
    }

    #[test]
    fn army_labels_and_flavor_are_distinct_ascii() {
        // Every selectable army has a distinct, non-empty ASCII name + flavour (no tofu in the default
        // font), and the flavour anchors the real-platform identity (factions.md §4).
        let labels: Vec<&str> = SELECTABLE_ARMIES.iter().map(|&a| army_label(a)).collect();
        let flavors: Vec<&str> = SELECTABLE_ARMIES.iter().map(|&a| army_flavor(a)).collect();
        for text in labels.iter().chain(flavors.iter()) {
            assert!(!text.is_empty() && text.is_ascii(), "{text:?} must be non-empty ASCII");
        }
        assert_ne!(labels[0], labels[1], "the two armies have distinct names");
        assert_ne!(flavors[0], flavors[1], "the two armies have distinct flavour");
        assert_eq!(army_label(Army::Us), "US Army");
        assert_eq!(army_label(Army::Fr), "French Army");
    }

    #[test]
    fn army_round_trips_each_selectable_pick() {
        // Each real army survives a save→load round-trip through the codec (the ordinal is the sim/wire
        // tag order), independent of the other prefs.
        for &army in &SELECTABLE_ARMIES {
            let a = ArmySelectState { selected: army };
            let blob = encode_shell_prefs(
                &SettingsState::default(),
                &ProfileState::default(),
                &LoadoutEditor::new(),
                &a,
            );
            let (_, _, _, a2) = decode_shell_prefs(&blob);
            assert_eq!(a2.selected, army, "{army:?} must survive the round-trip");
        }
    }

    #[test]
    fn decode_army_rejects_neutral_and_missing_falling_back_to_default() {
        // A stored Neutral ordinal (0) is not a valid player pick → the US default; a missing key →
        // the US default. A real ordinal decodes faithfully.
        let default = ArmySelectState::default().selected;
        assert_eq!(decode_army(Some(&"0")), default, "Neutral ordinal → default (US)");
        assert_eq!(decode_army(None), default, "missing key → default (US)");
        assert_eq!(decode_army(Some(&"2")), Army::Fr, "ordinal 2 → French Army");
        assert_eq!(decode_army(Some(&"1")), Army::Us, "ordinal 1 → US Army");
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
