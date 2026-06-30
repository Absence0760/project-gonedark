package com.jaredhoward.goingdark

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for the pure gunsmith seam ([LoadoutSelection]). The Compose UI that consumes it
 * ([GunsmithScreen]) is device-gated chrome (D32) and exempt; the selection/label/cycle logic is
 * testable here with no device — so it is tested (CLAUDE.md testing rule).
 *
 * These also pin the D79 mirror against `core/src/gunsmith.rs`: the option labels, their order
 * (index 0/1/2 = wire index), and `SLOT_MAX` must match the Rust source and [LaunchConfig].
 */
class LoadoutSelectionTest {
    @Test
    fun default_is_all_standard() {
        val sel = LoadoutSelection()
        assertEquals(0, sel.optic)
        assertEquals(0, sel.barrel)
        assertEquals(0, sel.magazine)
        for (slot in Slot.entries) {
            assertEquals("Standard", LoadoutSelection.label(slot, sel.index(slot)))
        }
    }

    @Test
    fun cycle_forward_wraps_0_1_2_0_on_each_slot() {
        for (slot in Slot.entries) {
            var sel = LoadoutSelection()
            assertEquals(0, sel.index(slot))
            sel = sel.cycle(slot, forward = true)
            assertEquals(1, sel.index(slot))
            sel = sel.cycle(slot, forward = true)
            assertEquals(2, sel.index(slot))
            sel = sel.cycle(slot, forward = true)
            assertEquals("forward wraps 2 -> 0", 0, sel.index(slot))
        }
    }

    @Test
    fun cycle_backward_wraps_0_2_1_0_on_each_slot() {
        for (slot in Slot.entries) {
            var sel = LoadoutSelection()
            sel = sel.cycle(slot, forward = false)
            assertEquals("backward wraps 0 -> 2", 2, sel.index(slot))
            sel = sel.cycle(slot, forward = false)
            assertEquals(1, sel.index(slot))
            sel = sel.cycle(slot, forward = false)
            assertEquals(0, sel.index(slot))
        }
    }

    @Test
    fun cycle_forward_then_backward_is_identity() {
        for (slot in Slot.entries) {
            for (start in 0..LoadoutSelection.SLOT_MAX) {
                val sel = LoadoutSelection().withIndex(slot, start)
                assertEquals(sel, sel.cycle(slot, forward = true).cycle(slot, forward = false))
                assertEquals(sel, sel.cycle(slot, forward = false).cycle(slot, forward = true))
            }
        }
    }

    @Test
    fun cycle_touches_only_the_named_slot() {
        // Cycling Optic must not move Barrel or Magazine, etc.
        val base = LoadoutSelection(optic = 0, barrel = 1, magazine = 2)
        val opticCycled = base.cycle(Slot.Optic, forward = true)
        assertEquals(1, opticCycled.optic)
        assertEquals(1, opticCycled.barrel)
        assertEquals(2, opticCycled.magazine)
    }

    @Test
    fun labels_mirror_the_rust_source_order() {
        // core/src/gunsmith.rs: each enum's ALL order is index 0/1/2.
        assertEquals(listOf("Standard", "Marksman", "Close-Quarters"),
            (0..LoadoutSelection.SLOT_MAX).map { LoadoutSelection.label(Slot.Optic, it) })
        assertEquals(listOf("Standard", "Heavy", "Light"),
            (0..LoadoutSelection.SLOT_MAX).map { LoadoutSelection.label(Slot.Barrel, it) })
        assertEquals(listOf("Standard", "Extended", "Quickdraw"),
            (0..LoadoutSelection.SLOT_MAX).map { LoadoutSelection.label(Slot.Magazine, it) })
    }

    @Test
    fun trade_hints_mirror_the_desktop_strings_verbatim() {
        // ASCII `<->`, byte-identical to app/src/shell.rs::slot_trade_hint — not the `↔` glyph — so
        // the two shells show the same hint text (D79 mirror; a glyph swap trips this).
        assertEquals("range <-> fire-rate", LoadoutSelection.tradeHint(Slot.Optic))
        assertEquals("damage <-> reserve", LoadoutSelection.tradeHint(Slot.Barrel))
        assertEquals("capacity <-> handling", LoadoutSelection.tradeHint(Slot.Magazine))
    }

    @Test
    fun reset_returns_the_neutral_all_standard_baseline() {
        // RESET (mirrors LoadoutEditor::reset) drops every slot back to index 0 from any build.
        val built = LoadoutSelection(optic = 1, barrel = 2, magazine = 1)
        assertEquals(LoadoutSelection(), built.reset())
        assertEquals(LoadoutSelection.STANDARD, built.reset())
        for (slot in Slot.entries) {
            assertEquals(0, built.reset().index(slot))
        }
        // Reset of the baseline is a no-op.
        assertEquals(LoadoutSelection(), LoadoutSelection().reset())
    }

    @Test
    fun slot_max_equals_launch_config_slot_max() {
        // D79 mirror: the indices this screen emits ARE LaunchConfig's opt/bar/mag wire values.
        assertEquals(LaunchConfig.SLOT_MAX, LoadoutSelection.SLOT_MAX)
    }

    @Test
    fun every_slot_has_exactly_slot_max_plus_one_options() {
        for (slot in Slot.entries) {
            // Cycling forward SLOT_MAX+1 times returns to the start — exactly OPTION_COUNT options.
            var sel = LoadoutSelection()
            repeat(LoadoutSelection.OPTION_COUNT) { sel = sel.cycle(slot, forward = true) }
            assertEquals(0, sel.index(slot))
        }
        assertEquals(LoadoutSelection.SLOT_MAX + 1, LoadoutSelection.OPTION_COUNT)
    }

    @Test
    fun indices_stay_in_range_under_any_cycling() {
        // Walk every slot through many cycles in both directions; index never leaves 0..SLOT_MAX.
        for (slot in Slot.entries) {
            var sel = LoadoutSelection()
            for (step in 0 until 25) {
                sel = sel.cycle(slot, forward = step % 2 == 0)
                val idx = sel.index(slot)
                assertTrue("index $idx out of 0..${LoadoutSelection.SLOT_MAX}",
                    idx in 0..LoadoutSelection.SLOT_MAX)
            }
        }
    }

    @Test
    fun with_index_clamps_out_of_range_input() {
        for (slot in Slot.entries) {
            assertEquals(0, LoadoutSelection().withIndex(slot, -5).index(slot))
            assertEquals(LoadoutSelection.SLOT_MAX,
                LoadoutSelection().withIndex(slot, 99).index(slot))
        }
    }
}
