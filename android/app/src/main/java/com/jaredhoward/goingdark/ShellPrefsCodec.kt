package com.jaredhoward.goingdark

/**
 * Pure, Android-free **persistence codec** for the out-of-match Compose shell (Compose shell parity —
 * `docs/plans/compose-shell-parity.md`, Tiers 1–2). The shell's three player-owned state objects —
 * [SettingsState] (audio/look), [ProfileState] (callsign/faction/record), and the gunsmith
 * [LoadoutSelection] — currently live only in Compose `remember` and are lost on app exit. This seam
 * turns that aggregate state into/out of a flat `String`→`String` map so a thin SharedPreferences
 * layer ([ShellPrefs]) can persist it across restarts.
 *
 * **Why a pure map codec (the `BuildStamp.kt` / `LaunchConfig.kt` pattern, D32).** All the real
 * decision/validation logic lives here as plain Kotlin with **no Android imports**, so it is
 * JVM-unit-testable off-device ([ShellPrefsCodecTest]). The Android glue in [ShellPrefs] is reduced to
 * a trivial read-all / write-all loop over `SharedPreferences`, which is the only test-exempt part
 * (un-unit-testable platform glue, like the JNI reader in `pal-android`).
 *
 * **Tolerant [decode] (the forward-compat + corruption-safety contract).** A missing key, an
 * unparseable value, an out-of-range int, or an out-of-range enum ordinal all fall back to *that
 * field's* default — never an exception. Decode therefore reuses the existing types' own validation:
 * [sanitizeCallsign] for the callsign, range clamps mirrored from each type's companion bounds for the
 * numeric fields, and ordinal-bounds-checked lookups for the [Quality]/[FactionPref] enums. An empty
 * map (first launch, or wiped prefs) decodes to [ShellState.defaults]. Encode always writes the
 * canonical, already-clamped representation, so a save→load round-trip is stable.
 *
 * **Wire shape.** Values are stored as strings under stable, namespaced [keys][KEY_MASTER] (e.g.
 * `settings.master`, `profile.callsign`, `loadout.optic`). Enums are stored by **ordinal** (an
 * integer string) so a renamed enum constant can't silently invalidate stored data; the ordinal is
 * range-checked on decode. Booleans are `"1"`/`"0"`.
 */
data class ShellState(
    val settings: SettingsState = SettingsState.defaults(),
    val profile: ProfileState = ProfileState(),
    val loadout: LoadoutSelection = LoadoutSelection(),
    val campaign: CampaignProgress = CampaignProgress(),
) {
    companion object {
        /** The shipped defaults for every shell surface — first-launch state. */
        fun defaults(): ShellState = ShellState(
            settings = SettingsState.defaults(),
            profile = ProfileState(),
            loadout = LoadoutSelection(),
            campaign = CampaignProgress(),
        )
    }
}

/**
 * The pure codec. Stable key names are public consts so the persistence layer and the tests reference
 * the exact same strings (no stringly-typed drift).
 */
object ShellPrefsCodec {
    // --- Settings keys ---
    const val KEY_MASTER = "settings.master"
    const val KEY_SFX = "settings.sfx"
    const val KEY_MUSIC = "settings.music"
    const val KEY_SENS = "settings.sens"
    const val KEY_INVERT_Y = "settings.invertY"
    const val KEY_QUALITY = "settings.quality"

    // --- Profile keys ---
    const val KEY_CALLSIGN = "profile.callsign"
    const val KEY_FACTION = "profile.faction"
    const val KEY_MATCHES = "profile.matchesPlayed"
    const val KEY_WINS = "profile.wins"

    // --- Loadout keys ---
    const val KEY_OPTIC = "loadout.optic"
    const val KEY_BARREL = "loadout.barrel"
    const val KEY_MAGAZINE = "loadout.magazine"

    // --- Campaign key (the cleared-set blob; see CampaignProgress.encodeCleared) ---
    const val KEY_CAMPAIGN = "campaign.cleared"

