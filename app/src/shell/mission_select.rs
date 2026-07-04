//! The Operations-hub mission-select screen — the campaign's nodes as status-coded tiles,
//! grouped by the conflict atlas (D98: conflict → operation → battle).
//!
//! Two pure seams gate the egui glue: [`playable_node`] (every tile click) and [`hub_sections`]
//! (the grouped section order + rollups), plus the glue itself ([`mission_tile`],
//! [`mission_select_ui`]) that renders the hub over the backdrop. Reads the campaign through
//! [`Campaign::mission_select`] and the atlas accessors (host-side, never the sim — invariants
//! #1/#7).

use crate::shell::theme::*;
use crate::shell::widgets::*;
use crate::shell::briefing::difficulty_label;
use gonedark_core::campaign::{
    Campaign, Conflict, ConflictId, GroupProgress, MissionSelectEntry, NodeId, NodeProgress,
    Operation, OperationId,
};
use gonedark_render::globe_backdrop::{project_pin, GlobeFlight, GlobePin, GlobeView, PinTone};

/// An action the mission-select (Operations-hub) screen can emit in a frame. The hub reads the
/// campaign through [`Campaign::mission_select`] (host-side, never the sim — invariants #1/#7); the
/// outcomes are launching a node's briefing, a look-around camera nudge, or backing out.
/// (`Look` carries `f32` drag deltas, so the enum is `PartialEq` but not `Eq`.)
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum MissionSelectAction {
    /// Open the briefing for the clicked node (only ever a *playable* node — see [`playable_node`]).
    OpenNode(NodeId),
    /// A drag on the battlefield surface by `(dx, dy)` logical points — nudges the clamped
    /// look-around offset ([`apply_hub_look`]); never a screen transition.
    Look(f32, f32),
    /// Back out of the hub (to the conflict atlas, D104).
    Back,
}

/// The node a mission-select **tile** click resolves to — `Some(node)` only when the tile is
/// playable ([`NodeProgress::is_playable`]: Available **or** already-Cleared/replayable), `None` for
/// a Locked tile. This is the single gate the egui builder routes every tile click through, so a
/// locked tile can never launch even if it somehow reports a click. Pure — unit-tested without a GPU
/// (the rendering of the tile is the exempt glue; this *decision* is what's tested).
pub(crate) fn playable_node(entry: &MissionSelectEntry) -> Option<NodeId> {
    entry.progress.is_playable().then_some(entry.node)
}

/// The title screen's NEXT OPERATION card model: which node CONTINUE deep-links to, its display
/// title, the campaign completion tally, and whether the pick is a replay (everything cleared) or
/// fresh progress. Pure data derived from the campaign — never the sim.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct NextOperation {
    /// The node CONTINUE opens the briefing for.
    pub node: NodeId,
    /// The node's display title.
    pub title: String,
    /// How many operations are cleared (any tier).
    pub cleared: usize,
    /// Total operations in the campaign.
    pub total: usize,
    /// `true` when every operation is cleared and the pick is a replay of the last one.
    pub replay: bool,
}

/// Derive the NEXT OPERATION card from the campaign: the first **Available** node, or — once the
/// campaign is fully cleared — the **last** playable node offered as a replay. `None` only for an
/// empty campaign (the card simply doesn't draw). Pure — the title screen's one campaign decision,
/// unit-tested; the card rendering is the exempt glue.
pub(crate) fn next_operation(campaign: &Campaign) -> Option<NextOperation> {
    let entries = campaign.mission_select();
    let total = entries.len();
    let cleared = entries
        .iter()
        .filter(|e| matches!(e.progress, NodeProgress::Cleared { .. }))
        .count();
    let pick = entries
        .iter()
        .find(|e| matches!(e.progress, NodeProgress::Available))
        .or_else(|| entries.iter().rev().find(|e| e.progress.is_playable()))?;
    Some(NextOperation {
        node: pick.node,
        title: pick.title.clone(),
        cleared,
        total,
        replay: matches!(pick.progress, NodeProgress::Cleared { .. }),
    })
}

/// One renderable section of the grouped Operations hub — the conflict-atlas (D98) shape the tile
/// list draws top-to-bottom: an optional conflict header (present only on the conflict's *first*
/// section, so a multi-operation conflict draws its header once), an optional operation sub-header,
/// and that operation's battle tiles. The trailing untitled section (`conflict`/`operation` both
/// `None`) carries the ungrouped nodes, so a plain [`Campaign::new`] hub degrades to exactly one
/// untitled section — the pre-atlas flat list. Pure data derived from the campaign, never the sim.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct HubSection {
    /// Set when this section opens a new conflict: the header the hub draws once above it.
    pub conflict: Option<(ConflictId, GroupProgress)>,
    /// The operation these tiles belong to, or `None` for the trailing ungrouped section.
    pub operation: Option<(OperationId, GroupProgress)>,
    /// The section's battle tiles, in authored (`NodeId`) order.
    pub nodes: Vec<NodeId>,
}

