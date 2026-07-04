package com.jaredhoward.goingdark

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/**
 * D79 cross-language mirror parity — the Android (Kotlin) half.
 *
 * The Android shell hand-mirrors slices of Rust engine data (the **D79 mirror** pattern):
 * `CampaignModel.kt` mirrors `engine::mission_registry::default_campaign()`, and `Battlefield.kt`
 * mirrors `engine::map_library::BATTLEFIELDS`. The two sides sit in separate build systems (gradle
 * vs cargo), so no single unit test sees both — and they have drifted **silently** before (the
 * campaign lagged at 12 vs 15 nodes; `Battlefield.kt` was missing library maps).
 *
 * The guard: a single committed fixture, `parity/d79-mirror.txt` (repo root), is the shared
 * contract. The Rust test `engine/tests/d79_mirror_parity.rs` pins the **live Rust** data to it (so
 * it can't go stale); THIS test rebuilds the same facts from the **Android mirror** and asserts they
 * match the same file. A Rust campaign/map change that isn't hand-mirrored here fails this test
 * instead of shipping a silent cross-shell divergence.
 *
 * Regenerate the fixture (Rust side) with:
 *   UPDATE_D79_FIXTURE=1 cargo test -p gonedark-engine --test d79_mirror_parity
 * then bring `CampaignModel.kt` / `Battlefield.kt` back into step until this test is green.
 *
 * Only *structural* facts are pinned (counts, ids, gating, scene tokens, years, map ids) — not
 * briefing/blurb **prose**, which the per-node verbatim tests on each side own. When you add a new
 * D79 mirror surface, extend both [canonicalFacts] here and `canonical_facts()` in the Rust test.
 */
class MirrorParityTest {
    /** Rebuild the canonical mirror facts from the Android data — the exact key set the Rust test emits. */
    private fun canonicalFacts(): Map<String, String> {
        val out = LinkedHashMap<String, String>()

        out["campaign.conflict_count"] = campaignConflicts.size.toString()
        out["campaign.operation_count"] = campaignOperations.size.toString()
        out["campaign.node_count"] = campaignNodes.size.toString()

        campaignConflicts.forEachIndexed { i, c ->
            val p = "%02d".format(i)
            out["campaign.conflict.$p.name"] = c.name
            out["campaign.conflict.$p.start_year"] = c.startYear.toString()
            out["campaign.conflict.$p.end_year"] = c.endYear.toString()
        }
        campaignOperations.forEachIndexed { i, op ->
            val p = "%02d".format(i)
            out["campaign.operation.$p.name"] = op.name
            out["campaign.operation.$p.conflict"] = op.conflict.toString()
        }
        campaignNodes.forEachIndexed { i, node ->
            val p = "%02d".format(i)
            out["campaign.node.$p.scene_token"] = node.sceneToken
            out["campaign.node.$p.operation"] = node.operation?.toString() ?: ""
            out["campaign.node.$p.prerequisites"] = node.prerequisites.sorted().joinToString(",")
        }

        out["battlefields.count"] = shellBattlefields.size.toString()
        val mapIds = ArrayList<String>()
        shellBattlefields.forEachIndexed { i, bf ->
            val p = "%02d".format(i)
            out["battlefield.$p.id"] = bf.id
            out["battlefield.$p.boot"] = when {
                bf.sceneToken != null -> "scene:${bf.sceneToken}"
                bf.mapId != null -> { mapIds.add(bf.mapId!!); "map:${bf.mapId}" }
                else -> error("battlefield ${bf.id} boots no way")
            }
        }
        out["maps.count"] = mapIds.size.toString()
        out["maps.ids"] = mapIds.joinToString(",")

        return out
    }

    @Test
    fun android_mirror_matches_the_committed_rust_parity_fixture() {
        val fixture = parseFixture(fixtureFile().readText())
        val live = canonicalFacts()

        val missing = (fixture.keys - live.keys).sorted()
        val extra = (live.keys - fixture.keys).sorted()
        assertTrue(
            "The Android mirror is MISSING facts the Rust source-of-truth pins (D79 drift — the " +
                "Rust campaign/maps grew and CampaignModel.kt/Battlefield.kt lagged): $missing",
            missing.isEmpty(),
        )
        assertTrue(
            "The Android mirror carries EXTRA facts the Rust source-of-truth does not (D79 drift — " +
                "regenerate the fixture if the Rust side changed): $extra",
            extra.isEmpty(),
        )

        val mismatches = live.entries
            .filter { (k, v) -> fixture[k] != v }
            .map { (k, v) -> "$k: android=\"$v\" rust=\"${fixture[k]}\"" }
            .sorted()
        assertTrue(
            "The Android mirror DIVERGES from the Rust source-of-truth (D79 drift). Bring " +
                "CampaignModel.kt / Battlefield.kt back in step, or re-bless the fixture if the " +
                "Rust side changed on purpose: $mismatches",
            mismatches.isEmpty(),
        )
    }

    @Test
    fun known_map_ids_cover_exactly_the_rust_map_library() {
        // The exact class of the reported bug: Battlefield.kt's KNOWN_MAP_IDS set fell behind the
        // Rust MAP_LIBRARY (missing maps). Pin the set directly against the fixture's map ids.
        val fixture = parseFixture(fixtureFile().readText())
        val rustMapIds = fixture.getValue("maps.ids").split(",").filter { it.isNotEmpty() }.toSet()
        assertEquals(
            "KNOWN_MAP_IDS drifted from the Rust MAP_LIBRARY — a map id is missing or extra",
            rustMapIds,
            KNOWN_MAP_IDS,
        )
    }

    private fun parseFixture(text: String): Map<String, String> {
        val out = LinkedHashMap<String, String>()
        for (raw in text.lineSequence()) {
            val line = raw.trim()
            if (line.isEmpty() || line.startsWith("#")) continue
            val eq = line.indexOf('=')
            require(eq >= 0) { "malformed D79 fixture line (no '='): $raw" }
            out[line.substring(0, eq)] = line.substring(eq + 1)
        }
        return out
    }

    /**
     * Locate `parity/d79-mirror.txt` by walking up from the test's working directory (gradle runs
     * unit tests with the module dir as CWD, so repo root is a couple of parents up). Robust to
     * being run from the module, the `android/` dir, or the repo root.
     */
    private fun fixtureFile(): File {
        var dir: File? = File("").absoluteFile
        while (dir != null) {
            val candidate = File(dir, "parity/d79-mirror.txt")
            if (candidate.isFile) return candidate
            dir = dir.parentFile
        }
        error(
            "could not locate parity/d79-mirror.txt walking up from ${File("").absoluteFile}. " +
                "It is committed at the repo root and generated by the Rust d79_mirror_parity test.",
        )
    }
}
