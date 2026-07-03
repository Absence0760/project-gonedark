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
        assertEquals(0, d.stock) // D85 slots default to Standard
        assertEquals(0, d.muzzle)
        assertEquals(80, d.masterPct)
        assertEquals(80, d.sfxPct)
        assertEquals(100, d.sensX100)
        assertFalse(d.invertY)
        assertEquals(0, d.diff) // Recruit — the neutral campaign tier
        assertEquals(0, d.node) // the root campaign node
        assertEquals(LaunchConfig.ARMY_DEFAULT, d.army) // US Army — Neutral is never a player pick
        assertFalse(d.colorblindCues) // accessibility cues opt-in, default OFF
        assertFalse(d.visualSoundCues)
        assertEquals(LaunchConfig.ENEMY_ARMY_UNSET, d.enemyArmy) // no explicit enemy pick
        assertFalse(d.skirmish) // not a configured-skirmish launch
        assertEquals("", d.map) // no library map — the scene battlefield boots
    }

    @Test
    fun the_map_key_round_trips_and_missing_or_empty_stays_unset() {
        // The D102 library-map key (mirrors launch.rs): the id survives the codec…
        val cfg = LaunchConfig(scene = "skirmish", skirmish = true, map = "crossroads")
        assertEquals("crossroads", LaunchConfig.decode(cfg.encode()).map)
        // …and a missing/empty key keeps the default (no library map).
        assertEquals("", LaunchConfig.decode("map=").map)
        assertEquals("", LaunchConfig.decode("v=1;scene=skirmish").map)
    }

    @Test
    fun enemy_army_and_skirmish_flag_round_trip_and_degrade_to_unset() {
        // The configured-skirmish keys (mirrors launch.rs): explicit picks round-trip…
        val cfg = LaunchConfig(scene = "seize", diff = 3, army = 2, enemyArmy = 1, skirmish = true)
        val decoded = LaunchConfig.decode(cfg.encode())
        assertEquals(1, decoded.enemyArmy)
        assertTrue(decoded.skirmish)
        // …a bad enemy ordinal means "no explicit pick" (the scenario default stands — unlike the
        // player key's forced-US fallback), and an old wire without the keys behaves as before.
        assertEquals(LaunchConfig.ENEMY_ARMY_UNSET, LaunchConfig.decode("earmy=0").enemyArmy)
        assertEquals(LaunchConfig.ENEMY_ARMY_UNSET, LaunchConfig.decode("earmy=7").enemyArmy)
        assertEquals(2, LaunchConfig.decode("earmy=2").enemyArmy)
        val old = LaunchConfig.decode("v=1;scene=skirmish;army=1")
        assertEquals(LaunchConfig.ENEMY_ARMY_UNSET, old.enemyArmy)
        assertFalse(old.skirmish)
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
        val cfg = LaunchConfig.decode(
            "v=1;scene=mission1;opt=1;bar=2;mag=1;stk=1;muz=2;vol=50;sfx=70;sens=250;invy=1;diff=2;node=3;army=2;cvd=1;snd=1",
        )
        assertEquals("mission1", cfg.scene)
        assertEquals(1, cfg.optic)
        assertEquals(2, cfg.barrel)
        assertEquals(1, cfg.magazine)
        assertEquals(1, cfg.stock) // Agile
        assertEquals(2, cfg.muzzle) // Suppressor
        assertEquals(50, cfg.masterPct)
        assertEquals(70, cfg.sfxPct)
        assertEquals(250, cfg.sensX100)
        assertTrue(cfg.invertY)
        assertEquals(2, cfg.diff) // Veteran
        assertEquals(3, cfg.node)
        assertEquals(2, cfg.army) // French Army
        assertTrue(cfg.colorblindCues)
        assertTrue(cfg.visualSoundCues)
    }

    @Test
    fun node_round_trips_and_missing_or_garbage_defaults_to_root() {
        assertEquals(0, LaunchConfig.decode("node=0").node)
        assertEquals(5, LaunchConfig.decode("node=5").node)
        // Missing → root (0); negative / garbage keep the default (0).
        assertEquals(0, LaunchConfig.decode("v=1;scene=mission1").node)
        assertEquals(0, LaunchConfig.decode("node=-1").node)
        assertEquals(0, LaunchConfig.decode("node=root").node)
    }

    @Test
    fun army_round_trips_and_collapses_neutral_or_out_of_range_to_us() {
        assertEquals(1, LaunchConfig.decode("army=1").army) // US
        assertEquals(2, LaunchConfig.decode("army=2").army) // French
        // Neutral (0) is never a player pick → US default (mirrors desktop decode_army).
        assertEquals(LaunchConfig.ARMY_DEFAULT, LaunchConfig.decode("army=0").army)
        // Out-of-range does NOT clamp to French — it degrades to the US default.
        assertEquals(LaunchConfig.ARMY_DEFAULT, LaunchConfig.decode("army=9").army)
        assertEquals(LaunchConfig.ARMY_DEFAULT, LaunchConfig.decode("army=-1").army)
        // Garbage / missing → the US default.
        assertEquals(LaunchConfig.ARMY_DEFAULT, LaunchConfig.decode("army=fr").army)
        assertEquals(LaunchConfig.ARMY_DEFAULT, LaunchConfig.decode("v=1;scene=mission1").army)
    }

    @Test
    fun accessibility_cues_round_trip_and_default_off() {
        assertTrue(LaunchConfig.decode("cvd=1").colorblindCues)
        assertTrue(LaunchConfig.decode("cvd=true").colorblindCues)
        assertFalse(LaunchConfig.decode("cvd=0").colorblindCues)
        assertTrue(LaunchConfig.decode("snd=1").visualSoundCues)
        assertFalse(LaunchConfig.decode("snd=false").visualSoundCues)
        // Missing → both OFF; garbage keeps the default (OFF).
        val d = LaunchConfig.decode("v=1;scene=skirmish")
        assertFalse(d.colorblindCues)
        assertFalse(d.visualSoundCues)
        val g = LaunchConfig.decode("cvd=maybe;snd=")
        assertFalse(g.colorblindCues)
        assertFalse(g.visualSoundCues)
    }

    @Test
    fun stock_and_muzzle_round_trip_and_a_pre_d85_wire_defaults_to_standard() {
        for (i in 0..LaunchConfig.SLOT_MAX) {
            assertEquals(i, LaunchConfig.decode("stk=$i").stock)
            assertEquals(i, LaunchConfig.decode("muz=$i").muzzle)
        }
        // Back-compat: a pre-D85 emitter (opt/bar/mag only) still decodes, both slots → Standard.
        val old = LaunchConfig.decode("v=1;scene=skirmish;opt=1;bar=2;mag=1")
        assertEquals(0, old.stock)
        assertEquals(0, old.muzzle)
        // Out-of-range / negative / garbage degrade exactly like the other slot keys.
        assertEquals(LaunchConfig.SLOT_MAX, LaunchConfig.decode("stk=9").stock)
        assertEquals(0, LaunchConfig.decode("muz=-1").muzzle)
        assertEquals(0, LaunchConfig.decode("stk=agile").stock)
    }

    @Test
    fun missing_diff_defaults_to_recruit() {
        // Back-compat: a pre-C3 emitter (no `diff` key) still decodes, campaign tier → Recruit (0).
        val cfg = LaunchConfig.decode("v=1;scene=mission1;opt=1;vol=50")
        assertEquals("mission1", cfg.scene)
        assertEquals(0, cfg.diff)
    }

    @Test
    fun diff_round_trips_every_tier_and_clamps_out_of_range() {
        for (tier in 0..LaunchConfig.DIFF_MAX) {
            assertEquals(tier, LaunchConfig.decode("diff=$tier").diff)
        }
        assertEquals(LaunchConfig.DIFF_MAX, LaunchConfig.decode("diff=9").diff)
        assertEquals(0, LaunchConfig.decode("diff=-1").diff)
        assertEquals(0, LaunchConfig.decode("diff=elite").diff)
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
            scene = "mission1", optic = 2, barrel = 1, magazine = 2, stock = 1, muzzle = 2,
            masterPct = 30, sfxPct = 65, sensX100 = 180, invertY = true, diff = 3,
            node = 4, army = 2, colorblindCues = true, visualSoundCues = true,
        )
        assertEquals(cfg, LaunchConfig.decode(cfg.encode()))
    }

    @Test
    fun the_exact_string_mainactivity_emits_decodes_back() {
        // The payload MainActivity.startMatch sends: a default-loadout Skirmish boot.
        val emitted = LaunchConfig(scene = "skirmish").encode()
        assertEquals(LaunchConfig(scene = "skirmish"), LaunchConfig.decode(emitted))
        // And it is the documented v1 shape (now carrying the D85 `stk`/`muz` slots, the campaign
        // `diff`/`node`, the `army` pick, the accessibility `cvd`/`snd` cues, the skirmish
        // `earmy`/`skirm` pair, and the D102 `map` id — all at defaults).
        assertEquals(
            "v=1;scene=skirmish;opt=0;bar=0;mag=0;stk=0;muz=0;vol=80;sfx=80;sens=100;invy=0;diff=0;node=0;army=1;cvd=0;snd=0;earmy=0;skirm=0;map=",
            emitted,
        )
    }
}
