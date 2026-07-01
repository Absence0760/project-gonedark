package com.jaredhoward.goingdark

/**
 * The pure model behind the **army-select** screen (US vs French) — the Kotlin twin of the desktop
 * `app::shell` army seam (`Army`, `army_label`, `army_flavor`, `SELECTABLE_ARMIES`,
 * `ArmySelectState`). Picking an army is a pre-deploy choice fielded at every match start via the
 * shared `Game::select_army` → `core::shell` SelectArmy seam; it persists across launches like the
 * loadout.
 *
 * **A third concept, distinct from [FactionPref].** [FactionPref] is the *profile* allegiance label
 * shown on the Profile screen; [Army] is the *real-army roster* the player deploys as (`docs/factions.md`,
 * D68). They mirror the desktop split (`core::components::Army` vs the shell's profile `FactionPref`).
 *
 * The [Army] enum's `ordinal` matches `core::components::Army::index` verbatim (Neutral=0, Us=1, Fr=2),
 * so [index] is the exact wire ordinal the `army=` [LaunchConfig] key carries and the desktop shell
 * persists — the [D79](../../../../../../../docs/decisions.md) mirrored-constants discipline (a test
 * pins the mapping). No Android imports: this is the JVM-testable seam the [ArmySelectScreen] renders.
 */
enum class Army {
    /** No real-army identity — the non-aligned default. Never a player pick (factions-plan WS-A). */
    Neutral,

    /** The US Army roster (D68). */
    Us,

    /** The French Army roster (D68). */
    Fr;

    /** Dense index into per-army state — matches `core::components::Army::index` (the wire ordinal). */
    val index: Int get() = ordinal

    /** On-screen name (the card + title readout). ASCII, identical to the desktop `army_label`. */
    fun label(): String = when (this) {
        Us -> "US Army"
        Fr -> "French Army"
        Neutral -> "Non-aligned"
    }

    /**
     * One-line identity blurb — the same real-platform anchors the desktop `army_flavor` uses (kept
     * character-for-character so copy stays identical across platforms). Flavour only: asymmetry is of
     * feel, never of power (the fairness bound, factions.md §2).
     */
    fun flavor(): String = when (this) {
        Us -> "M4 carbines, M1 Abrams armour, combat medics -- the US Army roster."
        Fr -> "FAMAS rifles, Leclerc armour, auxiliaires sanitaires -- the French Army roster."
        Neutral -> "No real-army identity -- the non-aligned default."
    }

    companion object {
        /**
         * The player-selectable armies, in the fixed display order — only the **combatant** rosters
         * (US, French). Neutral is the non-aligned default, never offered. Mirrors the desktop
         * `SELECTABLE_ARMIES`.
         */
        val SELECTABLE = listOf(Us, Fr)

        /** The default player pick — a real combatant roster, never Neutral. Mirrors desktop. */
        val DEFAULT = Us

        /**
         * The army at wire/persist ordinal [i], collapsing anything that is not a valid combatant pick
         * — Neutral (`0`), out-of-range, negative — to [DEFAULT] (US). Mirrors the desktop `decode_army`
         * (`Army::ALL.get(i)` → `None`/`Neutral` → US) and `LaunchConfig.clampArmy`.
         */
        fun fromOrdinal(i: Int): Army = when (i) {
            Us.index -> Us
            Fr.index -> Fr
            else -> DEFAULT
        }
    }
}
