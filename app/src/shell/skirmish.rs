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
//!
//! A selected **library map** additionally shows its **map card** (`modes.md` §3's picker
//! preview, shipped v1): a painter-primitive sketch of the `MapSpec` — spawn zones outlined in
//! the faction hues, cover props as kind-coloured cells, control points as amber markers — beside
//! the engine-derived [`MapCard`] metrics (cover density in permille, counts, zones). The baker's
//! lint PNG / balance metrics stay deferred behind D77; everything here re-derives from the spec
//! the library already embeds. A scene tile shows the one-line no-card note instead. The
//! cell→screen mapping ([`cell_sketch_rect`]), colour picks, and metric formatting are pure
//! seams (unit-tested); only the painter calls are glue.

use crate::shell::army::{army_label, SELECTABLE_ARMIES};
use crate::shell::briefing::{difficulty_label, next_difficulty};
use crate::shell::theme::*;
use crate::shell::widgets::*;
use gonedark_core::campaign::Difficulty;
use gonedark_core::components::Army;
use gonedark_core::flow_field::GRID;
use gonedark_engine::map_card::{MapCard, COVER_KINDS};
use gonedark_engine::map_format::{CoverPropKind, MapSpec};
use gonedark_engine::map_library::{library_spec, BattlefieldKind, BATTLEFIELDS, ENEMY_ZONE, PLAYER_ZONE};
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

// ---- The map card (modes.md §3 picker preview, shipped v1) --------------------------------------

/// The screen rect of one playfield cell inside the map-card sketch `panel`: a linear map of the
/// `GRID`×`GRID` cell space onto the panel (x right, y down — cell `(0, 0)` is the panel's
/// top-left corner; the card makes no compass claim). Each axis scales independently, so a
/// non-square panel simply stretches the field. Pure (`egui::Rect` is plain data, no `Ui`) — the
/// sketch's one piece of math, unit-tested: corners, the centre cell, a non-square panel.
pub(crate) fn cell_sketch_rect(panel: egui::Rect, x: i32, y: i32) -> egui::Rect {
    let g = GRID as f32;
    let cell = egui::vec2(panel.width() / g, panel.height() / g);
    egui::Rect::from_min_size(
        egui::pos2(panel.min.x + x as f32 * cell.x, panel.min.y + y as f32 * cell.y),
        cell,
    )
}

/// The sketch's cover-kind swatch — every pick an [`rgb8`] of an **existing renderer hue** (never
/// a new hex, the theme.rs rule), so the sketch shares the shell identity: supply amber for the
/// crate, territory green for the tree, the neutral stone grey for the rock, bone for the built
/// barricade, and the warn orange for the turret hard point. Pairwise distinct (unit-tested).
pub(crate) fn prop_kind_color(kind: CoverPropKind) -> egui::Color32 {
    use gonedark_render::theme as rt;
    match kind {
        CoverPropKind::Crate => rgb8(rt::DATA_RESOURCE),
        CoverPropKind::Tree => rgb8(rt::DATA_TERRITORY),
        CoverPropKind::Rock => rgb8(rt::NEUTRAL),
        CoverPropKind::Barricade => BONE,
        CoverPropKind::Turret => rgb8(rt::ALERT_WARN),
    }
}

/// The sketch legend's kind label. ASCII, uppercase — the chip convention.
pub(crate) fn prop_kind_label(kind: CoverPropKind) -> &'static str {
    match kind {
        CoverPropKind::Crate => "CRATE",
        CoverPropKind::Tree => "TREE",
        CoverPropKind::Rock => "ROCK",
        CoverPropKind::Barricade => "BARRICADE",
        CoverPropKind::Turret => "TURRET",
    }
}

/// A spawn zone's outline colour: the renderer's faction hues for the `player`/`enemy` deploy
/// zones every library map carries (blue = yours, everywhere in the game), ash for any other
/// authored name.
pub(crate) fn zone_outline_color(name: &str) -> egui::Color32 {
    use gonedark_render::theme as rt;
    if name == PLAYER_ZONE {
        rgb8(rt::PLAYER)
    } else if name == ENEMY_ZONE {
        rgb8(rt::ENEMY)
    } else {
        ASH
    }
}

