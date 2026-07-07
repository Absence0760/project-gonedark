//! The **conflict atlas** screen (D104) — the campaign's front door: a navigable 3D earth with a
//! **year scrubber**. Drag to turn the globe, scroll to zoom, scrub the timeline to light up that
//! era's conflicts, click a pin (or NEXT) to select one, ENTER to open its Operations hub. This
//! closes Q28 fork 2 for the desktop: the presentation endstate is the full navigable globe (the
//! hub/briefing keep the settled-globe *backdrop* from D103; Android's hub deliberately keeps the
//! grouped list until its own presentation decision).
//!
//! Pure seams (unit-tested, no GPU) carry every decision: [`AtlasState`] + [`apply_atlas_action`]
//! (drag/zoom/scrub/select/enter), [`year_domain`] + [`conflict_active_in`] (the scrubber's
//! model), [`atlas_pins_for`] (what the globe draws), and [`pick_conflict`] (click → pin, built on
//! the render crate's `project_pin` so picking can never disagree with drawing). The egui glue
//! ([`atlas_ui`]) only reports gestures. Everything here is host presentation over authored
//! campaign data — never the sim (invariants #1/#7).

use crate::shell::mission_select::focused_conflict;
use crate::shell::theme::*;
use crate::shell::widgets::*;
use gonedark_core::campaign::{Campaign, Conflict, NodeId};
use gonedark_render::globe_backdrop::{
    eye_elevation, project_pin, GlobeFlight, GlobePin, GlobeView,
};

/// Radians of globe rotation per logical point of drag at zoom 1 (halves as zoom doubles, so a
/// zoomed-in drag stays fine-grained over the region under the cursor). Shared by the operations
/// screen's look-around drag so both surfaces feel identical under the hand.
pub(crate) const DRAG_SENS: f32 = 0.006;
/// Zoom factor per scroll "line" (egui's scroll unit); >1 = scroll-up zooms in.
const ZOOM_STEP: f32 = 1.10;
/// Click-to-pin pick radius in NDC (fraction of the half-screen) — generous enough for a pin's
/// halo, tight enough that empty-ocean clicks don't select. Shared by the battlefield overview's
/// pin picking (D106) so both pick circles feel identical.
pub(crate) const PICK_RADIUS: f32 = 0.075;

/// The atlas screen's host state: where the player has turned the globe, the scrubbed year, and
/// the selected conflict. Session state (like the skirmish setup) — never persisted, never sim.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct AtlasState {
    /// The navigable view (yaw/pitch/zoom), kept clamped by every edit path.
    pub view: GlobeView,
    /// The scrubbed year — pins whose conflict spans it are "active"; others dim.
    pub year: i16,
    /// The selected conflict (an index into `campaign.conflicts()`), the ENTER target.
    pub selected: usize,
    /// An in-progress camera flight back from a battlefield (D107) — the return leg of the
    /// atlas → battlefield fly-in. While live it drives `view` each frame
    /// ([`tick_atlas_flight`]); any drag/zoom cancels it (the player takes the camera over).
    pub flight: Option<GlobeFlight>,
}

impl AtlasState {
    /// The state the atlas opens on: settled facing the [`focused_conflict`] (the war being
    /// fought), scrubbed to that conflict's opening year, with it selected. Pure — unit-tested.
    pub fn opened(campaign: &Campaign) -> AtlasState {
        let selected = focused_conflict(campaign);
        let (lon, year) = campaign
            .conflicts()
            .get(selected)
            .map_or((0.0, 0), |c| (c.lon_x10 as f32 / 10.0, c.start_year));
        AtlasState {
            view: GlobeView {
                yaw: -lon.to_radians(),
                pitch: 0.0,
                zoom: 1.0,
            },
            year,
            selected,
            flight: None,
        }
    }

    /// [`opened`](Self::opened), but **flown into** from an existing camera (D107): the return
    /// leg of the battlefield fly-in. Starts exactly at `from` (the hub's last backdrop view, so
    /// there is no cut) and flies to the same view a plain `opened` lands on — after the flight
    /// (or a cancelling drag/zoom) the atlas is indistinguishable from a plain open. Pure.
    pub fn opened_from(campaign: &Campaign, from: GlobeView) -> AtlasState {
        let mut state = AtlasState::opened(campaign);
        state.flight = Some(GlobeFlight::new(from, state.view));
        state.view = from;
        state
    }
}

/// Advance the atlas's return flight (D107) by a frame's wall-clock `dt`: while a flight is
/// live it owns `view`; on landing it is dropped, handing the (now-settled) camera back to the
/// drag/zoom paths. Pure — the per-frame decision, unit-tested; the run loop is the glue.
pub(crate) fn tick_atlas_flight(state: &mut AtlasState, dt: f32) {
    if let Some(flight) = &mut state.flight {
        state.view = flight.step(dt);
        if flight.done() {
            state.flight = None;
        }
    }
}

