//! Desktop host: a `winit` 0.30 `ApplicationHandler` that boots into the native **app shell** —
//! the egui title screen ([D36](../docs/decisions.md)) — and, on **Start**, drives the shared
//! [`gonedark_engine::Game`].
//!
//! The out-of-match host screens are the desktop counterpart of Android's `MainActivity →
//! NativeActivity` split ([D35](../docs/decisions.md)): the **Title** screen, the Pve/Pvp
//! **mode-select** and campaign **mission-select/briefing** deploy gates, the **Settings** screen and
//! the gunsmith **Loadout** customization screen behind it — all egui, in [`shell`] — and the in-match
//! **Game** (the shared engine loop — deterministic fixed-tick sim, render interpolation (invariant
//! #4), the embodiment input-source swap (invariant #5)). The shell holds no game logic and reaches
//! `core` only through host-side seams. **D81:** a play-mode / briefing DEPLOY creates the `Game`
//! directly (via [`App::enter_match`]), fielding the player's persisted `core::gunsmith::Loadout` via
//! [`Game::new_scene_with_loadout`] (WS-C, D60); the gunsmith is customization-only behind Settings,
//! not a play gate.
//!
//! This binary owns only the desktop concerns: the window, the wgpu surface, input plumbing, the
//! egui shell, and the wall clock that feeds per-frame `dt` into the engine's fixed-tick accumulator.

use gonedark_core::campaign::{Campaign, Difficulty, NodeId};
use gonedark_core::components::Faction;
use gonedark_engine::keybind::{GameAction, KeyId};
use gonedark_engine::loadout_ui::LoadoutEditor;
use gonedark_engine::mission_registry::{default_campaign, default_registry, MissionRegistry};
use gonedark_engine::objectives::MissionStatus;
use gonedark_engine::{pixel_to_ndc, Game, OverlayClick, Scene, DEFAULT_SEED};
use gonedark_pal_desktop::{DesktopAudio, DesktopInput, DesktopRenderSurface, DesktopThermalSensor};
use gonedark_render::tiers::QualityTier;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowAttributes, WindowId};

/// The device-default render tier the Settings `Auto` quality choice resolves to on desktop. Desktop
/// is the D22 flagship class (there is no per-device auto-detect yet), so it matches `Game::new`'s
/// starting `QualityTier::High`. Keep the two in sync: `Auto` must reproduce the shipped default.
const DESKTOP_DEFAULT_TIER: QualityTier = QualityTier::High;

mod shell;
use shell::{
    apply_army_select_action, apply_briefing_action, apply_loadout_action, apply_profile_action,
    apply_settings_action, build_channel, build_stamp, resolve_title_action, AboutReturn,
    ArmySelectState, ArmySelectStep, BriefingOutcome, EguiShell, HostTransition, LoadoutStep,
    MissionSelectAction, ModeSelectAction, ProfileState, ProfileStep, SettingsState, SettingsStep,
};

/// Which host screen is up: the out-of-match title shell, the pre-match gunsmith, or a running
/// match. Entering a match lazily constructs `Game` (it needs the GPU device that only exists after
/// `resumed`), fielding the loadout chosen on the gunsmith screen.
enum Screen {
    Title,
    /// The gunsmith / loadout customization screen (egui). **D81: reached from Settings,
    /// customization-only** (RESET / DONE, no Deploy) — it edits the persisted loadout, it does not
    /// start a match. The editable selection itself lives on [`App::loadout`] (host-side state), so
    /// this variant carries no data.
    Loadout,
    /// The out-of-match Settings screen (audio / controls / video prefs). State lives on
    /// [`App::settings`], so this variant carries no data.
    Settings,
    /// The out-of-match player Profile screen. State lives on [`App::profile`].
    Profile,
    /// The **army-select** screen — pick the US/FR roster the player deploys as (factions-plan WS-D,
    /// D68). State lives on [`App::army_select`]; carries no data of its own.
    ArmySelect,
    /// The Pve/Pvp **mode / map select** screen (D81). Reads the static
    /// [`gonedark_engine::shell_modes::SHELL_GAME_MODES`]; carries no host data. Picking a mode sets
    /// [`App::scene`] and deploys straight into the match with the persisted loadout.
    ModeSelect,
    /// The Operations-hub **mission-select** screen (PvE campaign, D58). Reads [`App::campaign`];
    /// carries no data of its own.
    MissionSelect,
    /// The **briefing** screen for one campaign node. Carries the [`NodeId`] whose briefing it shows;
    /// the replay-tier selector lives on [`App::briefing_difficulty`].
    Briefing(NodeId),
    /// The About / controls-reference screen. Carries the [`AboutReturn`] entry point BACK lands on —
    /// About is reachable from both the title (returns to title) and Settings (returns to Settings).
    About(AboutReturn),
    // `Game` is large (the renderer + sim state); box it so the idle `Title` variant doesn't carry
    // that footprint around (clippy::large_enum_variant).
    InMatch(Box<Game>),
}

/// The desktop host: the wgpu surface, the egui shell, the current host screen, input/audio, and
/// the wall clock for per-frame dt. The `Game` lives inside `Screen::InMatch` and is created on
/// Start, so a fresh launch sits on the title screen with no sim running.
struct App {
    surface: Option<DesktopRenderSurface>,
    shell: Option<EguiShell>,
    screen: Screen,
    input: DesktopInput,
    /// The desktop audio sink handed into `Game::frame` for the embodied mix (worker 3).
    audio: DesktopAudio,
    /// The desktop PAL thermal sensor (Phase 4 WS-C) handed into `Game::frame` so the render-tuning
    /// loop reacts to heat. On the dev workstation this is the synthetic stub (defaults to `Nominal`,
    /// `forced` drives the backoff for dev); on Android the real `PowerManager`/`BatteryManager`
    /// reader stands in its place. Render-only — it never feeds the sim (invariant #1/#4).
    thermal: DesktopThermalSensor,
    last_frame: Instant,

    /// Whether the OS cursor is currently locked+hidden. Tracked so we only call the (relatively
    /// costly) winit grab/visibility setters on a *change*, not every frame. Cursor capture is a
    /// pure desktop-host concern — it never touches the sim — so it lives here, not in the engine.
    cursor_captured: bool,
    /// Momentary "free the cursor" request — true while **Left Alt** is held (released on key-up).
    /// Lets an embodied player hand the pointer back transiently (e.g. to alt-tab) without opening
    /// the pause menu. A shell overlay (pause / reconnect / summary) frees the cursor on its own.
    alt_held: bool,
    /// Whether the window is currently in borderless fullscreen. Toggled by **F11** on any screen.
    /// A pure window concern — it never touches the sim — so it lives on the host like cursor state.
    fullscreen: bool,

    /// Which [`Scene`] a **Start** boots into — `Scene::Skirmish` (the playable two-base match)
    /// unless the `--scene <name>` launch flag selected another scene (the canned `--scene default`
    /// demo or a debug sandbox like `--scene duel`). A pure host launch choice; it only picks which
    /// `Game::new_scene` seeding runs.
    scene: Scene,

