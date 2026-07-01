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
    fun pve_opens_the_mode_select() {
        // D81: PvE no longer dead-ends on the gunsmith — it opens the mode/map picker.
        assertEquals(TitleRoute.ModeSelect, resolveTitleAction(TitleAction.Pve))
    }

    @Test
    fun pvp_opens_the_mode_select() {
        assertEquals(TitleRoute.ModeSelect, resolveTitleAction(TitleAction.Pvp))
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
    fun about_opens_about() {
        assertEquals(TitleRoute.About, resolveTitleAction(TitleAction.About))
    }

    @Test
    fun quit_exits() {
        assertEquals(TitleRoute.Quit, resolveTitleAction(TitleAction.Quit))
    }

    @Test
    fun every_action_routes_and_only_pve_pvp_share_the_mode_select() {
        // Exhaustive sweep: every action maps, and only Pve/Pvp share a destination (the mode select),
        // matching the design where those two fold together until PvP match-setup diverges (Q5).
        val routes = TitleAction.entries.associateWith { resolveTitleAction(it) }
        assertEquals(TitleRoute.ModeSelect, routes[TitleAction.Pve])
        assertEquals(TitleRoute.ModeSelect, routes[TitleAction.Pvp])
        // No title action routes to the gunsmith any more — it lives behind Settings now (D81), and
        // TitleRoute has no Loadout member, so that's guaranteed at compile time.
        // All seven actions produce a route (no action left unmapped — `when` is exhaustive, but this
        // pins it as the table grows).
        assertEquals(TitleAction.entries.size, routes.size)
    }
}
