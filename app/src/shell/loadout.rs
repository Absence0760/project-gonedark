//! The gunsmith / loadout screen — the pure decision seam plus its egui glue. Customization-only
//! (D81): reached from Settings, it edits the persisted loadout via `core::gunsmith`-backed
//! `LoadoutEditor` and never starts a match; the sidegrade readouts stay fixed-point (invariant #1).

use crate::shell::theme::*;
use crate::shell::widgets::*;
use gonedark_core::fixed::Fixed;
use gonedark_core::gunsmith::StatDelta;
use gonedark_engine::loadout_ui::{LoadoutEditor, LoadoutSlot};

// ---- The gunsmith / loadout screen — pure seam (unit-tested) -------------------------------------

/// An action the gunsmith / loadout screen can emit in a frame. **D81: the gunsmith is
/// customization-only** — reached from Settings, it edits the persisted loadout and never starts a
/// match (the mode/mission-select screens are the deploy gates). So it has no Deploy: only edits
/// (`Cycle`/`Reset`) and DONE ([`LoadoutAction::Done`], which returns to Settings).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LoadoutAction {
    /// Cycle the slot at on-screen index `slot_index` forward (`true`) or back (`false`) — an edit.
    Cycle { slot_index: usize, forward: bool },
    /// Reset every slot to the neutral all-`Standard` baseline.
    Reset,
    /// Finish customizing — leave the gunsmith and return to Settings (the edits persist).
    Done,
}

/// The screen-level outcome of a [`LoadoutAction`] once applied to the editor — what the host run
/// loop switches on. Separated from the egui glue so it is unit-testable without a window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LoadoutStep {
    /// Stay on the gunsmith (an edit was applied, or nothing happened this frame).
    Stay,
    /// Finished customizing — return to Settings (the gunsmith's entry point, D81).
    Done,
}

/// Apply a [`LoadoutAction`] to the player's [`LoadoutEditor`] and report the resulting screen step.
/// Edits (`Cycle`/`Reset`) mutate the editor and keep us on the gunsmith; `Done` is the screen
/// transition the run loop acts on (back to Settings — the gunsmith is customization-only under D81).
/// Pure (no egui/window) — the gunsmith's testable decision seam, mirroring [`resolve_title_action`].
/// The actual loadout *model* (validation + the sidegrade-fairness proof) lives in `core::gunsmith`
/// and is consumed through the editor read-only; this never touches the sim.
pub(crate) fn apply_loadout_action(action: LoadoutAction, editor: &mut LoadoutEditor) -> LoadoutStep {
    match action {
        LoadoutAction::Cycle {
            slot_index,
            forward,
        } => {
            // An out-of-range index is a harmless no-op (the editor tolerates stray slot values).
            editor.apply_input(slot_index, forward);
            LoadoutStep::Stay
        }
        LoadoutAction::Reset => {
            editor.reset();
            LoadoutStep::Stay
        }
        LoadoutAction::Done => LoadoutStep::Done,
    }
}

/// A short, one-line description of the *axis pair* a gunsmith slot trades — the readout that makes
/// the sidegrade nature legible (every option spends one of these axes to buy the other). Pure and
/// static, so it is unit-tested; the numeric per-axis deltas live in `core::gunsmith` and are not
/// surfaced here (they need fixed-point formatting and add nothing to "which way does this trade").
/// ASCII only so it can never tofu in egui's default font.
pub(crate) fn slot_trade_hint(slot: LoadoutSlot) -> &'static str {
    match slot {
        LoadoutSlot::Optic => "range <-> fire-rate",
        LoadoutSlot::Barrel => "damage <-> reserve",
        LoadoutSlot::Magazine => "capacity <-> handling",
        LoadoutSlot::Stock => "mobility <-> steadiness",
        LoadoutSlot::Muzzle => "suppression <-> downrange retention",
        // Grip is cosmetic/feel-only (D85): no sim trade, just recoil/hipfire feel.
        LoadoutSlot::Grip => "grip feel (cosmetic)",
    }
}

/// Format a [`Fixed`] axis value as a signed whole-unit decimal (e.g. `+2.00`, `-0.03`) for the
/// gunsmith readout. Scales by the type's own whole unit (`Fixed::from_int(1)`), so it is correct
/// regardless of the fixed-point Q-format and uses integer math only. Presentation-side (app crate):
/// floats would be fine here, but there is no need — the sim stays fixed-point (invariant #1).
pub(crate) fn fixed_signed(f: Fixed) -> String {
    let unit = Fixed::from_int(1).to_bits() as i64; // one whole unit, in bits
    let hundredths = f.to_bits() as i64 * 100 / unit;
    let sign = if hundredths < 0 { "-" } else { "+" };
    let mag = hundredths.abs();
    format!("{sign}{}.{:02}", mag / 100, mag % 100)
}