    /// The player's gunsmith selection (the `engine::loadout_ui` seam over `core::gunsmith`). Edited
    /// on the [`Screen::Loadout`] gunsmith screen (D81: customization-only, reached from Settings) and
    /// handed to [`Game::new_scene_with_loadout`] by [`App::enter_match`] whenever a match deploys.
    /// Host-side state — it never touches the sim until the scenario seeder applies it at match start
    /// (WS-C, D60). Persists across matches so the player keeps their build; the gunsmith's RESET
    /// button returns it to the neutral baseline.
    loadout: LoadoutEditor,

    /// Host-side player preferences (audio / look / video / quality) edited on the Settings screen.
    /// Presentation only — never reaches the sim; persists across screens for the session.
    settings: SettingsState,
    /// Host-side player identity + lifetime record shown on the Profile screen. Presentation only.
    profile: ProfileState,
    /// Host-side army pick (US/FR) edited on the army-select screen. **Match-setup config**: it never
    /// reaches the sim except through the `core::shell` SelectArmy seam at match start
    /// ([`App::enter_match`] → `Game::select_army`), and persists across launches in the shell prefs.
    army_select: ArmySelectState,

    /// The Operations-hub campaign model (PvE WS-B, D58) the mission-select / briefing screens read
    /// and a win advances. **Host-side meta-progression — never sim state, never in the per-tick
    /// checksum** (invariants #1/#7): its progress persists to its own host blob
    /// (`Campaign::serialize_progress`), separate from any sim snapshot. Initialised from
    /// `default_campaign()` and loaded from disk at startup.
    campaign: Campaign,
    /// The host-side `MissionId -> runnable mission` registry (`default_registry()`). Consulted when
    /// a node is launched to find the mission's authored enemy-commander tier (a host-side planning
    /// knob, never sim state) — see [`HostTransition::LaunchMission`].
    registry: MissionRegistry,
    /// The replay-tier selector shown on the active briefing screen (the campaign 4-tier
    /// coordinate). Reset when a briefing opens; its value is recorded against `Campaign::clear` on a
    /// win. Presentation only.
    briefing_difficulty: Difficulty,
    /// A campaign mission queued for launch (`node` + the player's chosen replay `difficulty`), set
    /// when a briefing's DEPLOY routes through the gunsmith, consumed when the match is created. The
    /// `difficulty` is the campaign tier the **clear** is recorded against — not the commander tier.
    pending_launch: Option<(NodeId, Difficulty)>,
    /// The campaign mission backing the *running* match, if any (`node` + the chosen replay
    /// `difficulty`), so a win can record the clear. `None` for non-campaign matches (PvE/PvP, debug
    /// scenes). Carried across the in-match screen; cleared on exit-to-title.
    active_mission: Option<(NodeId, Difficulty)>,
    /// Whether the current match's clear has already been recorded — so a win is recorded exactly
    /// once even though `mission_status()` reads `Won` every subsequent frame. Reset at match start.
    mission_recorded: bool,

    /// Effective music-bus gain (D75 follow-up). Refreshed each match frame from the Settings
    /// `master_volume`×`music_volume` via `gonedark_engine::music_gain` — the music analog of the SFX
    /// `DesktopAudio::set_gains` push — and pushed to the sink's looping music bed via
    /// `DesktopAudio::set_music_gain`. Presentation only — never a sim input (invariant #1/#4).
    music_gain: f32,
}

impl App {
    fn new(scene: Scene) -> Self {
        // The shipped Operations-hub campaign + its mission registry (PvE WS-B). Load any persisted
        // progress over the fresh graph — a missing/corrupt blob just leaves it at the start (the
        // load is all-or-nothing, never partial). Host-side meta-state only (invariants #1/#7).
        let mut campaign = default_campaign();
        load_campaign_progress(&mut campaign);
        // Restore the player's Settings / Profile / gunsmith loadout from the last session (falls back
        // to defaults on a fresh device or a corrupt blob). Host-side presentation state only — never
        // sim state (invariant #1), persisted to its own blob alongside campaign progress.
        let (settings, profile, loadout, army_select) = load_shell_prefs();
        App {
            surface: None,
            shell: None,
            screen: Screen::Title,
            input: DesktopInput::new(),
            audio: DesktopAudio::new(),
            thermal: DesktopThermalSensor::new(),
            last_frame: Instant::now(),
            cursor_captured: false,
            alt_held: false,
            fullscreen: false,
            scene,
            loadout,
            settings,
            profile,
            army_select,
            campaign,
            registry: default_registry(),
            briefing_difficulty: Difficulty::default(),
            pending_launch: None,
            active_mission: None,
            mission_recorded: false,
            // Recomputed each match frame from the Settings volumes; the default mirrors
            // SettingsState's shipped master×music until the first frame refreshes it.
            music_gain: 0.0,
        }
    }

    /// Toggle borderless fullscreen. `Fullscreen::Borderless(None)` takes the window's current
    /// monitor — no mode change, so it's instant and plays nice with the compositor (the right
    /// default on the dev box's Wayland session). Available on any screen via F11; cursor capture is
    /// orthogonal (`sync_cursor` reconciles it each frame regardless of window mode).
    fn toggle_fullscreen(&mut self) {
        let Some(surface) = self.surface.as_ref() else {
            return;
        };
        self.fullscreen = !self.fullscreen;
        let mode = self.fullscreen.then(|| Fullscreen::Borderless(None));
        surface.window().set_fullscreen(mode);
    }

