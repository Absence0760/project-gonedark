package com.jaredhoward.goingdark

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for the pure shell-persistence codec ([ShellPrefsCodec]). The Android glue
 * ([ShellPrefs]) that reads/writes SharedPreferences is device-gated, logic-free plumbing and exempt
 * (the `BuildStamp.kt` / D32 pattern); all the real encode/decode/tolerance rules are exercised here
 * with no device (CLAUDE.md testing rule).
 */
class ShellPrefsCodecTest {

    @Test
    fun defaults_round_trip() {
        val state = ShellState.defaults()
        val decoded = ShellPrefsCodec.decode(ShellPrefsCodec.encode(state))
        assertEquals(state, decoded)
    }

    @Test
    fun fully_customized_state_round_trips() {
        val state = ShellState(
            settings = SettingsState(
                masterPct = 35,
                sfxPct = 0,
                musicPct = 100,
                sensX100 = 250,
                invertLookY = true,
                quality = Quality.High,
                colorblindCues = true,
                visualSoundCues = true,
            ),
            profile = ProfileState(
                callsign = "Reaper-7",
                faction = FactionPref.FrenchArmy,
                matchesPlayed = 42,
                wins = 17,
            ),
            loadout = LoadoutSelection(optic = 2, barrel = 1, magazine = 2),
            army = Army.Fr,
        )
        val decoded = ShellPrefsCodec.decode(ShellPrefsCodec.encode(state))
        assertEquals(state, decoded)
        // Sanity: this isn't accidentally equal to defaults.
        assertNotEquals(ShellState.defaults(), decoded)
    }

    @Test
    fun customized_callsign_survives_the_round_trip() {
        val state = ShellState.defaults().copy(
            profile = ProfileState(callsign = "Ghost", faction = FactionPref.FrenchArmy),
        )
        val decoded = ShellPrefsCodec.decode(ShellPrefsCodec.encode(state))
        assertEquals("Ghost", decoded.profile.callsign)
        assertEquals(FactionPref.FrenchArmy, decoded.profile.faction)
    }

    @Test
    fun customized_loadout_survives_the_round_trip() {
        val state = ShellState.defaults().copy(
            loadout = LoadoutSelection(optic = 1, barrel = 2, magazine = 1),
        )
        val decoded = ShellPrefsCodec.decode(ShellPrefsCodec.encode(state))
        assertEquals(LoadoutSelection(optic = 1, barrel = 2, magazine = 1), decoded.loadout)
    }

    @Test
    fun customized_settings_survive_the_round_trip() {
        val settings = SettingsState(
            masterPct = 10,
            sfxPct = 90,
            musicPct = 5,
            sensX100 = 33,
            invertLookY = true,
            quality = Quality.Low,
        )
        val state = ShellState.defaults().copy(settings = settings)
        val decoded = ShellPrefsCodec.decode(ShellPrefsCodec.encode(state))
        assertEquals(settings, decoded.settings)
    }

    @Test
    fun empty_map_decodes_to_defaults() {
        assertEquals(ShellState.defaults(), ShellPrefsCodec.decode(emptyMap()))
    }

    @Test
    fun army_pick_survives_the_round_trip() {
        for (army in Army.SELECTABLE) {
            val state = ShellState.defaults().copy(army = army)
            val decoded = ShellPrefsCodec.decode(ShellPrefsCodec.encode(state))
            assertEquals(army, decoded.army)
        }
        // The army is written under its stable Army.index ordinal.
        val encoded = ShellPrefsCodec.encode(ShellState.defaults().copy(army = Army.Fr))
        assertEquals(Army.Fr.index.toString(), encoded[ShellPrefsCodec.KEY_ARMY])
    }

    @Test
    fun army_neutral_or_garbage_decodes_to_us_default() {
        // Neutral (0), out-of-range, and unparseable all collapse to the US default (never Neutral).
        assertEquals(Army.Us, ShellPrefsCodec.decode(mapOf(ShellPrefsCodec.KEY_ARMY to "0")).army)
        assertEquals(Army.Us, ShellPrefsCodec.decode(mapOf(ShellPrefsCodec.KEY_ARMY to "9")).army)
        assertEquals(Army.Us, ShellPrefsCodec.decode(mapOf(ShellPrefsCodec.KEY_ARMY to "fr")).army)
        // Missing key → the US default.
        assertEquals(Army.Us, ShellPrefsCodec.decode(emptyMap()).army)
    }

    @Test
    fun accessibility_cues_survive_the_round_trip() {
        val settings = SettingsState.defaults().copy(colorblindCues = true, visualSoundCues = true)
        val decoded = ShellPrefsCodec.decode(
            ShellPrefsCodec.encode(ShellState.defaults().copy(settings = settings)),
        )
        assertTrue(decoded.settings.colorblindCues)
        assertTrue(decoded.settings.visualSoundCues)
        // Missing keys → both default OFF (tolerant).
        val bare = ShellPrefsCodec.decode(emptyMap())
        assertFalse(bare.settings.colorblindCues)
        assertFalse(bare.settings.visualSoundCues)
    }

    @Test
    fun cleared_campaign_progress_survives_the_round_trip() {
        // Record the shipped root node cleared at Veteran, then round-trip the whole shell state.
        val campaign = CampaignProgress().recordClear(0, Difficulty.Veteran)
        val state = ShellState.defaults().copy(campaign = campaign)
        val decoded = ShellPrefsCodec.decode(ShellPrefsCodec.encode(state))
        assertEquals(campaign, decoded.campaign)
        assertEquals(Difficulty.Veteran, decoded.campaign.bestCleared(0))
    }

