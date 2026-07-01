package com.jaredhoward.goingdark

/**
 * Pure data + logic for the Android campaign mission-select / briefing surface — the native twin of
 * the desktop egui campaign screens (`app/src/shell.rs`). **No Android / Compose types** so it is
 * unit-testable on the plain JVM (`src/test`, no device), exactly like [BuildStamp.kt]: the
 * composables are device-gated chrome (D32) and exempt, but any real logic still gets a test
 * (CLAUDE.md testing rule), so the data model and the difficulty cycle live here, away from the UI.
 *
 * ## Rust mirror — keep in lock-step (D79 mirrored-constants discipline)
 *
 * [Difficulty] mirrors `core::campaign::Difficulty` (the **four-tier** campaign progression type —
 * `Recruit, Regular, Veteran, Elite`, NOT the three-tier `core::mission_tuning::Difficulty` the
 * commander reads): same variants, same ascending order, the same integer [tier] ranks (`0..=3`),
 * the same stable [id] strings, and the same wrapping cycle as the desktop's `shell::next_difficulty`
 * (`Recruit → Regular → Veteran → Elite → Recruit`). These are **mirrored constants** — if the Rust
 * side changes (a tier added/renamed, an id string changed), this file must change in the same commit
 * or the two shells silently disagree. The [CampaignModelTest] pins the id strings and the cycle so a
 * drift trips a test rather than shipping.
 *
 * [campaignNodes] mirrors `engine::mission_registry::default_campaign()`: the WS-B **two-node
 * chain** — the root *Seize* mission ("10 troops, take the base") and, gated behind it, the *Hold
 * the Line* defense (unlocks once Seize is cleared). Integration (not this file) resolves a node's
 * [MissionNode.sceneToken] to a real launchable scene (via the Rust `Scene::for_mission` seam —
 * `mission1`/`mission2`) and wires the Campaign → MissionSelect → Briefing flow; this model only
 * names the mission.
 */

/**
 * A campaign difficulty tier, for replay-at-higher-difficulty. Declared in **ascending** order so
 * the ordinal matches the Rust [tier] rank. Mirrors `core::campaign::Difficulty`.
 */
enum class Difficulty {
    Recruit,
    Regular,
    Veteran,
    Elite;

    /** The integer rank of this tier (`0..=3`) — the stable wire value. Mirrors Rust `tier()`. */
    fun tier(): Int = when (this) {
        Recruit -> 0
        Regular -> 1
        Veteran -> 2
        Elite -> 3
    }

    /**
     * A stable, human-readable id a localized label keys off (never the label itself). Mirrors Rust
     * `id()` — these strings are part of the cross-shell contract, so the test pins them.
     */
    fun id(): String = when (this) {
        Recruit -> "recruit"
        Regular -> "regular"
        Veteran -> "veteran"
        Elite -> "elite"
    }

    /** The display label for the briefing's difficulty cycler. Mirrors desktop `difficulty_label`. */
    fun label(): String = when (this) {
        Recruit -> "Recruit"
        Regular -> "Regular"
        Veteran -> "Veteran"
        Elite -> "Elite"
    }

    /**
     * The next tier, wrapping `Recruit → Regular → Veteran → Elite → Recruit`. Mirrors the desktop's
     * `shell::next_difficulty` cycle order (Rust's `Difficulty` derives `Ord` but ships no `next`).
     */
    fun next(): Difficulty {
        val all = entries
        return all[(ordinal + 1) % all.size]
    }

    companion object {
        /**
         * Inverse of [tier]: the tier for a rank, or `null` for an out-of-range value (a corrupt /
         * foreign value — rejected, never guessed). Mirrors Rust `from_tier`.
         */
        fun fromTier(tier: Int): Difficulty? = when (tier) {
            0 -> Recruit
            1 -> Regular
            2 -> Veteran
            3 -> Elite
            else -> null
        }
    }
}

/**
 * One operation in the campaign graph as the native shell renders it: a stable [id] (its position
 * in [campaignNodes], mirroring Rust's `NodeId(i) == nodes[i]` invariant), the [name] the
 * mission-select tile and briefing show, an opaque [sceneToken] integration resolves to a launchable
 * scene (the WS-A seam — this model never reads the mission *body*), authored [briefing] copy, and
 * the [prerequisites] (node ids that must be **cleared** before this one unlocks — empty ⇒ a root).
 *
 * Mirrors `core::campaign::OperationNode`: the [prerequisites] are the unlock topology, so the moment
 * a 2nd/gated node ships the lock/unlock derivation ([CampaignProgress.progress]) is already correct.
 */
data class MissionNode(
    val id: Int,
    val name: String,
    val sceneToken: String,
    val briefing: String,
    val prerequisites: List<Int> = emptyList(),
)

