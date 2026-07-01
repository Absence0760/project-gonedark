//! The **launch-config seam** (Compose shell parity, Tier 0 — `docs/plans/compose-shell-parity.md`
//! §3) + its pure, host-testable parser.
//!
//! ## What this module is — and is NOT
//!
//! The native Compose shell ([`MainActivity`]) and the engine (`android_main`, a separate
//! `NativeActivity`) are **two activities**; the shell hands the engine its match configuration
//! across that boundary as **one `Intent` string extra** ([`EXTRA_KEY`]). This module owns the pure
//! half of that hand-off: the [`LaunchConfig`] DTO and the tolerant [`parse_launch_config`] codec.
//! It is split along the CLAUDE.md *"extract the pure logic to a testable seam"* rule, exactly the
//! way [`crate::thermal`] keeps its integer→enum mapping host-compiled while only the JNI sensor is
//! android-gated:
//!
//!   * **pure codec** (this module) — **no Android deps, no engine deps**, std-only. It compiles
//!     and is unit-tested on every host target (the `tests` module below), so the wire contract is
//!     covered without a device. It carries `scene` as a **string token** (not `engine::Scene`)
//!     because `gonedark-engine` is android-target-only (it pulls wgpu); the android-gated glue maps
//!     the token via the already-host-tested [`gonedark_engine::Scene::parse`].
//!   * **JNI reader** (`read_launch_config` in `android_backend`, `#[cfg(target_os = "android")]`) —
//!     the thin part that reads the live `Intent` extra off the `NativeActivity` and feeds it here.
//!
//! ## The wire format (v1) — a versioned, tolerant `key=value` string
//!
//! `v=1;scene=skirmish;opt=0;bar=0;mag=0;vol=80;sfx=80;sens=100;invy=0;diff=0;node=0;army=1;cvd=0;snd=0`
//!
//! - `;`-separated `key=value` pairs.
//! - **Tolerant decode** (the forward-compat contract): unknown keys are ignored, missing keys take
//!   their [`LaunchConfig::default`] value, and an absent/empty/malformed string yields a full
//!   default config — it **never** panics. That tolerance is what lets later parity tiers start
//!   emitting new keys (`diff=`, …) without an older decoder choking. The Kotlin side
//!   (`LaunchConfig.kt`) mirrors these exact rules; a JVM test there and the tests here pin the same
//!   contract from both ends (the [D79](../../docs/decisions.md) mirrored-constants discipline).
//!
//! This seam carries **no game logic and never touches the sim** — it shapes a coarse launch intent
//! into primitives the host maps into an existing `core`/`engine` call. Determinism is unaffected:
//! a launch config is one-shot match-setup input, not a per-tick sim field.

/// Slot-option wire indices run `0..=2`, matching each gunsmith slot enum's `ALL` order
/// (`0` = `Standard`, `1` = the `+` trade, `2` = the `-` trade — `core::gunsmith`). Out-of-range
/// values clamp to `Standard` (`0`) so a malformed wire string degrades to the neutral loadout
/// rather than failing.
pub const SLOT_MAX: u8 = 2;

/// Audio gains are carried as integer percents `0..=100` (the wire stays float-free; the consumer
/// divides by 100.0 into the `f32` gain the audio backend wants).
pub const GAIN_PCT_MAX: u8 = 100;

/// Look sensitivity is carried as an integer `sensitivity * 100`, mirroring the desktop slider
/// bounds `0.1..=3.0` (`app::shell::SettingsState::SENS_MIN/MAX`) as `10..=300`.
pub const SENS_MIN: u16 = 10;
/// See [`SENS_MIN`].
pub const SENS_MAX: u16 = 300;

/// The campaign **replay difficulty** tier is carried as an integer rank `0..=3` (Recruit, Regular,
/// Veteran, Elite — `core::campaign::Difficulty::tier`). This is both the tier the campaign clear is
/// recorded at on a win (mirroring the desktop host's `active_mission` tier, `app::main`) **and**
/// (D83, resolving Q21) the tier that drives the launched fight: the android glue maps it back to
/// `campaign::Difficulty` and applies its combat tuning through the shared `Game::apply_campaign_tuning`
/// seam (the 4→3 enemy-commander band + the scenario situation modifiers). Out-of-range / missing
/// values clamp to `Recruit` (`0`) so a stale or older wire string degrades to the neutral tier
/// rather than failing.
pub const DIFF_MAX: u8 = 3;

