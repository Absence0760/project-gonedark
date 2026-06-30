package com.jaredhoward.goingdark

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for the pure campaign model (CampaignModel.kt) — the testable seam the Compose
 * campaign screens are exempt from (device-gated chrome, D32). These pin the **mirrored constants**
 * shared with the Rust `core::campaign::Difficulty` / `engine::default_campaign()` (D79): if the
 * Kotlin drifts from the Rust contract — a renamed tier, a changed id string, a broken cycle — a test
 * trips here rather than the two shells silently disagreeing.
 */
class CampaignModelTest {
    @Test
    fun next_wraps_through_all_four_tiers_in_order() {
        // Recruit → Regular → Veteran → Elite → Recruit (mirrors desktop next_difficulty).
        assertEquals(Difficulty.Regular, Difficulty.Recruit.next())
        assertEquals(Difficulty.Veteran, Difficulty.Regular.next())
        assertEquals(Difficulty.Elite, Difficulty.Veteran.next())
        assertEquals(Difficulty.Recruit, Difficulty.Elite.next())

        // Cycling four times from any tier returns to it (a clean 4-cycle, no fixed point).
        for (start in Difficulty.entries) {
            var d = start
            repeat(4) { d = d.next() }
            assertEquals(start, d)
        }
    }

    @Test
    fun tier_and_fromTier_round_trip() {
        for (d in Difficulty.entries) {
            assertEquals(d, Difficulty.fromTier(d.tier()))
        }
        // Ranks are exactly 0..3 in ascending order.
        assertEquals(0, Difficulty.Recruit.tier())
        assertEquals(1, Difficulty.Regular.tier())
        assertEquals(2, Difficulty.Veteran.tier())
        assertEquals(3, Difficulty.Elite.tier())
    }

    @Test
    fun fromTier_rejects_out_of_range() {
        // A corrupt / foreign rank is rejected (null), never guessed — mirrors Rust from_tier.
        assertNull(Difficulty.fromTier(-1))
        assertNull(Difficulty.fromTier(4))
        assertNull(Difficulty.fromTier(99))
    }

    @Test
    fun id_strings_match_the_rust_contract() {
        // These exact strings mirror core::campaign::Difficulty::id() — part of the cross-shell seam.
        assertEquals("recruit", Difficulty.Recruit.id())
        assertEquals("regular", Difficulty.Regular.id())
        assertEquals("veteran", Difficulty.Veteran.id())
        assertEquals("elite", Difficulty.Elite.id())

        // Ids are unique and stable across the whole set.
        val ids = Difficulty.entries.map { it.id() }
        assertEquals(ids.size, ids.toSet().size)
    }

    @Test
    fun labels_are_present_for_every_tier() {
        assertEquals("Recruit", Difficulty.Recruit.label())
        assertEquals("Regular", Difficulty.Regular.label())
        assertEquals("Veteran", Difficulty.Veteran.label())
        assertEquals("Elite", Difficulty.Elite.label())
    }

    @Test
    fun campaign_nodes_non_empty_and_carry_the_seize_mission() {
        assertTrue("campaign ships at least one node", campaignNodes.isNotEmpty())

        // The one shipped playable node mirrors engine::default_campaign(): the Seize mission.
        val seize = campaignNodes.firstOrNull { it.sceneToken == "mission1" }
        assertNotNull("a node wired to scene token mission1", seize)
        assertTrue("mission name is non-blank", seize!!.name.isNotBlank())
        assertTrue("briefing copy is non-blank", seize.briefing.isNotBlank())
    }
}
