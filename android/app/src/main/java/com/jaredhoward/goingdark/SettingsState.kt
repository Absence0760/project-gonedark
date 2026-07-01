package com.jaredhoward.goingdark

/**
 * The **Settings preferences seam** (Compose shell parity, Tier 1 — `docs/plans/compose-shell-parity.md`,
 * Settings row). The native Compose twin of the desktop egui `SettingsState` in `app/src/shell.rs`:
 * the same player preferences (volume buses, look sensitivity, invert-Y, render quality) the embodied
 * shell edits before handing off to the engine.
 *
 * **Integer ranges, not floats.** The desktop reference stores gains/sensitivity as `f32` (`0.0..=1.0`,
 * `SENS_MIN..=SENS_MAX`); the Compose shell instead stores them as the **wire-key integers** the
 * [LaunchConfig] payload carries — `vol`/`sfx`/`sens`/`invy` — so this state maps onto the launch
 * intent with no lossy float round-trip. The bounds below are therefore deliberately **mirrored from
 * [LaunchConfig]'s companion** ([LaunchConfig.GAIN_PCT_MAX], [LaunchConfig.SENS_MIN],
 * [LaunchConfig.SENS_MAX]) — the [D79](../../../../../../../docs/decisions.md) mirrored-constants
 * discipline (and a test pins them equal). `music` has no wire key yet (the desktop `music_volume` is a
 * dormant stored pref); it reuses the gain-percent bounds.
 *
 * Kept as plain Kotlin with **no Android/Compose types** so it is JVM-unit-testable off-device — the
 * testable seam the Compose UI ([SettingsScreen]) is exempt from (the `BuildStamp.kt` / D32 pattern).
 * The composable is a pure render of this immutable state; every edit produces a [clamp]ed copy.
 */
data class SettingsState(
    /** Master output gain percent, `0..GAIN_PCT_MAX`. Mirrors desktop `master_volume`. */
    val masterPct: Int = 80,
    /** SFX bus gain percent, `0..GAIN_PCT_MAX`. Mirrors desktop `sfx_volume`. */
    val sfxPct: Int = 80,
    /** Music bus gain percent, `0..GAIN_PCT_MAX`. Mirrors desktop `music_volume` (no wire key yet). */
    val musicPct: Int = 60,
    /** Look sensitivity ×100, `SENS_MIN..SENS_MAX` (100 = 1.0×). Mirrors desktop `mouse_sensitivity`. */
    val sensX100: Int = 100,
    /** Invert the embodied vertical look axis. Mirrors desktop `invert_look_y`. */
    val invertLookY: Boolean = false,
    /** Render-quality preference. Mirrors desktop `QualityChoice`. */
    val quality: Quality = Quality.Auto,
    /**
     * Accessibility — add the colorblind (CVD) text labels to the embodied alert HUD. Default OFF (an
     * opt-in intensifier; the base alerts already carry shape + luminance redundancy). Mirrors desktop
     * `colorblind_cues`; carried to the engine via the `cvd` launch-wire key → `set_accessibility_prefs`.
     */
    val colorblindCues: Boolean = false,
    /**
     * Accessibility — draw the hard-of-hearing visual echoes of the audio-only signals. Default OFF.
     * Mirrors desktop `visual_sound_cues`; carried via the `snd` launch-wire key.
     */
    val visualSoundCues: Boolean = false,
) {
    /** Return a copy with every field pinned into its valid range. Pure — the post-edit re-bound. */
    fun clamp(): SettingsState = copy(
        masterPct = masterPct.coerceIn(0, GAIN_PCT_MAX),
        sfxPct = sfxPct.coerceIn(0, GAIN_PCT_MAX),
        musicPct = musicPct.coerceIn(0, GAIN_PCT_MAX),
        sensX100 = sensX100.coerceIn(SENS_MIN, SENS_MAX),
    )

    /** Restore the shipped defaults — the Settings RESET button. Pure (returns a fresh default). */
    fun reset(): SettingsState = defaults()

    companion object {
        // Bounds mirrored verbatim from LaunchConfig's companion (D79 mirrored-constants discipline);
        // SettingsStateTest pins these equal so a drift in one is caught.
        const val GAIN_PCT_MAX = LaunchConfig.GAIN_PCT_MAX
        const val SENS_MIN = LaunchConfig.SENS_MIN
        const val SENS_MAX = LaunchConfig.SENS_MAX

        /** The shipped defaults — twin of the desktop `SettingsState::default()`. */
        fun defaults(): SettingsState = SettingsState(
            masterPct = 80,
            sfxPct = 80,
            musicPct = 60,
            sensX100 = 100,
            invertLookY = false,
            quality = Quality.Auto,
            colorblindCues = false,
            visualSoundCues = false,
        )
    }
}

/**
 * Render-quality preference — the Compose twin of the desktop `QualityChoice` enum. A discrete choice
 * cycled by a `<`/`>` style control (no slider). [Auto] lets the thermal/perf tier controller decide
 * (the shipped default).
 */
enum class Quality {
    Auto,
    Low,
    Medium,
    High;

    /** The on-screen label. */
    fun label(): String = when (this) {
        Auto -> "Auto"
        Low -> "Low"
        Medium -> "Medium"
        High -> "High"
    }

    /** The next choice, wrapping — what the cycler advances to (mirrors desktop `QualityChoice::next`). */
    fun next(): Quality {
        val all = entries
        return all[(ordinal + 1) % all.size]
    }
}
