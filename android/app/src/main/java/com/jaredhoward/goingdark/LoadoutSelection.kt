package com.jaredhoward.goingdark

/**
 * Pure, Android-free model for the Compose **gunsmith / loadout** screen (Compose shell parity,
 * Tier 2 — `docs/plans/compose-shell-parity.md` §5). The Compose UI that consumes it is device-gated
 * chrome (D32) and exempt from tests; this seam carries the actual selection logic and is
 * JVM-unit-testable off-device — the `BuildStamp.kt` / `LaunchConfig.kt` pattern.
 *
 * **Rust mirror (D79 mirrored-constants discipline).** The five slots, their options, the option
 * *order* (index 0/1/2 = the wire index), the labels, and the trade axes mirror `core/src/gunsmith.rs`
 * verbatim (Stock/Muzzle are the D85 gunsmith-breadth pair):
 *
 * | Slot     | index 0    | index 1    | index 2          | trade axis                          |
 * |----------|------------|------------|------------------|-------------------------------------|
 * | Optic    | `Standard` | `Marksman` | `Close-Quarters` | range <-> fire-rate                 |
 * | Barrel   | `Standard` | `Heavy`    | `Light`          | damage <-> reserve                  |
 * | Magazine | `Standard` | `Extended` | `Quickdraw`      | capacity <-> handling               |
 * | Stock    | `Standard` | `Agile`    | `Marksman`       | mobility <-> steadiness             |
 * | Muzzle   | `Standard` | `Brake`    | `Suppressor`     | suppression <-> downrange retention |
 *
 * Each enum's `ALL` order in the Rust source IS the wire index, and those indices are exactly the
 * `opt`/`bar`/`mag`/`stk`/`muz` fields of [LaunchConfig], each `0..SLOT_MAX`. A [LoadoutSelection]
 * therefore maps one-to-one onto the five slot fields the engine reads across the launch seam — so
 * the Compose Deploy button can pack these indices straight into a [LaunchConfig].
 */

/** The five gunsmith slots, in display order. Mirrors the slot enums in `core/src/gunsmith.rs`. */
enum class Slot {
    Optic,
    Barrel,
    Magazine,
    Stock,
    Muzzle,
}

/**
 * A complete pre-match weapon loadout: one option index per slot, each `0..SLOT_MAX`. The default is
 * all-`Standard` `(0, 0, 0, 0, 0)` — the neutral baseline a player with no unlocks fields,
 * byte-identical to the Rust `Loadout::default()` / `Loadout::STANDARD`.
 */
data class LoadoutSelection(
    /** Optic slot index, `0..SLOT_MAX` (0 = Standard). */
    val optic: Int = 0,
    /** Barrel slot index, `0..SLOT_MAX` (0 = Standard). */
    val barrel: Int = 0,
    /** Magazine slot index, `0..SLOT_MAX` (0 = Standard). */
    val magazine: Int = 0,
    /** Stock slot index, `0..SLOT_MAX` (0 = Standard) — gunsmith breadth, D85. */
    val stock: Int = 0,
    /** Muzzle slot index, `0..SLOT_MAX` (0 = Standard) — gunsmith breadth, D85. */
    val muzzle: Int = 0,
) {
    /** This selection's current option index for [slot]. */
    fun index(slot: Slot): Int = when (slot) {
        Slot.Optic -> optic
        Slot.Barrel -> barrel
        Slot.Magazine -> magazine
        Slot.Stock -> stock
        Slot.Muzzle -> muzzle
    }

    /** A copy of this selection with [slot] set to [index] (clamped into `0..SLOT_MAX`). */
    fun withIndex(slot: Slot, index: Int): LoadoutSelection {
        val clamped = index.coerceIn(0, SLOT_MAX)
        return when (slot) {
            Slot.Optic -> copy(optic = clamped)
            Slot.Barrel -> copy(barrel = clamped)
            Slot.Magazine -> copy(magazine = clamped)
            Slot.Stock -> copy(stock = clamped)
            Slot.Muzzle -> copy(muzzle = clamped)
        }
    }

    /**
     * Cycle [slot] one option [forward] (or backward), wrapping `0→1→2→0` — the gunsmith UI's
     * prev/next contract, mirroring the Rust slot enums' `next`/`prev`.
     */
    fun cycle(slot: Slot, forward: Boolean): LoadoutSelection {
        val n = OPTION_COUNT
        val cur = index(slot)
        val next = if (forward) (cur + 1) % n else (cur + n - 1) % n
        return withIndex(slot, next)
    }

    /** The neutral all-`Standard` baseline — the RESET control's target. Mirrors `LoadoutEditor::reset()`. */
    fun reset(): LoadoutSelection = STANDARD

    companion object {
        /**
         * Max option index, inclusive. **Must equal [LaunchConfig.SLOT_MAX]** (D79 mirror) — every
         * slot index this screen emits is one of the `opt`/`bar`/`mag` wire values.
         */
        const val SLOT_MAX = 2

        /** Number of options per slot (`Standard` + two opposed trades). */
        const val OPTION_COUNT = SLOT_MAX + 1

        /**
         * Option labels per slot, indexed `0..SLOT_MAX`. Mirrors the `labels { … }` of each slot enum
         * in `core/src/gunsmith.rs` verbatim.
         */
        private val LABELS: Map<Slot, List<String>> = mapOf(
            Slot.Optic to listOf("Standard", "Marksman", "Close-Quarters"),
            Slot.Barrel to listOf("Standard", "Heavy", "Light"),
            Slot.Magazine to listOf("Standard", "Extended", "Quickdraw"),
            Slot.Stock to listOf("Standard", "Agile", "Marksman"),
            Slot.Muzzle to listOf("Standard", "Brake", "Suppressor"),
        )

        /**
         * The neutral all-`Standard` baseline `(0, 0, 0, 0, 0)` — the build a player with no unlocks
         * fields, byte-identical to the Rust `Loadout::default()` / `Loadout::STANDARD`. The RESET
         * control returns the editor here; mirrors the desktop `LoadoutEditor::reset()`.
         */
        val STANDARD = LoadoutSelection()

        /**
         * A short trade-axis hint per slot, for the UI. Mirrors `app/src/shell.rs::slot_trade_hint`
         * **verbatim** — ASCII `<->` (not the `↔` glyph), exactly as the desktop string, so the two
         * shells show byte-identical hints: Optic range<->fire-rate, Barrel damage<->reserve, Magazine
         * capacity<->handling.
         */
        private val TRADE_HINTS: Map<Slot, String> = mapOf(
            Slot.Optic to "range <-> fire-rate",
            Slot.Barrel to "damage <-> reserve",
            Slot.Magazine to "capacity <-> handling",
            Slot.Stock to "mobility <-> steadiness",
            Slot.Muzzle to "suppression <-> downrange retention",
        )

        /** The human label for option [index] of [slot] (index clamped into range). */
        fun label(slot: Slot, index: Int): String {
            val opts = LABELS.getValue(slot)
            return opts[index.coerceIn(0, opts.size - 1)]
        }

        /** The short trade-axis hint for [slot] (e.g. `"range ↔ fire-rate"`). */
        fun tradeHint(slot: Slot): String = TRADE_HINTS.getValue(slot)
    }
}
