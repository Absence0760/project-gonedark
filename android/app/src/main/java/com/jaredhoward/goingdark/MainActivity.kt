package com.jaredhoward.goingdark

import android.app.NativeActivity
import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
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
 * Campaign opens the Operations hub → a mission briefing → Deploy into the briefed node's own scene
 * (`mission1`/`mission2` via [missionLaunchConfig] — every playable node, not just the root). The hub
 * renders per-node status pills (LOCKED /
 * AVAILABLE / CLEARED·tier) and disables locked tiles from the pure [CampaignProgress] model, and a
 * campaign win is recorded (best-tier) and persisted via [ShellPrefs] — the split-activity twin of
 * the desktop host's record-on-win, delivered back as an Activity result code. PvE/PvP open a
 * **mode/map select** → Deploy into the chosen scene (**D81**: the loadout gunsmith no longer gates
 * play — it moved behind Settings, so a play-mode tap goes straight toward the match). PvE and PvP
 * share the picker until PvP match-setup lands (Q5-Phase-3-blocked, phase-4-plan §2). The loadout
 * persists (via [ShellPrefs]) and is folded into every Deploy regardless of which flow launched it.
 * The briefing's **difficulty** selector is now threaded to the engine as the `diff` wire key (the
 * tier the clear is recorded at on a win). Look-sensitivity from Settings is likewise carried but not
 * yet applied on Android (the look delta is derived in `engine::touch_controls`, not scalable at the
 * PAL boundary).
 *
 * The title→screen routing goes through the unit-tested [resolveTitleAction] seam (D81), so the JVM
 * tests cover the navigation the app actually runs. NB the desktop egui shell (`app/src/shell.rs`)
 * still routes Pve/Pvp through the gunsmith — reconciling it to this flow is the owed D79 parity half.
 */
class MainActivity : ComponentActivity() {
    private lateinit var prefs: ShellPrefs

    /**
     * Campaign progress, hoisted to the Activity so the match-result callback (below) can record a
     * clear the shell then re-renders from. Seeded from [ShellPrefs] in [onCreate]; the rest of the
     * shell state (Settings/Profile/Loadout) stays in the [Shell] composable's own `remember`.
     */
    private var campaign by mutableStateOf(CampaignProgress())

