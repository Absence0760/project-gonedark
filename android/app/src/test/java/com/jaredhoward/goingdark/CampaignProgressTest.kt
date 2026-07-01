package com.jaredhoward.goingdark

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for the pure campaign **progress** seam ([CampaignProgress] / [NodeProgress] /
 * [CampaignResult] in CampaignModel.kt) — the native twin of `core::campaign::Campaign`'s read/clear
 * surface. The Compose campaign screens are device-gated chrome (D32) and exempt, so the
 * lock/unlock/clear transitions, best-tier tracking, the persistence codec, and the win result-code
 * packing are all tested here off-device (CLAUDE.md testing rule). These mirror the Rust
 * `core::campaign` and `pal-android/src/launch.rs` contracts so both ends stay in lock-step (D79).
 */
class CampaignProgressTest {

    // A small chain campaign A -> B -> C (each gates the next), mirroring the Rust `chain()` fixture.
    private fun chain(): CampaignProgress = CampaignProgress(
        nodes = listOf(
            MissionNode(0, "A", "sceneA", "take the outpost"),
            MissionNode(1, "B", "sceneB", "hold the ridge", prerequisites = listOf(0)),
            MissionNode(2, "C", "sceneC", "seize the base", prerequisites = listOf(1)),
        ),
    )

    // A diamond: A unlocks B and C; D needs BOTH B and C cleared.
    private fun diamond(): CampaignProgress = CampaignProgress(
        nodes = listOf(
            MissionNode(0, "A", "a", ""),
            MissionNode(1, "B", "b", "", prerequisites = listOf(0)),
            MissionNode(2, "C", "c", "", prerequisites = listOf(0)),
            MissionNode(3, "D", "d", "", prerequisites = listOf(1, 2)),
        ),
    )

    // ---- lock / unlock derivation --------------------------------------------------------------

    @Test
    fun roots_available_successors_locked_at_start() {
        val c = chain()
        assertEquals(NodeProgress.Available, c.progress(0))
        assertEquals(NodeProgress.Locked, c.progress(1))
        assertEquals(NodeProgress.Locked, c.progress(2))
        assertTrue(c.isUnlocked(0))
        assertFalse(c.isUnlocked(1))
    }

    @Test
    fun shipped_default_campaign_root_is_available() {
        // The shipped root node (Seize) has no prerequisites → immediately playable.
        val c = CampaignProgress()
        assertEquals(NodeProgress.Available, c.progress(0))
        assertTrue(c.progress(0).isPlayable)
    }

    @Test
    fun shipped_default_campaign_gates_hold_behind_seize() {
        // The WS-B 2-node chain over the real `campaignNodes`: Hold (node 1) is Locked until Seize
        // (node 0) is cleared, then unlocks and stays replayable — mirrors engine::default_campaign().
        var c = CampaignProgress()
        assertEquals(NodeProgress.Available, c.progress(0))
        assertEquals(NodeProgress.Locked, c.progress(1))
        assertFalse("Hold cannot be launched while locked", c.progress(1).isPlayable)

        c = c.recordClear(0, Difficulty.Veteran)
        assertEquals(NodeProgress.Cleared(Difficulty.Veteran), c.progress(0))
        assertEquals(NodeProgress.Available, c.progress(1))
        assertTrue("Hold unlocks once Seize is cleared", c.progress(1).isPlayable)

        // The cleared set round-trips over the shipped topology (Hold stays unlocked across a restart).
        val restored = CampaignProgress.decodeCleared(c.encodeCleared())
        assertEquals(NodeProgress.Available, restored.progress(1))
    }

    @Test
    fun clearing_a_node_opens_its_successor_and_only_it() {
        var c = chain()
        c = c.recordClear(0, Difficulty.Recruit)
        assertEquals(NodeProgress.Cleared(Difficulty.Recruit), c.progress(0))
        assertEquals(NodeProgress.Available, c.progress(1))
        assertEquals(NodeProgress.Locked, c.progress(2)) // C still needs B
        c = c.recordClear(1, Difficulty.Recruit)
        assertEquals(NodeProgress.Available, c.progress(2))
    }

    @Test
    fun diamond_requires_all_prerequisites_cleared() {
        var c = diamond()
        c = c.recordClear(0, Difficulty.Regular)
        assertEquals(NodeProgress.Available, c.progress(1))
        assertEquals(NodeProgress.Available, c.progress(2))
        assertEquals(NodeProgress.Locked, c.progress(3))
        c = c.recordClear(1, Difficulty.Regular) // B alone doesn't open D
        assertEquals(NodeProgress.Locked, c.progress(3))
        c = c.recordClear(2, Difficulty.Regular) // C — the last prerequisite — opens D
        assertEquals(NodeProgress.Available, c.progress(3))
    }

    // ---- clear gate + best-tier tracking -------------------------------------------------------