/// A gesture/control the atlas screen reports for one frame. Navigation edits stay on-screen;
/// `Enter`/`Back` are screen transitions.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum AtlasAction {
    /// Drag by `(dx, dy)` logical points — turns the globe (yaw/pitch).
    Drag(f32, f32),
    /// Scroll by `lines` — zooms the camera.
    Zoom(f32),
    /// Scrub the timeline to a year.
    SetYear(i16),
    /// Select the conflict at this index (a pin click, or NEXT cycling).
    SelectConflict(usize),
    /// Open the selected conflict's Operations hub.
    Enter,
    /// Return to the title.
    Back,
}

/// The screen-level outcome of an [`AtlasAction`] once applied — what the host run loop switches
/// on, mirroring the briefing/skirmish `*Step` convention.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum AtlasStep {
    /// Stay on the atlas (a navigation/selection edit, or nothing this frame).
    Stay,
    /// Open the Operations hub for the selected conflict index.
    Enter(usize),
    /// Return to the title.
    Back,
}

/// Apply an [`AtlasAction`] to the state in place and report the resulting step. Every navigation
/// edit funnels through [`GlobeView::clamped`] so no input path can flip the globe or escape the
/// zoom bounds; a drag's angular rate scales down with zoom so a zoomed-in drag stays precise.
/// Pure — the atlas's testable decision seam.
pub(crate) fn apply_atlas_action(
    action: AtlasAction,
    state: &mut AtlasState,
    campaign: &Campaign,
) -> AtlasStep {
    match action {
        AtlasAction::Drag(dx, dy) => {
            // A camera gesture cancels any in-progress return flight (D107): the player's hand
            // always wins over the automatic camera, instantly.
            state.flight = None;
            let s = DRAG_SENS / state.view.zoom;
            // Surface-following pan: the longitude delta grows by 1/cos(center latitude), so
            // the terrain under the cursor tracks the hand at every latitude and zoom. The old
            // raw `yaw += dx*s` rotated the whole world instead: at a zoomed-in 60°N view the
            // ground slid past the cursor at half the hand's rate. Pitch stays linear.
            let corr = lon_drag_correction(state.view);
            state.view = GlobeView {
                // Dragging right pulls the visible face right (the globe turns with the hand);
                // dragging down tips the northern hemisphere toward the viewer.
                yaw: state.view.yaw + dx * s * corr,
                pitch: state.view.pitch + dy * s,
                zoom: state.view.zoom,
            }
            .clamped();
            AtlasStep::Stay
        }
        AtlasAction::Zoom(lines) => {
            // Same player-takeover rule as Drag (D107).
            state.flight = None;
            state.view = GlobeView {
                zoom: state.view.zoom * ZOOM_STEP.powf(lines),
                ..state.view
            }
            .clamped();
            AtlasStep::Stay
        }
        AtlasAction::SetYear(year) => {
            let (lo, hi) = year_domain(campaign);
            state.year = year.clamp(lo, hi);
            AtlasStep::Stay
        }
        AtlasAction::SelectConflict(i) => {
            if i < campaign.conflicts().len() {
                state.selected = i;
            }
            AtlasStep::Stay
        }
        AtlasAction::Enter => {
            // Defensive re-clamp (a stale index can't outlive the conflict list), written back so
            // the state and the emitted step can never disagree.
            state.selected = state
                .selected
                .min(campaign.conflicts().len().saturating_sub(1));
            AtlasStep::Enter(state.selected)
        }
        AtlasAction::Back => AtlasStep::Back,
    }
}

/// The cap on [`lon_drag_correction`]: near the pitch clamp the view center's latitude passes
/// 70–100° where `1/cos` blows up toward infinity — a capped polar drag pans fast, never
/// explosively.
pub(crate) const LON_DRAG_CORRECTION_CAP: f32 = 3.0;

/// The **surface-following** longitude correction for a drag: one radian of yaw moves `cos(lat)`
/// less east-west ground at latitude `lat`, so the yaw delta scales by `1/cos(center_lat)` to
/// keep the terrain under the cursor glued to the hand. The latitude under the view center is
/// `pitch + eye_elevation(zoom)` — the exact inverse of `GlobeView::over`'s pitch mapping (the
/// render crate exports the term so the two sides can never drift). Capped at
/// [`LON_DRAG_CORRECTION_CAP`] (a near-polar view has `cos → 0`, and can even pass 90° at the
/// pitch clamp — the `.max` also guards the negative-cos side). Pure — unit-tested.
pub(crate) fn lon_drag_correction(view: GlobeView) -> f32 {
    let center_lat = view.pitch + eye_elevation(view.zoom);
    1.0 / center_lat.cos().max(1.0 / LON_DRAG_CORRECTION_CAP)
}

