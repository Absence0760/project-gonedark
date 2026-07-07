//! The Profile screen — callsign, faction preference, and the lifetime record.
//!
//! A pure decision seam ([`apply_profile_action`]) plus its immediate-mode glue ([`profile_ui`]),
//! split out of the shell monolith. Presentation only — never touches the sim. Behaviour-preserving.

use crate::shell::theme::*;
use crate::shell::widgets::*;

/// The player's preferred faction (the real-army roster, `docs/factions.md`). A cosmetic/pre-match
/// preference only — it never constrains fairness (the roster is fairness-bounded). Pure data.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum FactionPref {
    #[default]
    UsArmy,
    FrenchArmy,
}

impl FactionPref {
    /// Every faction, in a fixed order (the persisted-ordinal order and the cycle order).
    pub(crate) const ALL: [FactionPref; 2] = [FactionPref::UsArmy, FactionPref::FrenchArmy];

    /// The on-screen label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            FactionPref::UsArmy => "US Army",
            FactionPref::FrenchArmy => "French Army",
        }
    }

    /// The next faction, wrapping — what the cycler advances to.
    pub(crate) fn next(self) -> FactionPref {
        match self {
            FactionPref::UsArmy => FactionPref::FrenchArmy,
            FactionPref::FrenchArmy => FactionPref::UsArmy,
        }
    }

    /// This faction's stable index in [`Self::ALL`] — the persisted ordinal.
    pub(crate) fn index(self) -> usize {
        Self::ALL.iter().position(|&f| f == self).unwrap_or(0)
    }

    /// The faction at persisted index `i`, or the default ([`FactionPref::UsArmy`]) for an
    /// out-of-range ordinal — the tolerant decode side of [`Self::index`].
    pub(crate) fn from_index(i: usize) -> FactionPref {
        Self::ALL.get(i).copied().unwrap_or(FactionPref::UsArmy)
    }
}

/// Host-side player identity / record shown on the Profile screen. Presentation only — never touches
/// the sim. The lifetime record is a real counter the host *will* bump at match end (placeholder
/// zeroes today; the post-match summary is the natural writer). Persists across matches like the
/// loadout.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct ProfileState {
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
pub(crate) const DEFAULT_CALLSIGN: &str = "Commander";
/// Maximum callsign length (chars) — keeps it fitting the field and the in-match nameplate.
pub(crate) const CALLSIGN_MAX: usize = 18;

/// Normalise a raw callsign: trim surrounding whitespace, truncate to [`CALLSIGN_MAX`] characters,
/// and fall back to [`DEFAULT_CALLSIGN`] when the result is empty. Pure — the Profile screen's one bit
/// of real input validation, so it is unit-tested. Char-based truncation (not byte) so a multi-byte
/// name can't be split mid-codepoint.
pub(crate) fn sanitize_callsign(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return DEFAULT_CALLSIGN.to_string();
    }
    trimmed.chars().take(CALLSIGN_MAX).collect()
}

/// Win-rate percentage (`0..=100`), or `None` when no matches have been played (a clean "--" readout
/// instead of a divide-by-zero). Integer math, rounded down. Pure — unit-tested.
pub(crate) fn win_rate_pct(wins: u32, played: u32) -> Option<u32> {
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
pub(crate) enum ProfileAction {
    /// Cycle the preferred faction.
    CycleFaction,
    /// Zero the lifetime record.
    ResetStats,
    /// Return to the title screen (sanitises the callsign on the way out).
    Back,
}

/// The screen-level outcome of a [`ProfileAction`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ProfileStep {
    /// Stay on Profile.
    Stay,
    /// Return to the title screen.
    Back,
}

/// Apply a [`ProfileAction`] to the profile and report the resulting screen step. `Back` sanitises the
/// callsign (so an empty/over-long field commits a clean value). Pure — the Profile decision seam.
pub(crate) fn apply_profile_action(
    action: ProfileAction,
    profile: &mut ProfileState,
) -> ProfileStep {
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

/// The immediate-mode Profile screen: callsign, faction preference, and the lifetime record, centred
/// over the backdrop. The callsign `TextEdit` edits `profile` in place (length-capped to
/// [`CALLSIGN_MAX`]); the discrete controls return a [`ProfileAction`] the pure [`apply_profile_action`]
/// seam resolves (BACK sanitises the callsign). Glue.
pub(crate) fn profile_ui(ui: &mut egui::Ui, profile: &mut ProfileState) -> Option<ProfileAction> {
    use egui::{RichText, TextEdit};
    let mut action = None;

    over_backdrop_screen(ui, "profile", |ui| {
        screen_banner(ui, "PROFILE", 110.0);

        // Left-anchor the identity/record body to one margin (banner + footer stay centred).
        ui.vertical(|ui| {
            section_label(ui, "IDENTITY");
            egui::Grid::new("profile.identity")
                .num_columns(2)
                .min_col_width(96.0)
                .spacing([16.0, 10.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Callsign").color(BONE).size(TYPE_BODY));
                    ui.add(
                        TextEdit::singleline(&mut profile.callsign)
                            .char_limit(CALLSIGN_MAX)
                            .desired_width(f32::INFINITY),
                    );
                    ui.end_row();
                    ui.label(RichText::new("Faction").color(BONE).size(TYPE_BODY));
                    let fw = ui.available_width();
                    if value_chip(ui, profile.faction.label(), fw) {
                        action = Some(ProfileAction::CycleFaction);
                    }
                    ui.end_row();
                });

            section_divider(ui);
            section_label(ui, "RECORD");
            let rate = match win_rate_pct(profile.wins, profile.matches_played) {
                Some(p) => format!("{p}%"),
                None => "--".to_string(),
            };
            // A 3-up stat row: a big amber numeral over a small ash caption per stat, instead of one
            // flat grey sentence — the same numeral/caption relationship the rest of the shell uses.
            let stat_col = (ui.available_width() / 3.0 - 8.0).max(64.0);
            egui::Grid::new("profile.record")
                .num_columns(3)
                .min_col_width(stat_col)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    for (value, caption) in [
                        (profile.matches_played.to_string(), "MATCHES"),
                        (profile.wins.to_string(), "WINS"),
                        (rate.clone(), "WIN RATE"),
                    ] {
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new(value).color(AMBER).size(TYPE_STAT).strong());
                            ui.label(RichText::new(caption).color(ASH).size(TYPE_CAPTION));
                        });
                    }
                    ui.end_row();
                });
        }); // end left-anchored body

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
        if footer_button(ui, "BACK", Emphasis::Secondary) {
            action = Some(ProfileAction::Back);
        }
    });

    action
}
