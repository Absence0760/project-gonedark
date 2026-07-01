package com.jaredhoward.goingdark

/**
 * The pure model behind the Pve/Pvp **mode / map select** (D81) — the lightweight picker the player
 * lands on after tapping PvE or PvP, replacing the old "funnel every play mode through the gunsmith"
 * flow. Each entry names a launchable battle and carries the engine [sceneToken] `Scene::parse`
 * resolves (`engine::lib::Scene::parse`): picking one deploys straight into that scene with the
 * player's persisted loadout — no gunsmith gate.
 *
 * **No Android imports** on purpose: this is the testable seam (the `CampaignModel.kt` / D79 pattern)
 * that the device-gated [ModeSelectScreen] composable renders. The one bit of real logic here — that
 * every mode's [sceneToken] is one the engine actually understands — is pinned in `GameModeTest.kt`.
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
 * step with the Rust `Scene::parse` match by hand (D79), like the rest of the shell's pure seams.
 */
val KNOWN_SCENE_TOKENS = setOf("skirmish", "match", "infantry", "mission1", "seize")

/**
 * The modes offered on the Pve/Pvp picker today: the two standing battle scenes. Skirmish is the open
 * fight against the scripted enemy commander; Seize is the take-and-hold objective map. The list grows
 * as more scenes land (and splits per-mode once PvP match-setup exists — Q5).
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
