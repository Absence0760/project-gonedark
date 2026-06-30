package com.jaredhoward.goingdark

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * JVM unit tests for the pure Profile seam (ProfileLogic.kt), the Android mirror of the desktop
 * egui Profile seam in `app/src/shell.rs`. The Compose UI that consumes it is device-gated chrome
 * (D32) and exempt; the validation logic is testable here with no device, so it is tested (CLAUDE.md
 * testing rule). These cases mirror the Rust `sanitize_callsign_*` / `win_rate_pct_*` tests so the
 * two stay in lockstep (D79 mirror discipline).
 */
class ProfileLogicTest {
    // ---- sanitizeCallsign ----------------------------------------------------------------------

    @Test
    fun sanitize_trims_surrounding_whitespace() {
        assertEquals("Reaper", sanitizeCallsign("  Reaper  "))
    }

    @Test
    fun sanitize_blank_falls_back_to_default() {
        assertEquals(DEFAULT_CALLSIGN, sanitizeCallsign("   "))
        assertEquals(DEFAULT_CALLSIGN, sanitizeCallsign(""))
    }

    @Test
    fun sanitize_truncates_to_max_chars() {
        val long = "X".repeat(CALLSIGN_MAX + 10)
        assertEquals(CALLSIGN_MAX, sanitizeCallsign(long).length)
    }

    @Test
    fun sanitize_keeps_a_name_at_the_limit_intact() {
        val exact = "Y".repeat(CALLSIGN_MAX)
        assertEquals(exact, sanitizeCallsign(exact))
    }

    @Test
    fun sanitize_truncates_multibyte_names_without_splitting() {
        // Accented chars: char-based truncation must yield CALLSIGN_MAX whole characters, none split.
        val name = "é".repeat(CALLSIGN_MAX + 5)
        val out = sanitizeCallsign(name)
        assertEquals(CALLSIGN_MAX, out.length)
        assertEquals("é".repeat(CALLSIGN_MAX), out)
    }

    // ---- winRatePct ----------------------------------------------------------------------------

    @Test
    fun win_rate_is_null_with_no_matches() {
        assertNull(winRatePct(0, 0))
    }

    @Test
    fun win_rate_exact_and_floor_cases() {
        assertEquals(0, winRatePct(0, 4))
        assertEquals(50, winRatePct(2, 4))
        assertEquals(100, winRatePct(4, 4))
        // 1/3 = 33.33% → floors to 33.
        assertEquals(33, winRatePct(1, 3))
    }

    @Test
    fun win_rate_large_counts_do_not_overflow() {
        assertEquals(50, winRatePct(1_000_000, 2_000_000))
        // Near Int.MAX: wins * 100 would overflow Int; Long math keeps it correct.
        assertEquals(50, winRatePct(1_000_000_000, 2_000_000_000))
    }

    // ---- FactionPref -------------------------------------------------------------------------

    @Test
    fun faction_cycles_and_wraps() {
        assertEquals(FactionPref.FrenchArmy, FactionPref.UsArmy.next())
        assertEquals(FactionPref.UsArmy, FactionPref.FrenchArmy.next())
    }
}
