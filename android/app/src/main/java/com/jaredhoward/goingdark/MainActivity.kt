package com.jaredhoward.goingdark

import android.app.NativeActivity
import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import com.jaredhoward.goingdark.ui.theme.GoingDarkTheme

/**
 * The app's LAUNCHER: the native Jetpack-Compose **app shell** the player lands on (D32 surface 1,
 * "Boot & title"). It owns only out-of-match chrome and holds no game/sim state.
 *
 * "Start" launches the shared **Rust engine** ([NativeActivity], which loads
 * `libgonedark_pal_android.so` and runs `engine::Game`). The Compose shell and the engine live in
 * separate activities — the D32 native/in-engine split made concrete: out-of-match chrome is native,
 * the in-match (and in-session) surfaces are in-engine under avatar-only fog (invariant #6).
 *
 * Start now threads a [LaunchConfig] across the seam (Compose shell parity, Tier 0): it boots the
 * real **Skirmish** match (desktop's default boot), not the canned demo. The wire also carries
 * loadout / audio / look prefs for later tiers (the gunsmith + Settings surfaces) — tolerant-decoded
 * to defaults until those surfaces populate them. Full match-setup config (army / map / mode) stays
 * Q5/Phase-3-blocked (phase-4-plan §2/§4). Settings is a no-op placeholder until the Settings
 * surface lands.
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val stamp = buildStamp(buildChannel(BuildConfig.DEBUG), BuildConfig.VERSION_NAME)
        setContent {
            GoingDarkTheme {
                TitleScreen(
                    versionStamp = stamp,
                    onStart = ::startMatch,
                    onSettings = { /* Settings surface not built yet (phase-4-plan §2). */ },
                    onQuit = ::finish,
                )
            }
        }
    }

    /**
     * Hand off to the shared engine: launch the NativeActivity that loads the Rust cdylib, carrying
     * the [LaunchConfig] as an `Intent` string extra ([LaunchConfig.EXTRA_KEY]) that `android_main`
     * reads back over JNI. Boots the real Skirmish match; later tiers pass loadout/prefs here.
     */
    private fun startMatch() {
        val config = LaunchConfig(scene = "skirmish")
        startActivity(
            Intent(this, NativeActivity::class.java)
                .putExtra(LaunchConfig.EXTRA_KEY, config.encode()),
        )
    }
}