/// The player-army wire ordinals — `1` = US, `2` = French, matching `core::components::Army::index`
/// (`Neutral` = `0` is never a player pick). [`ARMY_MAX`] is the highest combatant ordinal;
/// [`ARMY_DEFAULT`] (US) is what a Neutral / out-of-range / missing / garbage value collapses to,
/// mirroring the desktop `app::shell::decode_army` (`Army::ALL.get(i)` → `None`/`Neutral` → US). Unlike
/// the slot keys this deliberately does NOT clamp-to-max: an out-of-range ordinal is a stale/garbage
/// pick, not "the highest army", so it degrades to the neutral default rather than silently to French.
pub const ARMY_MAX: u8 = 2;
/// See [`ARMY_MAX`] — the default player army ordinal (US Army), the non-Neutral fallback.
pub const ARMY_DEFAULT: u8 = 1;

/// The parsed launch payload the Compose shell hands the engine across the Activity boundary.
///
/// All fields are primitives (no `engine`/`core` types) so this stays a std-only, host-compiled
/// seam. The android-gated glue maps `scene` → [`gonedark_engine::Scene`] and the slot indices →
/// `core::gunsmith::Loadout`; the gains/sensitivity map into the backend audio/input setters.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LaunchConfig {
    /// The scene token (e.g. `"skirmish"`, `"mission1"`). Mapped via `engine::Scene::parse`; an
    /// unknown token falls back to the host's default scene.
    pub scene: String,
    /// Optic slot index, `0..=`[`SLOT_MAX`].
    pub optic: u8,
    /// Barrel slot index, `0..=`[`SLOT_MAX`].
    pub barrel: u8,
    /// Magazine slot index, `0..=`[`SLOT_MAX`].
    pub magazine: u8,
    /// Master volume percent, `0..=`[`GAIN_PCT_MAX`].
    pub master_pct: u8,
    /// SFX volume percent, `0..=`[`GAIN_PCT_MAX`].
    pub sfx_pct: u8,
    /// Look sensitivity ×100, [`SENS_MIN`]`..=`[`SENS_MAX`].
    pub sens_x100: u16,
    /// Invert the embodied vertical look axis.
    pub invert_y: bool,
    /// Campaign replay difficulty tier, `0..=`[`DIFF_MAX`] (Recruit..Elite). The tier a campaign
    /// clear is recorded at on a win; inert for non-campaign scenes. See [`DIFF_MAX`].
    pub diff: u8,
    /// Campaign **node index** (the `NodeId` ordinal) the launch targets, for a campaign (`Mission1`)
    /// scene. Mirrors the desktop host's `pending_launch` node (`app/src/main.rs`: `resolve_node`); the
    /// android glue resolves it through the SHARED mission registry (invariant #2) and records the win
    /// against it. Inert for non-campaign scenes. Missing / negative / garbage → `0` (the root node),
    /// the tolerant default.
    pub node: u32,
    /// Player **army** ordinal — `1` = US, `2` = French (`core::components::Army::index`). Never
    /// `Neutral` (`0`): the parser collapses Neutral / out-of-range / garbage to the US default
    /// ([`ARMY_DEFAULT`]), mirroring the desktop `decode_army` (a commander always fields a real army,
    /// factions-plan WS-A). Fielded at match start via the shared `Game::select_army`.
    pub army: u8,
    /// Accessibility: add the CVD text labels (FIRE/LOST/BASE/TERR) to the embodied alert HUD. Default
    /// OFF (an opt-in intensifier). Fed to the engine via the platform-agnostic
    /// `Game::set_accessibility_prefs`; the desktop twin is the `cvdcues` shell pref. Host /
    /// presentation only — never the sim or the per-tick checksum (invariants #1/#4/#6).
    pub colorblind_cues: bool,
    /// Accessibility: draw the hard-of-hearing visual echoes of the audio-only signals. Default OFF.
    /// The desktop twin is the `soundcues` shell pref. Host / presentation only — never the sim.
    pub visual_sound_cues: bool,
}