    /// Create the `Game` and switch to the in-match screen, fielding the persisted gunsmith loadout
    /// ([`App::loadout`]). If a campaign launch was queued ([`App::pending_launch`], set by a
    /// briefing's DEPLOY) it boots the *selected* node's mission scene (resolved via the shared
    /// `resolve_node` + [`Scene::for_mission`] seams — Seize → `Mission1`, Hold → `Mission2`) and applies the player's
    /// chosen replay tier's combat tuning via the shared [`Game::apply_campaign_tuning`] seam (D83:
    /// the 4→3 enemy-commander band + the scenario situation modifiers); otherwise it fields
    /// [`App::scene`] (the CLI default, or the scene a mode-select pick chose). The chosen tier is
    /// also carried in [`App::active_mission`] for clear-recording.
    ///
    /// Shared by the [`EnterMatch`](HostTransition::EnterMatch) and
    /// [`LaunchMission`](HostTransition::LaunchMission) transitions: under D81 both deploy **directly**
    /// (no gunsmith intermediate — the gunsmith is customization-only behind Settings now). The
    /// deterministic sim is untouched: the loadout only reaches the sim through the scenario seeder at
    /// match start (WS-C, D60), exactly as before.
    fn enter_match(&mut self) {
        let surface = self.surface.as_ref().expect("surface exists in resumed");
        let device = surface.device();
        let format = surface.format();
        let loadout = self.loadout.current();
        let mut game = if let Some((node, difficulty)) = self.pending_launch.take() {
            // Resolve the launched node's mission → its scene (Seize → `Mission1`, Hold →
            // `Mission2`) through the shared engine seams (`resolve_node` + `Scene::for_mission`,
            // never a per-platform fork, invariant #2), so the *selected* node boots its own scene
            // rather than a hardcoded Mission1. A node that doesn't resolve (defensively
            // unplayable/unregistered) tunes nothing and falls back to Mission1.
            let mission = self.registry.resolve_node(&self.campaign, node).map(|def| def.id);
            let scene = mission.and_then(Scene::for_mission).unwrap_or(Scene::Mission1);
            let mut game =
                Game::new_scene_with_loadout(device, format, DEFAULT_SEED, scene, loadout);
            // D83 (resolves Q21): the player's chosen replay tier drives the fight on both axes —
            // the 4→3 enemy-commander band and the scenario situation modifiers — through the shared
            // `core::campaign` mapping (never a per-platform fork, invariant #2). Applied before tick
            // 0, so it is deterministic match-setup like `select_army`. Guarded on `resolve_node` so a
            // (defensive) unplayable/unregistered node tunes nothing.
            if mission.is_some() {
                game.apply_campaign_tuning(difficulty);
            }
            self.active_mission = Some((node, difficulty));
            game
        } else {
            // Field the player's chosen gunsmith loadout at match start (WS-C, D60). For scenes that
            // carry no player loadout it is inert; `Loadout::STANDARD` (the untouched editor)
            // reproduces `new_scene` exactly.
            self.active_mission = None;
            Game::new_scene_with_loadout(device, format, DEFAULT_SEED, self.scene, loadout)
        };
        // Field the player's picked army at match setup. `Game::select_army` routes it through the
        // `core::shell` SelectArmy seam (the same lockstep-ordered command a peer would apply) and
        // records it on the sim before tick 0 — match-setup config, checksum-neutral (invariant #7),
        // so `gunsmith::pool_for` / `economy::unit_stats_for` field the chosen roster (WS-B). The
        // enemy side keeps its scene/mission-seeded army (not this host pick).
        game.select_army(Faction::Player, self.army_select.selected);
        self.mission_recorded = false;
        self.screen = Screen::InMatch(Box::new(game));
        // Don't charge the time spent on the out-of-match screens to the first sim tick.
        self.last_frame = Instant::now();
    }

    /// Desktop-host-only keys that apply on **every** screen (title or match): the
    /// [`GameAction::ToggleFullscreen`] binding (default **F11**) toggles borderless fullscreen. Like
    /// the cursor keys, these are not in the sim keymap, so handling them on the host leaves the
    /// deterministic input frame untouched. The physical key is resolved through the live rebind map
    /// (D75 follow-up) rather than a hardcoded `KeyCode`, so the player can rebind it.
    fn handle_global_keys(&mut self, event: &WindowEvent) {
        if let WindowEvent::KeyboardInput { event: key, .. } = event {
            if key.state == ElementState::Pressed && !key.repeat {
                if let Some(GameAction::ToggleFullscreen) = self.action_for_key(key.physical_key) {
                    self.toggle_fullscreen();
                }
            }
        }
    }

    /// Resolve a winit physical key to the rebindable [`GameAction`] it currently fires, if any — the
    /// desktop **app boundary** for the rebind editor. The engine `keybind` seam is winit-free
    /// (invariant #2), so the `winit::KeyCode` → [`KeyId`] conversion ([`keycode_to_keyid`]) lives
    /// here; the live [`KeyId`] → action lookup is the pure `KeybindMap::action_for` on
    /// `self.settings.keybinds`. Returns `None` for a non-`Code` key or one bound to nothing.
    fn action_for_key(&self, physical_key: PhysicalKey) -> Option<GameAction> {
        let PhysicalKey::Code(code) = physical_key else {
            return None;
        };
        keycode_to_keyid(code).and_then(|k| self.settings.keybinds.action_for(k))
    }

    /// Lock+hide the OS cursor while embodied so mouse motion drives the FPS look (raw device
    /// deltas) instead of the pointer drifting across on-screen items — and hand it back the moment
    /// the player surfaces, opens a shell overlay (pause / reconnect / summary), or holds Left-Alt.
    /// Idempotent: it only calls the winit setters when the desired state differs from the last one.
    ///
    /// `CursorGrabMode::Locked` (pointer pinned in place) is the Wayland/macOS path; X11 only
    /// supports `Confined`, so we fall back to it there. Either way the cursor is hidden and look
    /// reads from raw `DeviceEvent::MouseMotion`, so both behave the same for the player.
    fn sync_cursor(&mut self) {
        let Some(surface) = self.surface.as_ref() else {
            return;
        };
        let embodied = matches!(&self.screen, Screen::InMatch(game) if game.is_embodied());
        // A shell overlay (pause / reconnect / post-match summary) must free the cursor so the
        // player can click its buttons — even though they may have opened it while embodied.
        let overlay_up = matches!(&self.screen, Screen::InMatch(game) if game.shell_overlay_active());
        let cursor_free = self.alt_held || overlay_up;
        let want = want_cursor_capture(embodied, cursor_free);
        if want == self.cursor_captured {
            return;
        }
        let window = surface.window();
        if want {
            let _ = window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
            window.set_cursor_visible(false);
        } else {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
        }
        self.cursor_captured = want;
    }

    /// Desktop-host-only key handling for a running match: the pause/surrender entry + cursor
    /// controls. **Esc** toggles the in-session pause overlay (`Game::toggle_pause` — open the menu
    /// while playing, close it while paused; the menu's own **Surrender** button ends the match);
    /// **Left Alt** transiently frees the cursor while held; **F3** toggles the debug hitbox/facet
    /// overlay. None reach the sim (they're not in the keymap `DesktopInput` decodes) — pause is a
    /// host-side `SessionAction` and the overlay is presentation state, so neither enters the
    /// deterministic input frame and the checksum stream is untouched.
    fn handle_host_keys(&mut self, event: &WindowEvent) {
        if let WindowEvent::KeyboardInput { event: key, .. } = event {
            let pressed = key.state == ElementState::Pressed;
            // **Left Alt** (free the cursor) stays a hardcoded held-modifier gesture — it's a hold,
            // not a discrete rebindable trigger, so it's deliberately absent from the keybind map.
            if key.physical_key == PhysicalKey::Code(KeyCode::AltLeft) {
                self.alt_held = pressed;
                return;
            }
            // The rebindable in-match host actions (pause, debug overlay) route through the live
            // keybind map (D75 follow-up) instead of hardcoded `KeyCode`s. Press-once (no autorepeat);
            // fullscreen is handled globally on every screen (`handle_global_keys`), not here. None of
            // these reach the sim (they're not in the `DesktopInput` keymap) — the checksum stream is
            // untouched.
            if pressed && !key.repeat {
                if let Some(action) = self.action_for_key(key.physical_key) {
                    if let Screen::InMatch(game) = &mut self.screen {
                        match action {
                            GameAction::Pause => game.toggle_pause(),
                            GameAction::ToggleDebugOverlay => game.toggle_debug_hitboxes(),
                            // Fullscreen is a global (every-screen) key, handled in handle_global_keys.
                            GameAction::ToggleFullscreen => {}
                        }
                    }
                }
            }
        }
    }