    /**
     * Encode [state] to a flat string map, writing every field as its canonical, already-clamped /
     * sanitized representation. The result is exactly what [ShellPrefs.save] persists key-by-key.
     */
    fun encode(state: ShellState): Map<String, String> {
        val s = state.settings.clamp()
        val p = state.profile
        val l = state.loadout
        return mapOf(
            KEY_MASTER to s.masterPct.toString(),
            KEY_SFX to s.sfxPct.toString(),
            KEY_MUSIC to s.musicPct.toString(),
            KEY_SENS to s.sensX100.toString(),
            KEY_INVERT_Y to boolToWire(s.invertLookY),
            KEY_QUALITY to s.quality.ordinal.toString(),
            KEY_CALLSIGN to sanitizeCallsign(p.callsign),
            KEY_FACTION to p.faction.ordinal.toString(),
            KEY_MATCHES to p.matchesPlayed.coerceAtLeast(0).toString(),
            KEY_WINS to p.wins.coerceAtLeast(0).toString(),
            KEY_OPTIC to l.optic.coerceIn(0, LoadoutSelection.SLOT_MAX).toString(),
            KEY_BARREL to l.barrel.coerceIn(0, LoadoutSelection.SLOT_MAX).toString(),
            KEY_MAGAZINE to l.magazine.coerceIn(0, LoadoutSelection.SLOT_MAX).toString(),
            // Only the cleared set is persisted; the topology is re-supplied from campaignNodes.
            KEY_CAMPAIGN to state.campaign.encodeCleared(),
        )
    }

    /**
     * Tolerantly decode a stored map back to a [ShellState]. Any missing/garbage/out-of-range value
     * falls back to that field's default — never throws.
     */
    fun decode(map: Map<String, String>): ShellState {
        val ds = SettingsState.defaults()
        val dp = ProfileState()
        val dl = LoadoutSelection()

        val settings = SettingsState(
            masterPct = clampInt(map[KEY_MASTER], 0, SettingsState.GAIN_PCT_MAX, ds.masterPct),
            sfxPct = clampInt(map[KEY_SFX], 0, SettingsState.GAIN_PCT_MAX, ds.sfxPct),
            musicPct = clampInt(map[KEY_MUSIC], 0, SettingsState.GAIN_PCT_MAX, ds.musicPct),
            sensX100 = clampInt(map[KEY_SENS], SettingsState.SENS_MIN, SettingsState.SENS_MAX, ds.sensX100),
            invertLookY = parseBool(map[KEY_INVERT_Y], ds.invertLookY),
            quality = parseQuality(map[KEY_QUALITY], ds.quality),
        )

        val profile = ProfileState(
            // sanitizeCallsign turns a missing/blank value into DEFAULT_CALLSIGN (the field default).
            callsign = sanitizeCallsign(map[KEY_CALLSIGN] ?: ""),
            faction = parseFaction(map[KEY_FACTION], dp.faction),
            matchesPlayed = clampInt(map[KEY_MATCHES], 0, Int.MAX_VALUE, dp.matchesPlayed),
            wins = clampInt(map[KEY_WINS], 0, Int.MAX_VALUE, dp.wins),
        )

        val loadout = LoadoutSelection(
            optic = clampInt(map[KEY_OPTIC], 0, LoadoutSelection.SLOT_MAX, dl.optic),
            barrel = clampInt(map[KEY_BARREL], 0, LoadoutSelection.SLOT_MAX, dl.barrel),
            magazine = clampInt(map[KEY_MAGAZINE], 0, LoadoutSelection.SLOT_MAX, dl.magazine),
        )

        // The topology is re-supplied from campaignNodes; only the cleared set is decoded (tolerant).
        val campaign = CampaignProgress.decodeCleared(map[KEY_CAMPAIGN])

        return ShellState(settings = settings, profile = profile, loadout = loadout, campaign = campaign)
    }

    /** `"1"` for true, `"0"` for false. */
    private fun boolToWire(value: Boolean): String = if (value) "1" else "0"

    /** Parse an integer and clamp to `min..max`; on null/parse-failure keep `fallback`. */
    private fun clampInt(value: String?, min: Int, max: Int, fallback: Int): Int =
        value?.trim()?.toLongOrNull()
            ?.coerceIn(min.toLong(), max.toLong())
            ?.toInt()
            ?: fallback

    /** `1`/`true` → true, `0`/`false` → false, anything else (incl. null) → `fallback`. */
    private fun parseBool(value: String?, fallback: Boolean): Boolean = when (value?.trim()) {
        "1", "true" -> true
        "0", "false" -> false
        else -> fallback
    }

    /** Decode a [Quality] from its stored ordinal; out-of-range / garbage → `fallback`. */
    private fun parseQuality(value: String?, fallback: Quality): Quality {
        val ordinal = value?.trim()?.toIntOrNull() ?: return fallback
        val all = Quality.entries
        return if (ordinal in all.indices) all[ordinal] else fallback
    }

    /** Decode a [FactionPref] from its stored ordinal; out-of-range / garbage → `fallback`. */
    private fun parseFaction(value: String?, fallback: FactionPref): FactionPref {
        val ordinal = value?.trim()?.toIntOrNull() ?: return fallback
        val all = FactionPref.entries
        return if (ordinal in all.indices) all[ordinal] else fallback
    }
}
