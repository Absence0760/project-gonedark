//! The **skirmish match-setup** screen (`docs/modes.md` §3 — the free-configuration PvE match,
//! build-order step 1) — pure seams (unit-tested) plus the immediate-mode egui glue.
//!
//! The title's SKIRMISH door lands here instead of the bare mode picker: the player configures the
//! whole match — **battlefield** (the standing battles + the map library, [`BATTLEFIELDS`]), **both
//! armies** (US/FR for the player *and* the enemy commander), and the **opponent tier** (the D83
//! campaign [`Difficulty`], which carries both combat axes: the honest commander band + the
//! scenario situation modifiers) — then DEPLOYs straight into the match with the persisted gunsmith
//! loadout (D81). Everything here is **host-side match-setup config**, never sim state: the picks
//! reach the sim only through the landed pre-tick seams (`Game::new_scene_with_loadout`,
//! `Game::select_army`, `Game::apply_campaign_tuning`), so they are deterministic setup input, not
//! a checksum surface (invariants #1/#7).
//!
//! The battlefield list is the unified [`BATTLEFIELDS`] table (D102): the standing battle scenes
//! plus the embedded map library (`engine::map_library` — the `modes.md` §3 D34 manifest listing).
//! A scene tile deploys through `Scene::parse` exactly as before; a library-map tile carries its
//! map id into the launch, and the host boots it through `Game::new_map_skirmish_with_loadout`.

use crate::shell::army::{army_label, SELECTABLE_ARMIES};
use crate::shell::briefing::{difficulty_label, next_difficulty};
use crate::shell::theme::*;
use crate::shell::widgets::*;
use gonedark_core::campaign::Difficulty;
use gonedark_core::components::Army;
use gonedark_engine::map_library::{BattlefieldKind, BATTLEFIELDS};
use gonedark_engine::Scene;

/// Host-side skirmish setup state — the free-pick match configuration (`modes.md` §3). Session
/// state like the briefing's replay-tier selector, not a persisted pref: the *identity* army pick
/// persists on [`ArmySelectState`](crate::shell::army::ArmySelectState) and re-seeds
/// [`player_army`](Self::player_army) whenever the screen opens ([`Self::reseed_player_army`]);
/// the per-match overrides here live for the session.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct SkirmishSetupState {
    /// The picked battlefield as an index into [`BATTLEFIELDS`] (the standing battles + the map
    /// library, D102). Always kept in range by [`clamp_battlefield`]; resolved to a
    /// [`BattlefieldPick`] at Deploy.
    pub battlefield: usize,
    /// The army the player fields this match. Seeded from the persisted army-select pick on screen
    /// open; cycling it here is a per-match override, never a write-back to the identity pick.
    pub player_army: Army,
    /// The army the enemy commander fields (`modes.md` §3 step 2: "Pick the enemy's army too").
    pub enemy_army: Army,
    /// The opponent tier — the D83 campaign [`Difficulty`] whose `combat_tuning` carries both axes
    /// (the 3-tier honest-commander band + the scenario situation modifiers). Difficulty reshapes
    /// the *situation*, never the balance numbers (D30/D83).
    pub difficulty: Difficulty,
}

impl Default for SkirmishSetupState {
    /// The shipped default skirmish: the first battlefield (the open two-base skirmish), US vs FR,
    /// at **Regular** — the neutral D83 tier whose tuning is a byte-identical no-op, so the default
    /// deploy reproduces the pre-setup-screen match exactly.
    fn default() -> Self {
        SkirmishSetupState {
            battlefield: 0,
            player_army: Army::Us,
            enemy_army: Army::Fr,
            difficulty: Difficulty::Regular,
        }
    }
}