    /// One presented frame. On `Title` the egui shell draws and may return a host transition
    /// (start a match / open settings / quit); on `InMatch` the shared engine loop runs as before.
    fn render_frame(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        let Some(surface) = self.surface.as_mut() else {
            return;
        };

        // Run the current screen; defer any state transition until the screen borrow is released.
        let mut transition = None;
        match &mut self.screen {
            Screen::Title => {
                if let Some(sh) = self.shell.as_mut() {
                    if let Some(action) = sh.draw_title(surface) {
                        transition = Some(resolve_title_action(action));
                    }
                }
            }
            Screen::Loadout => {
                if let Some(sh) = self.shell.as_mut() {
                    if let Some(action) = sh.draw_loadout(surface, &self.loadout) {
                        // D81: customization-only. Edits mutate the editor in place (Stay); DONE
                        // returns to Settings (the gunsmith's entry point) with the edits persisted.
                        // There is no Deploy here — matches start from the mode/mission-select gates.
                        transition = match apply_loadout_action(action, &mut self.loadout) {
                            LoadoutStep::Stay => None,
                            LoadoutStep::Done => Some(HostTransition::OpenSettings),
                        };
                    }
                }
            }
            Screen::Settings => {
                if let Some(sh) = self.shell.as_mut() {
                    // The video checkbox reflects the host's live window mode; pref edits mutate
                    // `self.settings` in place (the Stay case).
                    if let Some(action) = sh.draw_settings(surface, &mut self.settings, self.fullscreen)
                    {
                        transition = match apply_settings_action(action, &mut self.settings) {
                            SettingsStep::Stay => None,
                            SettingsStep::ToggleFullscreen => Some(HostTransition::ToggleFullscreen),
                            SettingsStep::OpenLoadout => Some(HostTransition::OpenLoadout),
                            SettingsStep::About => {
                                Some(HostTransition::OpenAbout(AboutReturn::Settings))
                            }
                            SettingsStep::Back => Some(HostTransition::ExitToTitle),
                        };
                    }
                }
            }
            Screen::Profile => {
                if let Some(sh) = self.shell.as_mut() {
                    if let Some(action) = sh.draw_profile(surface, &mut self.profile) {
                        transition = match apply_profile_action(action, &mut self.profile) {
                            ProfileStep::Stay => None,
                            ProfileStep::Back => Some(HostTransition::ExitToTitle),
                        };
                    }
                }
            }
            Screen::ArmySelect => {
                if let Some(sh) = self.shell.as_mut() {
                    if let Some(action) = sh.draw_army_select(surface, &self.army_select) {
                        // Choosing an army edits the pick in place (Stay); CONFIRM returns to the
                        // title with the pick persisted (the persist gate below fires on the way out).
                        transition = match apply_army_select_action(action, &mut self.army_select) {
                            ArmySelectStep::Stay => None,
                            ArmySelectStep::Confirm => Some(HostTransition::ExitToTitle),
                        };
                    }
                }
            }
            Screen::ModeSelect => {
                if let Some(sh) = self.shell.as_mut() {
                    if let Some(action) = sh.draw_mode_select(surface) {
                        transition = match action {
                            // Pick a mode → resolve its scene (the engine-tested `GameMode::scene`
                            // seam) and deploy straight in with the persisted loadout (D81 — no
                            // gunsmith gate). An un-parseable token (forbidden by the shell_modes
                            // test) defensively keeps the current scene.
                            ModeSelectAction::Pick(mode) => {
                                if let Some(scene) = mode.scene() {
                                    self.scene = scene;
                                }
                                // A mode-select deploy is never a campaign launch.
                                self.pending_launch = None;
                                Some(HostTransition::EnterMatch)
                            }
                            ModeSelectAction::Back => Some(HostTransition::ExitToTitle),
                        };
                    }
                }
            }
            Screen::MissionSelect => {
                if let Some(sh) = self.shell.as_mut() {
                    if let Some(action) = sh.draw_mission_select(surface, &self.campaign) {
                        transition = match action {
                            // A playable tile → open that node's briefing (the click was already
                            // gated to playable nodes by the pure `playable_node` seam).
                            MissionSelectAction::OpenNode(node) => {
                                Some(HostTransition::OpenBriefing(node))
                            }
                            MissionSelectAction::Back => Some(HostTransition::ExitToTitle),
                        };
                    }
                }
            }
            Screen::Briefing(node) => {
                let node = *node;
                if let Some(sh) = self.shell.as_mut() {
                    if let Some(action) =
                        sh.draw_briefing(surface, &self.campaign, node, self.briefing_difficulty)
                    {
                        // The difficulty cycler edits the live selection in place (Stay); Deploy
                        // queues the launch through the gunsmith; Back returns to the hub.
                        transition = match apply_briefing_action(action, &mut self.briefing_difficulty)
                        {
                            BriefingOutcome::Stay => None,
                            BriefingOutcome::Launch { difficulty } => {
                                Some(HostTransition::LaunchMission { node, difficulty })
                            }
                            BriefingOutcome::Back => Some(HostTransition::OpenMissionSelect),
                        };
                    }
                }
            }
            Screen::About(ret) => {
                let ret = *ret;
                if let Some(sh) = self.shell.as_mut() {
                    // BACK from About returns to whichever entry point opened it (title or Settings).
                    if sh.draw_about(surface) {
                        transition = Some(match ret {
                            AboutReturn::Title => HostTransition::ExitToTitle,
                            AboutReturn::Settings => HostTransition::OpenSettings,
                        });
                    }
                }
            }
            Screen::InMatch(game) => {
                // Field the player's Settings prefs before this frame consumes input/audio: look
                // sensitivity + invert-Y shape the drained look axis (so it must precede the drain),
                // and the master/SFX volumes scale every cue queued during `game.frame`. Both are
                // host-side presentation only — neither reaches the deterministic sim.
                self.input
                    .set_look_prefs(self.settings.mouse_sensitivity, self.settings.invert_look_y);
                self.audio
                    .set_gains(self.settings.master_volume, self.settings.sfx_volume);
                // Music volume (D75 follow-up): compose the effective music-bus gain via the engine
                // seam and push it to the sink's looping music bed, exactly as `set_gains` carries
                // master/SFX. Presentation only — never the deterministic sim.
                self.music_gain = gonedark_engine::music_gain(
                    1.0,
                    self.settings.master_volume,
                    self.settings.music_volume,
                );
                self.audio.set_music_gain(self.music_gain);
                // Graphics tier (Phase 4 WS-C): the Settings quality choice drives `render::tiers`
                // through `Game::set_tier`. `Auto` resolves to the desktop device-default tier
                // (High — the D22 flagship class). RENDER-only (invariant #1/#4): the sim ticks the
                // same fixed 60 Hz at every tier, so the per-tick checksum stream is byte-identical.
                // Only switch on a real change so the running dyn-res scale isn't re-clamped every frame.
                let want_tier = self.settings.quality.to_tier(DESKTOP_DEFAULT_TIER);
                if game.tier() != want_tier {
                    game.set_tier(want_tier);
                }
                // Accessibility cues (invariant #6 fairness): the CVD alert-HUD labels, the
                // hard-of-hearing visual sound echoes, and the colourblind-safe faction palette (WS-D)
                // are host-side presentation chrome; push the stored settings into the engine before it
                // draws this frame. None reaches the deterministic sim.
                game.set_accessibility_prefs(
                    self.settings.colorblind_cues,
                    self.settings.visual_sound_cues,
                    self.settings.cvd_palette,
                );
                // The cross-modal alert cues (WS-D): the audio-ping / haptic equivalents of the
                // directional flash. A separate setter (its own setting) so the 3-arg
                // `set_accessibility_prefs` signature the Android host shares stays stable.
                game.set_alert_cue_mode(self.settings.alert_cue_mode);
                let mut input = self.input.drain_frame();
                let viewport = surface.size();
                // Shell overlay buttons (pause / reconnect / post-match summary). A click while an
                // overlay is up belongs to that overlay, not the match world: hit-test it in NDC and
                // either feed the resolved session action back to the shell or — for the terminal
                // summary's DISMISS — leave the match. When the click is consumed here we strip it
                // from `input` so the same release doesn't also drive a world selection underneath.
                if input.pointer_up {
                    if let Some((px, py)) = input.pointer {
                        let (w, h) = viewport;
                        // Shared pixel→NDC seam (engine; unit-tested) — Android runs the same one, so
                        // the leave-to-title hit-test can't diverge across platforms (invariant #2).
                        let ndc = pixel_to_ndc(px, py, w, h);
                        match game.overlay_click(ndc) {
                            Some(OverlayClick::Session(action)) => {
                                game.apply_session_action(action);
                                input.pointer_up = false;
                                input.pointer_down = false;
                            }
                            Some(OverlayClick::Dismiss) => {
                                transition = Some(HostTransition::ExitToTitle);
                            }
                            None => {}
                        }
                    }
                }
                // While a shell overlay is up the match is frozen underneath it: drop the rest of
                // this frame's input so a click that missed an overlay button (or a held key) can't
                // drive selection / fire the weapon / pan the camera behind the menu. The overlay's
                // own buttons were already resolved above, before this blanking. (Single-player also
                // halts the tick via `halts_local_tick`; this stops *world input*, not the clock.)
                if game.shell_overlay_active() {
                    input = Default::default();
                }
                // Skip the match frame entirely when we're leaving it this turn; the title screen
                // draws next frame.
                if transition.is_none() {
                    if let Some((frame, view)) = surface.acquire() {
                        game.frame(
                            &input,
                            dt,
                            viewport,
                            surface.device(),
                            surface.queue(),
                            &view,
                            &mut self.audio,
                            &self.thermal,
                        );
                        surface.present(frame);
                    }
                }
                // Record a campaign clear the first frame this match reads `Won`. The clear advances
                // host-side meta-progression and persists to its own blob — it is **never** sim state
                // and never folded into the per-tick checksum (invariants #1/#7), so it cannot
                // desync. `mission_status()` is a pure read of the host-side objective layer; a loss
                // (or exit) records nothing. `self.campaign` / `self.active_mission` are disjoint
                // fields from the `game` borrow above, so this split borrow is fine.
                if !self.mission_recorded {
                    if let Some((node, difficulty)) = self.active_mission {
                        if game.mission_status() == MissionStatus::Won {
                            // The campaign tier the player chose is what the clear records — the same
                            // tier that drove the fight's commander band + situation modifiers this
                            // match (D83). A rejected clear (shouldn't happen for a launched, playable
                            // node) is simply not persisted.
                            if self.campaign.clear(node, difficulty).is_ok() {
                                persist_campaign(&self.campaign);
                            }
                            self.mission_recorded = true;
                        }
                    }
                }
            }
        }

        // Persist the shell prefs whenever we leave a screen that edits them (Settings / Profile /
        // the gunsmith), so an edit survives a restart — the same best-effort lifecycle `campaign.dat`
        // rides. `self.screen` is still the screen we're leaving here (the swap happens below). This
        // captures the final in-place-edited state (sliders / callsign / slot cyclers) on the way out.
        if transition.is_some()
            && matches!(
                self.screen,
                Screen::Settings | Screen::Profile | Screen::Loadout | Screen::ArmySelect
            )
        {
            persist_shell_prefs(&self.settings, &self.profile, &self.loadout, &self.army_select);
        }

        match transition {
            // Settings → the gunsmith (D81: customization-only, reached from Settings). The editor
            // (App::loadout) is already populated; the screen edits it in place until DONE returns to
            // Settings with the edits persisted.
            Some(HostTransition::OpenLoadout) => {
                self.screen = Screen::Loadout;
                self.last_frame = Instant::now();
            }
            // PvE/PvP → the mode/map select (D81). Picking a mode there deploys straight into the
            // match with the persisted loadout (no gunsmith gate).
            Some(HostTransition::OpenModeSelect) => {
                self.screen = Screen::ModeSelect;
                self.last_frame = Instant::now();
            }
            // CAMPAIGN → the Operations-hub mission-select (PvE pillar, D58).
            Some(HostTransition::OpenMissionSelect) => {
                self.screen = Screen::MissionSelect;
                self.last_frame = Instant::now();
            }
            // A hub tile → that node's briefing. Reset the replay-tier selector to the lowest tier
            // the node offers (its briefing lists `available_difficulties` lowest-first); fall back to
            // the default for a (defensive) out-of-range node.
            Some(HostTransition::OpenBriefing(node)) => {
                self.briefing_difficulty = self
                    .campaign
                    .briefing(node)
                    .and_then(|b| b.available_difficulties.first().copied())
                    .unwrap_or_default();
                self.screen = Screen::Briefing(node);
                self.last_frame = Instant::now();
            }
            // A briefing's DEPLOY (D81): queue the campaign launch and deploy **directly** into the
            // mission — the gunsmith is no longer an intermediate step (it's customization-only behind
            // Settings). `enter_match` consumes `pending_launch` to boot the mission scene and
            // remember the node for clear-recording, fielding the persisted loadout.
            Some(HostTransition::LaunchMission { node, difficulty }) => {
                self.pending_launch = Some((node, difficulty));
                self.enter_match();
            }
            Some(HostTransition::EnterMatch) => self.enter_match(),
            // Out-of-match utility screens — drawn over the same 3D backdrop as the title.
            Some(HostTransition::OpenSettings) => {
                self.screen = Screen::Settings;
                self.last_frame = Instant::now();
            }
            Some(HostTransition::OpenProfile) => {
                self.screen = Screen::Profile;
                self.last_frame = Instant::now();
            }
            Some(HostTransition::OpenArmySelect) => {
                self.screen = Screen::ArmySelect;
                self.last_frame = Instant::now();
            }
            Some(HostTransition::OpenAbout(ret)) => {
                self.screen = Screen::About(ret);
                self.last_frame = Instant::now();
            }
            // The Settings video toggle: flip the window mode, stay on the current screen.
            Some(HostTransition::ToggleFullscreen) => self.toggle_fullscreen(),
            Some(HostTransition::Exit) => event_loop.exit(),
            // Return to the title screen, dropping any `Game` (the post-match DISMISS path, and the
            // gunsmith's BACK — which has no `Game` yet, so this is just a screen swap there). Clear
            // any campaign launch/active mission so a later non-campaign Start can't inherit a stale
            // node (a gunsmith BACK abandons a queued campaign launch; a match exit ends the active
            // one — its clear, if any, was already recorded the frame the match was Won).
            Some(HostTransition::ExitToTitle) => {
                self.pending_launch = None;
                self.active_mission = None;
                self.mission_recorded = false;
                self.screen = Screen::Title;
                self.last_frame = Instant::now();
            }
            None => {}
        }

        // Reconcile cursor capture against the (possibly just-changed) embodiment/screen state, so a
        // surface, death-eject, or match-exit hands the pointer back within the same frame.
        self.sync_cursor();
    }
}

