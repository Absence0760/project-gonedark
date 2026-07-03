package com.jaredhoward.goingdark

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * JVM unit tests for the title screen's pure routing seam (TitleAction.kt). The Compose UI that
 * consumes it is device-gated chrome (D32) and exempt, but the routing logic is testable here with no
 * device — so it is tested (CLAUDE.md testing rule, D79). These assert the same mapping the desktop
 * Rust `resolve_title_action` (`app/src/shell.rs`) implements, so a drift between the two is caught.
 */
class TitleActionTest {
    @Test
    fun campaign_opens_mission_select() {
        assertEquals(TitleRoute.MissionSelect, resolveTitleAction(TitleAction.Campaign))
    }

    @Test
    fun pve_opens_the_skirmish_match_setup() {
        // The SKIRMISH door opens the full `modes.md` §3 setup (battlefield, both armies, tier) —
        // the Compose twin of the desktop `TitleAction::Pve -> OpenSkirmishSetup` (parity §12
        // item 6); the gunsmith stays customization-only behind Settings (D81).
        assertEquals(TitleRoute.SkirmishSetup, resolveTitleAction(TitleAction.Pve))
    }

    @Test
    fun pvp_opens_its_own_staging_door() {
        // D101: PvP no longer shares skirmish's picker — it opens the staging screen (queues in
        // modes.md §5 build order, nothing joinable pre-net). Mirrors the desktop
        // `TitleAction::Pvp -> HostTransition::OpenPvp`.
        assertEquals(TitleRoute.Pvp, resolveTitleAction(TitleAction.Pvp))
    }

    @Test
    fun settings_opens_settings() {
        assertEquals(TitleRoute.Settings, resolveTitleAction(TitleAction.Settings))
    }

    @Test
    fun profile_opens_profile() {
        assertEquals(TitleRoute.Profile, resolveTitleAction(TitleAction.Profile))
    }

    @Test
    fun army_opens_the_army_select() {
        // Mirrors the desktop `TitleAction::Army -> HostTransition::OpenArmySelect`.
        assertEquals(TitleRoute.ArmySelect, resolveTitleAction(TitleAction.Army))
    }

    @Test
    fun about_opens_about() {
        assertEquals(TitleRoute.About, resolveTitleAction(TitleAction.About))
    }

    @Test
    fun quit_exits() {
        assertEquals(TitleRoute.Quit, resolveTitleAction(TitleAction.Quit))
    }

    @Test
    fun every_action_routes_and_no_two_play_modes_share_a_door() {
        // Exhaustive sweep: every action maps, and the three play modes each own a distinct
        // destination (`modes.md` §1, D101) — the old Pve/Pvp shared picker is retired.
        val routes = TitleAction.entries.associateWith { resolveTitleAction(it) }
        val playDoors = listOf(TitleAction.Campaign, TitleAction.Pve, TitleAction.Pvp).map { routes[it] }
        assertEquals(playDoors.size, playDoors.toSet().size)
        // No title action routes to the gunsmith any more — it lives behind Settings now (D81), and
        // TitleRoute has no Loadout member, so that's guaranteed at compile time.
        // Every action produces a route (no action left unmapped — `when` is exhaustive, but this
        // pins it as the table grows).
        assertEquals(TitleAction.entries.size, routes.size)
    }
}