impl Default for LaunchConfig {
    fn default() -> Self {
        // Defaults mirror the desktop shell's shipped defaults (app::shell::SettingsState::default)
        // and the real playable match (Scene::Skirmish — desktop's default boot), so a bare Start
        // with no extras behaves like the desktop default rather than the canned demo.
        LaunchConfig {
            scene: "skirmish".to_string(),
            optic: 0,
            barrel: 0,
            magazine: 0,
            master_pct: 80,
            sfx_pct: 80,
            sens_x100: 100,
            invert_y: false,
            diff: 0,
            node: 0,
            army: ARMY_DEFAULT,
            colorblind_cues: false,
            visual_sound_cues: false,
        }
    }
}

/// The `Intent` extra key the Compose shell writes and `android_main` reads. Mirrored verbatim in
/// `LaunchConfig.kt` (`LaunchConfig.EXTRA_KEY`).
pub const EXTRA_KEY: &str = "com.jaredhoward.goingdark.LAUNCH_CONFIG";

/// The wire-format version this build emits/understands. Bumped only on a breaking change; the
/// tolerant decode means additive changes (new keys) do NOT need a bump.
pub const WIRE_VERSION: u32 = 1;

/// Tolerantly parse the v1 wire string into a [`LaunchConfig`]. Pure + total: every malformed input
/// degrades to a sensible default, never a panic. Unknown keys are ignored; missing keys keep their
/// default; out-of-range numbers clamp. See the module docs for the contract.
pub fn parse_launch_config(raw: &str) -> LaunchConfig {
    let mut cfg = LaunchConfig::default();
    for pair in raw.split(';') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let Some((key, value)) = pair.split_once('=') else {
            continue; // not a key=value token — ignore, stay tolerant
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            // `v` is advisory: we parse tolerantly regardless, so an unknown version still decodes
            // as far as it can. (A future breaking format would branch here.)
            "v" => {}
            "scene" if !value.is_empty() => cfg.scene = value.to_string(),
            "opt" => cfg.optic = clamp_u8(value, SLOT_MAX, cfg.optic),
            "bar" => cfg.barrel = clamp_u8(value, SLOT_MAX, cfg.barrel),
            "mag" => cfg.magazine = clamp_u8(value, SLOT_MAX, cfg.magazine),
            "vol" => cfg.master_pct = clamp_u8(value, GAIN_PCT_MAX, cfg.master_pct),
            "sfx" => cfg.sfx_pct = clamp_u8(value, GAIN_PCT_MAX, cfg.sfx_pct),
            "sens" => cfg.sens_x100 = clamp_u16(value, SENS_MIN, SENS_MAX, cfg.sens_x100),
            "invy" => cfg.invert_y = parse_bool(value, cfg.invert_y),
            "diff" => cfg.diff = clamp_u8(value, DIFF_MAX, cfg.diff),
            "node" => cfg.node = clamp_u32(value, cfg.node),
            "army" => cfg.army = clamp_army(value, cfg.army),
            "cvd" => cfg.colorblind_cues = parse_bool(value, cfg.colorblind_cues),
            "snd" => cfg.visual_sound_cues = parse_bool(value, cfg.visual_sound_cues),
            _ => {} // unknown key — ignore (forward-compat)
        }
    }
    cfg
}

/// Parse `value` as a `u8` and clamp to `0..=max`; on parse failure keep `fallback`.
fn clamp_u8(value: &str, max: u8, fallback: u8) -> u8 {
    match value.parse::<i64>() {
        Ok(n) => n.clamp(0, max as i64) as u8,
        Err(_) => fallback,
    }
}

/// Parse `value` as a `u16` and clamp to `min..=max`; on parse failure keep `fallback`.
fn clamp_u16(value: &str, min: u16, max: u16, fallback: u16) -> u16 {
    match value.parse::<i64>() {
        Ok(n) => n.clamp(min as i64, max as i64) as u16,
        Err(_) => fallback,
    }
}