/// Whether the desktop host should lock+hide the OS cursor this frame. True only while **embodied**
/// and the player has NOT requested a free cursor (Esc toggle or held Left-Alt). In the command
/// (RTS) view the cursor is always free — it *is* the pointer; capture is purely the embodied-FPS
/// concern. Pure (no winit types) so it is unit-tested without a real `Window`.
fn want_cursor_capture(embodied: bool, cursor_free: bool) -> bool {
    embodied && !cursor_free
}

/// Map a winit [`KeyCode`] to the engine's platform-neutral [`KeyId`], or `None` for a key outside
/// the rebind vocabulary. The desktop **app boundary** for the rebind editor (D75 follow-up): the
/// engine `keybind` seam is winit-free (invariant #2), so this `winit::KeyCode` → [`KeyId`] mapping
/// lives here (its egui twin, `shell::egui_key_to_keyid`, handles the capture side). Pure (a total
/// match over plain enums) — unit-tested without a window.
fn keycode_to_keyid(code: KeyCode) -> Option<KeyId> {
    Some(match code {
        KeyCode::F1 => KeyId::F1,
        KeyCode::F2 => KeyId::F2,
        KeyCode::F3 => KeyId::F3,
        KeyCode::F4 => KeyId::F4,
        KeyCode::F5 => KeyId::F5,
        KeyCode::F6 => KeyId::F6,
        KeyCode::F7 => KeyId::F7,
        KeyCode::F8 => KeyId::F8,
        KeyCode::F9 => KeyId::F9,
        KeyCode::F10 => KeyId::F10,
        KeyCode::F11 => KeyId::F11,
        KeyCode::F12 => KeyId::F12,
        KeyCode::KeyA => KeyId::A,
        KeyCode::KeyB => KeyId::B,
        KeyCode::KeyC => KeyId::C,
        KeyCode::KeyD => KeyId::D,
        KeyCode::KeyE => KeyId::E,
        KeyCode::KeyF => KeyId::F,
        KeyCode::KeyG => KeyId::G,
        KeyCode::KeyH => KeyId::H,
        KeyCode::KeyI => KeyId::I,
        KeyCode::KeyJ => KeyId::J,
        KeyCode::KeyK => KeyId::K,
        KeyCode::KeyL => KeyId::L,
        KeyCode::KeyM => KeyId::M,
        KeyCode::KeyN => KeyId::N,
        KeyCode::KeyO => KeyId::O,
        KeyCode::KeyP => KeyId::P,
        KeyCode::KeyQ => KeyId::Q,
        KeyCode::KeyR => KeyId::R,
        KeyCode::KeyS => KeyId::S,
        KeyCode::KeyT => KeyId::T,
        KeyCode::KeyU => KeyId::U,
        KeyCode::KeyV => KeyId::V,
        KeyCode::KeyW => KeyId::W,
        KeyCode::KeyX => KeyId::X,
        KeyCode::KeyY => KeyId::Y,
        KeyCode::KeyZ => KeyId::Z,
        KeyCode::Digit0 => KeyId::Digit0,
        KeyCode::Digit1 => KeyId::Digit1,
        KeyCode::Digit2 => KeyId::Digit2,
        KeyCode::Digit3 => KeyId::Digit3,
        KeyCode::Digit4 => KeyId::Digit4,
        KeyCode::Digit5 => KeyId::Digit5,
        KeyCode::Digit6 => KeyId::Digit6,
        KeyCode::Digit7 => KeyId::Digit7,
        KeyCode::Digit8 => KeyId::Digit8,
        KeyCode::Digit9 => KeyId::Digit9,
        KeyCode::Escape => KeyId::Escape,
        KeyCode::Tab => KeyId::Tab,
        KeyCode::Space => KeyId::Space,
        KeyCode::Enter => KeyId::Enter,
        KeyCode::Backspace => KeyId::Backspace,
        KeyCode::Insert => KeyId::Insert,
        KeyCode::Delete => KeyId::Delete,
        KeyCode::Home => KeyId::Home,
        KeyCode::End => KeyId::End,
        KeyCode::PageUp => KeyId::PageUp,
        KeyCode::PageDown => KeyId::PageDown,
        KeyCode::ArrowUp => KeyId::Up,
        KeyCode::ArrowDown => KeyId::Down,
        KeyCode::ArrowLeft => KeyId::Left,
        KeyCode::ArrowRight => KeyId::Right,
        KeyCode::Minus => KeyId::Minus,
        KeyCode::Equal => KeyId::Equals,
        KeyCode::Backquote => KeyId::Backquote,
        // Any other physical key (modifiers, punctuation, media keys, …) is not bindable here.
        _ => return None,
    })
}

