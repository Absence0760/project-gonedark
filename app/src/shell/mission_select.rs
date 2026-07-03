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

/// An action the mission-select (Operations-hub) screen can emit in a frame. The hub reads the
/// campaign through [`Campaign::mission_select`] (host-side, never the sim — invariants #1/#7); the
/// only outcomes are launching a node's briefing or backing out to the title.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MissionSelectAction {
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
/// routing go through the pure [`playable_node`] seam. Glue.
pub(crate) fn mission_select_ui(ui: &mut egui::Ui, campaign: &Campaign) -> Option<MissionSelectAction> {
    use egui::RichText;
    let mut action = None;

    over_backdrop_screen(ui, "operations", |ui| {
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
        let sections = hub_sections(campaign);
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