    @Test
    fun campaign_key_is_written_and_defaults_empty() {
        // A fresh (uncleared) campaign encodes its key as an empty string; decode restores no clears.
        val encoded = ShellPrefsCodec.encode(ShellState.defaults())
        assertEquals("", encoded[ShellPrefsCodec.KEY_CAMPAIGN])
        val decoded = ShellPrefsCodec.decode(encoded)
        assertEquals(emptyMap<Int, Difficulty>(), decoded.campaign.clearedByNode)
    }

    @Test
    fun garbage_campaign_blob_falls_back_to_no_clears() {
        val map = mapOf(ShellPrefsCodec.KEY_CAMPAIGN to "totally-bogus;;99:99")
        val decoded = ShellPrefsCodec.decode(map)
        assertEquals(emptyMap<Int, Difficulty>(), decoded.campaign.clearedByNode)
    }

    @Test
    fun garbage_int_values_fall_back_to_field_defaults() {
        val ds = SettingsState.defaults()
        val dl = LoadoutSelection()
        val map = mapOf(
            ShellPrefsCodec.KEY_MASTER to "not-a-number",
            ShellPrefsCodec.KEY_SENS to "",
            ShellPrefsCodec.KEY_OPTIC to "abc",
            ShellPrefsCodec.KEY_MATCHES to "twelve",
        )
        val decoded = ShellPrefsCodec.decode(map)
        assertEquals(ds.masterPct, decoded.settings.masterPct)
        assertEquals(ds.sensX100, decoded.settings.sensX100)
        assertEquals(dl.optic, decoded.loadout.optic)
        assertEquals(0, decoded.profile.matchesPlayed)
    }

    @Test
    fun out_of_range_ints_are_clamped() {
        val map = mapOf(
            ShellPrefsCodec.KEY_MASTER to "9999",
            ShellPrefsCodec.KEY_SFX to "-50",
            ShellPrefsCodec.KEY_SENS to "100000",
            ShellPrefsCodec.KEY_OPTIC to "7",
            ShellPrefsCodec.KEY_BARREL to "-1",
        )
        val decoded = ShellPrefsCodec.decode(map)
        assertEquals(SettingsState.GAIN_PCT_MAX, decoded.settings.masterPct)
        assertEquals(0, decoded.settings.sfxPct)
        assertEquals(SettingsState.SENS_MAX, decoded.settings.sensX100)
        assertEquals(LoadoutSelection.SLOT_MAX, decoded.loadout.optic)
        assertEquals(0, decoded.loadout.barrel)
    }

    @Test
    fun bad_enum_ordinals_fall_back_to_field_defaults() {
        val ds = SettingsState.defaults()
        val dp = ProfileState()
        val map = mapOf(
            ShellPrefsCodec.KEY_QUALITY to "High", // a name, not an ordinal → garbage → default
            ShellPrefsCodec.KEY_FACTION to "99",   // out-of-range ordinal → default
        )
        val decoded = ShellPrefsCodec.decode(map)
        assertEquals(ds.quality, decoded.settings.quality)
        assertEquals(dp.faction, decoded.profile.faction)
    }

    @Test
    fun valid_enum_ordinals_decode() {
        val map = mapOf(
            ShellPrefsCodec.KEY_QUALITY to Quality.Medium.ordinal.toString(),
            ShellPrefsCodec.KEY_FACTION to FactionPref.FrenchArmy.ordinal.toString(),
        )
        val decoded = ShellPrefsCodec.decode(map)
        assertEquals(Quality.Medium, decoded.settings.quality)
        assertEquals(FactionPref.FrenchArmy, decoded.profile.faction)
    }

    @Test
    fun blank_callsign_falls_back_to_default() {
        val map = mapOf(ShellPrefsCodec.KEY_CALLSIGN to "   ")
        val decoded = ShellPrefsCodec.decode(map)
        assertEquals(DEFAULT_CALLSIGN, decoded.profile.callsign)
    }

    @Test
    fun overlong_callsign_is_truncated_on_encode_and_round_trips() {
        val long = "X".repeat(CALLSIGN_MAX + 10)
        val state = ShellState.defaults().copy(profile = ProfileState(callsign = long))
        val encoded = ShellPrefsCodec.encode(state)
        assertEquals(CALLSIGN_MAX, encoded[ShellPrefsCodec.KEY_CALLSIGN]!!.length)
        val decoded = ShellPrefsCodec.decode(encoded)
        assertEquals("X".repeat(CALLSIGN_MAX), decoded.profile.callsign)
    }

    @Test
    fun partial_map_keeps_present_fields_and_defaults_the_rest() {
        // Only a couple of keys present — present ones decode, absent ones default.
        val map = mapOf(
            ShellPrefsCodec.KEY_CALLSIGN to "Solo",
            ShellPrefsCodec.KEY_INVERT_Y to "1",
        )
        val decoded = ShellPrefsCodec.decode(map)
        assertEquals("Solo", decoded.profile.callsign)
        assertEquals(true, decoded.settings.invertLookY)
        // Everything else is default.
        assertEquals(SettingsState.defaults().masterPct, decoded.settings.masterPct)
        assertEquals(LoadoutSelection(), decoded.loadout)
        assertEquals(FactionPref.UsArmy, decoded.profile.faction)
    }
}
