package com.jaredhoward.goingdark

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for the atlas-grouping seam (CampaignModel.kt: [hubSections], the group rollups,
 * and the header labels) — the Kotlin mirror of the desktop `hub_sections` tests
 * (`app/src/shell/tests.rs`, D98/D79). The grouped Compose rendering in [MissionSelectScreen] is
 * exempt glue; the ordering/rollup decisions it draws are what's pinned here.
 */
class HubSectionsTest {
    /** A two-conflict atlas with an empty (content-pending) operation and one ungrouped node. */
    private val conflicts = listOf(
        Conflict(0, "The Channel Crisis", 2027, 2028, "first"),
        Conflict(1, "The Second Front", 2029, 2029, "second"),
    )
    private val operations = listOf(
        Operation(id = 0, conflict = 0, name = "Operation First Light"),
        Operation(id = 1, conflict = 0, name = "Operation Empty"), // content-pending
        Operation(id = 2, conflict = 1, name = "Operation Landfall"),
    )
    private val nodes = listOf(
        MissionNode(0, "Seize the Outpost", "seize", "Seize it.", operation = 0),
        MissionNode(1, "Hold the Line", "hold", "Hold it.", prerequisites = listOf(0), operation = 0),
        MissionNode(2, "Break the Line", "push", "Push through.", prerequisites = listOf(1), operation = 2),
        MissionNode(3, "Side Story", "seize", "Ungrouped extra.", prerequisites = listOf(0)),
    )

    private fun campaign(cleared: Map<Int, Difficulty> = emptyMap()) =
        CampaignProgress(nodes = nodes, clearedByNode = cleared)

    @Test
    fun sections_follow_authored_order_and_skip_empty_operations() {
        val sections = hubSections(campaign(), conflicts, operations)
        // Op 0 (conflict 0), op 2 (conflict 1), then the trailing ungrouped section — the empty
        // op 1 contributes no header scaffolding.
        assertEquals(3, sections.size)
        assertEquals(listOf(0, 1), sections[0].nodes.map { it.id })
        assertEquals(listOf(2), sections[1].nodes.map { it.id })
        assertEquals(listOf(3), sections[2].nodes.map { it.id })
    }

    @Test
    fun the_conflict_header_draws_once_per_conflict_and_never_on_the_ungrouped_tail() {
        val sections = hubSections(campaign(), conflicts, operations)
        assertEquals("The Channel Crisis", sections[0].conflict?.first?.name)
        assertEquals("The Second Front", sections[1].conflict?.first?.name)
        assertNull(sections[2].conflict)
        assertNull(sections[2].operation)
    }

    @Test
    fun a_multi_operation_conflict_draws_its_header_on_the_first_section_only() {
        // Give conflict 0 a second non-empty operation: the header must not repeat.
        val ops = operations + Operation(id = 3, conflict = 0, name = "Operation Encore")
        val withEncore = nodes + MissionNode(4, "Encore", "seize", "Again.", operation = 3)
        val sections = hubSections(CampaignProgress(nodes = withEncore), conflicts, ops)
        val conflictZeroSections = sections.filter { it.operation?.first?.conflict == 0 }
        assertEquals(2, conflictZeroSections.size)
        assertNotNull(conflictZeroSections[0].conflict)
        assertNull(conflictZeroSections[1].conflict)
    }

    @Test
    fun rollups_count_clears_and_report_playability() {
        val sections = hubSections(campaign(mapOf(0 to Difficulty.Regular)), conflicts, operations)
        // Op 0: node 0 cleared of 2; playable (node 1 unlocked by the clear).
        val opZero = sections[0].operation!!.second
        assertEquals(GroupProgress(cleared = 1, total = 2, playable = true), opZero)
        assertFalse(opZero.isComplete)
        // Conflict 1 / op 2: node 2 still gated behind node 1 — not playable, nothing cleared.
        val opTwo = sections[1].operation!!.second
        assertEquals(GroupProgress(cleared = 0, total = 1, playable = false), opTwo)
        // Conflict 0's rollup spans its operations' nodes only (the ungrouped node 3 is excluded).
        assertEquals(
            GroupProgress(cleared = 1, total = 2, playable = true),
            sections[0].conflict!!.second,
        )
    }

    @Test
    fun an_atlas_less_campaign_degrades_to_one_untitled_section() {
        // No conflicts/operations at all → exactly the pre-atlas flat list, in id order.
        val flatNodes = nodes.map { it.copy(operation = null) }
        val sections = hubSections(CampaignProgress(nodes = flatNodes), emptyList(), emptyList())
        assertEquals(1, sections.size)
        assertNull(sections[0].conflict)
        assertNull(sections[0].operation)
        assertEquals(flatNodes.map { it.id }, sections[0].nodes.map { it.id })
    }

    @Test
    fun header_labels_match_the_desktop_formatting() {
        // Year span collapses when start == end; names uppercase; the U+00B7 separator — identical
        // to the desktop `conflict_header_label` / `operation_header_label` output.
        assertEquals(
            "THE CHANNEL CRISIS · 2027-2028 · 0/2",
            conflictHeaderLabel(conflicts[0], GroupProgress(0, 2, true)),
        )
        assertEquals(
            "THE SECOND FRONT · 2029 · 1/1",
            conflictHeaderLabel(conflicts[1], GroupProgress(1, 1, true)),
        )
        assertEquals(
            "OPERATION FIRST LIGHT · 0/2",
            operationHeaderLabel(operations[0], GroupProgress(0, 2, true)),
        )
    }

    @Test
    fun the_shipped_campaign_groups_one_section_per_conflict() {
        // The shipped tables (D105 + D121 Normandy): five conflicts, one operation each, every
        // node grouped — so the hub renders one headed section per war and no ungrouped tail. Pins
        // the authored data's atlas integrity.
        val sections = hubSections(CampaignProgress())
        assertEquals(5, sections.size)
        assertEquals(campaignConflicts.map { it.name }, sections.map { it.conflict?.first?.name })
        assertEquals(campaignOperations.map { it.name }, sections.map { it.operation?.first?.name })
        // Each section carries its own conflict's three battles, in authored order, covering the
        // whole node list with no ungrouped tail.
        assertEquals(campaignNodes.map { it.id }, sections.flatMap { s -> s.nodes.map { it.id } })
        sections.forEach { section ->
            assertEquals(3, section.nodes.size)
            assertTrue("every war's root chain starts playable", section.operation!!.second.playable)
        }
    }
}
