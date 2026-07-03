package com.jaredhoward.goingdark

/**
 * The title screen's pure decision seam — the Kotlin mirror of the desktop egui shell's
 * `resolve_title_action` (`app/src/shell.rs`, `enum TitleAction` / `enum HostTransition`).
 *
 * Per **D79**, the shell's pure decision/validation logic is **re-implemented in plain Kotlin and
 * unit-tested on the JVM** (the `BuildStamp.kt` pattern) rather than single-sourced in `core::shell`
 * and called over JNI: routing a button press is presentation chrome (D32), not game logic — it
 * folds into no checksum and carries no determinism obligation, so dragging JNI onto the hot Compose
 * path to single-source it would be disproportionate. It does mean this routing table must be kept in
 * step with the Rust `resolve_title_action` by hand; both are tiny and covered by tests on each side.
 *
 * **No Android imports** live here on purpose: this is the testable seam the surrounding Compose UI
 * (which *is* device-gated, un-unit-testable glue) is exempt from. Any real logic still gets a test
 * (CLAUDE.md testing rule), so the one bit of routing logic lives here, away from the composables —
 * see `TitleActionTest.kt`.
 */

/**
 * A top-level action the player can pick on the title screen — the Kotlin mirror of the Rust
 * `app::shell::TitleAction`. Each play mode opens its own front door (`modes.md` §1): `Campaign`
 * the Operations hub, `Pve` the skirmish match-setup, `Pvp` the staging screen (D101). The
 * loadout **gunsmith is no longer a play gate** — it lives behind Settings now (D81) — so no title
 * action routes to it (see [resolveTitleAction]).
 *
 * `About` (the FIELD MANUAL button) has no Rust `TitleAction` counterpart — on desktop the About
 * screen is reached *from* Settings — but the Compose title surfaces it directly, so it gets its own
 * action here.
 */
enum class TitleAction {
    /** The PvE story campaign — the first shippable pillar (`docs/pve-campaign.md`, D58). */
    Campaign,

    /** A standalone PvE skirmish against the scripted enemy commander. */
    Pve,

    /** Player-vs-player — the lockstep-netcode match (queues are Phase-3-blocked; D101 staging). */
    Pvp,

    /** Open settings (audio / video / controls preferences). */
    Settings,

    /** Open the player profile / progression surface. */
    Profile,

    /** Open the army-select screen (US vs French) — the pre-deploy roster pick (mirrors desktop). */
    Army,

    /** Open the About / field-manual (controls-reference) screen. */
    About,

    /** Quit the app. */
    Quit,
}

/**
 * Where a [TitleAction] routes the Compose host — the Kotlin counterpart of the Rust
 * `HostTransition` (narrowed to just the title-reachable destinations). The host's nav graph
 * switches on this exactly as the desktop run loop switches on `HostTransition`.
 */
enum class TitleRoute {
    /** The Operations-hub mission-select screen — the PvE campaign entry (mirrors `OpenMissionSelect`). */
    MissionSelect,

    /** The skirmish match-setup screen (SKIRMISH's door — `modes.md` §3: battlefield, both armies, tier). */
    SkirmishSetup,

    /** The PvP staging screen (D101) — queues in build order, nothing joinable pre-net. */
    Pvp,

    /** The Settings screen (mirrors `OpenSettings`). */
    Settings,

    /** The player Profile screen (mirrors `OpenProfile`). */
    Profile,

    /** The army-select screen (mirrors the desktop `OpenArmySelect`). */
    ArmySelect,

    /** The About / field-manual screen (mirrors `OpenAbout`). */
    About,

    /** Tear down and exit the app (mirrors `Exit`). */
    Quit,
}

/**
 * Map a title action to the route it triggers — the pure nav decision (D81/D101, `modes.md` §1:
 * three distinct front doors, none sharing another's surface):
 *
 *  - `Campaign` opens the Operations-hub mission-select (the PvE pillar, D58);
 *  - `Pve` opens the skirmish match-setup (`modes.md` §3: battlefield, both armies, opponent tier),
 *    which deploys the configured match — the gunsmith no longer gates play (behind Settings, D81);
 *  - `Pvp` opens the staging screen (D101) — honest pre-net chrome, nothing joinable;
 *  - `Settings` / `Profile` / `About` open their like-named screens;
 *  - `Quit` exits.
 *
 * Kept pure (no Android types) so it is unit-testable on the plain JVM — see `TitleActionTest.kt`.
 * The live Compose router ([Shell] in `MainActivity.kt`) consumes THIS function, so the tests below
 * cover the navigation the app actually runs.
 */
fun resolveTitleAction(action: TitleAction): TitleRoute =
    when (action) {
        TitleAction.Campaign -> TitleRoute.MissionSelect
        TitleAction.Pve -> TitleRoute.SkirmishSetup
        TitleAction.Pvp -> TitleRoute.Pvp
        TitleAction.Settings -> TitleRoute.Settings
        TitleAction.Profile -> TitleRoute.Profile
        TitleAction.Army -> TitleRoute.ArmySelect
        TitleAction.About -> TitleRoute.About
        TitleAction.Quit -> TitleRoute.Quit
    }
