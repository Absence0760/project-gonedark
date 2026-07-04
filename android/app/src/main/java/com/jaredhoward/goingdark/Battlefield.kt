package com.jaredhoward.goingdark

/**
 * The unified **battlefield table** (D102) — everything the skirmish setup's battlefield picker
 * lists: the standing battle scenes (the former `GameMode`/`shellGameModes` table, D81/D101)
 * plus the engine's embedded **map library** (`engine::map_library::MAP_LIBRARY`). The Kotlin
 * mirror of the Rust `BATTLEFIELDS` (D79 hand-mirrored; [BattlefieldTest] pins the shape).
 *
 * A battlefield boots exactly one way: a scene entry carries [sceneToken] (resolved engine-side
 * via `Scene::parse`), a library-map entry carries [mapId] (booted via
 * `Game::new_map_skirmish_with_loadout` off the `map=` wire key). Exactly one of the two is set —
 * pinned by test, so a tile can never be ambiguous or dead.
 *
 * **No Android imports** on purpose: this is the testable seam (the `CampaignModel.kt` / D79
 * pattern) the device-gated [SkirmishSetupScreen] composable renders.
 */
data class Battlefield(
    /** Stable id (also the tap key). */
    val id: String,
    /** Display name, shown on the tile. */
    val name: String,
    /** One-line teaser under the name. */
    val blurb: String,
    /** The engine scene token, for a standing-battle entry (must be in [KNOWN_SCENE_TOKENS]). */
    val sceneToken: String? = null,
    /** The library-map id, for an authored-map entry (must be in [KNOWN_MAP_IDS]). */
    val mapId: String? = null,
)

/**
 * The scene tokens `engine::lib::Scene::parse` accepts — the guard the tests check every scene
 * battlefield (and every campaign node) against so a typo can't ship an un-launchable tile. Kept
 * in step with the Rust `Scene::parse` match by hand (D79) — the **full** token set, so the
 * campaign nodes' tokens (`mission2`/`mission3`) are guarded too.
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
 * The library-map ids `engine::map_library::MAP_LIBRARY` embeds — the [KNOWN_SCENE_TOKENS] twin
 * for map battlefields, kept in step with the Rust table by hand (D79). An id outside this set
 * would ride the wire and degrade to the plain open skirmish engine-side (graceful, but a dead
 * tile) — the test forbids shipping one.
 */
val KNOWN_MAP_IDS = setOf("crossroads", "prokhorovka")

/**
 * Every battlefield the skirmish setup offers, in display order: the standing battle scenes first
 * (the open skirmish stays the first tile — the engine's fallback battlefield), then the library
 * maps. Mirrors the Rust `BATTLEFIELDS` verbatim (names, blurbs, order — D79).
 */
val shellBattlefields = listOf(
    Battlefield(
        id = "skirmish",
        name = "Skirmish",
        blurb = "Open battle against the enemy commander. Grow your camp, then go dark and fight.",
        sceneToken = "skirmish",
    ),
    Battlefield(
        id = "seize",
        name = "Seize Ground",
        blurb = "Take and hold the objective before the enemy assault overruns it.",
        sceneToken = "seize",
    ),
    Battlefield(
        id = "crossroads",
        name = "Crossroads",
        blurb = "Three posts strung across an open junction. The library's first authored map.",
        mapId = "crossroads",
    ),
    Battlefield(
        id = "prokhorovka",
        name = "Prokhorovka",
        blurb = "The 1943 Kursk tank battle: a wide, even steppe. Deploy at opposite ends and cross.",
        mapId = "prokhorovka",
    ),
)
