//! The pure-seam unit tests for the whole shell module tree — the decision/formatting logic across
//! [`transitions`](super::transitions), [`settings`](super::settings), [`loadout`](super::loadout),
//! [`profile`](super::profile), [`army`](super::army), [`about`](super::about),
//! [`mode_select`](super::mode_select), [`mission_select`](super::mission_select),
//! [`briefing`](super::briefing), [`persist`](super::persist), [`util`](super::util) and
//! [`theme`](super::theme)/[`widgets`](super::widgets). The egui glue (`EguiShell`/`title_ui`/
//! `loadout_ui`/`run_and_paint`) needs a GPU + window and is the exempt device-gated chrome (D32 /
//! CLAUDE.md testing rule). Kept as one test module (as before the split) so the cross-cutting
//! persistence round-trips can build every state type from one `sample_state`.
use super::*;
// Externals the tests reference. Before the module split these came in through the monolith's
// private `use`s via the `super::*` glob; the thin `shell` root no longer re-exports crate-external
// types, so name them here. (Further gunsmith/campaign items are imported at their own use-sites
// below.)
use gonedark_core::campaign::{Campaign, Difficulty, NodeId, NodeProgress};
use gonedark_core::components::Army;
use gonedark_engine::keybind::{GameAction, KeyId, KeybindMap, RebindOutcome};
use gonedark_engine::loadout_ui::{LoadoutEditor, LoadoutSlot};
use gonedark_engine::AlertCueMode;
use gonedark_render::theme::PaletteMode;
use gonedark_render::tiers::QualityTier;

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
        assert!((SettingsState::FOV_MIN..=SettingsState::FOV_MAX).contains(&s.fov_deg));
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
            fov_deg: 200.0,
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
        assert_eq!(s.fov_deg, SettingsState::FOV_MAX, "over-range FOV clamps to the ceiling");
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
            // A non-default in-band FOV (distinct from the 90° default) so the round-trip proves it.
            fov_deg: 100.0,
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
