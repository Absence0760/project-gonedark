package com.jaredhoward.goingdark

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for the pure [SettingsState] seam (SettingsState.kt) — the Compose twin of the desktop
 * `SettingsState` in `app/src/shell.rs`. The composable that renders it ([SettingsScreen]) is device-
 * gated chrome (D32) and exempt, but the bounds/clamp/defaults/quality-cycle logic is testable here with
 * no device — so it is tested (CLAUDE.md testing rule), mirroring `BuildStampTest`.
 */
class SettingsStateTest {
    @Test
    fun defaults_match_the_desktop_settings_state() {
        val d = SettingsState.defaults()
        assertEquals(80, d.masterPct)
        assertEquals(80, d.sfxPct)
        assertEquals(60, d.musicPct)
        assertEquals(100, d.sensX100)
        assertFalse(d.invertLookY)
        assertEquals(Quality.Auto, d.quality)
    }

    @Test
    fun defaults_is_the_canonical_no_arg_constructor() {
        assertEquals(SettingsState(), SettingsState.defaults())
    }

    @Test
    fun clamp_pins_each_gain_above_max_down_to_max() {
        val clamped = SettingsState(masterPct = 500, sfxPct = 200, musicPct = 101).clamp()
        assertEquals(SettingsState.GAIN_PCT_MAX, clamped.masterPct)
        assertEquals(SettingsState.GAIN_PCT_MAX, clamped.sfxPct)
        assertEquals(SettingsState.GAIN_PCT_MAX, clamped.musicPct)
    }

    @Test
    fun clamp_pins_each_gain_below_min_up_to_zero() {
        val clamped = SettingsState(masterPct = -5, sfxPct = -1, musicPct = -100).clamp()
        assertEquals(0, clamped.masterPct)
        assertEquals(0, clamped.sfxPct)
        assertEquals(0, clamped.musicPct)
    }

    @Test
    fun clamp_pins_sensitivity_over_and_under_range() {
        assertEquals(SettingsState.SENS_MAX, SettingsState(sensX100 = 9999).clamp().sensX100)
        assertEquals(SettingsState.SENS_MIN, SettingsState(sensX100 = 0).clamp().sensX100)
    }

    @Test
    fun clamp_leaves_in_range_values_untouched() {
        val s = SettingsState(masterPct = 42, sfxPct = 7, musicPct = 99, sensX100 = 150)
        assertEquals(s, s.clamp())
    }

    @Test
    fun clamp_preserves_non_numeric_fields() {
        val clamped = SettingsState(invertLookY = true, quality = Quality.High, masterPct = 999).clamp()
        assertTrue(clamped.invertLookY)
        assertEquals(Quality.High, clamped.quality)
    }

    @Test
    fun reset_restores_defaults_from_any_edited_state() {
        val edited = SettingsState(
            masterPct = 0,
            sfxPct = 0,
            musicPct = 0,
            sensX100 = SettingsState.SENS_MAX,
            invertLookY = true,
            quality = Quality.Low,
        )
        assertEquals(SettingsState.defaults(), edited.reset())
    }

    @Test
    fun quality_cycles_through_all_choices_and_wraps() {
        assertEquals(Quality.Low, Quality.Auto.next())
        assertEquals(Quality.Medium, Quality.Low.next())
        assertEquals(Quality.High, Quality.Medium.next())
        assertEquals(Quality.Auto, Quality.High.next())
    }

    @Test
    fun quality_labels_are_stable() {
        assertEquals("Auto", Quality.Auto.label())
        assertEquals("Low", Quality.Low.label())
        assertEquals("Medium", Quality.Medium.label())
        assertEquals("High", Quality.High.label())
    }

    @Test
    fun bounds_mirror_the_launch_config_wire_keys() {
        // D79 mirrored-constants discipline: the Settings bounds and the LaunchConfig wire-key bounds
        // are the same numbers, so a SettingsState round-trips into the launch intent losslessly.
        assertEquals(LaunchConfig.GAIN_PCT_MAX, SettingsState.GAIN_PCT_MAX)
        assertEquals(LaunchConfig.SENS_MIN, SettingsState.SENS_MIN)
        assertEquals(LaunchConfig.SENS_MAX, SettingsState.SENS_MAX)
    }
}