impl SkirmishSetupState {
    /// Re-seed the player side from the persisted identity pick (the army-select screen's state) —
    /// called by the host whenever the screen opens, so the setup always starts from the army the
    /// player has declared they field. If that collides with the current enemy pick, the enemy is
    /// bumped to the opposing roster so the default reads as a real two-army fight (a mirror match
    /// stays one click away, never the accidental default). Pure — unit-tested.
    pub fn reseed_player_army(&mut self, persisted: Army) {
        self.player_army = persisted;
        if self.enemy_army == persisted {
            self.enemy_army = next_army(persisted);
        }
    }
}

/// How a configured skirmish's battlefield boots — the resolved form of a [`BATTLEFIELDS`] entry's
/// kind: a standing battle [`Scene`], or a library map by id (booted through the engine's
/// `Game::new_map_skirmish_with_loadout`). `&'static str` keeps the config `Copy` for REMATCH.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BattlefieldPick {
    /// A code-seeded standing battle scene.
    Scene(Scene),
    /// An authored library map (`engine::map_library::MAP_LIBRARY` id).
    LibraryMap(&'static str),
}

/// The launch configuration a skirmish DEPLOY resolves to — everything the host needs to boot the
/// match through the landed seams: the battlefield pick, both army picks, and the D83 tier. Pure
/// data; carried on the `LaunchSkirmish` host transition and remembered across the match for
/// REMATCH.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct SkirmishConfig {
    /// The battlefield to boot (scene or library map), resolved at Deploy.
    pub battlefield: BattlefieldPick,
    /// The army the player fields (`Game::select_army(Faction::Player, ..)`).
    pub player_army: Army,
    /// The army the enemy commander fields (`Game::select_army(Faction::Enemy, ..)`).
    pub enemy_army: Army,
    /// The opponent tier (`Game::apply_campaign_tuning` — commander band + situation modifiers).
    pub difficulty: Difficulty,
}

/// An action the skirmish setup screen can emit in a frame. The four config edits stay on-screen;
/// `Deploy`/`Back` are screen transitions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SkirmishSetupAction {
    /// Pick the battlefield at this [`BATTLEFIELDS`] index (an in-place edit).
    ChooseBattlefield(usize),
    /// Advance the player's army to the next selectable roster (wrapping).
    CyclePlayerArmy,
    /// Advance the enemy commander's army to the next selectable roster (wrapping).
    CycleEnemyArmy,
    /// Advance the opponent tier to the next D83 difficulty (wrapping).
    CycleDifficulty,
    /// Deploy the configured match.
    Deploy,
    /// Return to the title screen.
    Back,
}

/// The screen-level outcome of a [`SkirmishSetupAction`] once applied — what the host run loop
/// switches on, mirroring [`BriefingOutcome`](crate::shell::briefing::BriefingOutcome).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SkirmishSetupStep {
    /// Stay on the setup screen (a config edit, or nothing this frame).
    Stay,
    /// Boot the configured match.
    Deploy(SkirmishConfig),
    /// Return to the title screen.
    Back,
}

/// The next selectable army, wrapping through [`SELECTABLE_ARMIES`] (`US -> FR -> US`). A
/// non-selectable input (the non-aligned [`Army::Neutral`], never a player pick) lands on the first
/// selectable roster rather than guessing. Pure — the army cyclers' one decision, unit-tested.
pub(crate) fn next_army(a: Army) -> Army {
    match SELECTABLE_ARMIES.iter().position(|&x| x == a) {
        Some(i) => SELECTABLE_ARMIES[(i + 1) % SELECTABLE_ARMIES.len()],
        None => SELECTABLE_ARMIES[0],
    }
}

/// Clamp a battlefield index into [`BATTLEFIELDS`] range (an out-of-range pick — impossible from
/// the tiles, defensive against a stale/foreign value — snaps to the first battlefield, never
/// panics). Pure — unit-tested.
pub(crate) fn clamp_battlefield(i: usize) -> usize {
    if i < BATTLEFIELDS.len() {
        i
    } else {
        0
    }
}

