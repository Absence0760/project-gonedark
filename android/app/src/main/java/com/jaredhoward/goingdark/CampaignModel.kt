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
 * [campaignNodes] mirrors `engine::mission_registry::default_campaign()`: today exactly **one**
 * playable node — the WS-A *Seize* mission ("10 troops, take the base"). Integration (not this file)
 * resolves a node's [MissionNode.sceneToken] to a real launchable scene and wires the
 * Campaign → MissionSelect → Briefing → gunsmith flow; this model only names the mission.
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
 * One operation in the campaign graph as the native shell renders it: a stable [id], the [name] the
 * mission-select tile and briefing show, an opaque [sceneToken] integration resolves to a launchable
 * scene (the WS-A seam — this model never reads the mission *body*), and authored [briefing] copy.
 *
 * Mirrors a `core::campaign::OperationNode` flattened for presentation (unlock topology is host-side
 * and out of scope here — today there is one root node, so every node is playable).
 */
data class MissionNode(
    val id: Int,
    val name: String,
    val sceneToken: String,
    val briefing: String,
)

/**
 * The shipped campaign nodes, mirroring `engine::mission_registry::default_campaign()`. Today exactly
 * one playable node: the WS-A *Seize* mission. The [name] mirrors `MISSION_ONE_BRIEFING.title` and the
 * [briefing] mirrors `MISSION_ONE_BRIEFING.situation` **verbatim** (the desktop briefing surface shows
 * only the situation, not the separate `objective_line`, so neither does this) — a D79 mirrored string
 * the [CampaignModelTest] pins so a future edit to the Rust copy can't silently diverge. More nodes
 * land here as more Rust missions ship — keep in lock-step.
 */
val campaignNodes: List<MissionNode> = listOf(
    MissionNode(
        id = 0,
        name = "Seize the Outpost",
        sceneToken = "mission1",
        briefing = "Ten of yours against a dug-in garrison. Command them — or go dark and fight one " +
            "yourself. Just don't stay blind too long.",
    ),
)