/// Parse `value` as a non-negative `u32` campaign node index; on parse failure or a negative value
/// keep `fallback`. No upper clamp — the mission registry resolves an out-of-range node to nothing
/// (a no-op), so an over-large index degrades safely without needing a bound here.
fn clamp_u32(value: &str, fallback: u32) -> u32 {
    match value.parse::<i64>() {
        Ok(n) if n >= 0 => n.min(u32::MAX as i64) as u32,
        _ => fallback,
    }
}

/// Parse the `army` wire ordinal into a valid **combatant** army index. Mirrors the desktop
/// `app::shell::decode_army`: only `1` (US) and `2` (French) are real player picks; Neutral (`0`),
/// out-of-range, or unparseable values all collapse to the US default ([`ARMY_DEFAULT`]) — Neutral is
/// never a player pick (factions-plan WS-A). This deliberately does NOT clamp-to-[`ARMY_MAX`] the way
/// the slot keys do (see [`ARMY_MAX`]): an out-of-range ordinal is a stale/garbage pick, so it maps to
/// the neutral default rather than silently to French. A field always holds `1` or `2` after decode.
fn clamp_army(value: &str, fallback: u8) -> u8 {
    match value.parse::<i64>() {
        Ok(1) => 1, // US Army
        Ok(2) => 2, // French Army
        Ok(_) => ARMY_DEFAULT, // Neutral (0) / out-of-range → US (never a player pick)
        Err(_) => fallback,
    }
}

/// Parse a wire bool: `1`/`true` → true, `0`/`false` → false, anything else keeps `fallback`.
fn parse_bool(value: &str, fallback: bool) -> bool {
    match value {
        "1" | "true" => true,
        "0" | "false" => false,
        _ => fallback,
    }
}

// ---------------------------------------------------------------------------------------
// Pure wire-primitive → backend-unit mappers.
//
// The wire stays float-free (integer percents / ×100 sensitivity — invariant #1 lives in the
// sim, but the wire mirrors that discipline so the JVM and Rust ends agree exactly). The audio /
// input backends, by contrast, want `f32` gains/multipliers. These two pure helpers do that one
// conversion, host-compiled + unit-tested below, exactly mirroring the desktop semantics:
//   * `pct_to_gain` mirrors `app::shell::SettingsState`'s percent→`[0,1]` gain (the value
//     `pal-desktop::audio::set_gains` then feeds `gonedark_pal::mix::scaled_gain`).
//   * `sens_x100_to_f32` mirrors the desktop sensitivity slider's ×100 wire → `f32` multiplier
//     (the value `pal-desktop::set_look_prefs` applies in `scale_look`).
// They carry no game logic and never touch the sim — the android glue calls them once at startup
// to seed the backend setters.
// ---------------------------------------------------------------------------------------

/// Map an integer volume percent (`0..=`[`GAIN_PCT_MAX`], already clamped by the parser) into the
/// `f32` linear gain `[0.0, 1.0]` the audio backend's `set_gains` wants. `0 → 0.0`, `100 → 1.0`.
/// Mirrors the desktop Settings percent→gain conversion so both platforms scale audio identically.
pub fn pct_to_gain(pct: u8) -> f32 {
    pct as f32 / 100.0
}

/// Map the ×100 wire sensitivity ([`SENS_MIN`]`..=`[`SENS_MAX`], already clamped by the parser)
/// into the `f32` look-sensitivity multiplier the input backend's `set_look_prefs` wants
/// (`100 → 1.0` = stock, `250 → 2.5`). Mirrors the desktop sensitivity slider's ×100→`f32` decode.
pub fn sens_x100_to_f32(x: u16) -> f32 {
    x as f32 / 100.0
}

