//! The Settings screen — render-quality/audio/controls/accessibility/keybind preferences and their
//! pure decision seam. Presentation-only host prefs; nothing here reaches the deterministic sim.

use crate::shell::theme::*;
use crate::shell::widgets::*;
use gonedark_engine::keybind::{GameAction, KeyId, KeybindMap, RebindOutcome};
use gonedark_engine::AlertCueMode;
use gonedark_render::theme::PaletteMode;
use gonedark_render::tiers::QualityTier;

/// The render-quality preference exposed on the Settings screen. `Auto` lets the in-match tier
/// controller (`render::tiers`) pick from thermals; the explicit tiers pin it. A small cycler enum so
/// the screen needs no slider for a discrete choice. Pure data — no GPU.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum QualityChoice {
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
pub(crate) struct SettingsState {
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
    /// Embodied **hip field-of-view** in degrees (PC-1), [`Self::FOV_MIN`]`..=`[`Self::FOV_MAX`].
    /// A seated mouse player wants a wider field than the 60° that reads as tunnel-vision on a
    /// monitor; the host pushes this to `Game::set_base_fov` each match frame. Presentation only —
    /// the camera frustum, never the sim (invariants #4/#5), and only the player's OWN view (#6).
    pub fov_deg: f32,
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
    /// The desktop key-rebind map (D90 host toggles + the Q27 gameplay keymap): which physical key
    /// fires each rebindable action — the host toggles (pause / fullscreen / debug overlay) `main.rs`
    /// routes itself, and every gameplay key (move/embody/build/train/…) the host pushes into
    /// `pal-desktop`'s `DesktopInput` each match frame (`set_keybinds`). The pure model lives in
    /// `gonedark_pal::keybind` (platform-free, invariant #2; re-exported as `engine::keybind`);
    /// `pal-desktop` maps `winit::KeyCode` ↔ `KeyId` at the winit boundary. Persisted by stable
    /// ordinal alongside the other prefs. Presentation only — a keybind never reaches the sim
    /// (invariants #1/#4).
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
            // A seated-PC default — wider than the 60° hip FOV that reads as tunnel-vision on a
            // monitor (PC-1). Android leaves the engine's 60° default (its own settings surface).
            fov_deg: 90.0,
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
    /// FOV slider bounds (degrees) — mirror the engine's embodied-FOV band so the slider can never
    /// request a value `Game::set_base_fov` would reject, keeping the two clamps in lock-step.
    pub const FOV_MIN: f32 = gonedark_engine::EMBODIED_FOV_MIN_DEG;
    pub const FOV_MAX: f32 = gonedark_engine::EMBODIED_FOV_MAX_DEG;

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
        self.fov_deg = self.fov_deg.clamp(Self::FOV_MIN, Self::FOV_MAX);
    }

    /// Restore the shipped defaults — the Settings RESET button.
    pub fn reset(&mut self) {
        *self = SettingsState::default();
    }
}

