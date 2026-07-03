package com.jaredhoward.goingdark

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for the unified battlefield table (Battlefield.kt, D102) — the Kotlin mirror of
 * the Rust `map_library` battlefield tests (D79). The Compose screen consuming it is device-gated
 * chrome and exempt; the table's launch-safety guards are the testable logic, so they are tested.
 */
class BattlefieldTest {
    @Test
    fun every_battlefield_boots_exactly_one_way() {
        // The D102 unification's core guard: each entry is EITHER a scene tile (engine-known
        // token) OR a library-map tile (engine-embedded id) — never both, never neither — so a
        // shipped tile can never be ambiguous or deploy into nothing.
        assertTrue(shellBattlefields.isNotEmpty())
        for (bf in shellBattlefields) {
            val kinds = listOfNotNull(bf.sceneToken, bf.mapId)
            assertEquals("battlefield ${bf.id} must boot exactly one way", 1, kinds.size)
            bf.sceneToken?.let {
                assertTrue("token $it of ${bf.id} must be engine-known", it in KNOWN_SCENE_TOKENS)
            }
            bf.mapId?.let {
                assertTrue("map id $it of ${bf.id} must be engine-embedded", it in KNOWN_MAP_IDS)
            }
        }
    }

    @Test
    fun the_first_battlefield_is_the_open_skirmish_fallback() {
        // A stale index / unknown map id degrades to shellBattlefields[0] (and engine-side to
        // Scene::Skirmish) — it must stay the standing open skirmish so degradation is playable.
        assertEquals("skirmish", shellBattlefields.first().id)
        assertEquals("skirmish", shellBattlefields.first().sceneToken)
    }

    @Test
    fun the_table_spans_scenes_and_the_map_library() {
        // The library seam is live, not vestigial: at least one standing battle and at least one
        // authored library map are offered, and every KNOWN_MAP_IDS entry has a tile (no orphans).
        assertTrue(shellBattlefields.any { it.sceneToken != null })
        assertTrue(shellBattlefields.any { it.mapId != null })
        for (id in KNOWN_MAP_IDS) {
            assertTrue("library map $id has no battlefield tile", shellBattlefields.any { it.mapId == id })
        }
    }

    @Test
    fun battlefield_ids_and_names_are_distinct_ascii() {
        for (bf in shellBattlefields) {
            for (field in listOf(bf.id, bf.name, bf.blurb)) {
                assertTrue("field of ${bf.id} must be non-empty ASCII", field.isNotEmpty())
                assertTrue("field of ${bf.id} must be ASCII", field.all { it.code in 32..126 })
            }
        }
        assertEquals(shellBattlefields.size, shellBattlefields.map { it.id }.toSet().size)
    }
}
