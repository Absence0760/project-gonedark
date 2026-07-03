//! The **PvP staging** screen — the title's PvP door, distinct from the campaign hub and the
//! skirmish setup (`docs/modes.md` §1: three front doors, none leaking into the others).
//!
//! PvP's real surfaces are gated on the Phase 3 net layer (`modes.md` §5 build order: the custom
//! lobby first, then quick match, then ranked — D58's fast-follow). Until that transport exists
//! this door is **deliberately a staging post, not a fake matchmaker**: it names the three queues
//! in their build order, shows the army the player would queue as (§4a: pick before queueing),
//! and offers nothing joinable — no live-looking queue chrome before a real session can back it.
//! The one decision — that no queue is joinable pre-net — is the pure [`queue_joinable`] seam, so
//! the honesty rule is a tested invariant, not a styling accident. Everything here is host-side
//! presentation; it never touches the sim (invariants #1/#7).

use crate::shell::army::army_label;
use crate::shell::theme::*;
use crate::shell::widgets::*;
use gonedark_core::components::Army;

/// One PvP queue on the staging screen: a stable id, the tile name + one-line blurb, and the
/// build-order status its chip shows. All fields `&'static str`, so the table is a `const` —
/// exactly the `GameMode` convention.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct PvpQueue {
    /// Stable id (also a tile key). ASCII.
    pub id: &'static str,
    /// Display name shown on the tile.
    pub name: &'static str,
    /// One-line teaser under the name.
    pub blurb: &'static str,
    /// The build-order status chip (`modes.md` §5) — honest scheduling, never a live state.
    pub status: &'static str,
}

/// The three PvP queues, in the `modes.md` §5 build order (custom lobby → quick match → ranked).
/// The **first entry is the first real PvP surface** — the direct-invite custom lobby, the
/// smallest thing that puts two humans in one lockstep match. Static presentation data; the
/// map-policy and ranking designs behind each queue live in `modes.md` §4b/§4c (ranking's rating
/// model is still Q29).
pub(crate) const PVP_QUEUES: &[PvpQueue] = &[
    PvpQueue {
        id: "custom",
        name: "Custom Lobby",
        blurb: "Invite an opponent, pick any lint-passing battlefield, ready up. \
                The first two-human match.",
        status: "FIRST UP",
    },
    PvpQueue {
        id: "quick",
        name: "Quick Match",
        blurb: "Curated map rotation, random pick. Unranked, low ceremony.",
        status: "PLANNED",
    },
    PvpQueue {
        id: "ranked",
        name: "Ranked",
        blurb: "Seasonal map pool with vetoes. Placements, tiers, a rating on the line.",
        status: "PLANNED",
    },
];

/// Whether a queue can currently be joined — the single gate every queue row routes through, the
/// PvP twin of [`playable_node`](crate::shell::mission_select::playable_node). **Always `false`
/// until the Phase 3 net layer lands** (`modes.md` §5: there is no session transport to join),
/// so the screen cannot grow a live-looking queue by accident; when the custom lobby ships, this
/// seam is where it flips per-queue. Pure — unit-tested.
pub(crate) fn queue_joinable(_queue: &PvpQueue) -> bool {
    false
}

/// An action the PvP staging screen can emit in a frame. BACK is the only live control — the
/// queues are staged, not joinable ([`queue_joinable`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PvpAction {
    /// Return to the title screen.
    Back,
}

/// One queue tile: the queue name beside its build-order status chip, over the one-line blurb, as
/// a full-width row. Disabled whenever [`queue_joinable`] says so (today: always), so the row
/// renders as information, never as a dead button that swallows clicks. Glue (needs a live `Ui`);
/// the joinability decision is the pure seam. ASCII only.
fn queue_tile(ui: &mut egui::Ui, queue: &PvpQueue) {
    use egui::RichText;
    let joinable = queue_joinable(queue);
    // FIRST UP reads amber (the next thing that becomes real); the rest stay muted.
    let status_color = if queue.status == "FIRST UP" { AMBER } else { MUTED };
    selectable_row(ui, ("pvp_queue", queue.id), joinable, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(queue.name.to_uppercase())
                    .color(BONE)
                    .size(TYPE_SUBHEAD)
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                status_chip(ui, queue.status, status_color);
            });
        });
        ui.label(RichText::new(queue.blurb).color(ASH).size(TYPE_CAPTION));
    });
}

/// The immediate-mode PvP staging screen: the three queues in build order (none joinable pre-net,
/// per [`queue_joinable`]), the §4a pre-queue identity line (the persisted army pick, read-only
/// here — edited on the army-select screen), then BACK. Honest by construction: the copy says the
/// net layer is what's missing, and nothing on the screen pretends otherwise. Glue.
pub(crate) fn pvp_ui(ui: &mut egui::Ui, player_army: Army) -> Option<PvpAction> {
    use egui::RichText;
    let mut action = None;

    over_backdrop_screen(ui, "pvp", |ui| {
        screen_banner(ui, "PVP", 130.0);
        ui.label(
            RichText::new(
                "Live commanders over lockstep -- the divided-attention mind game against a \
                 human. The net layer lands in Phase 3; until it does, no queue is joinable.",
            )
            .color(ASH)
            .size(TYPE_BODY),
        );
        ui.add_space(16.0);

        section_label(ui, "QUEUES");
        for (i, queue) in PVP_QUEUES.iter().enumerate() {
            queue_tile(ui, queue);
            if i + 1 < PVP_QUEUES.len() {
                ui.add_space(8.0);
            }
        }
        ui.add_space(10.0);

        section_label(ui, "YOU QUEUE AS");
        card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(army_label(player_army))
                    .color(BONE)
                    .size(TYPE_SUBHEAD)
                    .strong(),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Your army and gunsmith loadout travel into every queue. Change them under \
                     ARMY and Settings on the title.",
                )
                .color(MUTED)
                .size(TYPE_CAPTION),
            );
        });

        ui.add_space(FOOTER_GAP);
        // Sole exit on this screen — Secondary, not the dimmest Tertiary (the mission-select rule
        // for a screen whose only control is the way out).
        if footer_button(ui, "BACK", Emphasis::Secondary) {
            action = Some(PvpAction::Back);
        }
    });

    action
}
