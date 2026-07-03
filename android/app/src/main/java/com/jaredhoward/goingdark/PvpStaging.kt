package com.jaredhoward.goingdark

/**
 * The **PvP staging** model (D101) — the pure seam behind [PvpScreen], the Kotlin mirror of the
 * desktop `app/src/shell/pvp.rs` (`PvpQueue` / `PVP_QUEUES` / `queue_joinable`).
 *
 * Until the Phase 3 net layer exists the PvP door is a *staging post, not a fake matchmaker*
 * (`docs/modes.md` §1/§5): it names the three queues in build order and offers nothing joinable.
 * The honesty rule is [queueJoinable] — a pure, JVM-tested gate every queue row routes through
 * (the PvP twin of the hub's `isPlayable`), so a live-looking queue can't ship by styling
 * accident. D79 mirrored constants: [PvpStagingTest] pins the table against the Rust copy's
 * shape; keep the strings in lock-step with `app/src/shell/pvp.rs` by hand.
 *
 * **No Android imports** on purpose — this is the testable seam the Compose chrome is exempt from.
 */

/**
 * One PvP queue on the staging screen — a stable id, the tile name + one-line blurb, and the
 * build-order status its trailing label shows (honest scheduling, never a live state). The Kotlin
 * twin of the Rust `PvpQueue`.
 */
data class PvpQueue(
    val id: String,
    val name: String,
    val blurb: String,
    val status: String,
)

/**
 * The three PvP queues, in the `modes.md` §5 build order (custom lobby → quick match → ranked).
 * The **first entry is the first real PvP surface** — the direct-invite custom lobby, the smallest
 * thing that puts two humans in one lockstep match. Strings mirror the desktop `PVP_QUEUES`
 * verbatim (D79); ranked's rating model is still Q29.
 */
val pvpQueues: List<PvpQueue> = listOf(
    PvpQueue(
        id = "custom",
        name = "Custom Lobby",
        blurb = "Invite an opponent, pick any lint-passing battlefield, ready up. " +
            "The first two-human match.",
        status = "FIRST UP",
    ),
    PvpQueue(
        id = "quick",
        name = "Quick Match",
        blurb = "Curated map rotation, random pick. Unranked, low ceremony.",
        status = "PLANNED",
    ),
    PvpQueue(
        id = "ranked",
        name = "Ranked",
        blurb = "Seasonal map pool with vetoes. Placements, tiers, a rating on the line.",
        status = "PLANNED",
    ),
)

/**
 * Whether a queue can currently be joined — the single gate every queue row routes through.
 * **Always `false` until the Phase 3 net layer lands** (there is no session transport to join);
 * when the custom lobby ships, this seam is where joinability flips per-queue. Pure — unit-tested
 * ([PvpStagingTest]), mirroring the Rust `queue_joinable`.
 */
@Suppress("UNUSED_PARAMETER")
fun queueJoinable(queue: PvpQueue): Boolean = false
