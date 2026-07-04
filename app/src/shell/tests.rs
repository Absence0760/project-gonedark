//! The pure-seam unit tests for the whole shell module tree — the decision/formatting logic across
//! [`transitions`](super::transitions), [`settings`](super::settings), [`loadout`](super::loadout),
//! [`profile`](super::profile), [`army`](super::army), [`about`](super::about),
//! [`pvp`](super::pvp), [`mission_select`](super::mission_select),
//! [`briefing`](super::briefing), [`skirmish`](super::skirmish), [`persist`](super::persist),
//! [`util`](super::util) and
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
    fn campaign_opens_the_conflict_atlas() {
        // CAMPAIGN routes to the conflict atlas (the navigable globe, D104); the Operations hub
        // is reached from there by picking a conflict.
        assert_eq!(resolve_title_action(TitleAction::Campaign), HostTransition::OpenAtlas);
    }

    #[test]
    fn each_play_mode_opens_its_own_door() {
        // The three front doors stay distinct (`modes.md` §1): SKIRMISH (TitleAction::Pve) opens
        // the skirmish match-setup screen (§3, build-order step 1) and PvP opens its own staging
        // door (§5) — no play mode shares another's surface, and the gunsmith stays
        // customization-only behind Settings (D81).
        assert_eq!(
            resolve_title_action(TitleAction::Pve),
            HostTransition::OpenSkirmishSetup
        );
        assert_eq!(resolve_title_action(TitleAction::Pvp), HostTransition::OpenPvp);
        assert_ne!(
            resolve_title_action(TitleAction::Pvp),
            resolve_title_action(TitleAction::Pve)
        );
        assert_ne!(
            resolve_title_action(TitleAction::Pvp),
            resolve_title_action(TitleAction::Campaign)
        );
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
        assert!(TYPE_HEADING > TYPE_STAT, "a stat numeral stays below the screen banner");
        assert!(TYPE_STAT > TYPE_SUBHEAD, "a stat numeral reads as a figure, not body text");
        assert!(TYPE_HEADING > TYPE_SUBHEAD);
        assert!(TYPE_SUBHEAD >= TYPE_BUTTON);
        assert!(TYPE_BUTTON > TYPE_BODY);
        assert!(TYPE_BODY > TYPE_CAPTION);
    }

    #[test]
    fn shell_style_makes_controls_legible_on_the_dark_card() {
        let style = shell_style();
        // Sliders read their value at a glance (amber trailing fill, not a bare hairline).
        assert!(style.visuals.slider_trailing_fill);
        // Checkbox / radio glyphs are large enough to see on PANEL (egui's 14px default vanished).
        assert!(style.spacing.icon_width >= 18.0);
        assert!(style.spacing.icon_width_inner >= 10.0);
        // Overflowing cards (Settings on a short window) show a solid, always-drawn scrollbar.
        assert!(!style.spacing.scroll.floating, "scrollbar must be solid, not hover-only");
    }

    // ---- The over-backdrop card placement seam ---------------------------------------------------

    #[test]
    fn over_backdrop_top_settles_at_the_optical_centre_and_clamps() {
        // First frame (no remembered height): the fixed top band.
        assert_eq!(over_backdrop_top(1000.0, None), 100.0);
        // A short card sits at the optical centre: 42% of the leftover space above it.
        let top = over_backdrop_top(1000.0, Some(400.0));
        assert!((top - 252.0).abs() < 0.01, "expected (1000-400)*0.42, got {top}");
        // Its bottom slack exceeds its top slack (optical, not geometric, centring).
        assert!(1000.0 - (top + 400.0) > top);
        // A card taller than the viewport clamps to the minimum margin instead of going negative.
        assert_eq!(over_backdrop_top(600.0, Some(2000.0)), SHELL_CARD_MARGIN);
        // A degenerate viewport still yields a sane, non-panicking offset.
        assert!(over_backdrop_top(10.0, Some(50.0)) >= 0.0);
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
        // A remapped keybind — host toggle AND gameplay key (Q27) — survives encode→decode
        // alongside the other prefs.
        let mut s = SettingsState::default();
        assert_eq!(s.keybinds.rebind(GameAction::Pause, KeyId::P), RebindOutcome::Bound);
        assert_eq!(
            s.keybinds.rebind(GameAction::ToggleDebugOverlay, KeyId::G),
            RebindOutcome::Bound
        );
        assert_eq!(s.keybinds.rebind(GameAction::Jump, KeyId::V), RebindOutcome::Bound);
        assert_eq!(s.keybinds.rebind(GameAction::Embody, KeyId::T), RebindOutcome::Bound);
        let blob = encode_shell_prefs(
            &s,
            &ProfileState::default(),
            &LoadoutEditor::new(),
            &ArmySelectState::default(),
        );
        let (s2, _, _, _) = decode_shell_prefs(&blob);
        assert_eq!(s2.keybinds, s.keybinds, "keybinds survive the round-trip");
        assert_eq!(s2.keybinds.key_for(GameAction::Pause), KeyId::P);
        assert_eq!(s2.keybinds.key_for(GameAction::Jump), KeyId::V);
        assert_eq!(s2.keybinds.key_for(GameAction::Embody), KeyId::T);

        // A blob missing the key (an older save) decodes to the shipped default bindings, never panics.
        let (s3, _, _, _) = decode_shell_prefs("gonedark-shell 1\nmaster=0.5\n");
        assert_eq!(s3.keybinds, KeybindMap::default(), "missing keybinds → defaults");
        // A garbage value also falls back to defaults (KeybindMap::decode is total).
        let (s4, _, _, _) = decode_shell_prefs("keybinds=wat,nope\n");
        assert_eq!(s4.keybinds, KeybindMap::default(), "garbage keybinds → defaults");
        // A pre-Q27 blob (three host-toggle fields only) keeps its host rebind and leaves every
        // gameplay key at its shipped default — the frozen 0–2 ordinal contract.
        let (s5, _, _, _) = decode_shell_prefs("keybinds=27,10,2\n");
        assert_eq!(s5.keybinds.key_for(GameAction::Pause), KeyId::P);
        assert_eq!(s5.keybinds.key_for(GameAction::MoveUp), KeyId::W);
        assert_eq!(s5.keybinds.key_for(GameAction::SelectFire), KeyId::X);
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

    use gonedark_core::gunsmith::{Barrel, Loadout, Magazine, Muzzle, Optic, Stock};

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
        // Every slot non-default — including the D85 Stock/Muzzle pair, so the round-trip proves
        // the encoder writes them (the §12-item-5 encode hole: decode read the keys, encode didn't).
        let loadout = LoadoutEditor::with_loadout(Loadout {
            optic: Optic::Marksman,
            barrel: Barrel::Heavy,
            magazine: Magazine::Extended,
            stock: Stock::Agile,
            muzzle: Muzzle::Suppressor,
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
                    stock=9\nmuzzle=xyz\narmy=42\ncallsign=   \n";
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
    fn campaign_routes_through_the_atlas_then_hub_then_briefing() {
        // The full title -> atlas -> hub -> briefing wiring at the seam level (D104): CAMPAIGN
        // opens the atlas; picking a conflict there Enters its hub; a hub tile opens a briefing.
        assert_eq!(resolve_title_action(TitleAction::Campaign), HostTransition::OpenAtlas);
        let campaign = atlas_campaign();
        let mut state = AtlasState::opened(&campaign);
        assert_eq!(
            apply_atlas_action(AtlasAction::Enter, &mut state, &campaign),
            AtlasStep::Enter(state.selected)
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
    fn next_operation_walks_the_chain_then_offers_a_replay() {
        let mut campaign = chain_campaign();
        // Fresh campaign: CONTINUE points at the first Available node, nothing cleared.
        let op = next_operation(&campaign).unwrap();
        assert_eq!(op.node, NodeId(0));
        assert_eq!(op.title, "Alpha");
        assert_eq!((op.cleared, op.total), (0, 2));
        assert!(!op.replay);
        // Clearing A advances CONTINUE to the newly-opened B and bumps the tally.
        campaign.clear(NodeId(0), Difficulty::Regular).unwrap();
        let op = next_operation(&campaign).unwrap();
        assert_eq!(op.node, NodeId(1));
        assert_eq!((op.cleared, op.total), (1, 2));
        assert!(!op.replay);
        // Fully cleared: CONTINUE degrades to a replay of the last operation (never disappears).
        campaign.clear(NodeId(1), Difficulty::Regular).unwrap();
        let op = next_operation(&campaign).unwrap();
        assert_eq!(op.node, NodeId(1));
        assert_eq!((op.cleared, op.total), (2, 2));
        assert!(op.replay);
    }

    #[test]
    fn next_operation_prefers_fresh_progress_over_a_higher_index_replay() {
        // Two parallel branches: root A (node 0) untouched, root B (node 1) cleared through its
        // successor (node 2). CONTINUE must point at the low-index Available node — fresh progress —
        // not the higher-index Cleared ones (pins the find(Available)-before-replay-fallback order).
        let mut campaign = Campaign::new(vec![
            OperationNode::new(NodeId(0), MissionId(1), "Alpha", "take the outpost"),
            OperationNode::new(NodeId(1), MissionId(2), "Bravo", "hold the ridge"),
            OperationNode::new(NodeId(2), MissionId(3), "Charlie", "push the line")
                .requires([NodeId(1)]),
        ]);
        campaign.clear(NodeId(1), Difficulty::Regular).unwrap();
        campaign.clear(NodeId(2), Difficulty::Regular).unwrap();
        let op = next_operation(&campaign).unwrap();
        assert_eq!(op.node, NodeId(0), "fresh progress beats a higher-index replay");
        assert_eq!((op.cleared, op.total), (2, 3));
        assert!(!op.replay);
    }

    #[test]
    fn next_operation_is_absent_for_an_empty_campaign() {
        // No nodes → no card (the title simply doesn't draw it).
        assert_eq!(next_operation(&Campaign::new(vec![])), None);
    }

    // ---- The conflict-atlas grouping seam (D98) -------------------------------------------------

    use gonedark_core::campaign::{Conflict, ConflictId, GroupProgress, Operation, OperationId};
    use gonedark_render::globe_backdrop::{project_pin, GlobeView, PinTone};

    /// An atlas-grouped campaign exercising every grouping shape at once: conflict 0 with two
    /// operations (op 0: Alpha → Bravo; op 1: Charlie gated on Bravo), conflict 1 with only a
    /// content-pending (empty) operation, and an ungrouped node Delta.
    fn atlas_campaign() -> Campaign {
        Campaign::with_atlas(
            vec![
                Conflict {
                    id: ConflictId(0),
                    name: "The Channel Crisis".into(),
                    start_year: 2027,
                    end_year: 2028,
                    summary: "a fictional modern flashpoint".into(),
                    lat_x10: 500,
                    lon_x10: -15,
                },
                Conflict {
                    id: ConflictId(1),
                    name: "Battle of Normandy".into(),
                    start_year: 1944,
                    end_year: 1944,
                    summary: "content pending".into(),
                    lat_x10: 494,
                    lon_x10: -6,
                },
            ],
            vec![
                Operation {
                    id: OperationId(0),
                    conflict: ConflictId(0),
                    name: "Operation First Light".into(),
                },
                Operation {
                    id: OperationId(1),
                    conflict: ConflictId(0),
                    name: "Operation Ember".into(),
                },
                Operation {
                    id: OperationId(2),
                    conflict: ConflictId(1),
                    name: "Pointe du Hoc".into(),
                },
            ],
            vec![
                // Conflict 0's battles carry battlefield anchors (D106) spread around the
                // conflict pin; Charlie is deliberately UN-anchored (a battle with no authored
                // ground never pins), and Delta stays fully ungrouped.
                OperationNode::new(NodeId(0), MissionId(1), "Alpha", "")
                    .in_operation(OperationId(0))
                    .at(496, -13),
                OperationNode::new(NodeId(1), MissionId(2), "Bravo", "")
                    .requires([NodeId(0)])
                    .in_operation(OperationId(0))
                    .at(494, -13),
                OperationNode::new(NodeId(2), MissionId(3), "Charlie", "")
                    .requires([NodeId(1)])
                    .in_operation(OperationId(1)),
                OperationNode::new(NodeId(3), MissionId(4), "Delta", ""),
            ],
        )
    }

    #[test]
    fn hub_sections_order_conflicts_then_operations_then_trailing_ungrouped() {
        let sections = hub_sections(&atlas_campaign());
        assert_eq!(sections.len(), 3);

        // Section 0 opens conflict 0 (header present) with operation 0's tiles, in authored order.
        assert_eq!(sections[0].conflict.map(|(id, _)| id), Some(ConflictId(0)));
        assert_eq!(sections[0].operation.map(|(id, _)| id), Some(OperationId(0)));
        assert_eq!(sections[0].nodes, vec![NodeId(0), NodeId(1)]);

        // Section 1 continues the SAME conflict — no repeated conflict header, just op 1's tiles.
        assert_eq!(sections[1].conflict, None, "a conflict header draws once, on its first section");
        assert_eq!(sections[1].operation.map(|(id, _)| id), Some(OperationId(1)));
        assert_eq!(sections[1].nodes, vec![NodeId(2)]);

        // Conflict 1's only operation is content-pending (no nodes): no header scaffolding at all.
        // The trailing section is the untitled ungrouped one.
        assert_eq!(sections[2].conflict, None);
        assert_eq!(sections[2].operation, None, "ungrouped nodes render in an untitled section");
        assert_eq!(sections[2].nodes, vec![NodeId(3)]);
    }

    #[test]
    fn hub_sections_index_the_mission_select_entries_exactly() {
        // The glue looks tiles up as `entries[node.0]` — pin that every sectioned node id indexes
        // its own entry (mission_select() is in NodeId order), and that every node appears exactly
        // once across the sections, so grouping can never drop or duplicate a tile.
        let campaign = atlas_campaign();
        let entries = campaign.mission_select();
        let mut seen: Vec<NodeId> = hub_sections(&campaign)
            .iter()
            .flat_map(|s| s.nodes.iter().copied())
            .collect();
        for &node in &seen {
            assert_eq!(entries[node.0 as usize].node, node);
        }
        seen.sort_unstable();
        let all: Vec<NodeId> = (0..campaign.len() as u32).map(NodeId).collect();
        assert_eq!(seen, all, "every node renders exactly once");
    }

    #[test]
    fn the_globe_focuses_the_conflict_being_fought_and_pins_every_conflict() {
        // Fresh atlas: the first conflict is in progress → it holds the focus, and every authored
        // conflict gets exactly one pin at its tenth-degree anchor converted to degrees (D103).
        let mut campaign = atlas_campaign();
        assert_eq!(focused_conflict(&campaign), 0);
        let pins = atlas_pins(&campaign);
        assert_eq!(pins.len(), 2);
        assert!((pins[0].lat_deg - 50.0).abs() < 1e-6);
        assert!((pins[0].lon_deg - -1.5).abs() < 1e-6);
        assert!(pins[0].focused);
        assert!(!pins[1].focused);
        assert_eq!(pins.iter().filter(|p| p.focused).count(), 1, "exactly one focus");

        // Clear everything in conflict 0 → the focus advances to the next unfinished conflict.
        for node in 0..campaign.len() as u32 {
            let _ = campaign.clear(NodeId(node), Difficulty::Regular);
        }
        assert_eq!(focused_conflict(&campaign), 1);
        assert!(atlas_pins(&campaign)[1].focused);
    }

    // ---- atlas: the navigable globe + year scrubber (D104) -------------------------------------

    #[test]
    fn the_atlas_opens_settled_on_the_fought_conflict_at_its_opening_year() {
        let campaign = atlas_campaign();
        let state = AtlasState::opened(&campaign);
        assert_eq!(state.selected, focused_conflict(&campaign));
        assert_eq!(state.year, 2027, "opens scrubbed to the fought conflict's first year");
        // The opened yaw faces the selected conflict's longitude (the D103 settle, sway-free).
        assert!((state.view.yaw - (1.5f32).to_radians()).abs() < 1e-5);
        assert_eq!(state.view.zoom, 1.0);
    }

    #[test]
    fn atlas_navigation_clamps_but_never_blocks() {
        let campaign = atlas_campaign();
        let mut state = AtlasState::opened(&campaign);
        // A drag turns the globe…
        let before = state.view;
        assert_eq!(
            apply_atlas_action(AtlasAction::Drag(120.0, -40.0), &mut state, &campaign),
            AtlasStep::Stay
        );
        // …with the documented feel pinned by sign: dragging right (+dx) pulls yaw up, dragging
        // up (−dy) tips north away (pitch down) — an inverted-drag refactor fails here.
        assert!(state.view.yaw > before.yaw, "+dx drags yaw up");
        assert!(state.view.pitch < before.pitch, "-dy tips pitch down");
        // …a wild drag can never flip the globe past the pitch limit…
        apply_atlas_action(AtlasAction::Drag(0.0, 1_000_000.0), &mut state, &campaign);
        assert!(state.view.pitch <= gonedark_render::globe_backdrop::GlobeView::PITCH_LIMIT);
        // …and zoom clamps at both ends (scroll-up in, scroll-down out).
        apply_atlas_action(AtlasAction::Zoom(1_000.0), &mut state, &campaign);
        assert_eq!(state.view.zoom, gonedark_render::globe_backdrop::GlobeView::ZOOM_MAX);
        apply_atlas_action(AtlasAction::Zoom(-1_000.0), &mut state, &campaign);
        assert_eq!(state.view.zoom, gonedark_render::globe_backdrop::GlobeView::ZOOM_MIN);
        // Zoomed-in drags are finer than zoomed-out ones (the region under the cursor tracks).
        let mut zoomed = AtlasState::opened(&campaign);
        apply_atlas_action(AtlasAction::Zoom(1_000.0), &mut zoomed, &campaign);
        let y0 = zoomed.view.yaw;
        apply_atlas_action(AtlasAction::Drag(100.0, 0.0), &mut zoomed, &campaign);
        let fine = zoomed.view.yaw - y0;
        let mut wide = AtlasState::opened(&campaign);
        let y0 = wide.view.yaw;
        apply_atlas_action(AtlasAction::Drag(100.0, 0.0), &mut wide, &campaign);
        assert!((wide.view.yaw - y0).abs() > fine.abs());
    }

    #[test]
    fn the_scrubber_spans_the_authored_wars_and_dims_the_out_of_era() {
        let campaign = atlas_campaign(); // Channel Crisis 2027-2028 + Normandy 1944
        assert_eq!(year_domain(&campaign), (1944, 2028));
        let mut state = AtlasState::opened(&campaign);
        // Scrubbed to 2027: the Channel Crisis is live, Normandy is out of era (dim).
        let pins = atlas_pins_for(&campaign, &state);
        assert!(pins[0].active && !pins[1].active);
        // Scrub to 1944: the eras flip; the selection (and focus) is unchanged by scrubbing.
        apply_atlas_action(AtlasAction::SetYear(1944), &mut state, &campaign);
        let pins = atlas_pins_for(&campaign, &state);
        assert!(!pins[0].active && pins[1].active);
        assert!(pins[0].focused, "scrubbing never steals the selection");
        // The scrub clamps into the authored domain (a stale slider value can't escape).
        apply_atlas_action(AtlasAction::SetYear(1200), &mut state, &campaign);
        assert_eq!(state.year, 1944);
        apply_atlas_action(AtlasAction::SetYear(3000), &mut state, &campaign);
        assert_eq!(state.year, 2028);
    }

    #[test]
    fn selecting_and_entering_carry_the_picked_conflict() {
        let campaign = atlas_campaign();
        let mut state = AtlasState::opened(&campaign);
        assert_eq!(
            apply_atlas_action(AtlasAction::SelectConflict(1), &mut state, &campaign),
            AtlasStep::Stay
        );
        assert_eq!(state.selected, 1);
        assert!(atlas_pins_for(&campaign, &state)[1].focused);
        // An out-of-range select (stale index) is ignored, never a panic.
        apply_atlas_action(AtlasAction::SelectConflict(99), &mut state, &campaign);
        assert_eq!(state.selected, 1);
        assert_eq!(
            apply_atlas_action(AtlasAction::Enter, &mut state, &campaign),
            AtlasStep::Enter(1)
        );
        assert_eq!(
            apply_atlas_action(AtlasAction::Back, &mut state, &campaign),
            AtlasStep::Back
        );
    }

    #[test]
    fn clicking_the_focused_pin_picks_it_and_empty_ocean_picks_nothing() {
        let campaign = atlas_campaign();
        let state = AtlasState::opened(&campaign);
        let aspect = 1.6;
        // Project the selected conflict's pin with the SAME seam the picker uses, then "click" it.
        let c = &campaign.conflicts()[state.selected];
        let p = gonedark_render::globe_backdrop::project_pin(
            state.view,
            aspect,
            c.lat_x10 as f32 / 10.0,
            c.lon_x10 as f32 / 10.0,
        )
        .expect("the opened view faces the selected conflict");
        assert_eq!(pick_conflict(&campaign, &state, aspect, p), Some(state.selected));
        // A click far from any pin selects nothing (the empty-ocean guard).
        assert_eq!(pick_conflict(&campaign, &state, aspect, [0.9, -0.9]), None);
    }

    #[test]
    fn a_continue_deep_link_resolves_its_node_to_the_right_conflict() {
        // The CONTINUE resync seam (the code-review finding): a node reached without passing
        // through the atlas must still resolve to its own conflict, so the hub the briefing
        // escapes to is filtered to the war actually being played — never a stale selection.
        let campaign = atlas_campaign();
        // Every grouped node resolves to the conflict that owns its operation.
        for node in 0..campaign.len() as u32 {
            let node = NodeId(node);
            if let Some(i) = conflict_index_of(&campaign, node) {
                let op = campaign.node(node).unwrap().operation.unwrap();
                assert_eq!(campaign.operation(op).unwrap().conflict, campaign.conflicts()[i].id);
            } else {
                // Only an ungrouped node resolves to nothing.
                assert!(campaign.node(node).unwrap().operation.is_none());
            }
        }
        // The fixture's grouped nodes all resolve (to conflict 0 — its second conflict is
        // deliberately content-pending) and the ungrouped tail resolves to nothing, so the
        // resync leaves a stale selection alone only when the node genuinely has no conflict.
        let indices: Vec<Option<usize>> = (0..campaign.len() as u32)
            .map(|n| conflict_index_of(&campaign, NodeId(n)))
            .collect();
        assert!(indices.contains(&Some(0)) && indices.contains(&None));
        // And the SHIPPED campaign resolves every node (fully grouped — no CONTINUE deep-link
        // can ever land on an unresolvable node in the real game).
        let shipped = gonedark_engine::mission_registry::default_campaign();
        for node in 0..shipped.len() as u32 {
            assert!(conflict_index_of(&shipped, NodeId(node)).is_some());
        }
    }

    #[test]
    fn the_atlas_card_line_formats_both_year_shapes() {
        let campaign = atlas_campaign();
        // Multi-year span + rollup…
        assert_eq!(
            atlas_card_line(&campaign.conflicts()[0], 0, 3),
            "2027-2028 \u{00B7} 0/3 OPERATIONS CLEARED"
        );
        // …and a single-year conflict collapses the span (the hub header rule).
        assert_eq!(
            atlas_card_line(&campaign.conflicts()[1], 1, 2),
            "1944 \u{00B7} 1/2 OPERATIONS CLEARED"
        );
    }

    #[test]
    fn the_hub_filtered_to_a_conflict_shows_only_its_operations() {
        let campaign = atlas_campaign();
        // Unfiltered = the full pre-D104 hub.
        assert_eq!(hub_sections_for(&campaign, None), hub_sections(&campaign));
        // Filtered to conflict 0: every section resolves to conflict 0, none to the other, and
        // the ungrouped tail is excluded (a filtered hub can't leak unowned tiles).
        let only = hub_sections_for(&campaign, Some(ConflictId(0)));
        assert!(!only.is_empty());
        for s in &only {
            let (op, _) = s.operation.expect("filtered sections are always operation-backed");
            assert_eq!(campaign.operation(op).unwrap().conflict, ConflictId(0));
        }
        assert!(only.len() < hub_sections(&campaign).len());
    }

    // ---- the battlefield overview (D106) --------------------------------------------------------

    /// The overview camera + pins compose end-to-end for every SHIPPED war: each anchored battle
    /// projects on-screen under its own conflict's overview view, no two battle pins of one war
    /// land within a pick radius of each other (they must read as separate grounds AND stay
    /// unambiguously pickable), and every authored anchor sits on the land mask — a wet pin reads
    /// as a bug on the Natural Earth globe.
    #[test]
    fn the_shipped_wars_frame_their_battlefields_on_screen() {
        use gonedark_engine::mission_registry::default_campaign;
        use gonedark_render::globe_backdrop::{land_at, project_pin};
        let campaign = default_campaign();
        for conflict in campaign.conflicts() {
            let view = overview_view(&campaign, conflict.id)
                .expect("every shipped war has anchored battles");
            let mut projected: Vec<[f32; 2]> = Vec::new();
            for op in campaign.operations_in(conflict.id) {
                for n in campaign.nodes_in(op) {
                    let (lat, lon) = campaign.node(n).unwrap().anchor.expect("anchored");
                    let (lat, lon) = (lat as f32 / 10.0, lon as f32 / 10.0);
                    assert!(
                        land_at(lat, lon),
                        "{}'s battle {n:?} anchors in the sea at ({lat}, {lon})",
                        conflict.name,
                    );
                    let p = project_pin(view, 16.0 / 9.0, lat, lon).unwrap_or_else(|| {
                        panic!("{}'s battle {n:?} is not visible in its overview", conflict.name)
                    });
                    assert!(
                        p[0].abs() <= 0.9 && p[1].abs() <= 0.9,
                        "{}'s battle {n:?} projects off-screen at {p:?}",
                        conflict.name,
                    );
                    projected.push(p);
                }
            }
            for (i, a) in projected.iter().enumerate() {
                for b in &projected[..i] {
                    let (dx, dy) = ((a[0] - b[0]) * (16.0 / 9.0), a[1] - b[1]);
                    let d = (dx * dx + dy * dy).sqrt();
                    // Half a pick radius ≈ a pin's drawn diameter: far enough apart to read as
                    // two grounds, and a dead-center click is always nearer its own pin than any
                    // neighbour (nearest-wins picking needs only nonzero separation).
                    assert!(
                        d >= PICK_RADIUS / 2.0,
                        "two of {}'s battle pins overlap on screen (separation {d})",
                        conflict.name,
                    );
                }
            }
        }
    }

    /// Battle pins carry campaign progress (D106): tones map Locked/Available/Cleared, the next
    /// battle is focused, an un-anchored battle never pins, and progressing the war re-tones the
    /// same ground.
    #[test]
    fn battle_pins_tone_and_focus_follow_progress() {
        let mut campaign = atlas_campaign();
        // Fresh: Alpha available (amber, focused — it is the next battle), Bravo locked (slate).
        // Charlie has no anchor, so conflict 0 pins exactly its two anchored battles.
        assert_eq!(battle_tone(NodeProgress::Available), PinTone::Neutral);
        assert_eq!(next_battle_in(&campaign, ConflictId(0)), Some(NodeId(0)));
        let pins =
            battlefield_pins(&campaign, ConflictId(0), next_battle_in(&campaign, ConflictId(0)));
        assert_eq!(pins.len(), 2, "only anchored battles pin");
        assert_eq!((pins[0].tone, pins[0].focused), (PinTone::Neutral, true));
        assert_eq!((pins[1].tone, pins[1].focused), (PinTone::Locked, false));
        assert!(pins.iter().all(|p| p.scale > 1.0), "battle pins draw larger than conflict pins");

        // Clear Alpha: it goes green, Bravo opens amber and takes the focus.
        campaign.clear(NodeId(0), Difficulty::Recruit).unwrap();
        assert_eq!(next_battle_in(&campaign, ConflictId(0)), Some(NodeId(1)));
        let pins =
            battlefield_pins(&campaign, ConflictId(0), next_battle_in(&campaign, ConflictId(0)));
        assert_eq!((pins[0].tone, pins[0].focused), (PinTone::Cleared, false));
        assert_eq!((pins[1].tone, pins[1].focused), (PinTone::Neutral, true));

        // A war with no anchored battles has no overview — the hub falls back to the settled
        // framing (conflict 1's only node, Charlie, is un-anchored).
        assert_eq!(overview_view(&campaign, ConflictId(1)), None);
        // A fully cleared war focuses its LAST battle as the replay target (Charlie — conflict
        // 0's final node, even though it is un-anchored and so never pins).
        campaign.clear(NodeId(1), Difficulty::Recruit).unwrap();
        campaign.clear(NodeId(2), Difficulty::Recruit).unwrap();
        assert_eq!(next_battle_in(&campaign, ConflictId(0)), Some(NodeId(2)));
    }

    /// Clicking the battlefield resolves through the same playable gate as a tile (D106): the
    /// available battle picks at its projected position, a LOCKED battle refuses the click even
    /// dead-center, and empty ground picks nothing.
    #[test]
    fn picking_a_battle_honours_the_playable_gate() {
        let campaign = atlas_campaign();
        let view = overview_view(&campaign, ConflictId(0)).expect("conflict 0 is anchored");
        let aspect = 16.0 / 9.0;
        let at = |node: NodeId| {
            let (lat, lon) = campaign.node(node).unwrap().anchor.unwrap();
            project_pin(view, aspect, lat as f32 / 10.0, lon as f32 / 10.0)
                .expect("battle visible in its overview")
        };
        let (alpha, bravo) = (at(NodeId(0)), at(NodeId(1)));
        // Alpha (Available) picks at its own pin.
        assert_eq!(pick_battle(&campaign, ConflictId(0), view, aspect, alpha), Some(NodeId(0)));
        // Bravo is Locked — a dead-center click on it picks nothing (never launches).
        assert_eq!(pick_battle(&campaign, ConflictId(0), view, aspect, bravo), None);
        // Empty ground picks nothing.
        assert_eq!(pick_battle(&campaign, ConflictId(0), view, aspect, [0.95, 0.95]), None);

        // Clear Alpha: Bravo opens and now picks; Alpha stays pickable (replayable).
        let mut campaign = campaign;
        campaign.clear(NodeId(0), Difficulty::Recruit).unwrap();
        assert_eq!(pick_battle(&campaign, ConflictId(0), view, aspect, bravo), Some(NodeId(1)));
        assert_eq!(pick_battle(&campaign, ConflictId(0), view, aspect, alpha), Some(NodeId(0)));
    }

    // ---- the atlas ↔ battlefield camera fly-in (D107) -------------------------------------------

    /// The hub's per-frame camera resolution: a live flight owns the view (advanced by dt, dropped
    /// on landing), no flight shows the target directly, and a vanished target (no anchored war)
    /// drops any stale flight and falls back to the settled framing.
    #[test]
    fn the_hub_backdrop_view_flies_then_lands_on_the_target() {
        use gonedark_render::globe_backdrop::GlobeFlight;
        let from = GlobeView { yaw: 0.0, pitch: 0.0, zoom: 1.0 };
        let to = GlobeView { yaw: 1.0, pitch: 0.5, zoom: 2.4 };

        // Mid-flight: the flown view (neither endpoint) is what the frame renders with.
        let mut flight = Some(GlobeFlight::new(from, to));
        let mid = hub_backdrop_view(&mut flight, Some(to), GlobeFlight::DURATION / 2.0)
            .expect("a live flight always yields a view");
        assert!(mid != from && mid != to, "mid-flight is between the endpoints");
        assert!(flight.is_some(), "still flying");

        // Landing: the final frame yields (effectively) the target and drops the flight; from
        // then on the target passes straight through.
        let landed = hub_backdrop_view(&mut flight, Some(to), GlobeFlight::DURATION).unwrap();
        assert!((landed.zoom - to.zoom).abs() < 1e-5 && (landed.pitch - to.pitch).abs() < 1e-6);
        assert!(flight.is_none(), "the flight is dropped on landing");
        assert_eq!(hub_backdrop_view(&mut flight, Some(to), 0.016), Some(to));

        // No target (a war with no anchored battles): a stale flight is dropped, settled fallback.
        let mut stale = Some(GlobeFlight::new(from, to));
        assert_eq!(hub_backdrop_view(&mut stale, None, 0.016), None);
        assert!(stale.is_none(), "a stale flight can't outlive its target");
    }

    /// The atlas's return leg: `opened_from` starts exactly at the hub's camera (no cut), flies
    /// to the same view a plain `opened` lands on, and the player's own drag/zoom cancels the
    /// flight instantly (the hand always beats the autopilot) — while scrub/select do not.
    #[test]
    fn the_atlas_return_flight_lands_on_the_opened_view_unless_the_player_grabs_it() {
        use gonedark_render::globe_backdrop::GlobeFlight;
        let campaign = atlas_campaign();
        let hub_view = overview_view(&campaign, ConflictId(0)).expect("conflict 0 is anchored");
        let opened = AtlasState::opened(&campaign);

        // Starts at the hub's camera, targeting the plain opened view.
        let mut state = AtlasState::opened_from(&campaign, hub_view);
        assert_eq!(state.view, hub_view, "no cut: the first frame is the hub's camera");
        assert_eq!((state.year, state.selected), (opened.year, opened.selected));

        // Fly to landing: indistinguishable from a plain open afterwards.
        tick_atlas_flight(&mut state, GlobeFlight::DURATION / 2.0);
        assert!(state.flight.is_some() && state.view != hub_view && state.view != opened.view);
        tick_atlas_flight(&mut state, GlobeFlight::DURATION);
        assert!(state.flight.is_none(), "the flight is dropped on landing");
        assert_eq!(state.view, opened.view, "landed exactly on the plain opened view");
        // A landed state ticks as a no-op.
        tick_atlas_flight(&mut state, 0.016);
        assert_eq!(state.view, opened.view);

        // A drag mid-flight cancels it and applies immediately...
        let mut grabbed = AtlasState::opened_from(&campaign, hub_view);
        tick_atlas_flight(&mut grabbed, 0.1);
        assert!(grabbed.flight.is_some());
        apply_atlas_action(AtlasAction::Drag(10.0, 0.0), &mut grabbed, &campaign);
        assert!(grabbed.flight.is_none(), "a drag hands the camera to the player");
        // ...and so does a zoom.
        let mut zoomed = AtlasState::opened_from(&campaign, hub_view);
        apply_atlas_action(AtlasAction::Zoom(1.0), &mut zoomed, &campaign);
        assert!(zoomed.flight.is_none(), "a zoom hands the camera to the player");
        // Scrubbing the year or picking a pin is not a camera gesture — the flight keeps flying.
        let mut scrubbed = AtlasState::opened_from(&campaign, hub_view);
        apply_atlas_action(AtlasAction::SetYear(2028), &mut scrubbed, &campaign);
        apply_atlas_action(AtlasAction::SelectConflict(1), &mut scrubbed, &campaign);
        assert!(scrubbed.flight.is_some(), "scrub/select never steal the camera");
    }

    #[test]
    fn the_list_cap_always_leaves_the_footer_on_screen() {
        // The un-pinned-BACK regression (found via screenshot): the old fixed 5×72 cap ignored the
        // window height, so a short window overflowed the card and pushed BACK below the fold. The
        // cap must leave the footer reserve visible at every height…
        const FOOTER_RESERVE: f32 = FOOTER_GAP + 46.0 + 8.0;
        for available in [200.0_f32, 300.0, 380.0, 450.0, 600.0, 900.0] {
            let cap = list_viewport_cap(available);
            assert!(
                cap + FOOTER_RESERVE <= available || cap == 72.0,
                "cap {cap} at available {available} pushes the footer off-screen"
            );
        }
        // …grow to (and stop at) the roomy five-row cap…
        assert_eq!(list_viewport_cap(1000.0), 5.0 * 72.0);
        assert_eq!(list_viewport_cap(10_000.0), 5.0 * 72.0);
        // …never collapse below one tile-row on a degenerate viewport (the min-window floor)…
        assert_eq!(list_viewport_cap(0.0), 72.0);
        assert_eq!(list_viewport_cap(90.0), 72.0);
        // …and shrink smoothly in between (monotone: more room never means a smaller list).
        assert!(list_viewport_cap(300.0) <= list_viewport_cap(400.0));
        assert!(list_viewport_cap(400.0) <= list_viewport_cap(500.0));
    }

    #[test]
    fn hub_section_rollups_track_clears() {
        let mut campaign = atlas_campaign();

        // Fresh: nothing cleared; op 0 has the playable root, op 1 is fully gated (greyed).
        let sections = hub_sections(&campaign);
        assert_eq!(
            sections[0].conflict.map(|(_, r)| r),
            Some(GroupProgress { cleared: 0, total: 3, playable: true })
        );
        assert_eq!(
            sections[0].operation.map(|(_, r)| r),
            Some(GroupProgress { cleared: 0, total: 2, playable: true })
        );
        assert_eq!(
            sections[1].operation.map(|(_, r)| r),
            Some(GroupProgress { cleared: 0, total: 1, playable: false }),
            "a fully gated operation reads not-playable (the glue greys its header)"
        );

        // Clear Alpha then Bravo: op 0 completes (2/2), Bravo's clear opens Charlie so op 1 turns
        // playable, and the conflict rollup advances to 2/3.
        campaign.clear(NodeId(0), Difficulty::Regular).unwrap();
        campaign.clear(NodeId(1), Difficulty::Regular).unwrap();
        let sections = hub_sections(&campaign);
        assert_eq!(
            sections[0].conflict.map(|(_, r)| r),
            Some(GroupProgress { cleared: 2, total: 3, playable: true })
        );
        assert_eq!(
            sections[0].operation.map(|(_, r)| r),
            Some(GroupProgress { cleared: 2, total: 2, playable: true })
        );
        assert_eq!(
            sections[1].operation.map(|(_, r)| r),
            Some(GroupProgress { cleared: 0, total: 1, playable: true })
        );
    }

    #[test]
    fn hub_sections_degrade_to_one_untitled_section_without_an_atlas() {
        // A plain `Campaign::new` hub (empty atlas, all nodes ungrouped) is exactly the pre-atlas
        // flat list: one untitled section carrying every node in authored order.
        let sections = hub_sections(&chain_campaign());
        assert_eq!(
            sections,
            vec![HubSection {
                conflict: None,
                operation: None,
                nodes: vec![NodeId(0), NodeId(1)],
            }]
        );
        // And an empty campaign renders no sections at all (the hub just shows BACK).
        assert_eq!(hub_sections(&Campaign::new(vec![])), Vec::<HubSection>::new());
    }

    #[test]
    fn hub_header_labels_format_names_years_and_rollups() {
        let campaign = atlas_campaign();
        // Multi-year conflict: NAME · span · rollup (ASCII + U+00B7 only — the no-tofu rule).
        let channel = campaign.conflict(ConflictId(0)).unwrap();
        assert_eq!(
            conflict_header_label(channel, campaign.conflict_progress(ConflictId(0))),
            "THE CHANNEL CRISIS \u{00B7} 2027-2028 \u{00B7} 0/3"
        );
        // Single-year conflict collapses the span to one year.
        let normandy = campaign.conflict(ConflictId(1)).unwrap();
        assert_eq!(
            conflict_header_label(normandy, campaign.conflict_progress(ConflictId(1))),
            "BATTLE OF NORMANDY \u{00B7} 1944 \u{00B7} 0/0"
        );
        // Operation sub-header: NAME · its own rollup.
        let first_light = campaign.operation(OperationId(0)).unwrap();
        assert_eq!(
            operation_header_label(first_light, campaign.operation_progress(OperationId(0))),
            "OPERATION FIRST LIGHT \u{00B7} 0/2"
        );
    }

    #[test]
    fn continue_deep_links_into_the_operations_briefing() {
        // The title hub's CONTINUE reuses the hub's own briefing transition — same flow, one hop
        // shorter — so the shortcut can never diverge from the canonical CAMPAIGN path.
        assert_eq!(
            resolve_title_action(TitleAction::ContinueCampaign(NodeId(1))),
            HostTransition::OpenBriefing(NodeId(1))
        );
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

    // ---- The skirmish match-setup pure seam (modes.md §3) ----------------------------------------

    use gonedark_engine::map_library::{BattlefieldKind, BATTLEFIELDS};
    use gonedark_engine::Scene;

    #[test]
    fn skirmish_default_is_the_neutral_shipped_match() {
        // The default deploy must reproduce the pre-setup-screen skirmish: the first battlefield
        // resolves to the standing two-base Scene::Skirmish, US vs FR (two distinct combatant
        // rosters), at Regular — the neutral D83 tier whose combat tuning is a byte-identical
        // no-op (`Difficulty::Regular.scenario_modifiers() == ScenarioModifiers::default()`).
        let state = SkirmishSetupState::default();
        let cfg = resolve_skirmish_config(&state);
        assert_eq!(cfg.battlefield, BattlefieldPick::Scene(Scene::Skirmish));
        assert_eq!(cfg.player_army, Army::Us);
        assert_eq!(cfg.enemy_army, Army::Fr);
        assert_ne!(cfg.player_army, cfg.enemy_army, "the default reads as a two-army fight");
        assert_eq!(cfg.difficulty, Difficulty::Regular);
        assert_eq!(
            Difficulty::Regular.scenario_modifiers(),
            gonedark_core::mission_tuning::ScenarioModifiers::default(),
            "Regular must stay the neutral (no-op) baseline the default deploy relies on"
        );
    }

    #[test]
    fn next_army_wraps_the_selectable_rosters_and_rejects_neutral() {
        // The cycler walks every selectable roster exactly once, then wraps.
        let mut a = SELECTABLE_ARMIES[0];
        let mut seen = Vec::new();
        for _ in 0..SELECTABLE_ARMIES.len() {
            seen.push(a);
            a = next_army(a);
        }
        for army in SELECTABLE_ARMIES {
            assert!(seen.contains(&army), "{army:?} must appear in the cycle");
        }
        assert_eq!(a, SELECTABLE_ARMIES[0], "the cycle wraps back to the start");
        // The non-aligned Neutral is never a pick; a (defensive) Neutral input lands on the first
        // selectable roster rather than guessing.
        assert_eq!(next_army(Army::Neutral), SELECTABLE_ARMIES[0]);
    }

    #[test]
    fn every_battlefield_resolves_to_a_real_deploy() {
        // The launch decision is total over the unified battlefield list (D102): a scene tile
        // resolves through the engine-tested `Battlefield::scene` seam, a map tile carries its
        // library id — and every entry's index round-trips to its own pick, never a neighbour's.
        for (i, entry) in BATTLEFIELDS.iter().enumerate() {
            let state = SkirmishSetupState { battlefield: i, ..Default::default() };
            let cfg = resolve_skirmish_config(&state);
            match entry.kind {
                BattlefieldKind::Scene(_) => {
                    assert_eq!(
                        cfg.battlefield,
                        BattlefieldPick::Scene(entry.scene().unwrap()),
                        "battlefield {i}"
                    );
                }
                BattlefieldKind::LibraryMap(id) => {
                    assert_eq!(cfg.battlefield, BattlefieldPick::LibraryMap(id), "battlefield {i}");
                }
            }
        }
        // The list genuinely spans both kinds — the library seam is live, not vestigial.
        assert!(BATTLEFIELDS.iter().any(|b| matches!(b.kind, BattlefieldKind::LibraryMap(_))));
    }

    #[test]
    fn out_of_range_battlefield_clamps_to_the_first_and_never_panics() {
        // A stale/foreign index (impossible from the tiles, defensive) snaps to the first
        // battlefield — the standing skirmish — both in the clamp and through the full resolution.
        assert_eq!(clamp_battlefield(BATTLEFIELDS.len()), 0);
        assert_eq!(clamp_battlefield(usize::MAX), 0);
        let state = SkirmishSetupState { battlefield: usize::MAX, ..Default::default() };
        assert_eq!(
            resolve_skirmish_config(&state).battlefield,
            BattlefieldPick::Scene(Scene::Skirmish)
        );
    }

    #[test]
    fn skirmish_config_edits_apply_in_place_and_stay() {
        let mut state = SkirmishSetupState::default();

        // Battlefield: an in-range pick lands; an out-of-range one clamps to the first.
        assert_eq!(
            apply_skirmish_setup_action(SkirmishSetupAction::ChooseBattlefield(1), &mut state),
            SkirmishSetupStep::Stay
        );
        assert_eq!(state.battlefield, 1);
        apply_skirmish_setup_action(
            SkirmishSetupAction::ChooseBattlefield(usize::MAX),
            &mut state,
        );
        assert_eq!(state.battlefield, 0);

        // The three cyclers advance their own field (and only it) in place.
        assert_eq!(
            apply_skirmish_setup_action(SkirmishSetupAction::CyclePlayerArmy, &mut state),
            SkirmishSetupStep::Stay
        );
        assert_eq!(state.player_army, Army::Fr);
        assert_eq!(state.enemy_army, Army::Fr, "cycling the player side never edits the enemy");
        assert_eq!(
            apply_skirmish_setup_action(SkirmishSetupAction::CycleEnemyArmy, &mut state),
            SkirmishSetupStep::Stay
        );
        assert_eq!(state.enemy_army, Army::Us);
        assert_eq!(
            apply_skirmish_setup_action(SkirmishSetupAction::CycleDifficulty, &mut state),
            SkirmishSetupStep::Stay
        );
        assert_eq!(state.difficulty, Difficulty::Veteran, "Regular cycles to Veteran");
    }

    #[test]
    fn skirmish_deploy_carries_the_configured_match_and_back_leaves_it_alone() {
        // A fully hand-configured setup: Deploy resolves it verbatim (a mirror FR-vs-FR match is a
        // legitimate pick), and Back is a pure transition.
        let mut state = SkirmishSetupState {
            battlefield: 1,
            player_army: Army::Fr,
            enemy_army: Army::Fr,
            difficulty: Difficulty::Elite,
        };
        let step = apply_skirmish_setup_action(SkirmishSetupAction::Deploy, &mut state);
        assert_eq!(
            step,
            SkirmishSetupStep::Deploy(SkirmishConfig {
                battlefield: BattlefieldPick::Scene(BATTLEFIELDS[1].scene().unwrap()),
                player_army: Army::Fr,
                enemy_army: Army::Fr,
                difficulty: Difficulty::Elite,
            })
        );
        let before = state;
        assert_eq!(
            apply_skirmish_setup_action(SkirmishSetupAction::Back, &mut state),
            SkirmishSetupStep::Back
        );
        assert_eq!(state, before, "Back never edits the configuration");
    }

    // ---- The skirmish map card (modes.md §3 picker preview, shipped v1) --------------------------

    use gonedark_engine::map_card::{MapCard, COVER_KINDS};
    use gonedark_engine::map_format::MapSpec;
    use gonedark_engine::map_library::library_spec;

    #[test]
    fn sketch_cell_mapping_covers_the_panel_exactly() {
        // The 128-cell grid tiles the panel edge to edge: cell (0,0) starts at the panel's min
        // corner, the last cell ends at its max, and each cell is an even 1/128 slice.
        let panel = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(256.0, 256.0));
        let first = cell_sketch_rect(panel, 0, 0);
        assert_eq!(first.min, panel.min);
        assert_eq!(first.size(), egui::vec2(2.0, 2.0));
        let last = cell_sketch_rect(panel, 127, 127);
        assert_eq!(last.max, panel.max);
    }

    #[test]
    fn sketch_centre_cell_starts_at_the_panel_midpoint() {
        // Cell (64, 64) — the playfield centre cell — begins exactly at the panel's midpoint
        // (the grid splits at GRID/2, same as the card's quadrants).
        let panel = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(256.0, 256.0));
        assert_eq!(cell_sketch_rect(panel, 64, 64).min, egui::pos2(128.0, 128.0));
    }

    #[test]
    fn sketch_mapping_scales_each_axis_of_a_non_square_panel() {
        // Each axis scales independently — a 128x64 panel offset from the origin stretches the
        // field, it never letterboxes or clips.
        let panel = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(138.0, 84.0));
        let r = cell_sketch_rect(panel, 1, 1);
        assert_eq!(r.min, egui::pos2(11.0, 20.5));
        assert_eq!(r.size(), egui::vec2(1.0, 0.5));
        assert_eq!(cell_sketch_rect(panel, 127, 127).max, panel.max);
    }

    #[test]
    fn prop_kind_swatches_are_pairwise_distinct() {
        // Five kinds, five distinguishable swatches — the sketch's colour key is only honest if
        // no two kinds share a colour.
        for (i, &a) in COVER_KINDS.iter().enumerate() {
            for &b in &COVER_KINDS[i + 1..] {
                assert_ne!(prop_kind_color(a), prop_kind_color(b), "{a:?} vs {b:?}");
            }
            assert!(!prop_kind_label(a).is_empty());
            assert!(prop_kind_label(a).is_ascii());
        }
    }

    #[test]
    fn zone_outlines_wear_the_faction_hues() {
        // The player/enemy deploy zones read in the same faction blue/red as everywhere else in
        // the game; any other authored zone name falls back to ash.
        assert_eq!(zone_outline_color("player"), rgb8(gonedark_render::theme::PLAYER));
        assert_eq!(zone_outline_color("enemy"), rgb8(gonedark_render::theme::ENEMY));
        assert_eq!(zone_outline_color("flank"), ASH);
        assert_ne!(zone_outline_color("player"), zone_outline_color("enemy"));
    }

    #[test]
    fn crossroads_metric_lines_read_the_pinned_card() {
        // The full formatted card for the one shipped library map — pinned verbatim (the values
        // are the engine's pinned crossroads card; the Kotlin twin mirrors the same numbers).
        let spec = library_spec("crossroads").expect("shipped library map");
        let card = MapCard::derive(&spec);
        assert_eq!(
            map_card_metric_lines(&card),
            vec![
                "Control points: 3",
                "Cover: 6 props on 6 cells -- 0/1000 of the field",
                "Cover by quadrant (cells): 1 / 2 / 1 / 2",
                "Spawn zones: 2 -- player 7x9, enemy 7x9",
            ]
        );
    }

    #[test]
    fn metric_lines_handle_a_zoneless_card() {
        // A minimal map (terrain only) still formats a full card — the zone line says so
        // explicitly rather than trailing an empty list.
        let spec = MapSpec::load("MapSpec(terrain: 0)").unwrap();
        let lines = map_card_metric_lines(&MapCard::derive(&spec));
        assert_eq!(lines[0], "Control points: 0");
        assert_eq!(lines[1], "Cover: 0 props on 0 cells -- 0/1000 of the field");
        assert_eq!(lines[3], "Spawn zones: none");
    }

    #[test]
    fn reseed_player_army_follows_the_identity_pick_and_bumps_a_colliding_enemy() {
        // Opening the screen re-seeds the player side from the persisted army-select pick…
        let mut state = SkirmishSetupState::default();
        state.reseed_player_army(Army::Fr);
        assert_eq!(state.player_army, Army::Fr);
        // …and when that collides with the current enemy pick, the enemy bumps to the opposing
        // roster so the default reads as a real two-army fight (FR was the default enemy here).
        assert_eq!(state.enemy_army, Army::Us);

        // No collision → the enemy pick is left exactly as the player configured it.
        let mut state = SkirmishSetupState {
            enemy_army: Army::Us,
            ..Default::default()
        };
        state.reseed_player_army(Army::Fr);
        assert_eq!(state.player_army, Army::Fr);
        assert_eq!(state.enemy_army, Army::Us, "a non-colliding enemy pick is preserved");

        // Reseeding is idempotent for the already-consistent default — opening the screen twice
        // in a row changes nothing (the bump fires only on a genuine collision).
        let mut state = SkirmishSetupState::default();
        state.reseed_player_army(Army::Us);
        assert_eq!(state, SkirmishSetupState::default());
    }

    // ---- pvp: the staging door (`modes.md` §1/§5) ----------------------------------------------

    #[test]
    fn no_pvp_queue_is_joinable_before_the_net_layer() {
        // The staging screen's honesty rule as a tested invariant: with no Phase 3 session
        // transport, nothing on the PvP door may present as joinable — every queue row routes
        // through this gate exactly as mission tiles route through `playable_node`. When the
        // custom lobby lands, this test is what changes (per-queue), not the screen's structure.
        for queue in PVP_QUEUES {
            assert!(
                !queue_joinable(queue),
                "queue {:?} reads joinable with no net layer to back it",
                queue.id
            );
        }
    }

    #[test]
    fn pvp_queues_are_the_three_doors_in_build_order() {
        // The table mirrors `modes.md` §5: the custom lobby is first (the first real PvP surface —
        // the smallest thing that puts two humans in one lockstep match), then quick, then ranked.
        assert_eq!(PVP_QUEUES.len(), 3);
        assert_eq!(PVP_QUEUES[0].id, "custom");
        assert_eq!(PVP_QUEUES[1].id, "quick");
        assert_eq!(PVP_QUEUES[2].id, "ranked");
    }

    #[test]
    fn pvp_queue_table_is_distinct_ascii_and_complete() {
        // The mode-table hygiene rule (`shell_modes`' ASCII/uniqueness guard, applied here): every
        // field renders in egui's default font and every tile is uniquely keyed.
        for q in PVP_QUEUES {
            assert!(q.id.is_ascii() && q.name.is_ascii() && q.blurb.is_ascii() && q.status.is_ascii());
            assert!(!q.id.is_empty() && !q.name.is_empty() && !q.blurb.is_empty() && !q.status.is_empty());
        }
        for (i, a) in PVP_QUEUES.iter().enumerate() {
            for b in &PVP_QUEUES[i + 1..] {
                assert_ne!(a.id, b.id, "duplicate queue id {:?}", a.id);
            }
        }
    }