    @Test
    fun clearing_a_locked_node_is_rejected() {
        val c = chain()
        val after = c.recordClear(1, Difficulty.Recruit) // B is locked
        assertEquals(c, after) // unchanged
        assertFalse(after.isCleared(1))
    }

    @Test
    fun clearing_an_unknown_node_is_rejected() {
        val c = chain()
        assertEquals(c, c.recordClear(99, Difficulty.Recruit))
    }

    @Test
    fun replay_keeps_best_and_never_demotes() {
        var c = chain()
        c = c.recordClear(0, Difficulty.Regular)
        assertEquals(Difficulty.Regular, c.bestCleared(0))
        // Replay higher raises the best.
        c = c.recordClear(0, Difficulty.Elite)
        assertEquals(Difficulty.Elite, c.bestCleared(0))
        // Replay lower does NOT demote.
        c = c.recordClear(0, Difficulty.Recruit)
        assertEquals(Difficulty.Elite, c.bestCleared(0))
        // Equal tier is a no-op too (returns the same value object).
        val same = c.recordClear(0, Difficulty.Elite)
        assertEquals(c, same)
    }

    // ---- persistence codec (cleared set → string → cleared set) --------------------------------

    @Test
    fun fresh_progress_encodes_empty_and_round_trips() {
        val c = chain()
        assertEquals("", c.encodeCleared())
        assertEquals(c, CampaignProgress.decodeCleared(c.encodeCleared(), c.nodes))
    }

    @Test
    fun cleared_set_round_trips_through_the_string_blob() {
        var c = diamond()
        c = c.recordClear(0, Difficulty.Elite)
        c = c.recordClear(1, Difficulty.Regular)
        c = c.recordClear(2, Difficulty.Recruit)
        val blob = c.encodeCleared()
        val restored = CampaignProgress.decodeCleared(blob, c.nodes)
        assertEquals(c, restored)
        assertEquals(Difficulty.Elite, restored.bestCleared(0))
        assertEquals(Difficulty.Regular, restored.bestCleared(1))
        assertEquals(Difficulty.Recruit, restored.bestCleared(2))
        assertEquals(NodeProgress.Available, restored.progress(3)) // derived unlock survives
    }

    @Test
    fun decode_is_tolerant_of_garbage_and_foreign_nodes() {
        val nodes = chain().nodes
        // Garbage tokens, an unparseable id/tier, an out-of-range rank, and a node id this build
        // doesn't have are all dropped; the valid one survives.
        val decoded = CampaignProgress.decodeCleared("0:2,junk,x:1,1:9,7:0,,2:abc", nodes)
        assertEquals(Difficulty.Veteran, decoded.bestCleared(0)) // 0:2 kept
        assertNull(decoded.bestCleared(1)) // 1:9 (rank 9) dropped
        assertNull(decoded.bestCleared(2)) // 2:abc dropped
        assertFalse(decoded.isCleared(7)) // foreign node dropped
    }

    @Test
    fun decode_null_or_blank_is_no_clears() {
        assertEquals(emptyMap<Int, Difficulty>(), CampaignProgress.decodeCleared(null).clearedByNode)
        assertEquals(emptyMap<Int, Difficulty>(), CampaignProgress.decodeCleared("   ").clearedByNode)
    }

    // ---- win result-code packing (mirrors launch.rs::campaign_result_code) ---------------------

    @Test
    fun result_code_round_trips_node_and_tier() {
        for (node in 0..2) {
            for (tier in Difficulty.entries) {
                // code = 1 + node*4 + tier — the exact Rust packing.
                val code = 1 + node * 4 + tier.tier()
                val decoded = CampaignResult.fromResultCode(code)
                assertEquals(CampaignResult(node, tier), decoded)
            }
        }
        // The single shipped node (0) at each tier maps to codes 1..=4.
        assertEquals(CampaignResult(0, Difficulty.Recruit), CampaignResult.fromResultCode(1))
        assertEquals(CampaignResult(0, Difficulty.Elite), CampaignResult.fromResultCode(4))
    }

    @Test
    fun non_win_result_codes_decode_to_null() {
        assertNull(CampaignResult.fromResultCode(0)) // RESULT_CANCELED — no clear
        assertNull(CampaignResult.fromResultCode(-1)) // RESULT_OK — not a campaign win
    }

    // ---- clear-status line (mirrors desktop briefing_ui verbatim) ------------------------------

    @Test
    fun clear_status_line_mirrors_the_desktop_copy() {
        assertEquals("Locked.", clearStatusLine(NodeProgress.Locked))
        assertEquals("Not yet cleared.", clearStatusLine(NodeProgress.Available))
        assertEquals(
            "Cleared at Veteran -- replay to raise your best.",
            clearStatusLine(NodeProgress.Cleared(Difficulty.Veteran)),
        )
    }
}
