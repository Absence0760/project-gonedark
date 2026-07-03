//! The shell-prefs persistence codec — a pure, tolerant `key=value` blob encoder/decoder for the
//! player-owned shell state (settings, profile, gunsmith loadout, army pick) across launches. The
//! desktop counterpart of Android's `ShellPrefsCodec`. Presentation only — never sim state, never
//! checksummed. Pure (no fs/env); the file I/O around it is the exempt host glue in `main.rs`.

use crate::shell::army::ArmySelectState;
use crate::shell::profile::{sanitize_callsign, FactionPref, ProfileState};
use crate::shell::settings::{QualityChoice, SettingsState};
use gonedark_core::components::Army;
use gonedark_core::gunsmith::{Barrel, Loadout, Magazine, Muzzle, Optic, Stock};
use gonedark_engine::keybind::KeybindMap;
use gonedark_engine::loadout_ui::LoadoutEditor;
use gonedark_engine::AlertCueMode;
use gonedark_render::theme::PaletteMode;

/// The blob format version tag (the first line). Bumped only on an incompatible layout change; a
/// mismatched or missing tag is tolerated — decode still reads whatever keys it recognises.
pub(crate) const SHELL_PREFS_VERSION: &str = "gonedark-shell 1";

/// This [`Optic`]'s stable index in [`Optic::ALL`] — the persisted ordinal (an unknown ordinal
/// decodes to the slot's `Default`, so a reordered table can't inject an invalid selection).
pub(crate) fn optic_index(o: Optic) -> usize {
    Optic::ALL.iter().position(|&x| x == o).unwrap_or(0)
}
/// This [`Barrel`]'s stable index in [`Barrel::ALL`].
pub(crate) fn barrel_index(b: Barrel) -> usize {
    Barrel::ALL.iter().position(|&x| x == b).unwrap_or(0)
}
/// This [`Magazine`]'s stable index in [`Magazine::ALL`].
pub(crate) fn magazine_index(m: Magazine) -> usize {
    Magazine::ALL.iter().position(|&x| x == m).unwrap_or(0)
}

/// Serialize the three player-owned shell state objects — [`SettingsState`] (audio/look/video),
/// [`ProfileState`] (callsign/faction/record), and the gunsmith [`LoadoutEditor`] — to a flat,
/// line-based `key=value` blob for the host to persist across launches. The Rust counterpart of the
/// Android `ShellPrefsCodec.encode` (a desktop-appropriate blob, **not** the Kotlin wire format — the
/// format is not required to match, only the pattern). Every field is written in its canonical,
/// already-clamped / sanitized form (settings are clamped, the callsign sanitized, enums stored by
/// stable ordinal via [`QualityChoice::index`]/[`FactionPref::index`]), so a save→load round-trip is
/// stable. Pure (no fs/env) — the file I/O around it is the exempt host glue (in `main.rs`).
///
/// **Presentation only** — none of this is sim state (invariant #1 is about the sim's fixed-point
/// state, not host prefs), so it is never checksummed and can't desync anything.
pub(crate) fn encode_shell_prefs(
    settings: &SettingsState,
    profile: &ProfileState,
    loadout: &LoadoutEditor,
    army: &ArmySelectState,
) -> String {
    let mut s = *settings;
    s.clamp();
    let l = loadout.current();
    // Strip any newline from the free-text callsign so it can't break the line-based format (the one
    // value that isn't a number/ordinal). `sanitize_callsign` handles trim/truncate/empty-fallback.
    let callsign = sanitize_callsign(&profile.callsign).replace(['\n', '\r'], " ");
    format!(
        "{SHELL_PREFS_VERSION}\n\
         master={}\nsfx={}\nmusic={}\nsens={}\ninverty={}\nfov={}\nquality={}\n\
         cvdcues={}\nsoundcues={}\ncvdpal={}\nalertcue={}\n\
         callsign={}\nfaction={}\nmatches={}\nwins={}\n\
         optic={}\nbarrel={}\nmagazine={}\n\
         army={}\nkeybinds={}\n",
        s.master_volume,
        s.sfx_volume,
        s.music_volume,
        s.mouse_sensitivity,
        s.invert_look_y as u8,
        s.fov_deg,
        s.quality.index(),
        s.colorblind_cues as u8,
        s.visual_sound_cues as u8,
        s.cvd_palette.index(),
        s.alert_cue_mode.index(),
        callsign,
        profile.faction.index(),
        profile.matches_played,
        profile.wins,
        optic_index(l.optic),
        barrel_index(l.barrel),
        magazine_index(l.magazine),
        // The selected army as its stable `Army::index` ordinal (the same tag order the sim/wire
        // codecs use), tolerant-decoded back by [`decode_army`].
        army.selected.index(),
        // The rebind map as its own compact ordinal blob (`KeybindMap::encode`), tolerant-decoded
        // back by `KeybindMap::decode` — a missing/garbage value falls back to the shipped bindings.
        s.keybinds.encode(),
    )
}

