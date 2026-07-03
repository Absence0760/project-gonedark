package com.jaredhoward.goingdark

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for the skirmish match-setup seam (SkirmishSetup.kt) — the Kotlin mirror of the
 * desktop `app/src/shell/skirmish.rs` seam tests (D79, parity §12 item 6). The Compose screen is
 * device-gated chrome and exempt; every decision it renders is pinned here off-device.
 */
class SkirmishSetupTest {

    private val settings = SettingsState(masterPct = 55, sfxPct = 65, sensX100 = 150, invertLookY = true)
    private val loadout = LoadoutSelection(optic = 1, barrel = 2, magazine = 1, stock = 2, muzzle = 1)

    @Test
    fun default_is_the_neutral_shipped_match() {
        // The shipped default: the first battlefield (the open two-base skirmish), US vs FR, at
        // Regular — the neutral D83 tier, so the default deploy reproduces the pre-setup match.
        val d = SkirmishSetup()
        assertEquals(0, d.battlefield)
        assertEquals(Army.Us, d.playerArmy)
        assertEquals(Army.Fr, d.enemyArmy)
        assertEquals(Difficulty.Regular, d.difficulty)
    }

    @Test
    fun next_army_wraps_the_selectable_rosters_and_rejects_neutral() {
        assertEquals(Army.Fr, nextArmy(Army.Us))
        assertEquals(Army.Us, nextArmy(Army.Fr))
        // Neutral is never a player pick — a non-selectable input lands on the first roster.
        assertEquals(Army.SELECTABLE.first(), nextArmy(Army.Neutral))
    }

    @Test
    fun out_of_range_battlefield_clamps_to_the_first_and_never_throws() {
        assertEquals(0, clampBattlefield(-1))
        assertEquals(0, clampBattlefield(shellGameModes.size))
        assertEquals(0, clampBattlefield(Int.MAX_VALUE))
        for (i in shellGameModes.indices) assertEquals(i, clampBattlefield(i))
    }

    @Test
    fun reseed_player_army_follows_the_identity_pick_and_bumps_a_colliding_enemy() {
        // Opening the screen re-seeds the player side from the persisted army-select pick…
        var setup = SkirmishSetup().reseedPlayerArmy(Army.Fr)
        assertEquals(Army.Fr, setup.playerArmy)
        // …and when that collides with the current enemy pick (FR was the default enemy), the
        // enemy bumps to the opposing roster so the default reads as a real two-army fight.
        assertEquals(Army.Us, setup.enemyArmy)

        // No collision → the enemy pick is left exactly as the player configured it.
        setup = SkirmishSetup(enemyArmy = Army.Us).reseedPlayerArmy(Army.Fr)
        assertEquals(Army.Fr, setup.playerArmy)
        assertEquals(Army.Us, setup.enemyArmy)

        // Reseeding is idempotent for the already-consistent default — opening the screen twice
        // in a row changes nothing (the bump fires only on a genuine collision).
        assertEquals(SkirmishSetup(), SkirmishSetup().reseedPlayerArmy(Army.Us))
    }

    @Test
    fun every_battlefield_resolves_to_its_own_scene_token() {
        for ((i, mode) in shellGameModes.withIndex()) {
            val cfg = skirmishLaunchConfig(SkirmishSetup(battlefield = i), settings, loadout)
            assertEquals(mode.sceneToken, cfg.scene)
        }
    }

    @Test
    fun deploy_carries_the_configured_match_as_a_no_stakes_skirmish_wire() {
        // The full configured deploy: battlefield scene + both armies + tier ride the wire, with
        // `skirm=1` (no campaign clear can record) and `node=0` (not a campaign launch) — the
        // Kotlin twin of the desktop `resolve_skirmish_config` + `LaunchSkirmish` fielding.
        val setup = SkirmishSetup(
            battlefield = 1,
            playerArmy = Army.Fr,
            enemyArmy = Army.Us,
            difficulty = Difficulty.Elite,
        )
        val cfg = skirmishLaunchConfig(setup, settings, loadout)
        assertEquals(shellGameModes[1].sceneToken, cfg.scene)
        assertEquals(Army.Fr.index, cfg.army)
        assertEquals(Army.Us.index, cfg.enemyArmy)
        assertEquals(Difficulty.Elite.tier(), cfg.diff)
        assertTrue(cfg.skirmish)
        assertEquals(0, cfg.node)
        // The settings + gunsmith loadout carry in, like every deploy path.
        assertEquals(1, cfg.optic)
        assertEquals(2, cfg.stock)
        assertEquals(55, cfg.masterPct)
        assertTrue(cfg.invertY)

        // And the REAL wire codec round-trips all of it (mirrors launch.rs).
        val decoded = LaunchConfig.decode(cfg.encode())
        assertEquals(cfg, decoded)
    }

    @Test
    fun a_stale_battlefield_index_deploys_the_first_battle_not_a_crash() {
        val cfg = skirmishLaunchConfig(SkirmishSetup(battlefield = 99), settings, loadout)
        assertEquals(shellGameModes.first().sceneToken, cfg.scene)
    }

    @Test
    fun campaign_wires_stay_clear_of_the_skirmish_keys() {
        // A campaign Deploy must not read as a configured skirmish: no enemy-army pick, no skirm
        // flag — so the engine's campaign gate (clear recording) still fires for it.
        val hold = campaignNodes.first { it.id == 1 }
        val cfg = missionLaunchConfig(hold, settings, loadout, Army.Us, Difficulty.Veteran)
        assertEquals(LaunchConfig.ENEMY_ARMY_UNSET, cfg.enemyArmy)
        assertFalse(cfg.skirmish)
    }
}