/// Derive the hub's ordered section list from the campaign: conflicts in authored order, each
/// conflict's operations in authored order, each operation's nodes in authored order, then a
/// trailing untitled section for ungrouped nodes. A **content-pending** (empty) operation renders
/// nothing — no header scaffolding without tiles — and a conflict whose operations are all empty
/// therefore contributes no section at all. Pure — the grouping decision behind the egui glue,
/// unit-tested without a GPU (repo testing rule).
/// [`hub_sections`] filtered to one conflict (D104: the hub entered from the atlas shows the
/// selected war's operations alone; the ungrouped tail only renders in the unfiltered view, so a
/// filtered hub can never leak another conflict's — or no conflict's — tiles). `None` = the full
/// pre-D104 hub. Pure — unit-tested.
pub(crate) fn hub_sections_for(campaign: &Campaign, only: Option<ConflictId>) -> Vec<HubSection> {
    let mut sections = hub_sections(campaign);
    if let Some(id) = only {
        // A section belongs to `id` if its operation resolves to that conflict (the conflict
        // header, when present, always matches its own operation's conflict by construction).
        sections.retain(|s| {
            s.operation
                .and_then(|(op, _)| campaign.operation(op))
                .is_some_and(|op| op.conflict == id)
        });
    }
    sections
}

pub(crate) fn hub_sections(campaign: &Campaign) -> Vec<HubSection> {
    let mut sections = Vec::new();
    for conflict in campaign.conflicts() {
        // `take()`n by the conflict's first non-empty operation, so the header draws exactly once.
        let mut header = Some((conflict.id, campaign.conflict_progress(conflict.id)));
        for op in campaign.operations_in(conflict.id) {
            let nodes = campaign.nodes_in(op);
            if nodes.is_empty() {
                continue; // content-pending operation: nothing to render yet
            }
            sections.push(HubSection {
                conflict: header.take(),
                operation: Some((op, campaign.operation_progress(op))),
                nodes,
            });
        }
    }
    let ungrouped: Vec<NodeId> = (0..campaign.len() as u32)
        .map(NodeId)
        .filter(|&id| campaign.node(id).is_some_and(|n| n.operation.is_none()))
        .collect();
    if !ungrouped.is_empty() {
        sections.push(HubSection { conflict: None, operation: None, nodes: ungrouped });
    }
    sections
}

/// The conflict header line, e.g. `THE CHANNEL CRISIS · 2027-2028 · 0/2` — name, year span
/// (collapsed to a single year when `start_year == end_year`), and the campaign-level rollup.
/// ASCII plus U+00B7 only (the one non-ASCII glyph proven to render in egui's default font — same
/// rule as the tile status pill). Pure formatting, unit-tested.
pub(crate) fn conflict_header_label(conflict: &Conflict, progress: GroupProgress) -> String {
    let years = if conflict.start_year == conflict.end_year {
        format!("{}", conflict.start_year)
    } else {
        format!("{}-{}", conflict.start_year, conflict.end_year)
    };
    format!(
        "{} \u{00B7} {} \u{00B7} {}/{}",
        conflict.name.to_uppercase(),
        years,
        progress.cleared,
        progress.total
    )
}

/// The operation sub-header line, e.g. `OPERATION FIRST LIGHT · 0/2` — name plus its own rollup.
/// Pure formatting, unit-tested; the greyed-when-unplayable colour pick is the glue's job.
pub(crate) fn operation_header_label(operation: &Operation, progress: GroupProgress) -> String {
    format!(
        "{} \u{00B7} {}/{}",
        operation.name.to_uppercase(),
        progress.cleared,
        progress.total
    )
}

/// Which conflict the globe backdrop settles on (D103): the **first not-yet-complete** conflict in
/// authored order — the one the player is actually fighting — falling back to the first when the
/// whole atlas is cleared (a finished campaign still frames its opening war). Pure — the backdrop's
/// one campaign decision, unit-tested; the rendering is the exempt glue.
pub(crate) fn focused_conflict(campaign: &Campaign) -> usize {
    campaign
        .conflicts()
        .iter()
        .position(|c| !campaign.conflict_progress(c.id).is_complete())
        .unwrap_or(0)
}