/// Resolve the current setup state into the [`SkirmishConfig`] a DEPLOY launches. The battlefield
/// index is clamped and resolved by kind: a scene entry through the `engine`-tested
/// `Battlefield::scene` seam (an un-parseable token — forbidden by the library test — defensively
/// falls back to the standing [`Scene::Skirmish`]), a library-map entry to its id (the host boots
/// it and holds the matching fallback). Total — a deploy can never resolve to nothing. Pure — the
/// screen's launch decision, unit-tested.
pub(crate) fn resolve_skirmish_config(state: &SkirmishSetupState) -> SkirmishConfig {
    let entry = &BATTLEFIELDS[clamp_battlefield(state.battlefield)];
    let battlefield = match entry.kind {
        BattlefieldKind::Scene(_) => {
            BattlefieldPick::Scene(entry.scene().unwrap_or(Scene::Skirmish))
        }
        BattlefieldKind::LibraryMap(id) => BattlefieldPick::LibraryMap(id),
    };
    SkirmishConfig {
        battlefield,
        player_army: state.player_army,
        enemy_army: state.enemy_army,
        difficulty: state.difficulty,
    }
}

/// Apply a [`SkirmishSetupAction`] to the setup state in place and report the resulting screen
/// step. Config edits cycle/select and stay; `Deploy` resolves the current state through
/// [`resolve_skirmish_config`] and carries it out. Pure (no egui/window) — the skirmish setup's
/// testable decision seam, mirroring [`apply_briefing_action`](crate::shell::briefing::apply_briefing_action).
pub(crate) fn apply_skirmish_setup_action(
    action: SkirmishSetupAction,
    state: &mut SkirmishSetupState,
) -> SkirmishSetupStep {
    match action {
        SkirmishSetupAction::ChooseBattlefield(i) => {
            state.battlefield = clamp_battlefield(i);
            SkirmishSetupStep::Stay
        }
        SkirmishSetupAction::CyclePlayerArmy => {
            state.player_army = next_army(state.player_army);
            SkirmishSetupStep::Stay
        }
        SkirmishSetupAction::CycleEnemyArmy => {
            state.enemy_army = next_army(state.enemy_army);
            SkirmishSetupStep::Stay
        }
        SkirmishSetupAction::CycleDifficulty => {
            state.difficulty = next_difficulty(state.difficulty);
            SkirmishSetupStep::Stay
        }
        SkirmishSetupAction::Deploy => SkirmishSetupStep::Deploy(resolve_skirmish_config(state)),
        SkirmishSetupAction::Back => SkirmishSetupStep::Back,
    }
}

/// One battlefield tile: the battle name over its one-line blurb as a full-width selectable row,
/// with the current pick reading amber plus a SELECTED chip (legible beyond colour alone — the
/// army-card convention). Clicking any tile emits [`SkirmishSetupAction::ChooseBattlefield`]. Glue
/// (needs a live `Ui`) — the selection/launch decisions are the pure seams above. ASCII only.
fn battlefield_tile(
    ui: &mut egui::Ui,
    index: usize,
    selected: bool,
) -> Option<SkirmishSetupAction> {
    use egui::RichText;
    let entry = &BATTLEFIELDS[index];
    let name_color = if selected { AMBER } else { BONE };
    let clicked = selectable_row(ui, ("skirmish_bf", entry.id), true, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(entry.name.to_uppercase())
                    .color(name_color)
                    .size(TYPE_SUBHEAD)
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if selected {
                    status_chip(ui, "SELECTED", AMBER);
                }
                // A library-map tile wears its provenance (the D102 manifest entries beside the
                // standing battles) — informational, muted, never a second click target.
                if matches!(entry.kind, BattlefieldKind::LibraryMap(_)) {
                    status_chip(ui, "MAP LIBRARY", MUTED);
                }
            });
        });
        ui.label(RichText::new(entry.blurb).color(ASH).size(TYPE_CAPTION));
    });
    clicked.then_some(SkirmishSetupAction::ChooseBattlefield(index))
}

