package com.jaredhoward.goingdark

/**
 * The Deploy **launch-resolution seam** — the pure functions that assemble the [LaunchConfig] the
 * engine receives when the Compose shell boots a match, the Kotlin mirror of the desktop host's
 * `pending_launch` assembly (`app/src/main.rs`: the scene + `resolve_node` pair the run loop hands
 * the engine).
 *
 * Per **D79**, this decision logic is plain Kotlin with **no Android imports** so it is
 * unit-testable on the plain JVM (the `TitleAction.kt` / `BuildStamp.kt` pattern) — the composable
 * host ([MainActivity]'s `Shell`) consumes THESE functions, so `MissionLaunchTest.kt` covers the
 * wiring the app actually runs. In particular [missionLaunchConfig] is the campaign node→launch
 * resolution: it threads **any playable [MissionNode]** — the root *Seize* or the gated *Hold* —
 * into the same wire (`scene=` its [MissionNode.sceneToken], `node=` its [MissionNode.id],
 * `diff=` the replay tier), matching the engine's `Scene::parse` / mission-registry resolution and
 * the node the win result-code is packed against.
 */

/**
 * Assemble the [LaunchConfig] the engine receives at Deploy: the chosen scene token, the
 * [LoadoutSelection] slot indices, the [SettingsState] audio / look / accessibility prefs, the picked
 * [army], and the campaign replay [diff] tier + [node] index folded into the wire keys
 * (`opt`/`bar`/`mag`/`stk`/`muz`, `vol`/`sfx`/`sens`/`invy`, `army`, `cvd`/`snd`, `diff`/`node`). Pure — kept out of
 * the composable so the wiring is obvious. [diff]/[node] are the campaign replay tier + node index; both
 * are inert (`0`) for non-campaign Deploys (ModeSelect), so those keep their prior behaviour.
 */
fun launchConfigOf(
    scene: String,
    settings: SettingsState,
    loadout: LoadoutSelection,
    army: Army,
    diff: Int = 0,
    node: Int = 0,
): LaunchConfig =
    LaunchConfig(
        scene = scene,
        optic = loadout.optic,
        barrel = loadout.barrel,
        magazine = loadout.magazine,
        stock = loadout.stock,
        muzzle = loadout.muzzle,
        masterPct = settings.masterPct,
        sfxPct = settings.sfxPct,
        sensX100 = settings.sensX100,
        invertY = settings.invertLookY,
        diff = diff,
        node = node,
        army = army.index,
        colorblindCues = settings.colorblindCues,
        visualSoundCues = settings.visualSoundCues,
    )

/**
 * Resolve a campaign [node] into its launch wire — the briefing Deploy path. The node's
 * [MissionNode.sceneToken] becomes the `scene=` key (engine-side `Scene::parse` — Seize →
 * `mission1`, Hold → `mission2`), its [MissionNode.id] (the `NodeId` ordinal) the `node=` key the
 * engine resolves through the shared mission registry and records the win against, and the chosen
 * replay [difficulty] the `diff=` key (the tier the clear is recorded at, C3). Every **playable**
 * node goes through this one function — the lock gate is upstream (a Locked tile never reaches the
 * briefing; [NodeProgress.isPlayable]), so nothing here re-guards it.
 */
fun missionLaunchConfig(
    node: MissionNode,
    settings: SettingsState,
    loadout: LoadoutSelection,
    army: Army,
    difficulty: Difficulty,
): LaunchConfig =
    launchConfigOf(
        scene = node.sceneToken,
        settings = settings,
        loadout = loadout,
        army = army,
        diff = difficulty.tier(),
        node = node.id,
    )