/// The index (into `campaign.conflicts()`) of the conflict a node belongs to, or `None` for an
/// ungrouped node. The CONTINUE-deep-link resync decision (D104): the title's NEXT OPERATION
/// shortcut opens a briefing *without* passing through the atlas, so the host resyncs the atlas
/// selection to the briefed node's conflict — otherwise escaping that briefing would land on a
/// hub filtered to a stale conflict, hiding the very mission the player was just in. Pure —
/// unit-tested.
pub(crate) fn conflict_index_of(campaign: &Campaign, node: NodeId) -> Option<usize> {
    let op = campaign.node(node)?.operation?;
    let conflict = campaign.operation(op)?.conflict;
    campaign.conflicts().iter().position(|c| c.id == conflict)
}

/// The scrubber's year domain: the earliest conflict start to the latest conflict end across the
/// authored atlas (a single-conflict campaign yields that conflict's own span; an atlas-less
/// campaign degrades to `(0, 0)` and the scrubber simply doesn't draw). Pure — unit-tested.
pub(crate) fn year_domain(campaign: &Campaign) -> (i16, i16) {
    let conflicts = campaign.conflicts();
    let lo = conflicts.iter().map(|c| c.start_year).min().unwrap_or(0);
    let hi = conflicts.iter().map(|c| c.end_year).max().unwrap_or(0);
    (lo, hi)
}

/// Whether a conflict is live in a scrubbed year (inclusive span). Pure.
pub(crate) fn conflict_active_in(conflict: &Conflict, year: i16) -> bool {
    conflict.start_year <= year && year <= conflict.end_year
}

/// The globe pins under the atlas state: every authored conflict, the **selected** one focused
/// (bright, pulsing), and each pin's era-activity from the scrubbed year — an out-of-era conflict
/// stays locatable but dims (the shader's D104 `active` lane). Pure data. Unlike the hub backdrop's
/// `atlas_pins`, focus follows the *selection*, not campaign progress.
pub(crate) fn atlas_pins_for(campaign: &Campaign, state: &AtlasState) -> Vec<GlobePin> {
    campaign
        .conflicts()
        .iter()
        .enumerate()
        .map(|(i, c)| {
            GlobePin::conflict(
                c.lat_x10 as f32 / 10.0,
                c.lon_x10 as f32 / 10.0,
                i == state.selected,
                conflict_active_in(c, state.year),
            )
        })
        .collect()
}

