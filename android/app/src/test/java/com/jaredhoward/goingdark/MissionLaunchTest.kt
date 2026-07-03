package com.jaredhoward.goingdark

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for the pure Deploy **launch-resolution seam** (MissionLaunch.kt) — the node→wire
 * routing the Compose host ([MainActivity]'s `Shell`) actually runs at Deploy. The Compose screens
 * are device-gated chrome (D32) and exempt, so the resolution logic is pinned here off-device
 * (CLAUDE.md testing rule): every **playable** campaign node — the root *Seize* AND the gated
 * *Hold* — must resolve to its own scene token + node index on the launch wire, and the win that
 * comes back must record against the same node. These mirror the `pal-android/src/launch.rs` /
 * `engine::Scene` contracts so both ends stay in lock-step (D79).
 */
class MissionLaunchTest {

    private val settings = SettingsState(
        masterPct = 55,
        sfxPct = 65,
        sensX100 = 150,
        invertLookY = true,
        colorblindCues = true,
        visualSoundCues = true,
    )
    private val loadout = LoadoutSelection(optic = 1, barrel = 2, magazine = 1, stock = 2, muzzle = 1)

    // ---- node → launch wire resolution ----------------------------------------------------------

    @Test
    fun every_shipped_campaign_node_resolves_to_its_own_scene_and_node_index() {
        // The load-bearing routing: NO node is pinned to the root's scene/index — each playable
        // node threads its OWN sceneToken + NodeId ordinal into the wire.
        for (node in campaignNodes) {
            val cfg = missionLaunchConfig(node, settings, loadout, Army.Us, Difficulty.Recruit)
            assertEquals("scene for ${node.name}", node.sceneToken, cfg.scene)
            assertEquals("node index for ${node.name}", node.id, cfg.node)
        }
    }

    @Test
    fun the_gated_hold_node_launches_as_mission2_node_1_on_the_wire() {
        // The regression this seam exists to prevent: the second (gated) node must reach the engine
        // as mission2/node=1, not fall back to the root. Pinned through the REAL wire codec so the
        // string the Intent carries decodes back to the same scene + node (mirrors launch.rs).
        val hold = campaignNodes.first { it.sceneToken == "mission2" }
        val cfg = missionLaunchConfig(hold, settings, loadout, Army.Fr, Difficulty.Veteran)
        assertEquals("mission2", cfg.scene)
        assertEquals(1, cfg.node)

        val wire = cfg.encode()
        assertTrue("wire carries the scene", wire.contains("scene=mission2"))
        assertTrue("wire carries the node", wire.contains("node=1"))
        val decoded = LaunchConfig.decode(wire)
        assertEquals("mission2", decoded.scene)
        assertEquals(1, decoded.node)
        assertEquals(Difficulty.Veteran.tier(), decoded.diff)
        assertEquals(Army.Fr.index, decoded.army)
    }

    @Test
    fun difficulty_threads_as_the_diff_tier_for_every_tier() {
        val seize = campaignNodes.first()
        for (tier in Difficulty.entries) {
            val cfg = missionLaunchConfig(seize, settings, loadout, Army.Us, tier)
            assertEquals(tier.tier(), cfg.diff)
        }
    }

    @Test
    fun settings_loadout_and_army_fold_into_the_campaign_wire() {
        val hold = campaignNodes.first { it.id == 1 }
        val cfg = missionLaunchConfig(hold, settings, loadout, Army.Us, Difficulty.Elite)
        assertEquals(1, cfg.optic)
        assertEquals(2, cfg.barrel)
        assertEquals(1, cfg.magazine)
        assertEquals(2, cfg.stock) // the D85 pair rides the same wire (stk=/muz=)
        assertEquals(1, cfg.muzzle)
        assertEquals(55, cfg.masterPct)
        assertEquals(65, cfg.sfxPct)
        assertEquals(150, cfg.sensX100)
        assertTrue(cfg.invertY)
        assertTrue(cfg.colorblindCues)
        assertTrue(cfg.visualSoundCues)
        assertEquals(Army.Us.index, cfg.army)
    }

    // ---- the non-campaign path stays inert ------------------------------------------------------

    @Test
    fun mode_select_deploys_keep_diff_and_node_at_the_inert_root_defaults() {
        // The ModeSelect path (launchConfigOf without diff/node) must keep its prior behaviour:
        // diff=0/node=0, inert for non-campaign scenes.
        val cfg = launchConfigOf("skirmish", settings, loadout, Army.Us)
        assertEquals("skirmish", cfg.scene)
        assertEquals(0, cfg.diff)
        assertEquals(0, cfg.node)
    }

    // ---- launch → win → record round trip (the full non-root loop) ------------------------------

    @Test
    fun clearing_hold_round_trips_launch_win_record_and_persistence() {
        // The end-to-end pure loop for the GATED node: Seize cleared → Hold playable → Hold's launch
        // wire carries node 1 → the engine's win code (1 + node*4 + tier, launch.rs verbatim) decodes
        // back to node 1 → recordClear persists it → the unlock/clear state re-derives after a
        // simulated relaunch (decode of the persisted blob over the shipped topology).
        var campaign = CampaignProgress().recordClear(0, Difficulty.Regular)
        val hold = campaignNodes.first { it.id == 1 }
        assertTrue("Hold is playable once Seize is cleared", campaign.progress(hold.id).isPlayable)

        val cfg = missionLaunchConfig(hold, settings, loadout, Army.Us, Difficulty.Veteran)
        // The engine packs the win against the launched node + played tier (campaign_result_code).
        val resultCode = 1 + cfg.node * 4 + cfg.diff
        val win = CampaignResult.fromResultCode(resultCode)
        assertEquals(CampaignResult(node = 1, tier = Difficulty.Veteran), win)

        campaign = campaign.recordClear(win!!.node, win.tier)
        assertEquals(NodeProgress.Cleared(Difficulty.Veteran), campaign.progress(1))

        // Relaunch: only the cleared blob persists; the derived state survives the round trip.
        val restored = CampaignProgress.decodeCleared(campaign.encodeCleared())
        assertEquals(NodeProgress.Cleared(Difficulty.Veteran), restored.progress(1))
        assertEquals(NodeProgress.Cleared(Difficulty.Regular), restored.progress(0))
    }
}