/// The globe backdrop's pin list (D103): one [`GlobePin`] per authored conflict, at its
/// `lat_x10`/`lon_x10` anchor (integer tenth-degrees → render-side degrees — the float boundary,
/// invariant #1: the conversion happens here, on the render side of the seam), with exactly the
/// [`focused_conflict`] marked. Pure data derived from the campaign — never the sim.
pub(crate) fn atlas_pins(campaign: &Campaign) -> Vec<GlobePin> {
    let focus = focused_conflict(campaign);
    campaign
        .conflicts()
        .iter()
        .enumerate()
        .map(|(i, c)| {
            GlobePin::conflict(
                c.lat_x10 as f32 / 10.0,
                c.lon_x10 as f32 / 10.0,
                i == focus,
                // The hub/briefing backdrop has no year scrubber — every conflict reads in-era.
                true,
            )
        })
        .collect()
}

// ---- the battlefield overview (D106) -----------------------------------------------------------

/// How far the battlefield overview zooms into a war. Inside the D104 navigation clamp
/// ([`GlobeView::ZOOM_MAX`] = 2.6 — nothing the atlas player couldn't already reach by hand),
/// close enough that a war's battle anchors read as separate grounds (the shipped anchor
/// authoring keeps them ≥ ~0.1° apart — a test pins the on-screen separation). 2.6 was tried
/// for the operations-map overlay and rejected: it pushes the east-biased composition's first
/// anchor off frame (the framing test catches it) — the tuned pair is (2.4, bias 1.0°).
pub(crate) const OVERVIEW_ZOOM: f32 = 2.4;

/// How far EAST of the war the overview camera centers, in **effective** degrees (divided by
/// `cos(lat)` so the on-screen shift is the same at every latitude): the hub card parks at the
/// left margin, so the battlefield gets pushed into the clear right half of the screen.
pub(crate) const OVERVIEW_EAST_BIAS_DEG: f32 = 1.0;

/// A battle pin's [`PinTone`] from its node's campaign progress — the D106 progress lane:
/// available ground glows amber (the same "this is where you act" read as the tile's AVAILABLE
/// pill), locked ground goes cold, cleared ground goes green. Pure.
pub(crate) fn battle_tone(progress: NodeProgress) -> PinTone {
    match progress {
        NodeProgress::Locked => PinTone::Locked,
        NodeProgress::Available => PinTone::Neutral,
        NodeProgress::Cleared { .. } => PinTone::Cleared,
    }
}

/// The **next battle** in a conflict: its first Available node in authored order, or — once the
/// war is fully cleared — its last node (the replay target, mirroring [`next_operation`]). `None`
/// only for a conflict with no nodes. Pure — the overview's focus decision.
pub(crate) fn next_battle_in(campaign: &Campaign, conflict: ConflictId) -> Option<NodeId> {
    let nodes: Vec<NodeId> = campaign
        .operations_in(conflict)
        .into_iter()
        .flat_map(|op| campaign.nodes_in(op))
        .collect();
    nodes
        .iter()
        .copied()
        .find(|&n| campaign.progress(n) == NodeProgress::Available)
        .or_else(|| nodes.last().copied())
}

/// How large the overview's battle motes draw, as the render-side "conflict pin = 1.0" multiple.
/// Bumped past the render default (`BATTLE_PIN_SCALE` = 1.6) shell-side: with the operations-map
/// overlay's chips and titles floating over them, the motes must read as the grounds those labels
/// annotate, not dust under them.
pub(crate) const OVERVIEW_PIN_SCALE: f32 = 2.2;

/// The battlefield overview's pin list (D106): one [`GlobePin::battle`] per **anchored** node of
/// `conflict` (a node with no authored ground simply doesn't pin — the tile list still carries
/// it), toned by progress ([`battle_tone`]) with `focused` marked, at [`OVERVIEW_PIN_SCALE`].
/// Integer tenth-degrees convert to render-side degrees here — the same float boundary as
/// [`atlas_pins`] (invariant #1). Pure.
pub(crate) fn battlefield_pins(
    campaign: &Campaign,
    conflict: ConflictId,
    focused: Option<NodeId>,
) -> Vec<GlobePin> {
    campaign
        .operations_in(conflict)
        .into_iter()
        .flat_map(|op| campaign.nodes_in(op))
        .filter_map(|n| {
            let (lat, lon) = campaign.node(n)?.anchor?;
            let mut pin = GlobePin::battle(
                lat as f32 / 10.0,
                lon as f32 / 10.0,
                Some(n) == focused,
                battle_tone(campaign.progress(n)),
            );
            pin.scale = OVERVIEW_PIN_SCALE;
            Some(pin)
        })
        .collect()
}