impl ApplicationHandler for App {
    /// Create the window + GPU surface + the egui shell once the event loop is ready (`Game` is
    /// created later, on Start). On desktop `resumed` fires once at startup; guard a redundant
    /// second create.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.surface.is_some() {
            return;
        }
        let window = event_loop
            .create_window(WindowAttributes::default().with_title("Going Dark"))
            .expect("create winit window");
        let window: Arc<Window> = Arc::new(window);

        let surface = DesktopRenderSurface::new(window);
        let stamp = build_stamp(build_channel(cfg!(debug_assertions)), env!("CARGO_PKG_VERSION"));
        let shell = EguiShell::new(surface.device(), surface.format(), surface.window(), stamp);

        self.surface = Some(surface);
        self.shell = Some(shell);
        // Reset the clock so window-creation latency isn't charged to the first frame.
        self.last_frame = Instant::now();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Global host keys (F11 fullscreen) apply on every screen, before screen-specific routing.
        self.handle_global_keys(&event);

        // Route input by screen: the egui shell gets UI events on the title screen; the game input
        // accumulator gets them in a match. (A stray event in the other state is simply ignored, so
        // nothing leaks between the shell and the sim.)
        match self.screen {
            // The egui shell owns input on every out-of-match screen (title, gunsmith, settings,
            // profile, army-select, mode-select, mission-select, briefing, about).
            Screen::Title
            | Screen::Loadout
            | Screen::Settings
            | Screen::Profile
            | Screen::ArmySelect
            | Screen::ModeSelect
            | Screen::MissionSelect
            | Screen::Briefing(_)
            | Screen::About(_) => {
                if let (Some(sh), Some(surface)) = (self.shell.as_mut(), self.surface.as_ref()) {
                    sh.on_window_event(surface.window(), &event);
                }
            }
            Screen::InMatch(_) => {
                // Host-level keys (Esc → pause toggle, Left-Alt → free cursor) are consumed here,
                // then the event also feeds the sim input accumulator (they're not in its keymap, so
                // no double-use).
                self.handle_host_keys(&event);
                self.input.handle_window_event(&event);
            }
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(surface) = self.surface.as_mut() {
                    surface.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => self.render_frame(event_loop),
            _ => {}
        }
    }

    /// Raw mouse-look (the FPS look axis) arrives as device events — only meaningful in a match.
    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        // Only feed raw mouse-look while the cursor is actually captured: when the player has freed
        // the pointer for UI (Esc / Alt) or is in the command view, moving the mouse must NOT turn
        // the camera.
        if self.cursor_captured {
            self.input.handle_device_event(&event);
        }
    }

    /// Keep a continuous render loop: request another redraw as soon as the queue drains. (The egui
    /// title screen also needs steady repaints for hover/click feedback.)
    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(surface) = self.surface.as_ref() {
            surface.window().request_redraw();
        }
    }
}

