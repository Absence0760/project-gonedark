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
 * [campaignNodes] mirrors `engine::mission_registry::default_campaign()`: the WS-B **three-node
 * chain** — the root *Seize* mission ("10 troops, take the base"), the gated *Hold the Line*
 * defense, and the gated *Break the Line* push (each unlocks once the one before is cleared).
 * Integration (not this file) resolves a node's [MissionNode.sceneToken] to a real launchable scene
 * (via the Rust `Scene::for_mission` seam — `mission1`/`mission2`/`mission3`) and wires the
 * Campaign → MissionSelect → Briefing flow; this model only names the mission.
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
 * [operation] is the Q28 conflict-atlas grouping (the id of the [Operation] this battle belongs to,
 * `null` = ungrouped) — pure metadata, mirroring the Rust field; it never affects unlock logic.
 */
data class MissionNode(
    val id: Int,
    val name: String,
    val sceneToken: String,
    val briefing: String,
    val prerequisites: List<Int> = emptyList(),
    val operation: Int? = null,
)

/**
 * One **conflict** on the campaign atlas (Q28: conflict → operation → battle) — the Kotlin twin of
 * `core::campaign::Conflict`. Static authored grouping data, like [campaignNodes]: never persisted,
 * integer calendar years. The atlas *presentation* (world-map/timeline) is still an open fork (Q28);
 * this is only the data a future atlas surface renders.
 */
data class Conflict(
    val id: Int,
    val name: String,
    val startYear: Int,
    val endYear: Int,
    val summary: String,
    /**
     * Atlas-pin anchor in **tenths of a degree** (D103; `505` = 50.5°N / negative = south,
     * `-15` = 1.5°W / negative = west) — mirrors the Rust `Conflict::lat_x10`/`lon_x10`.
     * Presentation data like the years; Android doesn't render the globe yet, but the mirrored
     * model stays field-complete (D79) so the data can't drift when it does.
     */
    val latX10: Int = 0,
    val lonX10: Int = 0,
)

/**
 * One **operation** inside a [Conflict] — the Kotlin twin of `core::campaign::Operation`, grouping
 * the battle nodes ([MissionNode.operation]) a player progresses through.
 */
data class Operation(
    val id: Int,
    val conflict: Int,
    val name: String,
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
 * **three-node chain** — the root *Seize* mission, the gated *Hold the Line* defense, and the
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
        operation = 0,
    ),
    MissionNode(
        id = 1,
        name = "Hold the Line",
        sceneToken = "mission2",
        briefing = "They're coming for your dug-in line. Fight it from cover, or embody one rifle " +
            "and hold by hand — but go dark and the line you can't see is the one that breaks.",
        prerequisites = listOf(0),
        operation = 0,
    ),
    MissionNode(
        id = 2,
        name = "Break the Line",
        sceneToken = "mission3",
        briefing = "Three posts down one lane, every one of them held. Take them in order and " +
            "hold what you take — or embody a rifle and clear the way yourself. But the post you " +
            "rush blind is the one they take back behind you.",
        prerequisites = listOf(1),
        operation = 0,
    ),
)

/**
 * The shipped conflict atlas, mirroring `default_campaign()`'s Q28 grouping: one **placeholder**
 * modern fictional conflict (*The Channel Crisis* — a war the shipped US/FR roster plausibly
 * covers; the name/framing are content, not a lock) holding one operation (*Operation First
 * Light*) holding both [campaignNodes]. D79 mirrored constants — [CampaignModelTest] pins them
 * against the Rust copy. Keep in lock-step with `engine::mission_registry::default_campaign()`.
 */
val campaignConflicts: List<Conflict> = listOf(
    Conflict(
        id = 0,
        name = "The Channel Crisis",
        startYear = 2027,
        endYear = 2028,
        summary = "A fictional modern flashpoint between US and French expeditionary forces " +
            "on the Channel coast — the campaign's first (placeholder) conflict.",
        // Mid-Channel off the Cotentin coast (~50.0°N, 1.5°W) — the atlas pin (D103).
        latX10 = 500,
        lonX10 = -15,
    ),
)

/** The shipped operations — see [campaignConflicts]. */
val campaignOperations: List<Operation> = listOf(
    Operation(id = 0, conflict = 0, name = "Operation First Light"),
)

/**
 * A group's rollup on the atlas hub — the Kotlin twin of `core::campaign::GroupProgress`:
 * cleared-vs-total plus the "can the player enter this group at all" bit a header greys out on.
 * Derived, never stored (like [NodeProgress]).
 */
data class GroupProgress(val cleared: Int, val total: Int, val playable: Boolean) {
    /** Every node in the group cleared (and the group is non-empty). */
    val isComplete: Boolean get() = total > 0 && cleared == total
}