/// One integer axis token (`+10 mag`), or `None` when the axis is unchanged.
pub(crate) fn axis_i(v: i32, unit: &str) -> Option<String> {
    (v != 0).then(|| format!("{v:+} {unit}"))
}

/// One fixed-point axis token (`+2.00 rng`), or `None` when the axis is unchanged.
pub(crate) fn axis_f(f: Fixed, unit: &str) -> Option<String> {
    (f != Fixed::ZERO).then(|| format!("{} {unit}", fixed_signed(f)))
}

/// A compact, ASCII, signed readout of the REAL per-axis numbers a [`StatDelta`] moves — the
/// gunsmith's "what does this option actually cost and buy" line. Lists only the axes an option
/// touches (each slot's trade is disjoint, so a single option shows exactly its two poles), e.g.
/// `+6.00 dmg  -60 res` for a Heavy barrel. Empty (all-zero, e.g. a `Standard` option or the
/// cosmetic Grip) reads `no change`. Pure + static → unit-tested (mirrors [`slot_trade_hint`]); the
/// numeric deltas come from `core::gunsmith`, so this is the single legible surface for them.
pub(crate) fn stat_delta_summary(d: &StatDelta) -> String {
    let parts: Vec<String> = [
        axis_f(d.range, "rng"),
        axis_f(d.damage, "dmg"),
        axis_i(d.cooldown_ticks, "cd"),
        axis_i(d.mag_size, "mag"),
        axis_i(d.reload_ticks, "rld"),
        axis_i(d.reserve, "res"),
        axis_f(d.move_speed_delta, "spd"),
        axis_f(d.cone_cos_delta, "aim"),
        axis_f(d.supp_out_delta, "supp"),
        axis_f(d.falloff_delta, "fall"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        "no change".to_string()
    } else {
        parts.join("  ")
    }
}

/// The immediate-mode gunsmith / loadout screen, drawn into the root [`egui::Ui`] for the frame.
/// Reads the current selection from `editor` (host-side pre-match state — never the sim) and returns
/// the action whose control was used this frame. Layout: a centered card of the attachment slots
/// (five sim slots plus the cosmetic Grip row, D85) — each a `<` / `>` cycler over its current option
/// plus the slot's trade-axis hint — the
/// sidegrade explainer, then DONE / RESET (D81: customization-only, no Deploy). All the decision
/// logic is in the pure seam
/// ([`apply_loadout_action`], [`slot_trade_hint`], and the `core::gunsmith`-backed editor); this fn
/// is just the egui glue.
pub(crate) fn loadout_ui(ui: &mut egui::Ui, editor: &LoadoutEditor) -> Option<LoadoutAction> {
    use egui::{Button, Label, RichText};
    let mut action = None;

    egui::CentralPanel::default().show(ui, |ui| {
        let h = ui.available_height();
        ui.vertical_centered(|ui| {
            ui.add_space(h * 0.09);
            // One card wraps the WHOLE screen (banner through RESET), matching every sibling screen's
            // "everything in one card" convention — previously only the slot rows were boxed and the
            // banner/blurb/NET/buttons floated on bare ink. Opaque PANEL: the gunsmith has no backdrop.
            card_frame().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    // Screen banner + amber rule, mirroring the title hero treatment.
                    ui.label(
                        RichText::new("GUNSMITH")
                            .color(BONE)
                            .size(TYPE_HEADING)
                            .strong(),
                    );
                    ui.add_space(8.0);
                    accent_rule(ui, 100.0);
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(
                            "Every attachment is a sidegrade -- it spends one stat to buy another. \
                             No build is strictly better than any other.",
                        )
                        .color(ASH)
                        .size(TYPE_BODY),
                    );
                    ui.add_space(20.0);

                    // Column headers so the six rows read as one table, not six independent stacks —
                    // same fixed widths as the data rows below (the two blank cells sit over the
                    // `<`/`>` cycler columns).
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [104.0, 18.0],
                            Label::new(RichText::new("SLOT").color(MUTED).size(TYPE_CAPTION))
                                .halign(egui::Align::LEFT),
                        );
                        ui.add_sized([34.0, 18.0], Label::new(""));
                        ui.add_sized(
                            [150.0, 18.0],
                            Label::new(RichText::new("PICK").color(MUTED).size(TYPE_CAPTION))
                                .halign(egui::Align::LEFT),
                        );
                        ui.add_sized([34.0, 18.0], Label::new(""));
                        ui.add_sized(
                            [172.0, 18.0],
                            Label::new(RichText::new("TRADE").color(MUTED).size(TYPE_CAPTION))
                                .halign(egui::Align::LEFT),
                        );
                        ui.add_sized(
                            [200.0, 18.0],
                            Label::new(RichText::new("NET").color(MUTED).size(TYPE_CAPTION))
                                .halign(egui::Align::LEFT),
                        );
                    });
                    ui.add_space(6.0);

                    // One aligned row per attachment slot. The on-screen index `i` is exactly the index
                    // the editor's `apply_input` routes on (`LoadoutSlot::from_index`), so the cycler
                    // maps 1:1. Every text cell is `.halign(LEFT)` so the columns hold a stable left
                    // edge as picks cycle (bare `add_sized` centres its child, jogging the "columns").
                    for (i, &slot) in LoadoutSlot::ALL.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [104.0, 32.0],
                                Label::new(
                                    RichText::new(slot.label())
                                        .color(BONE)
                                        .size(TYPE_SUBHEAD)
                                        .strong(),
                                )
                                .halign(egui::Align::LEFT),
                            );
                            if ui
                                .add_sized([34.0, 32.0], Button::new(RichText::new("<").color(BONE)))
                                .clicked()
                            {
                                action = Some(LoadoutAction::Cycle {
                                    slot_index: i,
                                    forward: false,
                                });
                            }
                            ui.add_sized(
                                [150.0, 32.0],
                                Label::new(
                                    RichText::new(editor.option_label(slot))
                                        .color(AMBER)
                                        .size(TYPE_BODY)
                                        .strong(),
                                )
                                .halign(egui::Align::LEFT),
                            );
                            if ui
                                .add_sized([34.0, 32.0], Button::new(RichText::new(">").color(BONE)))
                                .clicked()
                            {
                                action = Some(LoadoutAction::Cycle {
                                    slot_index: i,
                                    forward: true,
                                });
                            }
                            ui.add_sized(
                                [172.0, 32.0],
                                Label::new(
                                    RichText::new(slot_trade_hint(slot))
                                        .color(MUTED)
                                        .size(TYPE_CAPTION),
                                )
                                .halign(egui::Align::LEFT),
                            );
                            // The REAL per-option trade numbers (D60/M3): they change as the slot
                            // cycles, so the sidegrade is legible ("+6.00 dmg  -60 res"), not just an
                            // axis pair.
                            ui.add_sized(
                                [200.0, 32.0],
                                Label::new(
                                    RichText::new(stat_delta_summary(&editor.option_delta(slot)))
                                        .color(ASH)
                                        .size(TYPE_CAPTION),
                                )
                                .halign(egui::Align::LEFT),
                            );
                        });
                        if i + 1 < LoadoutSlot::ALL.len() {
                            ui.add_space(8.0);
                        }
                    }

                    ui.add_space(14.0);
                    accent_rule(ui, 200.0);
                    ui.add_space(10.0);
                    // Build-wide net delta (the sum of the sim slots' trades). By the sidegrade rule
                    // it is never a flat upgrade over the baseline — surfacing it makes that legible.
                    ui.label(
                        RichText::new(format!("NET  {}", stat_delta_summary(&editor.net_delta())))
                            .color(AMBER)
                            .size(TYPE_CAPTION)
                            .strong(),
                    );
                    ui.add_space(16.0);
                    // D81: customization-only — DONE returns to Settings (the entry point), RESET
                    // clears to baseline. There is no Deploy here: the mode/mission-select screens
                    // start matches.
                    if menu_button(ui, "DONE", Emphasis::Primary) {
                        action = Some(LoadoutAction::Done);
                    }
                    // RESET wipes every attachment back to Standard — a real misclick target next to
                    // DONE, so it takes two clicks (arm, then confirm) AND gets a wider gap so the
                    // armed amber prompt can't be mistaken for DONE.
                    ui.add_space(24.0);
                    if confirm_menu_button(
                        ui,
                        "loadout.reset",
                        "RESET",
                        "RESET? CLICK AGAIN",
                        Emphasis::Secondary,
                    ) {
                        action = Some(LoadoutAction::Reset);
                    }
                });
            });
        });
    });

    action
}
