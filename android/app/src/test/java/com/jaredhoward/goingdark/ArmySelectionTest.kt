package com.jaredhoward.goingdark

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for the pure army-select model ([Army], ArmySelection.kt) — the Compose twin of the
 * desktop `app::shell` army seam. The composable that renders it ([ArmySelectScreen]) is device-gated
 * chrome (D32) and exempt, but the ordinal mapping, labels/flavour, and selectable-set logic are
 * testable here with no device (CLAUDE.md testing rule). These mirror the desktop's army tests so the
 * two ends can't drift (D79).
 */
class ArmySelectionTest {
    @Test
    fun index_matches_core_army_index_ordinal() {
        // The enum ordinal IS the wire/persist ordinal — Neutral=0, Us=1, Fr=2 (core::components::Army).
        assertEquals(0, Army.Neutral.index)
        assertEquals(1, Army.Us.index)
        assertEquals(2, Army.Fr.index)
    }

    @Test
    fun default_is_a_real_combatant_roster_not_neutral() {
        assertEquals(Army.Us, Army.DEFAULT)
        assertFalse(Army.SELECTABLE.contains(Army.Neutral))
    }

    @Test
    fun only_us_and_french_are_selectable_in_order() {
        assertEquals(listOf(Army.Us, Army.Fr), Army.SELECTABLE)
    }

    @Test
    fun from_ordinal_maps_combatants_and_collapses_neutral_or_out_of_range_to_us() {
        assertEquals(Army.Us, Army.fromOrdinal(1))
        assertEquals(Army.Fr, Army.fromOrdinal(2))
        // Neutral (0), out-of-range, and negative all collapse to the US default (never a player pick).
        assertEquals(Army.Us, Army.fromOrdinal(0))
        assertEquals(Army.Us, Army.fromOrdinal(9))
        assertEquals(Army.Us, Army.fromOrdinal(-1))
    }

    @Test
    fun from_ordinal_round_trips_every_selectable_army() {
        for (army in Army.SELECTABLE) {
            assertEquals(army, Army.fromOrdinal(army.index))
        }
    }

    @Test
    fun labels_match_the_desktop_army_label() {
        assertEquals("US Army", Army.Us.label())
        assertEquals("French Army", Army.Fr.label())
        assertEquals("Non-aligned", Army.Neutral.label())
    }

    @Test
    fun flavor_lines_are_distinct_and_non_empty() {
        val flavors = Army.SELECTABLE.map { it.flavor() }
        assertTrue(flavors.all { it.isNotBlank() })
        assertEquals(flavors.size, flavors.toSet().size) // no two are identical
    }
}
