package com.jaredhoward.goingdark

/**
 * The **launch-config seam** (Compose shell parity, Tier 0 — `docs/plans/compose-shell-parity.md`
 * §3). The Compose shell ([MainActivity]) and the engine (a separate `NativeActivity` running the
 * Rust cdylib) are two activities; this is the typed payload the shell hands the engine across that
 * boundary as one `Intent` string extra ([EXTRA_KEY]).
 *
 * It is the Kotlin twin of the Rust codec in `pal-android/src/launch.rs` and **mirrors its wire
 * format and rules verbatim** — the [D79](../../../../../../../docs/decisions.md) mirrored-constants
 * discipline. The wire string is a versioned, tolerant `key=value;…` list:
 *
 *   `v=1;scene=skirmish;opt=0;bar=0;mag=0;vol=80;sfx=80;sens=100;invy=0`
 *
 * **Tolerant [decode]:** unknown keys are ignored, missing keys keep their default, and a null /
 * empty / malformed string yields a full default config — never an exception. That tolerance is the
 * forward-compat contract: a later parity tier can emit new keys (e.g. `diff=`) without an older
 * decoder choking. Kept as plain Kotlin (no Android types) so it is JVM-unit-testable off-device —
 * the testable seam the Compose UI is exempt from (the `BuildStamp.kt` pattern, D32 chrome).
 */
