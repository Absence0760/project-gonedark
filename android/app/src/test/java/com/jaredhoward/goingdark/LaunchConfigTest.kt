package com.jaredhoward.goingdark

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for the launch-config seam ([LaunchConfig], Compose shell parity Tier 0). The
 * Compose UI that produces it is device-gated chrome (D32) and exempt, but the encode/decode codec
 * is testable here with no device — so it is tested (CLAUDE.md testing rule). These mirror the Rust
 * `pal-android/src/launch.rs` tests so the wire contract is pinned from both ends (D79).
 */
class LaunchConfigTest {
    @Test
    fun default_is_the_skirmish_desktop_default() {
        val d = LaunchConfig()
        assertEquals("skirmish", d.scene)
        assertEquals(0, d.optic)
        assertEquals(0, d.barrel)
        assertEquals(0, d.magazine)
        assertEquals(80, d.masterPct)
        assertEquals(80, d.sfxPct)
        assertEquals(100, d.sensX100)
        assertFalse(d.invertY)
    }

    @Test
    fun null_empty_or_garbage_decodes_to_default() {
        assertEquals(LaunchConfig(), LaunchConfig.decode(null))
        assertEquals(LaunchConfig(), LaunchConfig.decode(""))
        assertEquals(LaunchConfig(), LaunchConfig.decode("   "))
        assertEquals(LaunchConfig(), LaunchConfig.decode("not a config at all"))
        assertEquals(LaunchConfig(), LaunchConfig.decode(";;;==;"))
    }

    @Test
    fun decodes_a_full_v1_string() {
        val cfg = LaunchConfig.decode("v=1;scene=mission1;opt=1;bar=2;mag=1;vol=50;sfx=70;sens=250;invy=1")
        assertEquals("mission1", cfg.scene)
        assertEquals(1, cfg.optic)
        assertEquals(2, cfg.barrel)
        assertEquals(1, cfg.magazine)
        assertEquals(50, cfg.masterPct)
        assertEquals(70, cfg.sfxPct)
        assertEquals(250, cfg.sensX100)
        assertTrue(cfg.invertY)
    }

    @Test
    fun missing_keys_keep_defaults() {
        val cfg = LaunchConfig.decode("v=1;scene=skirmish")
        assertEquals(LaunchConfig(), cfg)
    }

    @Test
    fun unknown_keys_are_ignored() {
        val cfg = LaunchConfig.decode("scene=mission1;diff=3;newthing=foo;opt=2")
        assertEquals("mission1", cfg.scene)
        assertEquals(2, cfg.optic)
        assertEquals(0, cfg.barrel)
    }

    @Test
    fun out_of_range_numbers_clamp() {
        val cfg = LaunchConfig.decode("opt=9;bar=255;mag=-4;vol=900;sfx=-1;sens=9000")
        assertEquals(LaunchConfig.SLOT_MAX, cfg.optic)
        assertEquals(LaunchConfig.SLOT_MAX, cfg.barrel)
        assertEquals(0, cfg.magazine)
        assertEquals(LaunchConfig.GAIN_PCT_MAX, cfg.masterPct)
        assertEquals(0, cfg.sfxPct)
        assertEquals(LaunchConfig.SENS_MAX, cfg.sensX100)
    }

    @Test
    fun sens_below_min_clamps_up() {
        assertEquals(LaunchConfig.SENS_MIN, LaunchConfig.decode("sens=0").sensX100)
        assertEquals(LaunchConfig.SENS_MIN, LaunchConfig.decode("sens=5").sensX100)
    }

    @Test
    fun unparseable_numbers_keep_default() {
        val cfg = LaunchConfig.decode("opt=abc;vol=lots;sens=fast;invy=maybe")
        assertEquals(0, cfg.optic)
        assertEquals(80, cfg.masterPct)
        assertEquals(100, cfg.sensX100)
        assertFalse(cfg.invertY)
    }

    @Test
    fun bool_forms() {
        assertTrue(LaunchConfig.decode("invy=1").invertY)
        assertTrue(LaunchConfig.decode("invy=true").invertY)
        assertFalse(LaunchConfig.decode("invy=0").invertY)
        assertFalse(LaunchConfig.decode("invy=false").invertY)
    }

    @Test
    fun whitespace_around_pairs_is_tolerated() {
        val cfg = LaunchConfig.decode(" scene = skirmish ; opt = 1 ")
        assertEquals("skirmish", cfg.scene)
        assertEquals(1, cfg.optic)
    }

    @Test
    fun duplicate_keys_last_wins() {
        assertEquals(2, LaunchConfig.decode("opt=1;opt=2").optic)
    }

    @Test
    fun encode_then_decode_round_trips() {
        val cfg = LaunchConfig(
            scene = "mission1", optic = 2, barrel = 1, magazine = 2,
            masterPct = 30, sfxPct = 65, sensX100 = 180, invertY = true,
        )
        assertEquals(cfg, LaunchConfig.decode(cfg.encode()))
    }

    @Test
    fun the_exact_string_mainactivity_emits_decodes_back() {
        // The payload MainActivity.startMatch sends: a default-loadout Skirmish boot.
        val emitted = LaunchConfig(scene = "skirmish").encode()
        assertEquals(LaunchConfig(scene = "skirmish"), LaunchConfig.decode(emitted))
        // And it is the documented v1 shape.
        assertEquals("v=1;scene=skirmish;opt=0;bar=0;mag=0;vol=80;sfx=80;sens=100;invy=0", emitted)
    }
}
