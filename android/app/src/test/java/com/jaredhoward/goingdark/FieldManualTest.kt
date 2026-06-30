package com.jaredhoward.goingdark

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for the pure field-manual content seam (FieldManual.kt). The Compose screen that
 * renders it (AboutScreen.kt) is device-gated chrome (D32) and exempt, but the keymap/blurb data is
 * real content and is shape-tested here with no device (CLAUDE.md testing rule) — mirroring the
 * desktop `controls_reference` shape test (`app/src/shell.rs`).
 */
class FieldManualTest {
    @Test
    fun sections_list_is_non_empty() {
        assertTrue("expected at least one manual section", fieldManualSections.isNotEmpty())
    }

    @Test
    fun every_section_has_a_non_blank_title() {
        for (section in fieldManualSections) {
            assertTrue("section title is blank", section.title.isNotBlank())
        }
    }

    @Test
    fun every_section_has_at_least_one_row() {
        for (section in fieldManualSections) {
            assertTrue("section '${section.title}' has no rows", section.rows.isNotEmpty())
        }
    }

    @Test
    fun no_row_has_a_blank_action_or_binding() {
        for (section in fieldManualSections) {
            for (row in section.rows) {
                assertFalse(
                    "blank action in section '${section.title}'",
                    row.action.isBlank(),
                )
                assertFalse(
                    "blank binding in section '${section.title}' (action='${row.action}')",
                    row.binding.isBlank(),
                )
            }
        }
    }

    @Test
    fun covers_the_command_and_embodied_keymap_layers() {
        // The two core layers from the desktop keymap must be present (parity guard).
        val titles = fieldManualSections.map { it.title }
        assertTrue("missing COMMAND layer", titles.contains("COMMAND"))
        assertTrue("missing EMBODIED layer", titles.contains("EMBODIED"))
    }

    @Test
    fun blurb_is_non_blank() {
        assertTrue("concept blurb is blank", FIELD_MANUAL_BLURB.isNotBlank())
    }
}