/// A two-column setup row — label flush-left, a cycling [`value_chip`] flush-right (the briefing's
/// difficulty-cycler layout, shared by the three cyclers here). Returns whether the chip was
/// clicked. Glue.
fn cycle_row(ui: &mut egui::Ui, label: &str, value: &str) -> bool {
    use egui::RichText;
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(BONE).size(TYPE_BODY));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            clicked = value_chip(ui, value, 200.0);
        });
    });
    clicked
}

/// The immediate-mode skirmish match-setup screen (`modes.md` §3): battlefield tiles, the two army
/// cyclers, the opponent-tier cycler with the 4-pip ladder, then DEPLOY / BACK — in the order a
/// player thinks (map, army, opponent; the loadout pointer notes the gunsmith carries in). Reads
/// the host-side [`SkirmishSetupState`]; every control routes through the pure
/// [`apply_skirmish_setup_action`] seam at the host. Glue.
pub(crate) fn skirmish_setup_ui(
    ui: &mut egui::Ui,
    state: &SkirmishSetupState,
) -> Option<SkirmishSetupAction> {
    use egui::RichText;
    let mut action = None;

    over_backdrop_screen(ui, "skirmish_setup", |ui| {
        screen_banner(ui, "SKIRMISH", 130.0);
        ui.label(
            RichText::new(
                "Pick your battle: the sandbox match against the honest enemy commander. No \
                 gating, no stakes -- rehearse a battlefield, an army, or a tier.",
            )
            .color(ASH)
            .size(TYPE_BODY),
        );
        ui.add_space(12.0);

        section_label(ui, "BATTLEFIELD");
        let selected_bf = clamp_battlefield(state.battlefield);
        for i in 0..BATTLEFIELDS.len() {
            if let Some(act) = battlefield_tile(ui, i, i == selected_bf) {
                action = Some(act);
            }
            if i + 1 < BATTLEFIELDS.len() {
                ui.add_space(8.0);
            }
        }
        ui.add_space(10.0);

        section_label(ui, "FORCES");
        card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            if cycle_row(ui, "Your army", army_label(state.player_army)) {
                action = Some(SkirmishSetupAction::CyclePlayerArmy);
            }
            ui.add_space(6.0);
            if cycle_row(ui, "Enemy army", army_label(state.enemy_army)) {
                action = Some(SkirmishSetupAction::CycleEnemyArmy);
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Asymmetry is of flavour and feel, never of power. Your gunsmith loadout \
                     carries in -- edit it under Settings.",
                )
                .color(MUTED)
                .size(TYPE_CAPTION),
            );
        });
        ui.add_space(10.0);

        section_label(ui, "OPPONENT");
        card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            // The D83 tier — commander band + situation modifiers, exactly the campaign replay
            // vocabulary (a harder tier is a better commander, never an omniscient one).
            if cycle_row(ui, "Difficulty", difficulty_label(state.difficulty)) {
                action = Some(SkirmishSetupAction::CycleDifficulty);
            }
            ui.add_space(6.0);
            // The briefing's 4-pip ladder (Recruit -> Elite), so the cycle reads as "n of 4".
            ui.horizontal(|ui| {
                for d in Difficulty::ALL {
                    let filled = d <= state.difficulty;
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(30.0, 4.0), egui::Sense::hover());
                    ui.painter().rect_filled(
                        rect,
                        egui::CornerRadius::same(2),
                        if filled { AMBER } else { RIM },
                    );
                    ui.add_space(4.0);
                }
            });
            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "Difficulty reshapes the situation -- a sharper commander, a faster enemy \
                     reinforcement drip -- never the balance numbers.",
                )
                .color(MUTED)
                .size(TYPE_CAPTION),
            );
        });

        ui.add_space(FOOTER_GAP);
        if footer_button(ui, "DEPLOY", Emphasis::Primary) {
            action = Some(SkirmishSetupAction::Deploy);
        }
        ui.add_space(10.0);
        if footer_button(ui, "BACK", Emphasis::Tertiary) {
            action = Some(SkirmishSetupAction::Back);
        }
    });

    action
}