// ---------------------------------------------------------------------------------------
// Campaign win → Activity-result code (the engine → shell return channel).
//
// Campaign progress is HOST-side and, on Android, lives in the Compose shell's `SharedPreferences`
// (the `ShellPrefs`/`CampaignProgress` seam), NOT in the engine's separate `NativeActivity`. So when
// a campaign mission is WON, the engine must report the win back to the shell for it to record the
// clear — the split-activity analogue of the desktop host's single-process record-on-win
// (`app::main`, where the same process owns both the match and the campaign). The lowest-friction,
// no-Intent-construction channel is the Activity **result code** (`Activity.setResult(int)`): the
// engine sets it before finishing, and the Compose `MainActivity`'s `ActivityResult` callback
// decodes it. A non-win finish (loss / back-out) leaves the default `RESULT_CANCELED` (0), so the
// shell records nothing — exactly the desktop's "a loss records nothing".
//
// The code packs `(node, tier)` into a single positive int at/above `RESULT_FIRST_USER` (1):
// `code = 1 + node*4 + tier` (tier is `0..=3`). `0` (RESULT_CANCELED) and negative
// (RESULT_OK/RESULT_FIRST_USER-relative) codes decode to "no clear". This pure packing is host-
// tested here and mirrored by the Kotlin `CampaignResult` decoder (D79 mirrored-constants).
// ---------------------------------------------------------------------------------------