data class LaunchConfig(
    /** Scene token (e.g. `"skirmish"`, `"mission1"`); mapped engine-side via `Scene::parse`. */
    val scene: String = "skirmish",
    /** Optic slot index, `0..SLOT_MAX` (0 = Standard). */
    val optic: Int = 0,
    /** Barrel slot index, `0..SLOT_MAX`. */
    val barrel: Int = 0,
    /** Magazine slot index, `0..SLOT_MAX`. */
    val magazine: Int = 0,
    /** Master volume percent, `0..GAIN_PCT_MAX`. */
    val masterPct: Int = 80,
    /** SFX volume percent, `0..GAIN_PCT_MAX`. */
    val sfxPct: Int = 80,
    /** Look sensitivity ×100, `SENS_MIN..SENS_MAX`. */
    val sensX100: Int = 100,
    /** Invert the embodied vertical look axis. */
    val invertY: Boolean = false,
    /**
     * Campaign replay difficulty tier, `0..DIFF_MAX` (Recruit..Elite). The tier a campaign clear is
     * recorded at on a win; inert for non-campaign scenes. Mirrors `launch.rs`'s `diff` key.
     */
    val diff: Int = 0,
    /**
     * Campaign **node index** (the `NodeId` ordinal) the launch targets, for a campaign (`mission1`)
     * scene. Mirrors `launch.rs`'s `node` key: the engine resolves it through the shared mission
     * registry and records the win against it. Inert for non-campaign scenes; missing/garbage → `0`
     * (the root node).
     */
    val node: Int = 0,
    /**
     * Player **army** ordinal — `1` = US, `2` = French (`core::components::Army::index`). Never
     * `Neutral` (`0`): [clampArmy] collapses Neutral / out-of-range / garbage to the US default
     * ([ARMY_DEFAULT]), mirroring the desktop `decode_army`. Fielded at match start via
     * `Game::select_army`. Mirrors `launch.rs`'s `army` key.
     */
    val army: Int = ARMY_DEFAULT,
    /**
     * Accessibility: add the CVD text labels to the embodied alert HUD. Default OFF (opt-in). Fed to
     * the engine via `Game::set_accessibility_prefs`; mirrors `launch.rs`'s `cvd` key (the desktop
     * `cvdcues` pref). Presentation only — never the sim.
     */
    val colorblindCues: Boolean = false,
    /**
     * Accessibility: draw the visual echoes of the audio-only signals. Default OFF. Mirrors
     * `launch.rs`'s `snd` key (the desktop `soundcues` pref). Presentation only — never the sim.
     */
    val visualSoundCues: Boolean = false,
) {
    /** Encode to the v1 wire string (clamping every field into range first). */
    fun encode(): String = buildString {
        append("v=").append(WIRE_VERSION)
        append(";scene=").append(scene)
        append(";opt=").append(optic.coerceIn(0, SLOT_MAX))
        append(";bar=").append(barrel.coerceIn(0, SLOT_MAX))
        append(";mag=").append(magazine.coerceIn(0, SLOT_MAX))
        append(";vol=").append(masterPct.coerceIn(0, GAIN_PCT_MAX))
        append(";sfx=").append(sfxPct.coerceIn(0, GAIN_PCT_MAX))
        append(";sens=").append(sensX100.coerceIn(SENS_MIN, SENS_MAX))
        append(";invy=").append(if (invertY) 1 else 0)
        append(";diff=").append(diff.coerceIn(0, DIFF_MAX))
        append(";node=").append(node.coerceAtLeast(0))
        append(";army=").append(clampArmy(army.toString(), ARMY_DEFAULT))
        append(";cvd=").append(if (colorblindCues) 1 else 0)
        append(";snd=").append(if (visualSoundCues) 1 else 0)
    }

    companion object {
        /** The `Intent` extra key. Mirrors `pal-android/src/launch.rs::EXTRA_KEY`. */
        const val EXTRA_KEY = "com.jaredhoward.goingdark.LAUNCH_CONFIG"
        const val WIRE_VERSION = 1
        const val SLOT_MAX = 2
        const val GAIN_PCT_MAX = 100
        const val SENS_MIN = 10
        const val SENS_MAX = 300
        const val DIFF_MAX = 3

        /**
         * Army wire ordinals — `1` = US, `2` = French (`core::components::Army::index`); Neutral (`0`)
         * is never a player pick. [ARMY_MAX] is the highest combatant ordinal; [ARMY_DEFAULT] (US) is
         * the collapse target for Neutral / out-of-range / garbage. Mirrors `launch.rs`'s consts.
         */
        const val ARMY_MAX = 2
        const val ARMY_DEFAULT = 1

        /** Tolerantly decode the v1 wire string. Null/empty/garbage → a default [LaunchConfig]. */
        fun decode(raw: String?): LaunchConfig {
            var cfg = LaunchConfig()
            if (raw == null) return cfg
            for (token in raw.split(';')) {
                val pair = token.trim()
                if (pair.isEmpty()) continue
                val eq = pair.indexOf('=')
                if (eq <= 0) continue // no key, or empty key — ignore (tolerant)
                val key = pair.substring(0, eq).trim()
                val value = pair.substring(eq + 1).trim()
                cfg = when (key) {
                    "v" -> cfg // advisory; we decode tolerantly regardless of version
                    "scene" -> if (value.isEmpty()) cfg else cfg.copy(scene = value)
                    "opt" -> cfg.copy(optic = clampInt(value, 0, SLOT_MAX, cfg.optic))
                    "bar" -> cfg.copy(barrel = clampInt(value, 0, SLOT_MAX, cfg.barrel))
                    "mag" -> cfg.copy(magazine = clampInt(value, 0, SLOT_MAX, cfg.magazine))
                    "vol" -> cfg.copy(masterPct = clampInt(value, 0, GAIN_PCT_MAX, cfg.masterPct))
                    "sfx" -> cfg.copy(sfxPct = clampInt(value, 0, GAIN_PCT_MAX, cfg.sfxPct))
                    "sens" -> cfg.copy(sensX100 = clampInt(value, SENS_MIN, SENS_MAX, cfg.sensX100))
                    "invy" -> cfg.copy(invertY = parseBool(value, cfg.invertY))
                    "diff" -> cfg.copy(diff = clampInt(value, 0, DIFF_MAX, cfg.diff))
                    "node" -> cfg.copy(node = clampInt(value, 0, Int.MAX_VALUE, cfg.node))
                    "army" -> cfg.copy(army = clampArmy(value, cfg.army))
                    "cvd" -> cfg.copy(colorblindCues = parseBool(value, cfg.colorblindCues))
                    "snd" -> cfg.copy(visualSoundCues = parseBool(value, cfg.visualSoundCues))
                    else -> cfg // unknown key — ignore (forward-compat)
                }
            }
            return cfg
        }

        /** Parse `value` as an integer and clamp to `min..max`; on parse failure keep `fallback`. */
        private fun clampInt(value: String, min: Int, max: Int, fallback: Int): Int =
            value.toLongOrNull()?.coerceIn(min.toLong(), max.toLong())?.toInt() ?: fallback

        /**
         * Parse the army ordinal into a valid combatant index. Mirrors the desktop `decode_army` and
         * `launch.rs::clamp_army`: only `1` (US) and `2` (French) are real picks; Neutral (`0`),
         * out-of-range, or unparseable values collapse to [ARMY_DEFAULT] (US). Deliberately does NOT
         * clamp-to-[ARMY_MAX] (an out-of-range ordinal degrades to the neutral default, not French).
         */
        private fun clampArmy(value: String, fallback: Int): Int = when (value.trim().toLongOrNull()) {
            1L -> 1 // US Army
            2L -> 2 // French Army
            null -> fallback // unparseable — keep the current value (defaults to US)
            else -> ARMY_DEFAULT // Neutral (0) / out-of-range → US (never a player pick)
        }

        /** Wire bool: `1`/`true` → true, `0`/`false` → false, else `fallback`. */
        private fun parseBool(value: String, fallback: Boolean): Boolean = when (value) {
            "1", "true" -> true
            "0", "false" -> false
            else -> fallback
        }
    }
}