/**
 * The derived unlock/clear state of a node, as the shell reads it — the Kotlin twin of
 * `core::campaign::NodeProgress`. **Derived, never stored:** [Locked] vs [Available] is recomputed
 * from the prerequisite clears on each read (see [CampaignProgress.progress]), so it can never drift
 * from the persisted cleared set.
 */
sealed interface NodeProgress {
    /** At least one prerequisite is not yet cleared — the node cannot be played. */
    data object Locked : NodeProgress

    /** Every prerequisite is cleared (or there are none) but the node itself is not cleared. */
    data object Available : NodeProgress

    /** Cleared; [best] is the highest difficulty it was cleared at (the replay surface). */
    data class Cleared(val best: Difficulty) : NodeProgress

    /** Whether the node can be launched now (Available or already-Cleared/replayable). */
    val isPlayable: Boolean get() = this !is Locked

    /** The best difficulty this node was cleared at, if cleared. */
    val bestCleared: Difficulty? get() = (this as? Cleared)?.best
}

/**
 * The Operations-hub campaign progress — the pure, JVM-testable twin of `core::campaign::Campaign`
 * (the read/clear surface the desktop egui hub uses). Holds the authored [nodes] topology plus the
 * per-node cleared set ([clearedByNode]: node id → best [Difficulty]); the lock/unlock state is
 * **derived** from the prerequisite clears, never stored. **No Android / Compose types** so the
 * lock/unlock/clear transitions and best-tier tracking are unit-tested off-device
 * ([CampaignProgressTest]), exactly like the rest of this file.
 *
 * Immutable: [recordClear] returns a new instance (mirroring the value-semantics the Compose shell
 * hoists in `remember`/`mutableStateOf`). Only the cleared set is persisted (via [encodeCleared] /
 * [decodeCleared], through [ShellPrefsCodec]); the topology is re-supplied from [campaignNodes] on
 * load, so a build that ships more nodes never needs a data migration.
 */
data class CampaignProgress(
    val nodes: List<MissionNode> = campaignNodes,
    val clearedByNode: Map<Int, Difficulty> = emptyMap(),
) {
    /** The authored node for an id, or `null` if out of range. */
    fun node(id: Int): MissionNode? = nodes.getOrNull(id)?.takeIf { it.id == id }

    /** The best difficulty a node was cleared at, or `null` (out of range or not cleared). */
    fun bestCleared(id: Int): Difficulty? = clearedByNode[id]

    /** Whether a node is cleared at any difficulty. */
    fun isCleared(id: Int): Boolean = clearedByNode.containsKey(id)

    /**
     * Whether a node is **unlocked** — every prerequisite cleared (a root with no prerequisites is
     * always unlocked). The derivation that makes "clearing a node opens its successors" hold
     * without storing edge state. An out-of-range id is not unlocked. Mirrors `Campaign::is_unlocked`.
     */
    fun isUnlocked(id: Int): Boolean {
        val n = node(id) ?: return false
        return n.prerequisites.all { isCleared(it) }
    }

    /** The derived [NodeProgress] for a node — the single source the tiles/briefing render from. */
    fun progress(id: Int): NodeProgress = when (val best = bestCleared(id)) {
        null -> if (isUnlocked(id)) NodeProgress.Available else NodeProgress.Locked
        else -> NodeProgress.Cleared(best)
    }

    /**
     * Record a clear of [id] at [tier], keeping only the **best** (highest) difficulty — a lower-tier
     * replay never demotes. Returns a new [CampaignProgress]; a clear of an unknown or still-**locked**
     * node is rejected (you cannot clear what you cannot play) and returns `this` unchanged. Mirrors
     * `Campaign::clear`'s best-tier + gate semantics (the desktop records the clear on a win).
     */
    fun recordClear(id: Int, tier: Difficulty): CampaignProgress {
        if (node(id) == null || !isUnlocked(id)) return this
        val prev = clearedByNode[id]
        if (prev != null && prev.tier() >= tier.tier()) return this // no raise — unchanged
        return copy(clearedByNode = clearedByNode + (id to tier))
    }

    /**
     * Serialize **only** the cleared set to a compact, stable string for [ShellPrefsCodec]:
     * `"node:tier"` pairs, comma-separated, in ascending node order (e.g. `"0:2,1:0"`). An empty set
     * encodes to `""`. The tier is the difficulty *rank* (`0..=3`), so a renamed enum constant can't
     * invalidate stored data. The topology is NOT written (re-supplied from [campaignNodes] on load).
     */
    fun encodeCleared(): String =
        clearedByNode.entries
            .sortedBy { it.key }
            .joinToString(",") { "${it.key}:${it.value.tier()}" }

    companion object {
        /**
         * Tolerantly rebuild a [CampaignProgress] over the given [nodes] topology from an
         * [encodeCleared] string. Every malformed/foreign/out-of-range token is dropped (never
         * throws): a non-`node:tier` token, an unparseable id/tier, a rank outside `0..=3`
         * ([Difficulty.fromTier] rejects it), or a node id not in [nodes]. `null`/blank → no clears.
         * The forward-compat + corruption-safety contract, mirroring the Rust progress-blob decode.
         */
        fun decodeCleared(raw: String?, nodes: List<MissionNode> = campaignNodes): CampaignProgress {
            val validIds = nodes.map { it.id }.toSet()
            val cleared = HashMap<Int, Difficulty>()
            if (!raw.isNullOrBlank()) {
                for (token in raw.split(',')) {
                    val pair = token.trim()
                    if (pair.isEmpty()) continue
                    val colon = pair.indexOf(':')
                    if (colon <= 0) continue // no id, or empty id — ignore (tolerant)
                    val id = pair.substring(0, colon).trim().toIntOrNull() ?: continue
                    val tier = pair.substring(colon + 1).trim().toIntOrNull() ?: continue
                    val diff = Difficulty.fromTier(tier) ?: continue // out-of-range rank — dropped
                    if (id !in validIds) continue // a node this build doesn't have — dropped
                    // Keep the best if a (corrupt) duplicate id appears twice.
                    val prev = cleared[id]
                    if (prev == null || prev.tier() < diff.tier()) cleared[id] = diff
                }
            }
            return CampaignProgress(nodes = nodes, clearedByNode = cleared)
        }
    }
}