    /**
     * The engine runs in a separate `NativeActivity`, so a campaign WIN comes back as an Activity
     * **result code** (`Activity.setResult`, packed by the engine — see
     * [CampaignResult.fromResultCode]). This is the split-activity twin of the desktop host's
     * single-process record-on-win: decode the win, record the clear at the played tier (best-tier
     * kept), and persist just the campaign key. A non-win return (`RESULT_CANCELED`) decodes to `null`
     * → nothing recorded.
     */
    private val matchResult =
        registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            val win = CampaignResult.fromResultCode(result.resultCode) ?: return@registerForActivityResult
            val updated = campaign.recordClear(win.node, win.tier)
            if (updated != campaign) {
                campaign = updated
                prefs.saveCampaign(updated)
            }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val stamp = buildStamp(buildChannel(BuildConfig.DEBUG), BuildConfig.VERSION_NAME)
        prefs = ShellPrefs(this)
        val initial = prefs.load()
        campaign = initial.campaign
        setContent {
            GoingDarkTheme {
                Shell(
                    versionStamp = stamp,
                    initial = initial,
                    campaign = campaign,
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
     * reads back over JNI. Launched **for a result** so a campaign win reports back (see [matchResult]).
     */
    private fun startMatch(config: LaunchConfig) {
        matchResult.launch(
            Intent(this, NativeActivity::class.java)
                .putExtra(LaunchConfig.EXTRA_KEY, config.encode()),
        )
    }
}

/** Which out-of-match shell surface is up — the Compose twin of the desktop host's `Screen` enum. */
private enum class ShellRoute { Title, ModeSelect, Pvp, Settings, Profile, ArmySelect, About, MissionSelect, Briefing, Gunsmith }

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
    campaign: CampaignProgress,
    onPersist: (ShellState) -> Unit,
    onQuit: () -> Unit,
    onDeploy: (LaunchConfig) -> Unit,
) {
    var route by remember { mutableStateOf(ShellRoute.Title) }
    var settings by remember { mutableStateOf(initial.settings) }
    var profile by remember { mutableStateOf(initial.profile) }
    var loadout by remember { mutableStateOf(initial.loadout) }
    // The player's picked real-army roster (US/French), fielded at every Deploy via the `army` wire
    // key → `Game::select_army`. Persisted like the loadout; the twin of the desktop `army_select`.
    var army by remember { mutableStateOf(initial.army) }
    // Campaign flow state: the node being briefed, and the selected replay difficulty (threaded to
    // the engine on Deploy via the `diff` wire key, C3). `campaign` (the cleared/locked progress) is
    // hoisted to MainActivity so the match-result callback can record a win; the shell only reads it.
    // The launch scene is not held here — each Deploy path (ModeSelect / Briefing) carries its own
    // scene token straight into the pure launch-resolution seam (`MissionLaunch.kt`).
    var briefedNode by remember { mutableStateOf(campaignNodes.first()) }
    var difficulty by remember { mutableStateOf(Difficulty.Recruit) }

    // Persist keeps the current (hoisted) campaign so a Settings/Profile/Loadout save never clobbers
    // the campaign key back to empty.
    fun persist() = onPersist(ShellState(settings, profile, loadout, campaign, army))

    // Route a title action through the SAME pure seam the JVM tests cover (D81), so the live
    // navigation can't silently drift from `resolveTitleAction`.
    fun applyTitle(action: TitleAction) {
        when (resolveTitleAction(action)) {
            TitleRoute.MissionSelect -> route = ShellRoute.MissionSelect
            TitleRoute.ModeSelect -> route = ShellRoute.ModeSelect
            TitleRoute.Pvp -> route = ShellRoute.Pvp
            TitleRoute.Settings -> route = ShellRoute.Settings
            TitleRoute.Profile -> route = ShellRoute.Profile
            TitleRoute.ArmySelect -> route = ShellRoute.ArmySelect
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
            onArmy = { applyTitle(TitleAction.Army) },
            onAbout = { applyTitle(TitleAction.About) },
            onQuit = { applyTitle(TitleAction.Quit) },
        )
        ShellRoute.ModeSelect -> ModeSelectScreen(
            modes = shellGameModes,
            // Pick a battlefield → Deploy straight into its scene with the persisted loadout + army
            // (no gunsmith). Non-campaign, so `node` stays 0 (inert for these scenes).
            onPick = { onDeploy(launchConfigOf(it.sceneToken, settings, loadout, army)) },
            onBack = { route = ShellRoute.Title },
        )
        // The PvP staging door (D101): read-only pre-net chrome — the queues in build order plus
        // the §4a identity line. Nothing deploys from here until the Phase 3 net layer lands.
        ShellRoute.Pvp -> PvpScreen(
            playerArmy = army,
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
        ShellRoute.ArmySelect -> ArmySelectScreen(
            selected = army,
            // Choosing an army edits the pick in place (stays on-screen so the two identities can be
            // compared, mirroring the desktop); it persists immediately so it survives a restart.
            onChoose = { army = it; persist() },
            // CONFIRM returns to the title; the confirmed pick is fielded at the next Deploy.
            onConfirm = { route = ShellRoute.Title },
            onBack = { route = ShellRoute.Title },
        )
        ShellRoute.About -> AboutScreen(
            versionStamp = versionStamp,
            onBack = { route = ShellRoute.Title },
        )
        ShellRoute.MissionSelect -> MissionSelectScreen(
            campaign = campaign,
            onOpenNode = { briefedNode = it; route = ShellRoute.Briefing },
            onBack = { route = ShellRoute.Title },
        )
        ShellRoute.Briefing -> BriefingScreen(
            node = briefedNode,
            progress = campaign.progress(briefedNode.id),
            difficulty = difficulty,
            onCycleDifficulty = { difficulty = difficulty.next() },
            // Briefing Deploy boots this mission's scene directly with the persisted loadout — the
            // gunsmith is no longer an intermediate step (D81) — threading the chosen replay tier as
            // `diff` (C3) so the engine records the clear at it on a win.
            // Resolve the briefed node — root or gated, any playable node — through the pure,
            // JVM-tested seam (MissionLaunch.kt): its sceneToken + node index (`NodeId` ordinal) +
            // the chosen replay tier all thread the launch wire, matching the desktop host's
            // `pending_launch` scene/node pair.
            onDeploy = { onDeploy(missionLaunchConfig(briefedNode, settings, loadout, army, difficulty)) },
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
