//! The About / field-manual screen — the one-line pitch, the real default keymap (grouped), and the
//! build stamp. The static control-reference data is a pure, unit-tested seam; the renderer is glue.

use crate::shell::theme::*;
use crate::shell::widgets::*;

/// One control-reference row: the input and what it does, grouped by layer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ControlRow {
    /// The layer this binding belongs to ("COMMAND", "EMBODIED", "GLOBAL").
    pub group: &'static str,
    /// The key/mouse input (ASCII only — it renders in egui's default font).
    pub keys: &'static str,
    /// What the input does.
    pub action: &'static str,
}

/// The one-paragraph "what is this game" pitch shown atop the About / field-manual screen — the
/// canonical blurb, kept **verbatim** in step with Android's `FIELD_MANUAL_BLURB`
/// (`FieldManual.kt`) so both shells read identically (A2 parity). Android's fuller three-sentence
/// copy is the source of truth; this must match it byte-for-byte. Pure `&'static str`, so it's
/// unit-tested (non-empty, ASCII — never a tofu glyph in egui's default font).
pub(crate) const FIELD_MANUAL_BLURB: &str =
    "Command and grow your camps from above, then possess a single soldier and fight it in first \
     person while the strategic map goes dark. One commander does both jobs; the tension is your \
     divided attention. Stay embodied too long and the map you left behind moves without you.";

/// The desktop controls reference shown on the About screen — the **real** default keymap (kept in
/// sync with `pal-desktop`'s `DesktopInput` doc + `app`'s host keys). Static data, so it's unit-tested
/// for shape (every group present, no empty cells). ASCII only — never a tofu glyph.
///
/// The list is **prefixed by a non-keybinding "GOING DARK" concept section** (A1 parity with
/// Android's `fieldManualSections`): those rows reuse the `ControlRow` shape with the concept name in
/// the `keys` column and its one-line framing in `action`, so the grouped `about_ui` renderer draws
/// them ahead of the COMMAND/EMBODIED/GLOBAL keymap groups needing no special case for grouping or
/// ordering. (`about_ui` *does* special-case their *styling* — plain amber text, not a keycap chip —
/// since a chip would imply a pressable key.) Content is Android's verbatim, with its "Going dark"
/// em-dash rendered as ASCII `--` (the file's default-font/no-tofu rule is the one deviation).
pub(crate) fn controls_reference() -> &'static [ControlRow] {
    const fn row(group: &'static str, keys: &'static str, action: &'static str) -> ControlRow {
        ControlRow {
            group,
            keys,
            action,
        }
    }
    // A `static` (not a returned temporary) so the slice is genuinely `'static`.
    static ROWS: &[ControlRow] = &[
        // The "GOING DARK" concept block — the game's framing ahead of the keymap (mirrors Android's
        // `fieldManualSections` leading section). Not keybindings: the `keys` cell is the concept
        // name, `action` its one-line explanation.
        row("GOING DARK", "Embodiment", "Possess one unit and fight it in first person"),
        row("GOING DARK", "Going dark", "Embodying blacks out the strategic map -- alerts, not intel"),
        row("GOING DARK", "Surface", "Eject back to command; death also ejects you (no respawn)"),
        row("GOING DARK", "Stay fair", "While dark you get a directional flash + audio, never a map reveal"),
        // Command layer (RTS) — pal-desktop keymap (D42 classic-RTS split).
        row("COMMAND", "Left-click", "Select / band-select"),
        row("COMMAND", "Right-click", "Move or attack-move the selection"),
        row("COMMAND", "B", "Place a Camp at the cursor"),
        row("COMMAND", "R / H", "Queue a Rifleman / Heavy at the camp"),
        row("COMMAND", "U", "Upgrade the active camp"),
        row("COMMAND", "1 - 0", "Order / stance vocabulary slots"),
        // Embodiment layer (FPS).
        row("EMBODIED", "E", "Embody the targeted unit"),
        row("EMBODIED", "Q", "Surface (eject back to command)"),
        row("EMBODIED", "W A S D", "Move"),
        row("EMBODIED", "Mouse", "Look"),
        row("EMBODIED", "Left-click / Space", "Fire"),
        // Global host keys (app/src/main.rs).
        row("GLOBAL", "Esc", "Pause / resume"),
        row("GLOBAL", "Left Alt", "Free the cursor (hold)"),
        row("GLOBAL", "F11", "Toggle fullscreen"),
        row("GLOBAL", "F3", "Toggle the debug overlay"),
    ];
    ROWS
}

/// The immediate-mode About / field-manual screen: the one-line pitch, the real default keymap
/// (grouped), and the build stamp, centred over the backdrop. Returns `true` on BACK. Static content
/// from the pure [`controls_reference`] seam. Glue.
pub(crate) fn about_ui(ui: &mut egui::Ui, stamp: &str) -> bool {
    use egui::{Grid, RichText};
    let mut back = false;

    over_backdrop_screen(ui, 0.06, |ui| {
        screen_banner(ui, "FIELD MANUAL", 120.0);
        ui.label(RichText::new(FIELD_MANUAL_BLURB).color(ASH).size(TYPE_BODY));
        ui.add_space(14.0);

        // The keymap, grouped by layer. ONE Grid per group so every row's key column shares a single
        // width (a per-row Grid let a wide cell like "Left-click / Space" jog only its own row's
        // action column). No nested ScrollArea — `over_backdrop_screen`'s own scroll handles a short
        // window, matching every sibling screen. Left-anchored so headings/rows share one margin.
        ui.vertical(|ui| {
            for (gi, group) in controls_reference().chunk_by(|a, b| a.group == b.group).enumerate() {
                if gi > 0 {
                    section_divider(ui);
                }
                section_label(ui, group[0].group);
                // The leading GOING DARK block holds concept names in the `keys` column, not real
                // key bindings — render those as plain amber text; a keycap chip there would imply a
                // pressable key that doesn't exist. The literal COMMAND/EMBODIED/GLOBAL rows get chips.
                let is_concept = group[0].group == "GOING DARK";
                Grid::new(("about.controls", group[0].group))
                    .num_columns(2)
                    .min_col_width(if is_concept { 92.0 } else { 96.0 })
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        for row in group {
                            if is_concept {
                                ui.label(
                                    RichText::new(row.keys).color(AMBER).size(TYPE_BODY).strong(),
                                );
                            } else {
                                keycap_chip(ui, row.keys);
                            }
                            ui.label(RichText::new(row.action).color(BONE).size(TYPE_BODY));
                            ui.end_row();
                        }
                    });
            }
        });

        ui.add_space(14.0);
        ui.label(RichText::new(stamp).color(MUTED).size(TYPE_CAPTION));
        ui.add_space(12.0);
        if menu_button(ui, "BACK", Emphasis::Primary) {
            back = true;
        }
    });

    back
}
