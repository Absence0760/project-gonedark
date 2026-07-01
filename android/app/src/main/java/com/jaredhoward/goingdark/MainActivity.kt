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
 * "Boot & title") and the out-of-match shell around it (Compose shell parity, Tier 1/2): Settings,
 * Profile, Field Manual, the campaign Operations hub (mission-select → briefing), and the pre-match
 * gunsmith. It owns only out-of-match chrome — its state is which surface is up, the player's prefs
 * / profile / loadout (persisted across launches via [ShellPrefs]), and the in-flight campaign
 * selection. This mirrors the desktop host's in-memory `Screen`/`SettingsState`/`ProfileState`/
 * `LoadoutEditor`/campaign (`app/src/main.rs`).
 *
 * Deploy launches the shared **Rust engine** ([NativeActivity], which loads
 * `libgonedark_pal_android.so` and runs `engine::Game`), threading a [LaunchConfig] across the seam
 * (Tier 0): the chosen scene + gunsmith loadout + audio prefs are folded into the wire and consumed
 * by `android_main`. The Compose shell and the engine live in separate activities — the D32
 * native/in-engine split: out-of-match chrome is native, the in-match (and in-session) surfaces are
 * in-engine under avatar-only fog (invariant #6).
 *
 * Campaign opens the Operations hub → a mission briefing → Deploy into the mission scene (`mission1`,
 * where the loadout applies to the player's troops). PvE/PvP open a **mode/map select** → Deploy into
 * the chosen scene (**D81**: the loadout gunsmith no longer gates play — it moved behind Settings, so
 * a play-mode tap goes straight toward the match). PvE and PvP share the picker until PvP match-setup
 * lands (Q5-Phase-3-blocked, phase-4-plan §2). The loadout persists (via [ShellPrefs]) and is folded
 * into every Deploy regardless of which flow launched it. The briefing's **difficulty** selector is
 * shown but not yet threaded to the engine on Android (owed — needs a `diff` wire key + mission-tuning
 * plumbing; the desktop already threads it). Look-sensitivity from Settings is likewise owed (the
 * Android look delta is derived in `engine::touch_controls`, not scalable at the PAL boundary).
 *
 * The title→screen routing goes through the unit-tested [resolveTitleAction] seam (D81), so the JVM
 * tests cover the navigation the app actually runs. NB the desktop egui shell (`app/src/shell.rs`)
 * still routes Pve/Pvp through the gunsmith — reconciling it to this flow is the owed D79 parity half.
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val stamp = buildStamp(buildChannel(BuildConfig.DEBUG), BuildConfig.VERSION_NAME)
        val prefs = ShellPrefs(this)
        setContent {
            GoingDarkTheme {
                Shell(
                    versionStamp = stamp,
                    initial = prefs.load(),
                    onPersist = prefs::save,
                    onQuit = ::finish,
                    onDeploy = ::startMatch,
                )
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
private enum class ShellRoute { Title, ModeSelect, Settings, Profile, About, MissionSelect, Briefing, Gunsmith }

/**
 * The out-of-match shell navigator: a flat `when` over [ShellRoute] holding the player's prefs,
 * profile, loadout, and in-flight campaign selection in `remember`. State is seeded from `initial`
 * (loaded from [ShellPrefs]) and pushed back through `onPersist` whenever the player edits Settings,
 * Profile, or the loadout — so it survives a restart. `onDeploy` boots the engine with the assembled
 * [LaunchConfig]; `onQuit` finishes the activity.
 */
@Composable
private fun Shell(
    versionStamp: String,
    initial: ShellState,
    onPersist: (ShellState) -> Unit,
    onQuit: () -> Unit,
    onDeploy: (LaunchConfig) -> Unit,
) {
    var route by remember { mutableStateOf(ShellRoute.Title) }
    var settings by remember { mutableStateOf(initial.settings) }
    var profile by remember { mutableStateOf(initial.profile) }
    var loadout by remember { mutableStateOf(initial.loadout) }
    // Campaign flow state: the node being briefed, and the selected replay difficulty (shown in the
    // briefing; engine-application owed). The launch scene is no longer held here — each Deploy path
    // (ModeSelect / Briefing) carries its own scene token straight into `launchConfigOf`.
    var briefedNode by remember { mutableStateOf(campaignNodes.first()) }
    var difficulty by remember { mutableStateOf(Difficulty.Recruit) }

    fun persist() = onPersist(ShellState(settings, profile, loadout))

    // Route a title action through the SAME pure seam the JVM tests cover (D81), so the live
    // navigation can't silently drift from `resolveTitleAction`.
    fun applyTitle(action: TitleAction) {
        when (resolveTitleAction(action)) {
            TitleRoute.MissionSelect -> route = ShellRoute.MissionSelect
            TitleRoute.ModeSelect -> route = ShellRoute.ModeSelect
            TitleRoute.Settings -> route = ShellRoute.Settings
            TitleRoute.Profile -> route = ShellRoute.Profile
            TitleRoute.About -> route = ShellRoute.About
            TitleRoute.Quit -> onQuit()
        }
    }

    when (route) {
        ShellRoute.Title -> TitleScreen(
            versionStamp = versionStamp,
            onCampaign = { applyTitle(TitleAction.Campaign) },
            onPve = { applyTitle(TitleAction.Pve) },
            onPvp = { applyTitle(TitleAction.Pvp) },
            onSettings = { applyTitle(TitleAction.Settings) },
            onProfile = { applyTitle(TitleAction.Profile) },
            onAbout = { applyTitle(TitleAction.About) },
            onQuit = { applyTitle(TitleAction.Quit) },
        )
        ShellRoute.ModeSelect -> ModeSelectScreen(
            modes = shellGameModes,
            // Pick a mode → Deploy straight into its scene with the persisted loadout (no gunsmith).
            onPick = { onDeploy(launchConfigOf(it.sceneToken, settings, loadout)) },
            onBack = { route = ShellRoute.Title },
        )
        ShellRoute.Settings -> SettingsScreen(
            state = settings,
            onChange = { settings = it; persist() },
            // The gunsmith now lives under Settings (D81) — loadout customization, not a play gate.
            onOpenLoadout = { route = ShellRoute.Gunsmith },
            onBack = { route = ShellRoute.Title },
        )
        ShellRoute.Profile -> ProfileScreen(
            state = profile,
            onChange = { profile = it; persist() },
            onBack = { route = ShellRoute.Title },
        )
        ShellRoute.About -> AboutScreen(
            versionStamp = versionStamp,
            onBack = { route = ShellRoute.Title },
        )
        ShellRoute.MissionSelect -> MissionSelectScreen(
            nodes = campaignNodes,
            onOpenNode = { briefedNode = it; route = ShellRoute.Briefing },
            onBack = { route = ShellRoute.Title },
        )
        ShellRoute.Briefing -> BriefingScreen(
            node = briefedNode,
            difficulty = difficulty,
            onCycleDifficulty = { difficulty = difficulty.next() },
            // Briefing Deploy boots this mission's scene directly with the persisted loadout — the
            // gunsmith is no longer an intermediate step (D81).
            onDeploy = { onDeploy(launchConfigOf(briefedNode.sceneToken, settings, loadout)) },
            onBack = { route = ShellRoute.MissionSelect },
        )
        ShellRoute.Gunsmith -> GunsmithScreen(
            selection = loadout,
            onChange = { loadout = it; persist() },
            onReset = { loadout = loadout.reset(); persist() },
            // Customization only (D81): DONE returns to Settings, where the gunsmith is reached from.
            onBack = { route = ShellRoute.Settings },
        )
    }
}

/**
 * Assemble the [LaunchConfig] the engine receives at Deploy: the chosen scene token, the
 * [LoadoutSelection] slot indices, and the [SettingsState] audio prefs folded into the wire keys
 * (`opt`/`bar`/`mag`, `vol`/`sfx`/`sens`/`invy`). Pure — kept out of the composable so the wiring is
 * obvious. (Sensitivity/invert are carried but not yet applied on Android — see the class doc.)
 */
private fun launchConfigOf(
    scene: String,
    settings: SettingsState,
    loadout: LoadoutSelection,
): LaunchConfig =
    LaunchConfig(
        scene = scene,
        optic = loadout.optic,
        barrel = loadout.barrel,
        magazine = loadout.magazine,
        masterPct = settings.masterPct,
        sfxPct = settings.sfxPct,
        sensX100 = settings.sensX100,
        invertY = settings.invertLookY,
    )