/// Format the [`MapCard`] metrics as the card's caption lines — control points, cover (props,
/// occupied cells, density as `n/1000` of the field), the per-quadrant cell breakdown (the
/// asymmetry read), and the spawn zones with their cell extents. Pure formatting, ASCII only —
/// unit-tested against the pinned crossroads card.
pub(crate) fn map_card_metric_lines(card: &MapCard) -> Vec<String> {
    let mut lines = vec![
        format!("Control points: {}", card.control_points),
        format!(
            "Cover: {} props on {} cells -- {}/1000 of the field",
            card.prop_counts.iter().sum::<u32>(),
            card.covered_cells,
            card.cover_permille
        ),
        format!(
            "Cover by quadrant (cells): {}",
            card.quadrant_cells.iter().map(u32::to_string).collect::<Vec<_>>().join(" / ")
        ),
    ];
    if card.spawn_zones.is_empty() {
        lines.push("Spawn zones: none".to_owned());
    } else {
        let zones = card
            .spawn_zones
            .iter()
            .map(|z| format!("{} {}x{}", z.name, z.hi.0 - z.lo.0 + 1, z.hi.1 - z.lo.1 + 1))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("Spawn zones: {} -- {}", card.spawn_zones.len(), zones));
    }
    lines
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

/// The sketch's side in points: 128 cells at ~1.3 px each — a schematic, not a minimap. Single-cell
/// marks are inflated a pixel at draw time so they stay visible at this scale.
const SKETCH_SIDE: f32 = 168.0;

/// Draw the map-card sketch: the grid-bounds rect (ink ground, RIM hairline), spawn zones as
/// faction-hued outlines, cover props as kind-coloured filled cells, control points as amber
/// markers (the lone signal accent). Zones come off the derived card (sorted extents); props and
/// posts straight off the spec. Glue (needs a live `Ui`) — the mapping/colour decisions are the
/// pure seams above.
fn map_card_sketch(ui: &mut egui::Ui, spec: &MapSpec, card: &MapCard) {
    let (panel, _) =
        ui.allocate_exact_size(egui::vec2(SKETCH_SIDE, SKETCH_SIDE), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(panel, egui::CornerRadius::same(4), INK);
    p.rect_stroke(
        panel,
        egui::CornerRadius::same(4),
        egui::Stroke::new(1.0, RIM),
        egui::StrokeKind::Inside,
    );
    for zone in &card.spawn_zones {
        let rect = cell_sketch_rect(panel, zone.lo.0, zone.lo.1)
            .union(cell_sketch_rect(panel, zone.hi.0, zone.hi.1));
        p.rect_stroke(
            rect,
            egui::CornerRadius::ZERO,
            egui::Stroke::new(1.0, zone_outline_color(&zone.name)),
            egui::StrokeKind::Inside,
        );
    }
    for prop in &spec.cover_props {
        let cell = cell_sketch_rect(panel, prop.cell.x, prop.cell.y).expand(1.0);
        p.rect_filled(cell, egui::CornerRadius::ZERO, prop_kind_color(prop.kind));
    }
    for cp in &spec.control_points {
        p.circle_filled(cell_sketch_rect(panel, cp.x, cp.y).center(), 3.0, AMBER);
    }
}

/// The selected battlefield's map card: for a library map, the sketch beside the derived
/// [`MapCard`] metrics and the cover-kind colour key; for a code-seeded scene (or — defensively,
/// forbidden by the library test — an id that no longer validates), a one-line note. The spec is
/// re-parsed and the card re-derived per frame: the library entries are small embedded consts,
/// so this stays trivially cheap — cache it if the library ever grows past a handful. Glue.
fn map_card_panel(ui: &mut egui::Ui, kind: BattlefieldKind) {
    use egui::RichText;
    card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        let id = match kind {
            BattlefieldKind::Scene(_) => {
                ui.label(
                    RichText::new("Code-seeded scene -- no map card.")
                        .color(MUTED)
                        .size(TYPE_CAPTION),
                );
                return;
            }
            BattlefieldKind::LibraryMap(id) => id,
        };
        let Some(spec) = library_spec(id) else {
            ui.label(RichText::new("Map unavailable.").color(MUTED).size(TYPE_CAPTION));
            return;
        };
        let card = MapCard::derive(&spec);
        ui.horizontal(|ui| {
            map_card_sketch(ui, &spec, &card);
            ui.vertical(|ui| {
                for line in map_card_metric_lines(&card) {
                    ui.label(RichText::new(line).color(ASH).size(TYPE_CAPTION));
                }
                ui.add_space(4.0);
                // The sketch's colour key — only the kinds this map actually fields.
                for kind in COVER_KINDS {
                    let count = card.prop_count(kind);
                    if count == 0 {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        let (swatch, _) = ui
                            .allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().rect_filled(
                            swatch,
                            egui::CornerRadius::same(2),
                            prop_kind_color(kind),
                        );
                        ui.label(
                            RichText::new(format!("{} x{}", prop_kind_label(kind), count))
                                .color(MUTED)
                                .size(TYPE_CAPTION),
                        );
                    });
                }
            });
        });
    });
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

        // The selected battlefield's map card (`modes.md` §3's picker preview, shipped v1):
        // a library map shows its derived sketch + metrics, a scene the one-line note.
        section_label(ui, "MAP CARD");
        map_card_panel(ui, BATTLEFIELDS[selected_bf].kind);
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
