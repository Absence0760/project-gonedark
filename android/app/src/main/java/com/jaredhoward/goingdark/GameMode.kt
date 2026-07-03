package com.jaredhoward.goingdark

/**
 * The **standing-battle table** (D81) — the launchable free-pick battles the skirmish setup lists
 * as its battlefields, the Kotlin mirror of `engine::shell_modes::SHELL_GAME_MODES`. Born as the
 * shared Pve/Pvp "mode / map select"; with the three front doors distinct (`modes.md` §1, D101),
 * it backs the SKIRMISH door's battlefield picker alone. Each entry names a launchable battle and
 * carries the engine [sceneToken] `Scene::parse` resolves; a Deploy boots straight into that scene
 * with the player's persisted loadout — no gunsmith gate. It grows into the `modes.md` §3
 * map-library manifest when the D34 listing seam lands.
 *
 * **No Android imports** on purpose: this is the testable seam (the `CampaignModel.kt` / D79 pattern)
 * that the device-gated [SkirmishSetupScreen] composable renders. The one bit of real logic here —
 * that every mode's [sceneToken] is one the engine actually understands — is pinned in `GameModeTest.kt`.
 */
data class GameMode(
    /** Stable id (also the tap key). */
    val id: String,
    /** Display name, shown on the tile. */
    val name: String,
    /** The engine scene token handed to `Scene::parse` at Deploy (must be in [KNOWN_SCENE_TOKENS]). */
    val sceneToken: String,
    /** One-line teaser under the name. */
    val blurb: String,
)

/**
 * The scene tokens `engine::lib::Scene::parse` accepts — the guard the [GameModeTest] checks every
 * [GameMode] against so a typo (e.g. `"skrimish"`) can't ship an un-launchable mode tile. Kept in
 * step with the Rust `Scene::parse` match by hand (D79), like the rest of the shell's pure seams —
 * the **full** token set, so the campaign nodes' tokens (`mission2`/`mission3`) are guarded too.
 */
val KNOWN_SCENE_TOKENS = setOf(
    "default", "demo",
    "skirmish", "match",
    "duel", "infantry",
    "mission1", "seize",
    "mission2", "hold",
    "mission3", "push",
    "map", "inspect", "pointe",
)

/**
 * The standing battles the skirmish battlefield picker offers today. Skirmish is the open fight
 * against the scripted enemy commander; Seize is the take-and-hold objective map (the same
 * battlefield campaign mission 1 is authored on — content is mode-agnostic, D76). The list grows
 * as more scenes land, and becomes the map-library manifest when the D34 listing seam does.
 */
val shellGameModes = listOf(
    GameMode(
        id = "skirmish",
        name = "Skirmish",
        sceneToken = "skirmish",
        blurb = "Open battle against the enemy commander. Grow your camp, then go dark and fight.",
    ),
    GameMode(
        id = "seize",
        name = "Seize Ground",
        sceneToken = "seize",
        blurb = "Take and hold the objective before the enemy assault overruns it.",
    ),
)
