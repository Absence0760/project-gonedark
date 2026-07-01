package com.jaredhoward.goingdark

import android.content.Context
import android.content.SharedPreferences

/**
 * The thin Android persistence glue for the out-of-match shell — the device-gated companion to the
 * pure [ShellPrefsCodec]. It opens a private [SharedPreferences] file (`gonedark_shell`) and shuttles
 * the codec's flat string map in and out of it so the player's Settings, Profile, and gunsmith Loadout
 * survive app restarts (today they live only in Compose `remember`).
 *
 * **No logic lives here — by design.** Every clamp / sanitize / tolerant-fallback rule is in
 * [ShellPrefsCodec], which is plain Kotlin and JVM-unit-tested ([ShellPrefsCodecTest]). This class is a
 * trivial read-all / write-all loop over `SharedPreferences`, so it is the test-exempt glue — exactly
 * like the JNI reader in `pal-android` or the winit/android event glue in the engine: un-unit-testable
 * platform plumbing with no decisions of its own. Reads/writes are synchronous; `SharedPreferences`
 * caches in memory, so [load] is cheap and `apply()` flushes off the main thread.
 *
 * Integration (not this class) calls [load] on shell start and [save] when state changes, inside
 * `MainActivity`.
 */
class ShellPrefs(context: Context) {
    private val prefs: SharedPreferences =
        context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

    /**
     * Read every persisted key into a string map and hand it to the codec. A first launch (empty file)
     * or a partially-written file decodes to [ShellState.defaults] / per-field defaults, because the
     * codec is tolerant.
     */
    fun load(): ShellState {
        val map = HashMap<String, String>(ALL_KEYS.size)
        for (key in ALL_KEYS) {
            val value = prefs.getString(key, null)
            if (value != null) {
                map[key] = value
            }
        }
        return ShellPrefsCodec.decode(map)
    }

    /**
     * Encode [state] and write each key, then `apply()` (async flush). Encoding produces the canonical
     * clamped/sanitized representation, so what lands on disk is always valid.
     */
    fun save(state: ShellState) {
        val map = ShellPrefsCodec.encode(state)
        val editor = prefs.edit()
        for ((key, value) in map) {
            editor.putString(key, value)
        }
        editor.apply()
    }

    /**
     * Persist **only** the campaign cleared-set key, leaving Settings/Profile/Loadout untouched. Used
     * for record-on-win from the match-result callback, which owns the campaign progress but not the
     * rest of the shell state (those live in the Compose shell's own `remember`). Writing just the one
     * key keeps that split clean — a subsequent full [save] still round-trips everything.
     */
    fun saveCampaign(campaign: CampaignProgress) {
        prefs.edit().putString(ShellPrefsCodec.KEY_CAMPAIGN, campaign.encodeCleared()).apply()
    }

    companion object {
        /** The private SharedPreferences file backing the out-of-match shell. */
        const val PREFS_NAME = "gonedark_shell"

        /** Every key the codec owns — the read loop pulls exactly these out of the prefs file. */
        private val ALL_KEYS = listOf(
            ShellPrefsCodec.KEY_MASTER,
            ShellPrefsCodec.KEY_SFX,
            ShellPrefsCodec.KEY_MUSIC,
            ShellPrefsCodec.KEY_SENS,
            ShellPrefsCodec.KEY_INVERT_Y,
            ShellPrefsCodec.KEY_QUALITY,
            ShellPrefsCodec.KEY_CVD_CUES,
            ShellPrefsCodec.KEY_SOUND_CUES,
            ShellPrefsCodec.KEY_CALLSIGN,
            ShellPrefsCodec.KEY_FACTION,
            ShellPrefsCodec.KEY_MATCHES,
            ShellPrefsCodec.KEY_WINS,
            ShellPrefsCodec.KEY_OPTIC,
            ShellPrefsCodec.KEY_BARREL,
            ShellPrefsCodec.KEY_MAGAZINE,
            ShellPrefsCodec.KEY_CAMPAIGN,
            ShellPrefsCodec.KEY_ARMY,
        )
    }
}