/// The battlefield overview's camera (D106): centered on the **centroid** of `conflict`'s battle
/// anchors at [`OVERVIEW_ZOOM`] — the whole war on screen, dead-on, stable (no sway; the pins
/// are pickable). `None` when the conflict has no anchored battles — the hub then falls back to
/// the settled D103 framing, so a list-only campaign renders exactly as before. Pure.
pub(crate) fn overview_view(campaign: &Campaign, conflict: ConflictId) -> Option<GlobeView> {
    let anchors: Vec<(i16, i16)> = campaign
        .operations_in(conflict)
        .into_iter()
        .flat_map(|op| campaign.nodes_in(op))
        .filter_map(|n| campaign.node(n)?.anchor)
        .collect();
    if anchors.is_empty() {
        return None;
    }
    let n = anchors.len() as f32;
    let lat = anchors.iter().map(|&(la, _)| la as f32 / 10.0).sum::<f32>() / n;
    let lon = anchors.iter().map(|&(_, lo)| lo as f32 / 10.0).sum::<f32>() / n;
    // Center WEST of the war so the war sits right of screen center, clear of the left-parked
    // hub card. Divided by cos(lat) so the shift reads the same at Gotland as at the equator
    // (guarded away from the poles, capped so a polar war can't spin the camera off its ground).
    let bias = (OVERVIEW_EAST_BIAS_DEG / lat.to_radians().cos().max(0.2)).min(3.0);
    Some(GlobeView::over(lat, lon - bias, OVERVIEW_ZOOM))
}

/// Resolve the hub backdrop's camera for one frame (D107): while a fly-in `flight` is live it
/// owns the camera (advanced by the frame's wall-clock `dt`, dropped on landing); otherwise the
/// battlefield `target` shows directly. A `None` target (no picked conflict, or a war with no
/// anchored battles) drops any stale flight and hands back `None` — the settled D103 fallback,
/// exactly as before D107. Pure — the per-frame decision, unit-tested; the run loop is glue.
pub(crate) fn hub_backdrop_view(
    flight: &mut Option<GlobeFlight>,
    target: Option<GlobeView>,
    dt: f32,
) -> Option<GlobeView> {
    match (flight.as_mut(), target) {
        (Some(f), Some(_)) => {
            let view = f.step(dt);
            if f.done() {
                *flight = None;
            }
            Some(view)
        }
        (_, None) => {
            *flight = None;
            None
        }
        (None, target) => target,
    }
}

// ---- look-around + the operations-map overlay (the zoomed conflict detail view) ----------------

/// How far the operations screen's look-around drag can peek the camera off the overview, in
/// radians of yaw. Deliberately small: at [`OVERVIEW_ZOOM`] the eye almost touches the globe, so
/// even these offsets swing the battlefield most of the way across the screen. The clamp is what
/// turns "you can tilt around the sites but never spin the planet" into a guarantee — a test
/// projects every anchored battle at the window's extremes and holds it on screen.
pub(crate) const LOOK_YAW_LIMIT: f32 = 0.035;
/// The pitch half of the look window — see [`LOOK_YAW_LIMIT`].
pub(crate) const LOOK_PITCH_LIMIT: f32 = 0.02;

/// Nudge the hub's look-around offset by a drag, clamped into the LOOK window. Same feel as the
/// atlas drag: [`DRAG_SENS`](crate::shell::atlas::DRAG_SENS) scaled down by zoom, with the
/// surface-following longitude correction
/// ([`lon_drag_correction`](crate::shell::atlas::lon_drag_correction)) so the ground tracks the
/// cursor. Pure — the operations screen's one camera-gesture decision, unit-tested.
pub(crate) fn apply_hub_look(look: &mut (f32, f32), dx: f32, dy: f32, base: GlobeView) {
    let s = crate::shell::atlas::DRAG_SENS / base.zoom;
    let corr = crate::shell::atlas::lon_drag_correction(base);
    look.0 = (look.0 + dx * s * corr).clamp(-LOOK_YAW_LIMIT, LOOK_YAW_LIMIT);
    look.1 = (look.1 + dy * s).clamp(-LOOK_PITCH_LIMIT, LOOK_PITCH_LIMIT);
}

/// The camera the operations screen actually shows: the base view (the D107 fly-in mid-flight,
/// else the D106 overview) with the player's clamped look offsets added, re-clamped. The **one
/// effective view** — the host threads it to the backdrop, the overlay, and pin-picking alike,
/// so the three can never disagree (the D104 discipline). A `None` base (no battlefield) stays
/// `None`: there is nothing to peek around. Pure.
pub(crate) fn hub_effective_view(base: Option<GlobeView>, look: (f32, f32)) -> Option<GlobeView> {
    base.map(|v| GlobeView { yaw: v.yaw + look.0, pitch: v.pitch + look.1, zoom: v.zoom }.clamped())
}