/// An action the Settings screen can emit in a frame. Slider/checkbox edits mutate [`SettingsState`]
/// in place (no action — they're the "Stay" case); only these discrete controls are actions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SettingsAction {
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
pub(crate) enum SettingsStep {
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
/// Settings testable decision seam, mirroring `apply_loadout_action`.
pub(crate) fn apply_settings_action(
    action: SettingsAction,
    state: &mut SettingsState,
) -> SettingsStep {
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

/// Map an egui [`egui::Key`] to the engine's platform-neutral [`KeyId`], or `None` for a key the
/// rebind vocabulary doesn't cover. This is the Settings **app boundary** for the rebind editor: the
/// engine `keybind` seam is deliberately egui/winit-free (invariant #2), so the conversion of a real
/// key press into a bindable id lives here (its winit twin, `keycode_to_keyid`, lives in `main.rs`).
/// Pure (a total match over plain enums) — unit-tested below without a window.
pub(crate) fn egui_key_to_keyid(key: egui::Key) -> Option<KeyId> {
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
pub(crate) fn settings_ui(
    ui: &mut egui::Ui,
    state: &mut SettingsState,
    fullscreen: bool,
    rebinding: &mut Option<GameAction>,
    rebind_conflict: &mut Option<(GameAction, GameAction)>,
) -> Option<SettingsAction> {
    use egui::{RichText, Slider};
    let mut action = None;

    over_backdrop_screen_responsive(ui, "settings", |ui| {
        screen_banner(ui, "SETTINGS", 110.0);

        // On a wide desktop card AUDIO / CONTROLS / KEY BINDINGS take the left column and
        // ACCESSIBILITY / VIDEO / LOADOUT take the right; on the narrow mobile card they stack in
        // the original order. `two_col` threads `&mut state` (the one thing both columns edit — the
        // left column owns `rebinding`/`rebind_conflict`, the right owns `fullscreen`/the action).
        // Anchoring the body to a fixed-width column also fixes the old "sliders far-left, headings
        // centred" scatter: `over_backdrop_screen`'s bare centred column centred each row on its own
        // width. Banner above and footer below stay outside so they span the full card width.
        let (_, video_action) = two_col(
            ui,
            state,
            |ui, state| {
                section_label(ui, "AUDIO");
                egui::Grid::new("settings.audio")
                    .num_columns(2)
                    .min_col_width(SETTINGS_LABEL_W)
                    .spacing([16.0, 10.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Master").color(BONE).size(TYPE_BODY));
                        ui.add(Slider::new(&mut state.master_volume, 0.0..=1.0).fixed_decimals(2));
                        ui.end_row();
                        ui.label(RichText::new("SFX").color(BONE).size(TYPE_BODY));
                        ui.add(Slider::new(&mut state.sfx_volume, 0.0..=1.0).fixed_decimals(2));
                        ui.end_row();
                        ui.label(RichText::new("Music").color(BONE).size(TYPE_BODY));
                        ui.add(Slider::new(&mut state.music_volume, 0.0..=1.0).fixed_decimals(2));
                        ui.end_row();
                    });

                section_divider(ui);
                section_label(ui, "CONTROLS");
                // Embodied hip FOV (PC-1) — a seated mouse player expects a wider field than the 60° that
                // reads as tunnel-vision on a monitor. `clamp` re-bounds to the engine's embodied-FOV band;
                // `main.rs` pushes it to `Game::set_base_fov` each match frame (camera only, never the sim).
                egui::Grid::new("settings.controls")
                    .num_columns(2)
                    .min_col_width(SETTINGS_LABEL_W)
                    .spacing([16.0, 10.0])
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("Look sensitivity")
                                .color(BONE)
                                .size(TYPE_BODY),
                        );
                        ui.add(
                            Slider::new(
                                &mut state.mouse_sensitivity,
                                SettingsState::SENS_MIN..=SettingsState::SENS_MAX,
                            )
                            .fixed_decimals(2),
                        );
                        ui.end_row();
                        ui.label(RichText::new("Field of view").color(BONE).size(TYPE_BODY));
                        ui.add(
                            Slider::new(
                                &mut state.fov_deg,
                                SettingsState::FOV_MIN..=SettingsState::FOV_MAX,
                            )
                            .suffix("°")
                            .fixed_decimals(0),
                        );
                        ui.end_row();
                    });
                // A boolean toggle, not a label:value row — sits under the two sliders rather than inside
                // the grid.
                ui.checkbox(&mut state.invert_look_y, "Invert look Y");

                section_divider(ui);
                // The key-rebind editor (D90, widened by Q27). One row per rebindable action — the host
                // toggles (pause / fullscreen / debug overlay — the keys `main.rs` owns) AND the gameplay
                // keymap (move/embody/build/train/… — decoded by `pal-desktop` through this same map): its
                // label + a button showing the current binding. Clicking a button arms capture ("press a
                // key…"); the next mappable key press rebinds through the pure `KeybindMap::rebind`, which
                // rejects a key an overlapping-layer action already owns and reports the owner for conflict
                // feedback (a command-view and an embodied action MAY share a key — they are never live
                // together; the shipped R = train rifleman / reload). Direct-mutates `state.keybinds` (like
                // the sliders); persisted with the other prefs, read by `main.rs` each key event, and pushed
                // into `DesktopInput` each match frame.
                section_label(ui, "KEY BINDINGS");
                ui.label(
                RichText::new(
                    "Command-view and embodied actions may share one key (they are never active \
                 together). Esc cancels a capture.",
                )
                .color(BONE)
                .size(TYPE_CAPTION),
            );
                for act in GameAction::ALL {
                    let capturing = *rebinding == Some(act);
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [180.0, 28.0],
                            egui::Label::new(
                                RichText::new(act.label()).color(BONE).size(TYPE_BODY),
                            ),
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
                                egui::Button::new(
                                    RichText::new(btn_label).color(color).size(TYPE_BODY),
                                ),
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
                                key, pressed: true, ..
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
                                RebindOutcome::Conflict(owner) => {
                                    *rebind_conflict = Some((act, owner))
                                }
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
                // Sized to the full key/value row width so it reads as a footer action for the bindings
                // section rather than a stray shrink-wrapped button.
                if ui
                    .add_sized(
                        [SETTINGS_LABEL_W + 16.0 + 140.0, 28.0],
                        egui::Button::new(
                            RichText::new("Reset bindings").color(BONE).size(TYPE_BODY),
                        ),
                    )
                    .clicked()
                {
                    state.keybinds.reset();
                    *rebinding = None;
                    *rebind_conflict = None;
                }
            }, // end left column (AUDIO / CONTROLS / KEY BINDINGS)
            |ui, state| {
                let mut action: Option<SettingsAction> = None;
                // The going-dark fairness floor (invariant #6): the embodied alert channel is directional
                // flash + positioned audio. These two opt-in cues give colorblind / hard-of-hearing players a
                // non-color / visual equivalent so the core mechanic stays fair. Direct-mutate checkboxes (the
                // `invert_look_y` pattern), fed to the engine each match frame via `set_accessibility_prefs`.
                // The cycling palette / alert-cue buttons keep their TEXT labels (`.label()`) — the state is
                // never communicated by colour alone, which is the whole point of these controls.
                section_label(ui, "ACCESSIBILITY");
                ui.checkbox(&mut state.colorblind_cues, "Colorblind cues");
                ui.checkbox(&mut state.visual_sound_cues, "Visual sound cues");
                // Colourblind-safe faction palette + cross-modal alert cues (WS-D): cycling buttons over the
                // modes (direct edits — no action needed). A two-column grid so both buttons start at the
                // same x as the sliders above.
                egui::Grid::new("settings.accessibility")
                    .num_columns(2)
                    .min_col_width(SETTINGS_LABEL_W)
                    .spacing([16.0, 10.0])
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("Colorblind palette")
                                .color(BONE)
                                .size(TYPE_BODY),
                        );
                        if value_chip(ui, state.cvd_palette.label(), 140.0) {
                            state.cvd_palette = state.cvd_palette.next();
                        }
                        ui.end_row();
                        ui.label(RichText::new("Alert cues").color(BONE).size(TYPE_BODY));
                        if value_chip(ui, state.alert_cue_mode.label(), 140.0) {
                            state.alert_cue_mode = state.alert_cue_mode.next();
                        }
                        ui.end_row();
                    });

                section_divider(ui);
                section_label(ui, "VIDEO");
                // The window-mode source of truth is the host: reflect it, and emit the toggle action rather
                // than editing a second copy here.
                let mut fs = fullscreen;
                if ui.checkbox(&mut fs, "Fullscreen").clicked() {
                    action = Some(SettingsAction::ToggleFullscreen);
                }
                egui::Grid::new("settings.video")
                    .num_columns(2)
                    .min_col_width(SETTINGS_LABEL_W)
                    .spacing([16.0, 10.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Quality").color(BONE).size(TYPE_BODY));
                        // A single cycling chip over the discrete tiers (a direct edit — no action needed).
                        if value_chip(ui, state.quality.label(), 140.0) {
                            state.quality = state.quality.next();
                        }
                        ui.end_row();
                    });

                // Defensive re-clamp after the slider writes (sliders already bound, but a future edit path
                // might not).
                state.clamp();

                section_divider(ui);
                // The gunsmith lives here now (D81): customization-only, reached from Settings, not a play
                // gate. Its edits persist for the next match.
                section_label(ui, "LOADOUT");
                if footer_button(ui, "GUNSMITH", Emphasis::Secondary) {
                    action = Some(SettingsAction::OpenLoadout);
                }
                action
            }, // end right column (ACCESSIBILITY / VIDEO / LOADOUT)
        );
        // The right column owns the two discrete actions (fullscreen toggle, open gunsmith).
        action = action.or(video_action);

        // Footer navigation — outside the two columns so it stays centred under the card. A
        // divider marks "settings content ends, navigation starts".
        section_divider(ui);
        ui.add_space(18.0);
        // "FIELD MANUAL" everywhere (matches Android + this screen's own banner) — was "CONTROLS /
        // ABOUT" here and "MANUAL" on the title, three names for one screen.
        if footer_button(ui, "FIELD MANUAL", Emphasis::Secondary) {
            action = Some(SettingsAction::About);
        }
        ui.add_space(18.0);
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
        // Secondary per the shell emphasis policy: a back-out is never the amber CTA.
        ui.add_space(18.0);
        if footer_button(ui, "BACK", Emphasis::Secondary) {
            action = Some(SettingsAction::Back);
        }
    });

    action
}
