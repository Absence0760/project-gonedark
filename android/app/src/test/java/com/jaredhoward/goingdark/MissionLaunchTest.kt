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
    fun every_campaign_node_token_is_one_the_engine_understands() {
        // The stale-guard regression: KNOWN_SCENE_TOKENS once stopped at mission1/seize, so the
        // Hold/Push tokens were unguarded — and the engine glue's campaign gate silently missed
        // Mission3 (a Break-the-Line win recorded no clear). The set now mirrors Scene::parse in
        // full; every shipped node's token must be in it.
        for (node in campaignNodes) {
            assertTrue(
                "token ${node.sceneToken} of ${node.name} must be engine-known",
                node.sceneToken in KNOWN_SCENE_TOKENS,
            )
        }
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
    fun bare_deploys_keep_diff_node_and_the_skirmish_keys_at_their_inert_defaults() {
        // A bare launchConfigOf (no diff/node) keeps the inert defaults — and never reads as a
        // configured skirmish (that path is skirmishLaunchConfig, which sets earmy/skirm itself).
        val cfg = launchConfigOf("skirmish", settings, loadout, Army.Us)
        assertEquals("skirmish", cfg.scene)
        assertEquals(0, cfg.diff)
        assertEquals(0, cfg.node)
        assertEquals(LaunchConfig.ENEMY_ARMY_UNSET, cfg.enemyArmy)
        assertEquals(false, cfg.skirmish)
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