/// Extract the `--scene <name>` / `--scene=<name>` launch token from CLI args, if present. Pure
/// (no env / no `Scene::parse`), so it's host-tested without a window; `main` resolves the token to
/// a [`Scene`] and warns on an unknown name.
fn scene_token(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(name) = a.strip_prefix("--scene=") {
            return Some(name.to_string());
        }
        if a == "--scene" {
            return it.next().cloned();
        }
    }
    None
}

/// The campaign-progress blob filename, under the host data dir (see [`campaign_dir`]).
const CAMPAIGN_PROGRESS_FILE: &str = "campaign.dat";

/// Resolve the host directory the campaign-progress blob lives in, from the two env vars the real
/// [`campaign_progress_path`] reads: prefer `$XDG_DATA_HOME/gonedark`, else `$HOME/.local/share/
/// gonedark`, else `None` (no writable home → progress simply isn't persisted this session). Pure
/// (no env / no fs), so it is unit-tested without touching the real filesystem — the env read +
/// `fs` calls around it are the exempt host glue. There is no pre-existing settings/profile
/// persistence in this binary to mirror, so this picks the conventional XDG data location.
fn campaign_dir(xdg_data_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    if let Some(x) = xdg_data_home.filter(|s| !s.is_empty()) {
        return Some(Path::new(x).join("gonedark"));
    }
    if let Some(h) = home.filter(|s| !s.is_empty()) {
        return Some(Path::new(h).join(".local/share/gonedark"));
    }
    None
}

/// The full path to the campaign-progress blob, or `None` when no writable home dir is known. Reads
/// the environment (the impure part) and defers the decision to the pure [`campaign_dir`] seam.
fn campaign_progress_path() -> Option<PathBuf> {
    let xdg = std::env::var("XDG_DATA_HOME").ok();
    let home = std::env::var("HOME").ok();
    campaign_dir(xdg.as_deref(), home.as_deref()).map(|d| d.join(CAMPAIGN_PROGRESS_FILE))
}

/// Persist the campaign's progress to its **own host blob** (`Campaign::serialize_progress`),
/// creating the data dir if needed. Best-effort: a write failure is logged-by-silence (we never
/// crash the match on a save error) — the progress is in memory regardless. This blob is **separate**
/// from any sim snapshot and never folded into the per-tick checksum (invariants #1/#7). Glue (fs).
fn persist_campaign(campaign: &Campaign) {
    let Some(path) = campaign_progress_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, campaign.serialize_progress());
}

/// Load persisted campaign progress over `campaign`'s fresh topology, if a blob exists. A missing
/// file or a malformed/topology-skewed blob is ignored (the load is all-or-nothing — `apply_progress`
/// never partially applies), leaving the campaign at its start. Glue (fs).
fn load_campaign_progress(campaign: &mut Campaign) {
    let Some(path) = campaign_progress_path() else {
        return;
    };
    if let Ok(bytes) = std::fs::read(&path) {
        let _ = campaign.apply_progress(&bytes);
    }
}

/// The shell-prefs blob filename (Settings / Profile / gunsmith loadout), a sibling of
/// [`CAMPAIGN_PROGRESS_FILE`] under the same host data dir ([`campaign_dir`]). Separate from
/// campaign progress so a corrupt one can't take down the other.
const SHELL_PREFS_FILE: &str = "shell.dat";

/// The full path to the shell-prefs blob, or `None` when no writable home dir is known. Reuses the
/// pure [`campaign_dir`] seam the campaign blob uses (same env resolution), so both ride one location.
fn shell_prefs_path() -> Option<PathBuf> {
    let xdg = std::env::var("XDG_DATA_HOME").ok();
    let home = std::env::var("HOME").ok();
    campaign_dir(xdg.as_deref(), home.as_deref()).map(|d| d.join(SHELL_PREFS_FILE))
}

/// Load persisted Settings / Profile / gunsmith loadout, decoding through the pure
/// [`shell::decode_shell_prefs`] seam. A missing file or a malformed blob falls back to the shipped
/// defaults (decode is total — it never fails), so a fresh device or a corrupt save just starts
/// clean. **Host-side presentation state only** — never sim state, never checksummed (invariant #1).
/// Glue (fs); the decode logic + its tolerance are unit-tested in `shell`.
fn load_shell_prefs() -> (SettingsState, ProfileState, LoadoutEditor, ArmySelectState) {
    let defaults = || {
        (
            SettingsState::default(),
            ProfileState::default(),
            LoadoutEditor::new(),
            ArmySelectState::default(),
        )
    };
    let Some(path) = shell_prefs_path() else {
        return defaults();
    };
    match std::fs::read_to_string(&path) {
        Ok(blob) => shell::decode_shell_prefs(&blob),
        Err(_) => defaults(),
    }
}