/// One battle site on the operations-map overlay: its 1-based progression `order` within the
/// conflict (the authored prerequisite-chain walk — the same order [`battlefield_pins`] takes),
/// its campaign state, display title, and where it projects on screen. `ndc` is `None` for a
/// far-side/off-screen site — the glue skips drawing it, but the site **stays in the list** so
/// ordering and path adjacency stay stable. Locked sites are deliberately included: the map shows
/// the whole operation start → finish (the Normandy read — beach first, the inland grounds
/// visible but padlocked), locked ground never hides. Pure data — never the sim.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct SiteWaypoint {
    pub node: NodeId,
    pub order: usize,
    pub title: String,
    pub progress: NodeProgress,
    pub ndc: Option<[f32; 2]>,
}

/// A site keeps its screen position up to slightly past the frame edge, so a chip half off-frame
/// still draws instead of popping the instant its centre crosses the edge.
const SITE_NDC_MARGIN: f32 = 1.05;

/// The overlay's ordered site list for `conflict` under the live `view`: one waypoint per
/// **anchored** node, numbered 1..n in prerequisite-chain (authored) order, projected through the
/// SAME [`project_pin`] the renderer and [`pick_battle`] use. Pure — unit-tested; the painting is
/// the exempt glue.
pub(crate) fn site_waypoints(
    campaign: &Campaign,
    conflict: ConflictId,
    view: GlobeView,
    aspect: f32,
) -> Vec<SiteWaypoint> {
    let mut out = Vec::new();
    for op in campaign.operations_in(conflict) {
        for n in campaign.nodes_in(op) {
            let Some(node) = campaign.node(n) else { continue };
            let Some((lat, lon)) = node.anchor else { continue };
            let ndc = project_pin(view, aspect, lat as f32 / 10.0, lon as f32 / 10.0)
                .filter(|p| p[0].abs() <= SITE_NDC_MARGIN && p[1].abs() <= SITE_NDC_MARGIN);
            out.push(SiteWaypoint {
                node: n,
                order: out.len() + 1,
                title: node.title.clone(),
                progress: campaign.progress(n),
                ndc,
            });
        }
    }
    out
}

/// The progression path's drawable legs: consecutive sites' screen segments (straight lines —
/// the shipped anchors sit ≤ ~1.5° apart, no great-circle needed), each carrying the
/// **destination** site's progress (a leg is "the road to that ground": cleared legs read green,
/// the leg to the live battle amber, locked slate). A leg with either end far-side/off-screen
/// drops; the sites themselves still list. Pure.
pub(crate) fn site_path_legs(sites: &[SiteWaypoint]) -> Vec<([f32; 2], [f32; 2], NodeProgress)> {
    sites
        .windows(2)
        .filter_map(|w| match (w[0].ndc, w[1].ndc) {
            (Some(a), Some(b)) => Some((a, b, w[1].progress)),
            _ => None,
        })
        .collect()
}

/// A site's overlay colour — the shell twin of the pin shader's tone tints
/// (`globe_backdrop.wgsl` `fs_pin`: amber available, slate locked, green cleared), so the mote
/// and the chip annotating it always agree. Pure — unit-tested against those hues.
pub(crate) fn site_color(progress: NodeProgress) -> egui::Color32 {
    match progress {
        // fs_pin locked tint (0.42, 0.47, 0.58) — cold slate.
        NodeProgress::Locked => egui::Color32::from_rgb(107, 120, 148),
        // fs_pin neutral tint (0.96, 0.62, 0.20) — the signal amber.
        NodeProgress::Available => egui::Color32::from_rgb(245, 158, 51),
        // fs_pin cleared tint (0.36, 0.85, 0.48) — taken-ground green.
        NodeProgress::Cleared { .. } => egui::Color32::from_rgb(92, 217, 122),
    }
}

/// Which side of its chip a site's title sits **by default**: alternating by progression order
/// (odd right, even left) so two neighbouring sites' labels fan away from each other instead of
/// colliding — the shipped anchors are close enough that same-side labels would overlap. Pure.
pub(crate) fn label_on_right(order: usize) -> bool {
    order % 2 == 1
}

/// A label's lane is "crowded" when another visible site sits on that side within this screen
/// window (NDC): roughly a title's width horizontally, a chip-and-label's height vertically.
const LABEL_LANE_DX: f32 = 0.18;
const LABEL_LANE_DY: f32 = 0.10;
/// A site almost directly above/below (|dx| inside this dead zone) is NOT lane-crowding: the
/// order alternation already fans a stacked pair apart, and a hair's-width dx must not flip
/// sides arbitrarily (the fixture's stacked anchors project ~0.001 apart in x).
const LABEL_LANE_EPS: f32 = 0.02;