/// Resolve a click at `ndc` (`[-1,1]²`, x right / y up) to the conflict pin it lands on: the
/// nearest **visible** pin (far-side pins can't be picked — `project_pin`'s facing gate) within
/// [`PICK_RADIUS`], or `None` for empty space. Distances are aspect-corrected so the pick circle
/// is round on screen. Pure — the click decision, unit-tested against the same projection the
/// renderer draws with.
pub(crate) fn pick_conflict(
    campaign: &Campaign,
    state: &AtlasState,
    aspect: f32,
    ndc: [f32; 2],
) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, c) in campaign.conflicts().iter().enumerate() {
        let Some(p) = project_pin(
            state.view,
            aspect,
            c.lat_x10 as f32 / 10.0,
            c.lon_x10 as f32 / 10.0,
        ) else {
            continue;
        };
        let dx = (p[0] - ndc[0]) * aspect; // aspect-correct so the pick radius is circular
        let dy = p[1] - ndc[1];
        let d = (dx * dx + dy * dy).sqrt();
        if d <= PICK_RADIUS && best.is_none_or(|(_, bd)| d < bd) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

/// The selected conflict's card line, e.g. `2027-2028 · 0/3 OPERATIONS CLEARED`. Pure formatting.
pub(crate) fn atlas_card_line(conflict: &Conflict, cleared: u32, total: u32) -> String {
    let years = if conflict.start_year == conflict.end_year {
        format!("{}", conflict.start_year)
    } else {
        format!("{}-{}", conflict.start_year, conflict.end_year)
    };
    format!("{years} \u{00B7} {cleared}/{total} OPERATIONS CLEARED")
}

/// The immediate-mode conflict atlas screen (D104): a fullscreen drag/scroll surface over the
/// globe, the year scrubber along the bottom, and the selected conflict's card (name, years,
/// summary, rollup, ENTER) bottom-left — with BACK beneath it. Every gesture routes through the
/// pure seams above at the host. Glue (needs a live `Ui`).
pub(crate) fn atlas_ui(
    ui: &mut egui::Ui,
    campaign: &Campaign,
    state: &AtlasState,
) -> Option<AtlasAction> {
    use egui::RichText;
    let mut action = None;
    // The full viewport in logical points — `InputState.raw.screen_rect` carries the frame's
    // rect in this egui version; fall back to the ui clip rect if a frame ever omits it.
    let screen = ui
        .ctx()
        .input(|i| i.raw.screen_rect)
        .unwrap_or_else(|| ui.clip_rect());
    let aspect = if screen.height() > 1.0 {
        screen.width() / screen.height()
    } else {
        1.0
    };

    // The globe surface: one fullscreen interact area, added FIRST so every widget drawn after it
    // (scrubber, card) wins pointer priority over it. Drag turns the globe; click picks a pin;
    // scroll (anywhere) zooms.
    let surface = ui.interact(
        screen,
        ui.id().with("atlas_surface"),
        egui::Sense::click_and_drag(),
    );
    if surface.dragged() {
        let d = surface.drag_delta();
        if d.x != 0.0 || d.y != 0.0 {
            action = Some(AtlasAction::Drag(d.x, d.y));
        }
    }
    if surface.clicked() {
        if let Some(pos) = surface.interact_pointer_pos() {
            let ndc = crate::shell::util::pointer_to_ndc(
                [pos.x, pos.y],
                [screen.width(), screen.height()],
            );
            if let Some(i) = pick_conflict(campaign, state, aspect, ndc) {
                action = Some(AtlasAction::SelectConflict(i));
            }
        }
    }
    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
    if scroll.abs() > 0.1 {
        action = Some(AtlasAction::Zoom(scroll / 40.0));
    }

    // Banner, top-left — plus the interaction hint under it.
    egui::Area::new(ui.id().with("atlas_banner"))
        .fixed_pos(egui::pos2(40.0, 32.0))
        .show(ui.ctx(), |ui| {
            screen_banner(ui, "CONFLICT ATLAS", 150.0);
            ui.label(
                RichText::new("Drag to turn the earth. Scroll to zoom. Pick a war.")
                    .color(ASH)
                    .size(TYPE_CAPTION),
            );
        });

    // The selected conflict's card, bottom-left: identity, rollup, ENTER/BACK.
    if let Some(conflict) = campaign.conflicts().get(state.selected) {
        let rollup = campaign.conflict_progress(conflict.id);
        egui::Area::new(ui.id().with("atlas_card"))
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(40.0, -96.0))
            .show(ui.ctx(), |ui| {
                glass_card_frame().show(ui, |ui| {
                    ui.set_width(360.0);
                    ui.label(
                        RichText::new(conflict.name.to_uppercase())
                            .color(BONE)
                            .size(TYPE_SUBHEAD)
                            .strong(),
                    );
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(atlas_card_line(conflict, rollup.cleared, rollup.total))
                            .color(AMBER)
                            .size(TYPE_CAPTION),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(&conflict.summary)
                            .color(ASH)
                            .size(TYPE_CAPTION),
                    );
                    ui.add_space(10.0);
                    if footer_button(ui, "ENTER CONFLICT", Emphasis::Primary) {
                        action = Some(AtlasAction::Enter);
                    }
                    ui.add_space(6.0);
                    if footer_button(ui, "BACK", Emphasis::Tertiary) {
                        action = Some(AtlasAction::Back);
                    }
                });
            });
    }

    // The year scrubber, bottom-centre — hidden for a single-year atlas (nothing to scrub).
    let (lo, hi) = year_domain(campaign);
    if hi > lo {
        egui::Area::new(ui.id().with("atlas_scrubber"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -28.0))
            .show(ui.ctx(), |ui| {
                glass_card_frame().show(ui, |ui| {
                    ui.set_width((screen.width() * 0.42).clamp(280.0, 560.0));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("YEAR").color(ASH).size(TYPE_CAPTION));
                        let mut year = state.year as i32;
                        let slider = egui::Slider::new(&mut year, lo as i32..=hi as i32)
                            .show_value(false)
                            .integer();
                        let changed = ui
                            .scope(|ui| {
                                ui.spacing_mut().slider_width = ui.available_width() - 64.0;
                                ui.add(slider).changed()
                            })
                            .inner;
                        ui.label(
                            RichText::new(format!("{}", state.year))
                                .color(AMBER)
                                .size(TYPE_BODY)
                                .strong(),
                        );
                        if changed {
                            action = Some(AtlasAction::SetYear(year as i16));
                        }
                    });
                });
            });
    }

    action
}
