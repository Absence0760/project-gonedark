package com.jaredhoward.goingdark

import android.app.NativeActivity
import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import com.jaredhoward.goingdark.ui.theme.GoingDarkTheme

/**
 * The app's LAUNCHER: the native Jetpack-Compose **app shell** the player lands on (D32 surface 1,
 * "Boot & title") — and now the out-of-match shell around it (Compose shell parity, Tier 1/2):
 * Settings, Profile, Field Manual, and the pre-match gunsmith. It owns only out-of-match chrome and
 * holds no game/sim state — its only state is which shell surface is up and the player's
 * presentation prefs / loadout selection, all in Compose `remember` (parity with the desktop host's
 * in-memory `Screen`/`SettingsState`/`ProfileState`/`LoadoutEditor`, `app/src/main.rs`).
 *
 * Deploy launches the shared **Rust engine** ([NativeActivity], which loads
 * `libgonedark_pal_android.so` and runs `engine::Game`), threading a [LaunchConfig] across the seam
 * (Tier 0): it boots the real **Skirmish** match (desktop's default boot) with the chosen loadout +
 * audio/look prefs folded into the wire. The Compose shell and the engine live in separate
 * activities — the D32 native/in-engine split made concrete: out-of-match chrome is native, the
 * in-match (and in-session) surfaces are in-engine under avatar-only fog (invariant #6).
 *
 * Mode divergence (Campaign vs PvE vs PvP) is future work — all three currently open the gunsmith
 * and Deploy into Skirmish, mirroring the desktop shell where each still folds to the loadout
 * screen (`app/src/shell.rs::resolve_title_action`). Campaign's Operations-hub mission-select and
 * the PvP match-setup half stay pending (mission-select is the next parity surface; PvP is
 * Q5/Phase-3-blocked, phase-4-plan §2).
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val stamp = buildStamp(buildChannel(BuildConfig.DEBUG), BuildConfig.VERSION_NAME)
        setContent {
            GoingDarkTheme {
                Shell(versionStamp = stamp, onQuit = ::finish, onDeploy = ::startMatch)
            }
        }
    }

    /**
     * Hand off to the shared engine: launch the NativeActivity that loads the Rust cdylib, carrying
     * the [LaunchConfig] as an `Intent` string extra ([LaunchConfig.EXTRA_KEY]) that `android_main`
     * reads back over JNI.
     */
    private fun startMatch(config: LaunchConfig) {
        startActivity(
            Intent(this, NativeActivity::class.java)
                .putExtra(LaunchConfig.EXTRA_KEY, config.encode()),
        )
    }
}

/** Which out-of-match shell surface is up — the Compose twin of the desktop host's `Screen` enum. */
private enum class ShellRoute { Title, Settings, Profile, About, Gunsmith }

/**
 * The out-of-match shell navigator: a flat `when` over [ShellRoute] holding the player's prefs and
 * loadout in `remember` (no persistence yet — parity with the desktop host's in-memory state).
 * `onDeploy` boots the engine with the assembled [LaunchConfig]; `onQuit` finishes the activity.
 */
@Composable
private fun Shell(
    versionStamp: String,
    onQuit: () -> Unit,
    onDeploy: (LaunchConfig) -> Unit,
) {
    var route by remember { mutableStateOf(ShellRoute.Title) }
    var settings by remember { mutableStateOf(SettingsState.defaults()) }
    var profile by remember { mutableStateOf(ProfileState()) }
    var loadout by remember { mutableStateOf(LoadoutSelection()) }

    when (route) {
        ShellRoute.Title -> TitleScreen(
            versionStamp = versionStamp,
            // Campaign/PvE/PvP all open the gunsmith for now (mode divergence is future work),
            // exactly as the desktop title folds each play mode to the loadout screen.
            onCampaign = { route = ShellRoute.Gunsmith },
            onPve = { route = ShellRoute.Gunsmith },
            onPvp = { route = ShellRoute.Gunsmith },
            onSettings = { route = ShellRoute.Settings },
            onProfile = { route = ShellRoute.Profile },
            onAbout = { route = ShellRoute.About },
            onQuit = onQuit,
        )
        ShellRoute.Settings -> SettingsScreen(
            state = settings,
            onChange = { settings = it },
            onBack = { route = ShellRoute.Title },
        )
        ShellRoute.Profile -> ProfileScreen(
            state = profile,
            onChange = { profile = it },
            onBack = { route = ShellRoute.Title },
        )
        ShellRoute.About -> AboutScreen(onBack = { route = ShellRoute.Title })
        ShellRoute.Gunsmith -> GunsmithScreen(
            selection = loadout,
            onChange = { loadout = it },
            onDeploy = { onDeploy(launchConfigOf(settings, loadout)) },
            onBack = { route = ShellRoute.Title },
        )
    }
}

/**
 * Assemble the [LaunchConfig] the engine receives at Deploy: the chosen [LoadoutSelection] slot
 * indices and the [SettingsState] audio/look prefs folded into the wire keys (`opt`/`bar`/`mag`,
 * `vol`/`sfx`/`sens`/`invy`). Scene is the real Skirmish match. Pure — kept out of the composable so
 * the wiring is obvious and testable.
 */
private fun launchConfigOf(settings: SettingsState, loadout: LoadoutSelection): LaunchConfig =
    LaunchConfig(
        scene = "skirmish",
        optic = loadout.optic,
        barrel = loadout.barrel,
        magazine = loadout.magazine,
        masterPct = settings.masterPct,
        sfxPct = settings.sfxPct,
        sensX100 = settings.sensX100,
        invertY = settings.invertLookY,
    )
