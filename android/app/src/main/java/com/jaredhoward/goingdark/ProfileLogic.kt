package com.jaredhoward.goingdark

/**
 * Pure Profile-screen logic — the Android mirror of the desktop egui Profile seam in
 * `app/src/shell.rs` (`struct ProfileState`, `enum FactionPref`, `sanitize_callsign`,
 * `win_rate_pct`). Kept free of Android/Compose types so it is unit-testable on the plain JVM
 * (`src/test`, no device), exactly like BuildStamp.kt.
 *
 * **D79 mirror discipline:** these values and semantics must stay bit-for-bit identical to the Rust
 * source of record — same default callsign, same char-based max length, same integer win-rate math
 * that reports "no rate" (null / "--") when no matches have been played. When the Rust seam changes,
 * change this in lockstep (and its test). Do not let the two drift.
 */

/**
 * The player's preferred faction (the real-army roster, `docs/factions.md`). A cosmetic / pre-match
 * preference only — it never constrains fairness. Mirrors Rust `FactionPref`.
 */
enum class FactionPref {
    UsArmy,
    FrenchArmy;

    /** The on-screen label — mirrors `FactionPref::label`. */
    fun label(): String = when (this) {
        UsArmy -> "US Army"
        FrenchArmy -> "French Army"
    }

    /** The next faction, wrapping — what the cycler advances to. Mirrors `FactionPref::next`. */
    fun next(): FactionPref = when (this) {
        UsArmy -> FrenchArmy
        FrenchArmy -> UsArmy
    }
}

/**
 * Host-side player identity / record shown on the Profile screen. Presentation only — never touches
 * the sim. Mirrors Rust `ProfileState`. The lifetime record is a real counter the host will bump at
 * match end (placeholder zeroes today).
 */
data class ProfileState(
    val callsign: String = DEFAULT_CALLSIGN,
    val faction: FactionPref = FactionPref.UsArmy,
    val matchesPlayed: Int = 0,
    val wins: Int = 0,
) {
    /**
     * A copy with the lifetime record zeroed (matches + wins → 0), leaving callsign and faction
     * intact — what the "RESET RECORD" control commits. Mirrors the desktop `ProfileAction::ResetStats`.
     */
    fun resetRecord(): ProfileState = copy(matchesPlayed = 0, wins = 0)
}

/** The fallback callsign when the field is left empty. Mirrors Rust `DEFAULT_CALLSIGN`. */
const val DEFAULT_CALLSIGN: String = "Commander"

/** Maximum callsign length (chars). Mirrors Rust `CALLSIGN_MAX`. */
const val CALLSIGN_MAX: Int = 18

/**
 * Normalise a raw callsign: trim surrounding whitespace, truncate to [CALLSIGN_MAX] characters, and
 * fall back to [DEFAULT_CALLSIGN] when the result is empty. Mirrors Rust `sanitize_callsign`.
 *
 * Char-based truncation (`take` on the char sequence) — not byte — so a multi-byte name can't be
 * split mid-codepoint. Kotlin `String.take(n)` counts UTF-16 code units, which matches Rust's
 * `chars().take(n)` (Unicode scalar values) for the BMP names this field accepts; both cap the
 * display length without slicing a code unit/codepoint in half.
 */
fun sanitizeCallsign(raw: String): String {
    val trimmed = raw.trim()
    if (trimmed.isEmpty()) {
        return DEFAULT_CALLSIGN
    }
    return trimmed.take(CALLSIGN_MAX)
}

/**
 * Win-rate percentage (`0..=100`), or `null` when no matches have been played (a clean "--" readout
 * instead of a divide-by-zero). Integer math, rounded down. Mirrors Rust `win_rate_pct`.
 *
 * Uses [Long] for `wins * 100` so large lifetime counts can't overflow [Int].
 */
fun winRatePct(wins: Int, played: Int): Int? {
    if (played == 0) {
        return null
    }
    return ((wins.toLong() * 100L) / played.toLong()).toInt()
}