/// Pack a campaign win `(node, tier)` into the positive Activity result code the engine hands the
/// Compose shell via `Activity.setResult`. `tier` is a campaign difficulty rank `0..=`[`DIFF_MAX`]
/// (clamped). Always `>= 1` (RESULT_FIRST_USER), so it never collides with `RESULT_CANCELED` (0).
pub fn campaign_result_code(node: u32, tier: u8) -> i32 {
    let tier = tier.min(DIFF_MAX) as i32;
    1 + node as i32 * (DIFF_MAX as i32 + 1) + tier
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_the_skirmish_desktop_default() {
        let d = LaunchConfig::default();
        assert_eq!(d.scene, "skirmish");
        assert_eq!((d.optic, d.barrel, d.magazine), (0, 0, 0));
        assert_eq!((d.master_pct, d.sfx_pct), (80, 80));
        assert_eq!(d.sens_x100, 100);
        assert!(!d.invert_y);
        assert_eq!(d.diff, 0); // Recruit — the neutral campaign tier
        assert_eq!(d.node, 0); // the root campaign node
        assert_eq!(d.army, ARMY_DEFAULT); // US Army — Neutral is never a player pick
        assert!(!d.colorblind_cues); // accessibility cues opt-in, default OFF
        assert!(!d.visual_sound_cues);
    }

    #[test]
    fn empty_or_garbage_yields_default() {
        assert_eq!(parse_launch_config(""), LaunchConfig::default());
        assert_eq!(parse_launch_config("   "), LaunchConfig::default());
        assert_eq!(parse_launch_config("not a config at all"), LaunchConfig::default());
        assert_eq!(parse_launch_config(";;;==;"), LaunchConfig::default());
    }

    #[test]
    fn parses_a_full_v1_string() {
        let cfg = parse_launch_config(
            "v=1;scene=mission1;opt=1;bar=2;mag=1;vol=50;sfx=70;sens=250;invy=1;diff=2;node=3;army=2;cvd=1;snd=1",
        );
        assert_eq!(cfg.scene, "mission1");
        assert_eq!((cfg.optic, cfg.barrel, cfg.magazine), (1, 2, 1));
        assert_eq!((cfg.master_pct, cfg.sfx_pct), (50, 70));
        assert_eq!(cfg.sens_x100, 250);
        assert!(cfg.invert_y);
        assert_eq!(cfg.diff, 2); // Veteran
        assert_eq!(cfg.node, 3);
        assert_eq!(cfg.army, 2); // French Army
        assert!(cfg.colorblind_cues);
        assert!(cfg.visual_sound_cues);
    }

    #[test]
    fn node_round_trips_and_missing_or_garbage_defaults_to_root() {
        assert_eq!(parse_launch_config("node=0").node, 0);
        assert_eq!(parse_launch_config("node=5").node, 5);
        assert_eq!(parse_launch_config("node=4294967295").node, u32::MAX);
        // Missing → root (0); the tolerant default.
        assert_eq!(parse_launch_config("v=1;scene=mission1").node, 0);
        // Negative / garbage keep the default (0).
        assert_eq!(parse_launch_config("node=-1").node, 0);
        assert_eq!(parse_launch_config("node=root").node, 0);
    }

    #[test]
    fn army_round_trips_and_collapses_neutral_or_out_of_range_to_us() {
        // The two real combatant picks round-trip.
        assert_eq!(parse_launch_config("army=1").army, 1); // US
        assert_eq!(parse_launch_config("army=2").army, 2); // French
        // Neutral (0) is never a player pick → US default (mirrors desktop decode_army).
        assert_eq!(parse_launch_config("army=0").army, ARMY_DEFAULT);
        // Out-of-range does NOT clamp to French — it degrades to the US default.
        assert_eq!(parse_launch_config("army=9").army, ARMY_DEFAULT);
        assert_eq!(parse_launch_config("army=-1").army, ARMY_DEFAULT);
        // Garbage / missing → the US default.
        assert_eq!(parse_launch_config("army=fr").army, ARMY_DEFAULT);
        assert_eq!(parse_launch_config("v=1;scene=mission1").army, ARMY_DEFAULT);
    }

    #[test]
    fn accessibility_cues_round_trip_and_default_off() {
        assert!(parse_launch_config("cvd=1").colorblind_cues);
        assert!(parse_launch_config("cvd=true").colorblind_cues);
        assert!(!parse_launch_config("cvd=0").colorblind_cues);
        assert!(parse_launch_config("snd=1").visual_sound_cues);
        assert!(!parse_launch_config("snd=false").visual_sound_cues);
        // Missing → both OFF; garbage keeps the default (OFF).
        let d = parse_launch_config("v=1;scene=skirmish");
        assert!(!d.colorblind_cues && !d.visual_sound_cues);
        let g = parse_launch_config("cvd=maybe;snd=");
        assert!(!g.colorblind_cues && !g.visual_sound_cues);
    }

    #[test]
    fn missing_keys_keep_defaults() {
        // Only scene present → every other field stays default (forward-compat: an old emitter).
        let cfg = parse_launch_config("v=1;scene=skirmish");
        assert_eq!(cfg.scene, "skirmish");
        assert_eq!(cfg, LaunchConfig::default());
    }

    #[test]
    fn missing_diff_defaults_to_recruit() {
        // Back-compat: an emitter from before the `diff` key (the pre-C3 wire) still decodes, with
        // the campaign tier defaulting to Recruit (0) — the tolerant-decode contract.
        let cfg = parse_launch_config("v=1;scene=mission1;opt=1;vol=50");
        assert_eq!(cfg.scene, "mission1");
        assert_eq!(cfg.diff, 0);
    }

    #[test]
    fn diff_round_trips_every_tier_and_clamps_out_of_range() {
        for tier in 0u8..=DIFF_MAX {
            assert_eq!(parse_launch_config(&format!("diff={tier}")).diff, tier);
        }
        // Out-of-range / negative / garbage degrade to the neutral tier (clamp, keep-default).
        assert_eq!(parse_launch_config("diff=9").diff, DIFF_MAX);
        assert_eq!(parse_launch_config("diff=-1").diff, 0);
        assert_eq!(parse_launch_config("diff=elite").diff, 0);
    }

    #[test]
    fn unknown_keys_are_ignored() {
        // A still-future key an older decoder doesn't know must not break the rest.
        let cfg = parse_launch_config("scene=mission1;newthing=foo;opt=2");
        assert_eq!(cfg.scene, "mission1");
        assert_eq!(cfg.optic, 2);
        assert_eq!(cfg.barrel, 0); // untouched
    }

    #[test]
    fn campaign_result_code_packs_node_and_tier_above_result_cancelled() {
        // Every code is >= 1 (RESULT_FIRST_USER), never colliding with RESULT_CANCELLED (0).
        for node in 0u32..3 {
            for tier in 0u8..=DIFF_MAX {
                let code = campaign_result_code(node, tier);
                assert!(code >= 1, "code {code} must be >= 1");
                // Unpacks back to the same node/tier (the Kotlin side decodes identically).
                let base = code - 1;
                assert_eq!(base % (DIFF_MAX as i32 + 1), tier as i32);
                assert_eq!(base / (DIFF_MAX as i32 + 1), node as i32);
            }
        }
        // The single shipped campaign node (0) at each tier maps to 1..=4.
        assert_eq!(campaign_result_code(0, 0), 1);
        assert_eq!(campaign_result_code(0, 3), 4);
        // Out-of-range tier clamps rather than overflowing into the next node's range.
        assert_eq!(campaign_result_code(0, 99), campaign_result_code(0, DIFF_MAX));
    }

    #[test]
    fn out_of_range_numbers_clamp() {
        let cfg = parse_launch_config("opt=9;bar=255;mag=-4;vol=900;sfx=-1;sens=9000");
        assert_eq!((cfg.optic, cfg.barrel), (SLOT_MAX, SLOT_MAX));
        assert_eq!(cfg.magazine, 0); // negative clamps to 0
        assert_eq!(cfg.master_pct, GAIN_PCT_MAX);
        assert_eq!(cfg.sfx_pct, 0);
        assert_eq!(cfg.sens_x100, SENS_MAX);
    }

    #[test]
    fn sens_below_min_clamps_up() {
        assert_eq!(parse_launch_config("sens=0").sens_x100, SENS_MIN);
        assert_eq!(parse_launch_config("sens=5").sens_x100, SENS_MIN);
    }

    #[test]
    fn unparseable_numbers_keep_default() {
        let cfg = parse_launch_config("opt=abc;vol=lots;sens=fast;invy=maybe");
        assert_eq!(cfg.optic, 0);
        assert_eq!(cfg.master_pct, 80);
        assert_eq!(cfg.sens_x100, 100);
        assert!(!cfg.invert_y);
    }

    #[test]
    fn bool_forms() {
        assert!(parse_launch_config("invy=1").invert_y);
        assert!(parse_launch_config("invy=true").invert_y);
        assert!(!parse_launch_config("invy=0").invert_y);
        assert!(!parse_launch_config("invy=false").invert_y);
    }

    #[test]
    fn whitespace_around_pairs_is_tolerated() {
        let cfg = parse_launch_config(" scene = skirmish ; opt = 1 ");
        assert_eq!(cfg.scene, "skirmish");
        assert_eq!(cfg.optic, 1);
    }

    #[test]
    fn duplicate_keys_last_wins() {
        assert_eq!(parse_launch_config("opt=1;opt=2").optic, 2);
    }

    // ---- pure wire-primitive → backend-unit mappers --------------------------------------

    #[test]
    fn pct_to_gain_maps_percent_to_unit_interval() {
        assert_eq!(pct_to_gain(0), 0.0);
        assert_eq!(pct_to_gain(100), 1.0);
        assert!((pct_to_gain(80) - 0.8).abs() < 1e-6);
        assert!((pct_to_gain(50) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn pct_to_gain_matches_the_default_config() {
        // The shipped default (80%) decodes to 0.8 on both audio buses.
        let d = LaunchConfig::default();
        assert!((pct_to_gain(d.master_pct) - 0.8).abs() < 1e-6);
        assert!((pct_to_gain(d.sfx_pct) - 0.8).abs() < 1e-6);
    }

    #[test]
    fn sens_x100_to_f32_decodes_the_multiplier() {
        // Stock (100) is a 1.0 pass-through; the slider bounds map to 0.1..=3.0.
        assert!((sens_x100_to_f32(100) - 1.0).abs() < 1e-6);
        assert!((sens_x100_to_f32(SENS_MIN) - 0.1).abs() < 1e-6);
        assert!((sens_x100_to_f32(SENS_MAX) - 3.0).abs() < 1e-6);
        assert!((sens_x100_to_f32(250) - 2.5).abs() < 1e-6);
    }

    #[test]
    fn sens_x100_to_f32_matches_the_default_config() {
        // The shipped default (sens_x100 = 100) decodes to the 1.0 stock multiplier.
        assert!((sens_x100_to_f32(LaunchConfig::default().sens_x100) - 1.0).abs() < 1e-6);
    }
}