/// The side site `i`'s label actually takes: the [`label_on_right`] alternation, **collision-
/// nudged** — when the default side points straight at another visible site inside the label
/// lane (the shipped Channel Crisis zigzags its grounds close enough for exactly that) and the
/// opposite side is free, the label flips away. A tie (both sides crowded, or no screen
/// position) keeps the default, so degenerate layouts stay deterministic. Pure — unit-tested.
pub(crate) fn label_side_for(sites: &[SiteWaypoint], i: usize) -> bool {
    let default_right = label_on_right(sites[i].order);
    let Some(p) = sites[i].ndc else {
        return default_right;
    };
    let crowded = |right: bool| {
        sites.iter().enumerate().any(|(j, s)| {
            if j == i {
                return false;
            }
            let Some(q) = s.ndc else { return false };
            let dx = q[0] - p[0];
            let toward = if right { dx > LABEL_LANE_EPS } else { dx < -LABEL_LANE_EPS };
            toward && dx.abs() < LABEL_LANE_DX && (q[1] - p[1]).abs() < LABEL_LANE_DY
        })
    };
    if crowded(default_right) && !crowded(!default_right) {
        !default_right
    } else {
        default_right
    }
}

/// Resolve a click at `ndc` on the battlefield overview to the battle it lands on: the nearest
/// visible **playable** anchored node of `conflict` within [`PICK_RADIUS`](crate::shell::atlas) —
/// the same gate as a tile click ([`playable_node`]), so a locked pin can never launch. Projected
/// with the SAME view the backdrop rendered, so picking can never disagree with the drawn pixels
/// (the D104 discipline). Pure.
pub(crate) fn pick_battle(
    campaign: &Campaign,
    conflict: ConflictId,
    view: GlobeView,
    aspect: f32,
    ndc: [f32; 2],
) -> Option<NodeId> {
    let mut best: Option<(NodeId, f32)> = None;
    for op in campaign.operations_in(conflict) {
        for n in campaign.nodes_in(op) {
            if !campaign.progress(n).is_playable() {
                continue;
            }
            let Some((lat, lon)) = campaign.node(n).and_then(|node| node.anchor) else {
                continue;
            };
            let Some(p) = project_pin(view, aspect, lat as f32 / 10.0, lon as f32 / 10.0) else {
                continue;
            };
            let dx = (p[0] - ndc[0]) * aspect;
            let dy = p[1] - ndc[1];
            let d = (dx * dx + dy * dy).sqrt();
            if d <= crate::shell::atlas::PICK_RADIUS && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((n, d));
            }
        }
    }
    best.map(|(n, _)| n)
}

/// The mission list's scroll-viewport cap for the height actually available inside the card —
/// small enough that the footer (BACK) below the list **always stays on screen**, large enough to
/// show up to five tile-rows on a roomy window. This is the fix for the un-pinned-BACK defect: the
/// old fixed `5 × 72` cap ignored the window height, so once the D98 atlas headers grew the card,
/// a short window overflowed the *outer* card scroll and pushed BACK below the fold — defeating
/// the pinning this cap exists for. Reserves the footer gap + button + a breathing margin, floors
/// at one tile-row so the list never collapses. Pure — unit-tested; the glue passes
/// `ui.available_height()`.
pub(crate) fn list_viewport_cap(available: f32) -> f32 {
    /// What must stay visible below the list: [`FOOTER_GAP`] + the 46px footer button + margin.
    const FOOTER_RESERVE: f32 = FOOTER_GAP + 46.0 + 8.0;
    /// The roomy-window cap (the previous fixed value — five tile-rows).
    const MAX: f32 = 5.0 * 72.0;
    /// Never collapse below one tile-row, even on a degenerate viewport.
    const MIN: f32 = 72.0;
    (available - FOOTER_RESERVE).clamp(MIN, MAX)
}

/// One mission-select tile: a status pill (Locked/Available/Cleared, colour-coded) beside the node
/// title as a full-width button. A **playable** node (Available or already-Cleared/replayable) is an
/// enabled button that emits [`MissionSelectAction::OpenNode`]; a **Locked** node renders disabled and
/// cannot be clicked. The launchable decision is the pure [`playable_node`] seam (double-guarded on
/// the click), so this is the exempt egui glue. Returns the action on a click. ASCII status text only.
pub(crate) fn mission_tile(ui: &mut egui::Ui, entry: &MissionSelectEntry) -> Option<MissionSelectAction> {
    use egui::RichText;
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
    // Title first (primary), status as a right-aligned chip — the whole row is one selectable card.
    let clicked = selectable_row(ui, ("mission_tile", entry.node), playable, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(entry.title.clone()).color(title_color).size(TYPE_SUBHEAD).strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                status_chip(ui, &status, status_color);
            });
        });
    });
    clicked
        .then(|| playable_node(entry).map(MissionSelectAction::OpenNode))
        .flatten()
}