/// Persist the current Settings / Profile / gunsmith loadout via the pure
/// [`shell::encode_shell_prefs`] seam, creating the data dir if needed. Best-effort (a write failure
/// is swallowed — the state is in memory regardless), and on the same lifecycle as
/// [`persist_campaign`]. Presentation state only, never a sim/checksum surface. Glue (fs).
fn persist_shell_prefs(
    settings: &SettingsState,
    profile: &ProfileState,
    loadout: &LoadoutEditor,
    army: &ArmySelectState,
) {
    let Some(path) = shell_prefs_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, shell::encode_shell_prefs(settings, profile, loadout, army));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scene = match scene_token(&args) {
        Some(tok) => Scene::parse(&tok).unwrap_or_else(|| {
            eprintln!(
                "unknown --scene {tok:?}; using the skirmish \
                 (known scenes: skirmish, default, duel, infantry)"
            );
            Scene::Skirmish
        }),
        // No `--scene`: boot the playable two-base skirmish (the real match), not the canned demo.
        None => Scene::Skirmish,
    };

    let event_loop = EventLoop::new().expect("create winit event loop");
    let mut app = App::new(scene);
    event_loop.run_app(&mut app).expect("run winit app");
}

#[cfg(test)]
mod cursor_tests {
    //! The cursor-capture *decision* is the only logic worth testing here; the winit grab/visibility
    //! calls and the key/screen plumbing around it are thin, un-constructible glue (`Window`,
    //! `KeyEvent`, `ActiveEventLoop` have no public test constructors), so they're exercised by
    //! running the app, not unit tests — matching this crate's existing testable-seam convention.
    use super::want_cursor_capture;

    #[test]
    fn captures_only_while_embodied_and_not_freed() {
        // Embodied with the cursor not freed → lock+hide it (the FPS look path).
        assert!(want_cursor_capture(true, false));
        // Embodied but the player asked for the cursor (Esc toggle or held Alt) → hand it back.
        assert!(!want_cursor_capture(true, true));
    }

    #[test]
    fn never_captures_in_command_view() {
        // Not embodied (RTS command view, or title) → the cursor is always the free pointer,
        // regardless of the free-cursor request.
        assert!(!want_cursor_capture(false, false));
        assert!(!want_cursor_capture(false, true));
    }
}

#[cfg(test)]
mod keybind_boundary_tests {
    //! The winit→engine key mapping is the only rebind logic in this binary worth testing; the
    //! `KeyboardInput` event plumbing and `Game` toggles it drives are un-constructible glue
    //! (winit `KeyEvent` has no public test constructor), exercised by running the app. The pure
    //! rebind/conflict/persistence model is tested in `gonedark_engine::keybind`, and the egui capture
    //! boundary in `shell`.
    use super::{keycode_to_keyid, GameAction, KeyId};
    use gonedark_engine::keybind::KeybindMap;
    use winit::keyboard::KeyCode;

    #[test]
    fn maps_the_default_binding_keys_and_a_sample_of_others() {
        assert_eq!(keycode_to_keyid(KeyCode::Escape), Some(KeyId::Escape));
        assert_eq!(keycode_to_keyid(KeyCode::F11), Some(KeyId::F11));
        assert_eq!(keycode_to_keyid(KeyCode::F3), Some(KeyId::F3));
        assert_eq!(keycode_to_keyid(KeyCode::KeyP), Some(KeyId::P));
        assert_eq!(keycode_to_keyid(KeyCode::Digit5), Some(KeyId::Digit5));
        assert_eq!(keycode_to_keyid(KeyCode::ArrowUp), Some(KeyId::Up));
        assert_eq!(keycode_to_keyid(KeyCode::Equal), Some(KeyId::Equals));
        // A non-bindable key (a modifier) resolves to nothing.
        assert_eq!(keycode_to_keyid(KeyCode::ShiftLeft), None);
    }

    #[test]
    fn default_map_routes_the_historical_desktop_keys_to_their_actions() {
        // End-to-end of the boundary: winit key → KeyId → action through the shipped default map.
        let map = KeybindMap::default();
        let action = |code| keycode_to_keyid(code).and_then(|k| map.action_for(k));
        assert_eq!(action(KeyCode::Escape), Some(GameAction::Pause));
        assert_eq!(action(KeyCode::F11), Some(GameAction::ToggleFullscreen));
        assert_eq!(action(KeyCode::F3), Some(GameAction::ToggleDebugOverlay));
        // An unbound key drives nothing (so a stray press is a no-op).
        assert_eq!(action(KeyCode::KeyJ), None);
    }
}

#[cfg(test)]
mod scene_arg_tests {
    use super::scene_token;

    fn args(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn extracts_scene_token_in_both_forms() {
        assert_eq!(scene_token(&args(&["--scene", "duel"])).as_deref(), Some("duel"));
        assert_eq!(scene_token(&args(&["--scene=duel"])).as_deref(), Some("duel"));
        // Other flags around it don't interfere.
        assert_eq!(
            scene_token(&args(&["--foo", "--scene", "default", "--bar"])).as_deref(),
            Some("default"),
        );
    }

    #[test]
    fn absent_or_dangling_scene_flag_is_none() {
        assert_eq!(scene_token(&args(&[])), None);
        assert_eq!(scene_token(&args(&["--fullscreen"])), None);
        // `--scene` with no following value: nothing to take.
        assert_eq!(scene_token(&args(&["--scene"])), None);
    }
}

#[cfg(test)]
mod campaign_path_tests {
    //! The campaign-progress *directory* resolution is the only logic worth testing here; the env
    //! read + `std::fs` calls that wrap it (`persist_campaign`/`load_campaign_progress`) are thin,
    //! filesystem-touching glue, exempt per the crate's testable-seam convention.
    use super::campaign_dir;
    use std::path::Path;

    #[test]
    fn prefers_xdg_data_home_when_set() {
        let dir = campaign_dir(Some("/data"), Some("/home/me")).unwrap();
        assert_eq!(dir, Path::new("/data/gonedark"));
    }

    #[test]
    fn falls_back_to_home_local_share() {
        // No XDG (absent or empty) → $HOME/.local/share/gonedark.
        assert_eq!(
            campaign_dir(None, Some("/home/me")).unwrap(),
            Path::new("/home/me/.local/share/gonedark")
        );
        assert_eq!(
            campaign_dir(Some(""), Some("/home/me")).unwrap(),
            Path::new("/home/me/.local/share/gonedark")
        );
    }

    #[test]
    fn no_writable_home_yields_none() {
        // Neither var usable → progress simply isn't persisted (None), never a panic.
        assert_eq!(campaign_dir(None, None), None);
        assert_eq!(campaign_dir(Some(""), Some("")), None);
    }
}