/**
 * A campaign **win result** the engine (`NativeActivity`) hands the Compose shell back across the
 * Activity boundary as a packed `Activity.setResult(int)` code — the split-activity twin of the
 * desktop host's single-process record-on-win. The engine only reports a WIN (a loss/back-out leaves
 * the default `RESULT_CANCELED` (0), which decodes to `null` → nothing recorded).
 *
 * The packing mirrors `pal-android/src/launch.rs::campaign_result_code` **verbatim** (D79
 * mirrored-constants): `code = 1 + node*4 + tier`, so it is always `>= RESULT_FIRST_USER` (1) and
 * never collides with `RESULT_CANCELED`. [MainActivity] decodes the result code and records the clear.
 */
data class CampaignResult(val node: Int, val tier: Difficulty) {
    companion object {
        /** Tiers per node in the packing (Recruit..Elite). Mirrors Rust `DIFF_MAX + 1`. */
        private const val TIERS_PER_NODE = 4

        /**
         * Decode an Activity result [code] back to a [CampaignResult], or `null` for "no clear"
         * (`RESULT_CANCELED` (0), any non-positive code, or a tier that isn't a real difficulty rank).
         */
        fun fromResultCode(code: Int): CampaignResult? {
            if (code < 1) return null // RESULT_CANCELED / RESULT_OK — not a campaign win
            val base = code - 1
            val tier = Difficulty.fromTier(base % TIERS_PER_NODE) ?: return null
            return CampaignResult(node = base / TIERS_PER_NODE, tier = tier)
        }
    }
}

/**
 * The shipped campaign nodes, mirroring `engine::mission_registry::default_campaign()`: the WS-B
 * **two-node chain** — the root *Seize* mission and, gated behind it ([prerequisites] = `[0]`), the
 * *Hold the Line* defense. Each node's [name]/[briefing] mirror the Rust `MISSION_*_BRIEFING`
 * `title`/`situation` **verbatim** (the desktop/Compose briefing surface shows only the situation,
 * not the separate `objective_line`, so neither does this), and each [sceneToken] mirrors the Rust
 * `Scene::for_mission` mapping (Seize → `mission1`, Hold → `mission2`) — D79 mirrored strings the
 * [CampaignModelTest] pins so a future edit to the Rust copy can't silently diverge. More nodes land
 * here as more Rust missions ship — keep in lock-step.
 */
val campaignNodes: List<MissionNode> = listOf(
    MissionNode(
        id = 0,
        name = "Seize the Outpost",
        sceneToken = "mission1",
        briefing = "Ten of yours against a dug-in garrison. Command them — or go dark and fight one " +
            "yourself. Just don't stay blind too long.",
    ),
    MissionNode(
        id = 1,
        name = "Hold the Line",
        sceneToken = "mission2",
        briefing = "They're coming for your dug-in line. Fight it from cover, or embody one rifle " +
            "and hold by hand — but go dark and the line you can't see is the one that breaks.",
        prerequisites = listOf(0),
    ),
)