/// The immediate-mode Operations-hub mission-select screen: the campaign's nodes as
/// status-coded tiles in a card over the backdrop, then BACK. Reads
/// [`Campaign::mission_select`] (host-side, never the sim); each tile's launchability + the click
/// routing go through the pure [`playable_node`] seam. When the backdrop is the zoomed
/// **battlefield overview** (D106 — `overview` carries the exact view it rendered with), the
/// battle pins are a second launch surface: a click on a playable pin opens its briefing, through
/// the same [`pick_battle`] → playable gate as a tile. Glue.
pub(crate) fn mission_select_ui(
    ui: &mut egui::Ui,
    campaign: &Campaign,
    only: Option<ConflictId>,
    overview: Option<GlobeView>,
) -> Option<MissionSelectAction> {
    use egui::RichText;
    let mut action = None;

    // The battlefield surface: a fullscreen click+drag area added FIRST, so the card/tiles drawn
    // after it win pointer priority — exactly the atlas_ui pattern. Only live when the backdrop
    // really is the overview (picking must share the drawn view, never guess one). A drag is the
    // look-around gesture (a clamped peek, applied by the host through `apply_hub_look` — never a
    // free spin); a click picks a battle.
    if let (Some(view), Some(conflict)) = (overview, only) {
        let screen = ui.ctx().input(|i| i.raw.screen_rect).unwrap_or_else(|| ui.clip_rect());
        let aspect = if screen.height() > 1.0 { screen.width() / screen.height() } else { 1.0 };
        let surface = ui.interact(
            screen,
            ui.id().with("battlefield_surface"),
            egui::Sense::click_and_drag(),
        );
        if surface.dragged() {
            let d = surface.drag_delta();
            if d.x != 0.0 || d.y != 0.0 {
                action = Some(MissionSelectAction::Look(d.x, d.y));
            }
        }
        if surface.clicked() {
            if let Some(pos) = surface.interact_pointer_pos() {
                let ndc = crate::shell::util::pointer_to_ndc(
                    [pos.x, pos.y],
                    [screen.width(), screen.height()],
                );
                if let Some(node) = pick_battle(campaign, conflict, view, aspect, ndc) {
                    action = Some(MissionSelectAction::OpenNode(node));
                }
            }
        }
        // The operations-map overlay, painted with the SAME view the surface picks with.
        draw_operations_overlay(ui, campaign, conflict, view, aspect, screen);
    }

    // With the battlefield overview live, park the card at the left margin so the war (globe +
    // battle pins) stays visible beside it; without one (no picked conflict / no anchored
    // battles) keep the classic centred card.
    let left = overview.map(|_| 40.0);
    over_backdrop_screen_at(ui, "operations", SHELL_CARD_W, left, |ui| {
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

        // Each mission is its own selectable card, so they sit directly in the screen card (no
        // second enclosing frame). The list has its own bounded scroll so the banner and BACK stay
        // pinned as the campaign grows; a short list shows no scrollbar. Tiles are grouped by the
        // conflict atlas (D98): a conflict header, an operation sub-header, then that operation's
        // tiles — the ordering/rollup decisions are the pure [`hub_sections`] seam; this is glue.
        // `mission_select()` returns one entry per node in `NodeId` order, so `entries[node.0]` is
        // that node's tile — same entry, same [`mission_tile`], so launch behavior is unchanged.
        let entries = campaign.mission_select();
        let sections = hub_sections_for(campaign, only);
        egui::ScrollArea::vertical()
            .max_height(list_viewport_cap(ui.available_height()))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (si, section) in sections.iter().enumerate() {
                    if si > 0 {
                        ui.add_space(14.0);
                    }
                    if let Some((id, rollup)) = section.conflict {
                        if let Some(conflict) = campaign.conflict(id) {
                            ui.label(
                                RichText::new(conflict_header_label(conflict, rollup))
                                    .color(BONE)
                                    .size(TYPE_BODY)
                                    .strong(),
                            );
                            ui.add_space(6.0);
                        }
                    }
                    if let Some((id, rollup)) = section.operation {
                        if let Some(operation) = campaign.operation(id) {
                            // Greyed when nothing in the operation is playable yet (still gated).
                            let color = if rollup.playable { ASH } else { MUTED };
                            ui.label(
                                RichText::new(operation_header_label(operation, rollup))
                                    .color(color)
                                    .size(TYPE_CAPTION)
                                    .strong(),
                            );
                            ui.add_space(6.0);
                        }
                    }
                    for (i, &node) in section.nodes.iter().enumerate() {
                        if let Some(entry) = entries.get(node.0 as usize) {
                            if let Some(act) = mission_tile(ui, entry) {
                                action = Some(act);
                            }
                        }
                        if i + 1 < section.nodes.len() {
                            ui.add_space(8.0);
                        }
                    }
                }
            });

        ui.add_space(FOOTER_GAP);
        // Sole exit on this screen — Secondary, not the dimmest Tertiary. (Briefing keeps BACK
        // Tertiary because DEPLOY is the genuine primary action there.)
        if footer_button(ui, "BACK", Emphasis::Secondary) {
            action = Some(MissionSelectAction::Back);
        }
    });

    action
}

