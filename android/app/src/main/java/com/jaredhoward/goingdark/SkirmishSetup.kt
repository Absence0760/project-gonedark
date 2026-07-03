package com.jaredhoward.goingdark

/**
 * The **skirmish match-setup** model (`docs/modes.md` §3) — the pure seam behind
 * [SkirmishSetupScreen], the Kotlin mirror of the desktop `app/src/shell/skirmish.rs`
 * (`SkirmishSetupState` / `next_army` / `clamp_battlefield` / `resolve_skirmish_config`). This is
 * the `modes.md` §5 build-order step 1 remainder — the full setup surface behind the SKIRMISH door:
 * **battlefield** (the launchable standing battles, [shellGameModes]), **both armies** (US/FR for
 * the player *and* the enemy commander), and the **opponent tier** (the D83 [Difficulty], both
 * combat axes). Everything here is host-side match-setup config, never sim state: the picks reach
 * the sim only through the launch wire's pre-tick seams (invariants #1/#7).
 *
 * Immutable + **no Android imports** (the [CampaignProgress] value-semantics pattern), so every
 * decision is JVM-tested in [SkirmishSetupTest] — the Compose chrome is the exempt glue. D79
 * mirrored semantics: keep in lock-step with the Rust seam by hand.
 */
data class SkirmishSetup(
    /**
     * The picked battlefield as an index into [shellGameModes] (the launchable standing battles).
     * Kept in range by [clampBattlefield]; resolved to a scene token at Deploy.
     */
    val battlefield: Int = 0,
    /**
     * The army the player fields this match. Seeded from the persisted army-select pick on screen
     * open ([reseedPlayerArmy]); cycling it here is a per-match override, never a write-back to
     * the identity pick.
     */
    val playerArmy: Army = Army.Us,
    /** The army the enemy commander fields (`modes.md` §3 step 2: "Pick the enemy's army too"). */
    val enemyArmy: Army = Army.Fr,
    /**
     * The opponent tier — the D83 campaign [Difficulty] whose tuning carries both axes (the 3-tier
     * honest-commander band + the scenario situation modifiers). Difficulty reshapes the
     * *situation*, never the balance numbers (D30/D83).
     */
    val difficulty: Difficulty = Difficulty.Regular,
) {
    /**
     * Re-seed the player side from the persisted identity pick (the army-select screen's state) —
     * applied by the host whenever the screen opens, so the setup always starts from the army the
     * player has declared they field. If that collides with the current enemy pick, the enemy is
     * bumped to the opposing roster so the default reads as a real two-army fight (a mirror match
     * stays one tap away, never the accidental default). Pure — returns a new instance.
     */
    fun reseedPlayerArmy(persisted: Army): SkirmishSetup =
        copy(
            playerArmy = persisted,
            enemyArmy = if (enemyArmy == persisted) nextArmy(persisted) else enemyArmy,
        )
}

/**
 * The next selectable army, wrapping through [Army.SELECTABLE] (`US -> FR -> US`). A non-selectable
 * input (the non-aligned [Army.Neutral], never a player pick) lands on the first selectable roster
 * rather than guessing. Pure — the army cyclers' one decision, mirroring the desktop `next_army`.
 */
fun nextArmy(a: Army): Army {
    val i = Army.SELECTABLE.indexOf(a)
    return if (i >= 0) Army.SELECTABLE[(i + 1) % Army.SELECTABLE.size] else Army.SELECTABLE.first()
}

/**
 * Clamp a battlefield index into [shellGameModes] range (an out-of-range pick — impossible from
 * the tiles, defensive against a stale/foreign value — snaps to the first battlefield, never
 * throws). Pure — mirrors the desktop `clamp_battlefield`.
 */
fun clampBattlefield(i: Int): Int = if (i in shellGameModes.indices) i else 0

/**
 * Resolve the current setup into the [LaunchConfig] a DEPLOY boots — the Kotlin twin of the
 * desktop `resolve_skirmish_config` + `LaunchSkirmish` fielding. The battlefield's scene token
 * rides `scene=`, both army picks ride `army=`/`earmy=`, the opponent tier rides `diff=`, and
 * `skirm=1` marks the launch as the no-stakes sandbox — the engine applies the tuning + enemy
 * army but **never** records a campaign clear ([LaunchConfig.skirmish]). `node` stays 0 (inert —
 * this is not a campaign launch).
 */
fun skirmishLaunchConfig(
    setup: SkirmishSetup,
    settings: SettingsState,
    loadout: LoadoutSelection,
): LaunchConfig =
    launchConfigOf(
        scene = shellGameModes[clampBattlefield(setup.battlefield)].sceneToken,
        settings = settings,
        loadout = loadout,
        army = setup.playerArmy,
        diff = setup.difficulty.tier(),
        node = 0,
    ).copy(
        enemyArmy = setup.enemyArmy.index,
        skirmish = true,
    )
