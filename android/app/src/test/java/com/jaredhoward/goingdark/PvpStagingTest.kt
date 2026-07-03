package com.jaredhoward.goingdark

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for the PvP staging seam (PvpStaging.kt) — the Kotlin mirror of the desktop
 * `app/src/shell/pvp.rs` tests (D79/D101). The Compose screen consuming it is device-gated chrome
 * and exempt; the honesty rule and the queue table are the testable logic, so they are tested.
 */
class PvpStagingTest {
    @Test
    fun no_queue_is_joinable_before_the_net_layer() {
        // The staging screen's honesty rule as a tested invariant (D101): with no Phase 3 session
        // transport, nothing on the PvP door may present as joinable. When the custom lobby lands,
        // this test is what changes (per-queue), not the screen's structure.
        for (queue in pvpQueues) {
            assertFalse("queue ${queue.id} reads joinable with no net layer", queueJoinable(queue))
        }
    }

    @Test
    fun queues_are_the_three_doors_in_build_order() {
        // The table mirrors `modes.md` §5: custom lobby first (the first real PvP surface), then
        // quick, then ranked — and the desktop PVP_QUEUES verbatim (D79 mirrored constants).
        assertEquals(3, pvpQueues.size)
        assertEquals(listOf("custom", "quick", "ranked"), pvpQueues.map { it.id })
        assertEquals("FIRST UP", pvpQueues.first().status)
    }

    @Test
    fun queue_table_is_distinct_ascii_and_complete() {
        // The mode-table hygiene rule (the `GameModeTest` guard, applied here): every field is
        // non-empty ASCII and every tile is uniquely keyed.
        for (q in pvpQueues) {
            for (field in listOf(q.id, q.name, q.blurb, q.status)) {
                assertTrue("field of ${q.id} must be non-empty ASCII", field.isNotEmpty())
                assertTrue("field of ${q.id} must be ASCII", field.all { it.code in 32..126 })
            }
        }
        assertEquals(pvpQueues.size, pvpQueues.map { it.id }.toSet().size)
    }
}
