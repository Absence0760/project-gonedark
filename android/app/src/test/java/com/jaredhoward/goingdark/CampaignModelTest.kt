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

        // The root playable node mirrors engine::default_campaign(): the Seize mission.
        val seize = campaignNodes.firstOrNull { it.sceneToken == "mission1" }
        assertNotNull("a node wired to scene token mission1", seize)
        assertTrue("mission name is non-blank", seize!!.name.isNotBlank())
        assertTrue("briefing copy is non-blank", seize.briefing.isNotBlank())
    }

    @Test
    fun campaign_is_four_self_contained_seize_hold_push_chains() {
        // Mirrors engine::default_campaign()'s D105 graph: four conflicts, each a self-contained
        // Seize -> Hold -> Push chain (root open, gating within the conflict only). List index ==
        // node id (NodeId(i)==nodes[i]).
        assertEquals("four conflicts x three battles (D105)", 12, campaignNodes.size)

        for ((chain, root) in listOf(0, 3, 6, 9).withIndex()) {
            val seize = campaignNodes[root]
            assertEquals(root, seize.id)
            assertEquals("mission1", seize.sceneToken)
            assertTrue("chain $chain's Seize is a root (no prerequisites)", seize.prerequisites.isEmpty())

            val hold = campaignNodes[root + 1]
            assertEquals(root + 1, hold.id)
            assertEquals("mission2", hold.sceneToken)
            // Hold is gated behind its own conflict's Seize — mirrors `.requires([NodeId(root)])`.
            assertEquals(listOf(root), hold.prerequisites)

            val push = campaignNodes[root + 2]
            assertEquals(root + 2, push.id)
            assertEquals("mission3", push.sceneToken)
            // Push is gated behind its own conflict's Hold — the chain's third link.
            assertEquals(listOf(root + 1), push.prerequisites)
        }
    }

    @Test
    fun push_name_and_briefing_mirror_the_rust_source_verbatim() {
        // Pins the D79 mirror against core::mission_tuning::MISSION_THREE_BRIEFING (title +
        // situation). Like the other nodes, the briefing surface shows only `situation`.
        val push = campaignNodes.first { it.sceneToken == "mission3" }
        assertEquals("Break the Line", push.name)
        assertEquals(
            "Three posts down one lane, every one of them held. Take them in order and " +
                "hold what you take — or embody a rifle and clear the way yourself. But the post you " +
                "rush blind is the one they take back behind you.",
            push.briefing,
        )
    }

    @Test
    fun campaign_atlas_mirrors_the_rust_grouping() {
        // Pins the D79 mirror of default_campaign()'s Q28 conflict-atlas grouping — since D105
        // four fictional modern conflicts, one operation each, eras staggered 2027-2034 for the
        // atlas year scrubber. List index == id, mirroring Rust's ConflictId(i)/OperationId(i).
        assertEquals(4, campaignConflicts.size)
        assertEquals(
            listOf("The Channel Crisis", "The Meridian Crisis", "The Gotland Winter", "The Santo Crisis"),
            campaignConflicts.map { it.name },
        )
        assertEquals(
            listOf(2027 to 2028, 2029 to 2030, 2031 to 2032, 2033 to 2034),
            campaignConflicts.map { it.startYear to it.endYear },
        )
        campaignConflicts.forEachIndexed { i, conflict ->
            assertEquals(i, conflict.id)
            assertTrue("year span is not inverted", conflict.startYear <= conflict.endYear)
        }

        assertEquals(4, campaignOperations.size)
        assertEquals(
            listOf(
                "Operation First Light",
                "Operation Dry Season",
                "Operation Frostline",
                "Operation Trade Wind",
            ),
            campaignOperations.map { it.name },
        )
        campaignOperations.forEachIndexed { i, op ->
            assertEquals(i, op.id)
            assertEquals("operations link to their conflicts in order", i, op.conflict)
        }

        // Every shipped node sits in its conflict's operation — the grouping is total, three
        // battles per operation (node id / 3 == operation id).
        campaignNodes.forEachIndexed { i, node -> assertEquals(i / 3, node.operation) }

        // Every shipped battle carries its D106 battlefield anchor (field-complete D79 mirror of
        // `OperationNode::anchor`), within ~1.5 degrees of its own war's pin — the same authoring
        // bound the Rust `every_shipped_battle_is_anchored_near_its_war` test pins.
        campaignNodes.forEach { node ->
            val conflict = campaignConflicts[node.operation!!]
            val lat = node.latX10 ?: error("node ${node.id} has no battlefield anchor")
            val lon = node.lonX10 ?: error("node ${node.id} has no battlefield anchor")
            assertTrue(
                "node ${node.id} strays from ${conflict.name}'s pin",
                kotlin.math.abs(lat - conflict.latX10) <= 15 &&
                    kotlin.math.abs(lon - conflict.lonX10) <= 15,
            )
        }
        // Spot-pin one anchor verbatim against the Rust source (Visby Airport).
        assertEquals(577, campaignNodes[7].latX10)
        assertEquals(184, campaignNodes[7].lonX10)
    }

    @Test
    fun the_inline_authored_nodes_mirror_the_rust_source_verbatim() {
        // Nodes 3-11 have no Rust briefing const to diff against — their copy is authored inline
        // in `default_campaign()` (D105) — so this third copy is the D79 drift tripwire: a Rust
        // edit that isn't hand-mirrored into CampaignModel.kt fails here instead of shipping a
        // cross-shell content divergence, exactly like the MISSION_* verbatim pins above.
        val expected = mapOf(
            3 to Pair(
                "Take the Fuel Yard",
                "The port runs on one fuel yard, and their garrison is sitting on it. Ten of " +
                    "yours to take it. Command the assault from above — or go dark and breach the " +
                    "wire yourself. The dust hides them; it hides you too.",
            ),
            4 to Pair(
                "Hold the Causeway",
                "One causeway carries the only road into the port, and they want it back. Dig in " +
                    "and fight it from the map — or embody a rifle at the barricade. Go dark and " +
                    "the bank you can't see is the one they wade across.",
            ),
            5 to Pair(
                "Open the Corridor",
                "Three checkpoints between the port and the highway north, every one of them " +
                    "held. Take them in order and hold what you take — or clear each gate " +
                    "yourself, rifle in hand. But the checkpoint you rush blind is the one that " +
                    "closes behind you.",
            ),
            6 to Pair(
                "Seize the Quay",
                "A garrison winters on the ice-bound harbor, and command wants it before the " +
                    "strait refreezes. Ten of yours, no more coming. Direct them from above — or " +
                    "take a rifle onto the ice yourself, and remember the quay you can't see is " +
                    "still shooting.",
            ),
            7 to Pair(
                "Hold the Airfield",
                "The airstrip is yours; they want it back before first light. Fight the " +
                    "perimeter from the map, or embody one rifle in the snow and hold it by " +
                    "hand — but the treeline you go dark on is the one they come through.",
            ),
            8 to Pair(
                "Break the Coast Road",
                "Three strongpoints up the coast road to Visby, every one dug into the drifts. " +
                    "Take them in order and keep them taken — or clear each one point-blank " +
                    "yourself. But the strongpoint you rush blind is the one retaken behind you.",
            ),
            9 to Pair(
                "Seize the Wharf",
                "Their task force beat you ashore and dug in on Santo's deepwater wharf. Ten of " +
                    "yours to take it before their heavy lift arrives. Command the assault from " +
                    "the ridge — or wade in yourself, and hope the wharf you can't see isn't " +
                    "reinforcing.",
            ),
            10 to Pair(
                "Hold the Airstrip",
                "The wharf bought you the airstrip; now they want it back before dawn. Hold the " +
                    "perimeter from above, or put yourself behind a rifle on the wire — but the " +
                    "stretch of runway you go dark on is the one they land on.",
            ),
            11 to Pair(
                "Break the Road to Luganville",
                "Three strongpoints down the coast road to Luganville, every one of them held. " +
                    "Take them in order and hold what you take — the town falls when the road " +
                    "does. But the post you clear blind is theirs again before you turn around.",
            ),
        )
        expected.forEach { (id, pair) ->
            assertEquals("node $id name", pair.first, campaignNodes[id].name)
            assertEquals("node $id briefing", pair.second, campaignNodes[id].briefing)
        }
    }

    @Test
    fun hold_name_and_briefing_mirror_the_rust_source_verbatim() {
        // Pins the D79 mirror against core::mission_tuning::MISSION_TWO_BRIEFING (title + situation).
        // Like the Seize node, the briefing surface shows only `situation` (not `objective_line`).
        val hold = campaignNodes.first { it.sceneToken == "mission2" }
        assertEquals("Hold the Line", hold.name)
        assertEquals(
            "They're coming for your dug-in line. Fight it from cover, or embody one rifle " +
                "and hold by hand — but go dark and the line you can't see is the one that breaks.",
            hold.briefing,
        )
    }

    @Test
    fun seize_name_and_briefing_mirror_the_rust_source_verbatim() {
        // Pins the D79 mirror against core::mission_tuning::MISSION_ONE_BRIEFING (title + situation).
        // The desktop briefing surface shows only `situation` (not `objective_line`), so this must
        // too — a paraphrase or a merged-in objective line trips this rather than shipping a
        // cross-shell content divergence.
        val seize = campaignNodes.first { it.sceneToken == "mission1" }
        assertEquals("Seize the Outpost", seize.name)
        assertEquals(
            "Ten of yours against a dug-in garrison. Command them — or go dark and fight one " +
                "yourself. Just don't stay blind too long.",
            seize.briefing,
        )
    }
}