/**
 * One renderable section of the grouped Operations hub — the Kotlin twin of the desktop
 * `HubSection` (`app/src/shell/mission_select.rs`, D98: conflict → operation → battle). An
 * optional conflict header (present only on the conflict's *first* section, so a multi-operation
 * conflict draws its header once), an optional operation sub-header, and that operation's battle
 * nodes. The trailing untitled section (both headers `null`) carries the ungrouped nodes, so an
 * atlas-less campaign degrades to exactly one untitled section — the pre-atlas flat list. Where
 * the Rust seam carries ids and looks the groups back up at the render site, this one carries the
 * [Conflict]/[Operation] values directly (same information, no lookup indirection to test twice).
 */
data class HubSection(
    val conflict: Pair<Conflict, GroupProgress>?,
    val operation: Pair<Operation, GroupProgress>?,
    val nodes: List<MissionNode>,
)

/** The rollup over one operation's nodes ([MissionNode.operation] == its id). */
fun operationProgress(campaign: CampaignProgress, operation: Operation): GroupProgress =
    groupProgress(campaign, campaign.nodes.filter { it.operation == operation.id })

/** The rollup over every node in a conflict's operations. */
fun conflictProgress(
    campaign: CampaignProgress,
    conflict: Conflict,
    operations: List<Operation> = campaignOperations,
): GroupProgress {
    val opIds: Set<Int?> = operations.filter { it.conflict == conflict.id }.map { it.id }.toSet()
    return groupProgress(campaign, campaign.nodes.filter { it.operation != null && it.operation in opIds })
}

private fun groupProgress(campaign: CampaignProgress, nodes: List<MissionNode>): GroupProgress =
    GroupProgress(
        cleared = nodes.count { campaign.isCleared(it.id) },
        total = nodes.size,
        playable = nodes.any { campaign.progress(it.id).isPlayable },
    )

/**
 * Derive the hub's ordered section list — the Kotlin twin of the desktop `hub_sections` (D98),
 * same semantics by hand (D79): conflicts in authored order, each conflict's operations in
 * authored order, each operation's nodes in authored ([MissionNode.id]) order, then a trailing
 * untitled section for ungrouped nodes. A **content-pending** (empty) operation contributes no
 * section — no header scaffolding without tiles — and a conflict whose operations are all empty
 * therefore contributes nothing at all. Pure — JVM-tested in [CampaignModelTest], no Compose.
 */
fun hubSections(
    campaign: CampaignProgress,
    conflicts: List<Conflict> = campaignConflicts,
    operations: List<Operation> = campaignOperations,
): List<HubSection> {
    val sections = mutableListOf<HubSection>()
    for (conflict in conflicts) {
        // Consumed by the conflict's first non-empty operation, so the header draws exactly once.
        var header: Pair<Conflict, GroupProgress>? =
            conflict to conflictProgress(campaign, conflict, operations)
        for (op in operations.filter { it.conflict == conflict.id }) {
            val nodes = campaign.nodes.filter { it.operation == op.id }.sortedBy { it.id }
            if (nodes.isEmpty()) continue // content-pending operation: nothing to render yet
            sections +=
                HubSection(
                    conflict = header.also { header = null },
                    operation = op to operationProgress(campaign, op),
                    nodes = nodes,
                )
        }
    }
    val ungrouped = campaign.nodes.filter { it.operation == null }.sortedBy { it.id }
    if (ungrouped.isNotEmpty()) {
        sections += HubSection(conflict = null, operation = null, nodes = ungrouped)
    }
    return sections
}

/**
 * The conflict header line, e.g. `THE CHANNEL CRISIS · 2027-2028 · 0/2` — name, year span
 * (collapsed to a single year when start == end), and the campaign-level rollup. Identical
 * formatting to the desktop `conflict_header_label` (ASCII plus U+00B7, the [StatusPill] rule).
 */
fun conflictHeaderLabel(conflict: Conflict, progress: GroupProgress): String {
    val years =
        if (conflict.startYear == conflict.endYear) "${conflict.startYear}"
        else "${conflict.startYear}-${conflict.endYear}"
    return "${conflict.name.uppercase()} · $years · ${progress.cleared}/${progress.total}"
}

/**
 * The operation sub-header line, e.g. `OPERATION FIRST LIGHT · 0/2` — name plus its own rollup.
 * Identical formatting to the desktop `operation_header_label`; the greyed-when-unplayable colour
 * pick is the composable's job.
 */
fun operationHeaderLabel(operation: Operation, progress: GroupProgress): String =
    "${operation.name.uppercase()} · ${progress.cleared}/${progress.total}"
