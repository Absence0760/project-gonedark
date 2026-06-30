package com.jaredhoward.goingdark

/**
 * Pure data for the About / field-manual surface — the Compose port of the desktop egui About
 * screen (`app/src/shell.rs` `draw_about` / `controls_reference`, parity plan Tier 1, About row).
 *
 * Kept **free of any Android / Compose types** so it is unit-testable on the plain JVM
 * (`src/test`, no device) — the same testable-seam pattern as `BuildStamp.kt`. The composable
 * (`AboutScreen.kt`) is device-gated chrome (D32) and exempt from tests; this content is real data
 * and gets a shape test (`FieldManualTest.kt`, CLAUDE.md testing rule).
 *
 * The keymap mirrors the desktop default keymap. Bindings name the *physical* desktop inputs
 * (keyboard/mouse) so the reference stays the single source of truth across platforms; the on-touch
 * control scheme is a Phase-4 surface (parity plan §4) and will extend this data when it lands.
 */

/** One field-manual row: a player action and the input that performs it. */
data class ControlRow(val action: String, val binding: String)

/** A grouped block of the field manual: a heading and its rows (a keymap layer, or the concept blurb). */
data class ManualSection(val title: String, val rows: List<ControlRow>)

/**
 * One-paragraph "what is this game" framing — the desktop About pitch line, matching the
 * game-design glossary (Embodiment, Going dark).
 */
const val FIELD_MANUAL_BLURB: String =
    "Command and grow your camps from above, then possess a single soldier and fight it in first " +
        "person while the strategic map goes dark. One commander does both jobs; the tension is " +
        "your divided attention. Stay embodied too long and the map you left behind moves without you."

/**
 * The field manual, grouped by layer. Mirrors the desktop `controls_reference()` rows
 * (COMMAND / EMBODIED / GLOBAL) plus a leading concept section. Static — populated here so the
 * composable stays pure presentation.
 */
val fieldManualSections: List<ManualSection> = listOf(
    ManualSection(
        title = "GOING DARK",
        rows = listOf(
            ControlRow("Embodiment", "Possess one unit and fight it in first person"),
            ControlRow("Going dark", "Embodying blacks out the strategic map — alerts, not intel"),
            ControlRow("Surface", "Eject back to command; death also ejects you (no respawn)"),
            ControlRow("Stay fair", "While dark you get a directional flash + audio, never a map reveal"),
        ),
    ),
    ManualSection(
        title = "COMMAND",
        rows = listOf(
            ControlRow("Select / band-select", "Left-click"),
            ControlRow("Move or attack-move the selection", "Right-click"),
            ControlRow("Place a Camp at the cursor", "B"),
            ControlRow("Queue a Rifleman / Heavy at the camp", "R / H"),
            ControlRow("Upgrade the active camp", "U"),
            ControlRow("Order / stance vocabulary slots", "1 - 0"),
        ),
    ),
    ManualSection(
        title = "EMBODIED",
        rows = listOf(
            ControlRow("Embody the targeted unit", "E"),
            ControlRow("Surface (eject back to command)", "Q"),
            ControlRow("Move", "W A S D"),
            ControlRow("Look", "Mouse"),
            ControlRow("Fire", "Left-click / Space"),
        ),
    ),
    ManualSection(
        title = "GLOBAL",
        rows = listOf(
            ControlRow("Pause / resume", "Esc"),
            ControlRow("Free the cursor (hold)", "Left Alt"),
            ControlRow("Toggle fullscreen", "F11"),
            ControlRow("Toggle the debug overlay", "F3"),
        ),
    ),
)