/// The chip's radius (px) and how far it floats above its site's glowing mote — lifted so the pin
/// itself stays readable under the annotation.
const CHIP_R: f32 = 9.0;
const CHIP_LIFT: f32 = 22.0;

/// Paint the operations-map overlay over the battlefield: the dashed progression path, then each
/// visible site's order chip, padlock (locked ground only), and title. Painter shapes are not
/// widgets — they can never intercept the surface's or the card's clicks — and this runs before
/// the hub card is laid out, so the card paints over the overlay where they overlap. Every
/// decision lives in the pure seams ([`site_waypoints`], [`site_path_legs`], [`site_color`],
/// [`label_on_right`], [`ndc_to_pointer`](crate::shell::util::ndc_to_pointer)); this is the
/// exempt egui glue.
fn draw_operations_overlay(
    ui: &egui::Ui,
    campaign: &Campaign,
    conflict: ConflictId,
    view: GlobeView,
    aspect: f32,
    screen: egui::Rect,
) {
    use egui::{pos2, vec2, Align2, FontId, Shape, Stroke};
    let painter = ui.painter();
    let size = [screen.width(), screen.height()];
    let to_px = |ndc: [f32; 2]| {
        let p = crate::shell::util::ndc_to_pointer(ndc, size);
        pos2(screen.min.x + p[0], screen.min.y + p[1])
    };
    let sites = site_waypoints(campaign, conflict, view, aspect);
    // The progression path first (under the chips): dashed legs, each tinted by the ground it
    // leads TO — cleared green (taken), the live battle's amber, locked slate dimmed further
    // (visible, not yet reachable — the Normandy read).
    for (a, b, progress) in site_path_legs(&sites) {
        let dim = if progress == NodeProgress::Locked { 0.45 } else { 0.8 };
        let stroke = Stroke::new(1.5, site_color(progress).gamma_multiply(dim));
        painter.extend(Shape::dashed_line(&[to_px(a), to_px(b)], stroke, 6.0, 5.0));
    }
    for (i, site) in sites.iter().enumerate() {
        let Some(ndc) = site.ndc else { continue };
        let color = site_color(site.progress);
        let chip = to_px(ndc) + vec2(0.0, -CHIP_LIFT);
        painter.circle_filled(chip, CHIP_R, PANEL_GLASS);
        painter.circle_stroke(chip, CHIP_R, Stroke::new(1.5, color));
        painter.text(
            chip,
            Align2::CENTER_CENTER,
            site.order.to_string(),
            FontId::proportional(11.0),
            color,
        );
        // The label lane: a padlock (locked ground only), then the title, fanned to alternating
        // sides — collision-nudged away from a neighbouring site — so text never piles up.
        let right = label_side_for(&sites, i);
        let dir = if right { 1.0 } else { -1.0 };
        let mut x = chip.x + dir * (CHIP_R + 6.0);
        if site.progress == NodeProgress::Locked {
            draw_padlock(painter, pos2(x + dir * 5.0, chip.y), color);
            x += dir * 13.0;
        }
        let anchor = if right { Align2::LEFT_CENTER } else { Align2::RIGHT_CENTER };
        painter.text(pos2(x, chip.y), anchor, &site.title, FontId::proportional(TYPE_CAPTION), color);
    }
}

/// A padlock in painter primitives — no emoji (the shell's no-tofu rule: egui's default font has
/// no lock glyph) and no new assets: a rounded-rect body under a stroked semicircular shackle.
/// Glue (needs a live painter); the decision to show it is `site.progress == Locked`, pinned by
/// the [`site_waypoints`] tests.
fn draw_padlock(painter: &egui::Painter, center: egui::Pos2, color: egui::Color32) {
    use egui::{pos2, vec2, CornerRadius, Rect, Shape, Stroke};
    let body = Rect::from_center_size(center + vec2(0.0, 1.5), vec2(7.5, 6.0));
    painter.rect_filled(body, CornerRadius::same(2), color);
    // The shackle: a semicircle sampled into a polyline (egui's painter has no arc primitive).
    let r = 2.4;
    let hinge = pos2(center.x, body.min.y);
    let arc: Vec<egui::Pos2> = (0..=8)
        .map(|i| {
            let t = std::f32::consts::PI * i as f32 / 8.0;
            pos2(hinge.x - r * t.cos(), hinge.y - r * t.sin())
        })
        .collect();
    painter.add(Shape::line(arc, Stroke::new(1.3, color)));
}
