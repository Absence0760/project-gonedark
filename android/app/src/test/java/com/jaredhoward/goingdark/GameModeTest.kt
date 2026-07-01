package com.jaredhoward.goingdark

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for the Pve/Pvp mode-select model (GameMode.kt). The composable that renders it is
 * device-gated chrome and exempt, but the one bit of real logic — that every offered mode resolves to
 * a scene the engine can actually launch — is testable here, so it is (CLAUDE.md testing rule, D81).
 * A typo'd scene token would otherwise ship a mode tile that Deploys into nothing.
 */
class GameModeTest {
    @Test
    fun every_mode_uses_a_scene_token_the_engine_understands() {
        for (mode in shellGameModes) {
            assertTrue(
                "mode '${mode.id}' has scene token '${mode.sceneToken}', not one Scene::parse accepts",
                mode.sceneToken in KNOWN_SCENE_TOKENS,
            )
        }
    }

    @Test
    fun mode_ids_are_unique() {
        val ids = shellGameModes.map { it.id }
        assertEquals("mode ids must be unique (they are the tap keys)", ids.size, ids.toSet().size)
    }

    @Test
    fun at_least_one_mode_is_offered() {
        // The picker must never be empty — a play-mode tap has to lead somewhere deployable.
        assertTrue(shellGameModes.isNotEmpty())
    }

    @Test
    fun the_two_standing_battles_are_present() {
        val tokens = shellGameModes.map { it.sceneToken }.toSet()
        assertTrue("skirmish battle is offered", "skirmish" in tokens)
        assertTrue("seize battle is offered", "seize" in tokens)
    }
}