/// Tolerantly decode a [`encode_shell_prefs`] blob back to the three state objects. Any missing,
/// unparseable, or out-of-range value falls back to that field's default — this **never** panics
/// (mirroring the Android codec's forward-compat + corruption-safety contract). An empty/garbage blob
/// therefore decodes to the shipped defaults. Settings are re-clamped and the callsign re-sanitized on
/// the way out, so the result is always valid. Pure — unit-tested without touching the filesystem.
pub(crate) fn decode_shell_prefs(
    blob: &str,
) -> (SettingsState, ProfileState, LoadoutEditor, ArmySelectState) {
    use std::collections::HashMap;

    // Parse `key=value` lines (split on the FIRST '=', so a value may itself contain '='). The
    // version tag and any unrecognised line are simply ignored.
    let map: HashMap<&str, &str> = blob
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect();

    let ds = SettingsState::default();
    let mut settings = SettingsState {
        master_volume: parse_or(map.get("master"), ds.master_volume),
        sfx_volume: parse_or(map.get("sfx"), ds.sfx_volume),
        music_volume: parse_or(map.get("music"), ds.music_volume),
        mouse_sensitivity: parse_or(map.get("sens"), ds.mouse_sensitivity),
        invert_look_y: parse_bool(map.get("inverty"), ds.invert_look_y),
        fov_deg: parse_or(map.get("fov"), ds.fov_deg),
        quality: QualityChoice::from_index(parse_or::<usize>(map.get("quality"), 0)),
        colorblind_cues: parse_bool(map.get("cvdcues"), ds.colorblind_cues),
        visual_sound_cues: parse_bool(map.get("soundcues"), ds.visual_sound_cues),
        // Tolerant ordinal decode (an unknown/missing ordinal → `Off`), the `quality` pattern.
        cvd_palette: PaletteMode::from_index(parse_or::<usize>(map.get("cvdpal"), 0)),
        alert_cue_mode: AlertCueMode::from_index(parse_or::<usize>(map.get("alertcue"), 0)),
        // The rebind map from its compact ordinal blob. `KeybindMap::decode` is total (a missing key
        // → `""` → the shipped default bindings; a corrupt/duplicate blob → defaults), never panics.
        keybinds: KeybindMap::decode(map.get("keybinds").copied().unwrap_or("")),
    };
    // The clamp guards a stored-but-out-of-range numeric (e.g. a hand-edited blob) exactly as the
    // Settings sliders do.
    settings.clamp();

    let dp = ProfileState::default();
    let profile = ProfileState {
        // `sanitize_callsign("")` yields the default callsign, so a missing key is handled here.
        callsign: sanitize_callsign(map.get("callsign").copied().unwrap_or("")),
        faction: FactionPref::from_index(parse_or::<usize>(map.get("faction"), 0)),
        matches_played: parse_or(map.get("matches"), dp.matches_played),
        wins: parse_or(map.get("wins"), dp.wins),
    };

    let loadout = LoadoutEditor::with_loadout(Loadout {
        optic: Optic::ALL
            .get(parse_or::<usize>(map.get("optic"), 0))
            .copied()
            .unwrap_or_default(),
        barrel: Barrel::ALL
            .get(parse_or::<usize>(map.get("barrel"), 0))
            .copied()
            .unwrap_or_default(),
        magazine: Magazine::ALL
            .get(parse_or::<usize>(map.get("magazine"), 0))
            .copied()
            .unwrap_or_default(),
        // Gunsmith breadth (D85): decode the two new sim slots the same way. A missing key defaults
        // to Standard (a pre-D85 save has no stock/muzzle key), so old saves round-trip unchanged.
        stock: Stock::ALL
            .get(parse_or::<usize>(map.get("stock"), 0))
            .copied()
            .unwrap_or_default(),
        muzzle: Muzzle::ALL
            .get(parse_or::<usize>(map.get("muzzle"), 0))
            .copied()
            .unwrap_or_default(),
    });

    let army = ArmySelectState {
        selected: decode_army(map.get("army")),
    };

    (settings, profile, loadout, army)
}

/// Decode a stored [`Army`] ordinal to a **player-selectable** army, defaulting to the shipped
/// [`ArmySelectState::default`] (US) for a missing, unparseable, out-of-range, or non-combatant
/// value. [`Army::Neutral`] is never a valid player pick (factions-plan WS-A), so a stored `0`
/// (Neutral) decodes to the default just like garbage would — the tolerant, corruption-safe read
/// mirroring the enum-ordinal fields above.
pub(crate) fn decode_army(value: Option<&&str>) -> Army {
    let default = ArmySelectState::default().selected;
    match value
        .and_then(|s| s.parse::<usize>().ok())
        .and_then(|i| Army::ALL.get(i).copied())
    {
        Some(Army::Neutral) | None => default,
        Some(a) => a,
    }
}

/// Parse a stored value to `T`, falling back to `fallback` on a missing key or a parse failure. The
/// numeric decode primitive the codec leans on (mirrors the Android codec's tolerant field reads).
pub(crate) fn parse_or<T: std::str::FromStr>(value: Option<&&str>, fallback: T) -> T {
    value.and_then(|s| s.parse::<T>().ok()).unwrap_or(fallback)
}

/// Decode a stored boolean: `"1"`/`"true"` → true, `"0"`/`"false"` → false, anything else → fallback.
pub(crate) fn parse_bool(value: Option<&&str>, fallback: bool) -> bool {
    match value.map(|s| *s) {
        Some("1") | Some("true") => true,
        Some("0") | Some("false") => false,
        _ => fallback,
    }
}
