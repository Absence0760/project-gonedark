//! The **standing-battle table** (D81) — the launchable free-pick battles the shells' skirmish
//! surfaces list. Born as the shared Pve/Pvp "mode / map select"; with the three front doors now
//! distinct (`modes.md` §1 — PvP has its own staging door, nothing joinable pre-net), this table
//! backs the **skirmish battlefield picker** (desktop `shell::skirmish`, Android's Compose twin).
//! Each [`GameMode`] names a launchable battle and carries the engine scene token [`Scene::parse`]
//! resolves; a Deploy boots straight into that scene with the player's persisted loadout — no
//! gunsmith gate (the gunsmith moved behind Settings, D81). It grows into the `modes.md` §3
//! map-library manifest when the D34 listing seam lands.
//!
//! This is the **pure, testable seam** the device-gated shell chrome renders — the Rust counterpart
//! of Android's `GameMode.kt` / `shellGameModes`. It holds no game state and never touches the sim
//! (invariants #1/#7): it only maps a picked mode to a [`Scene`] the host then boots. Because the one
//! bit of real logic here — that every mode's token is one the engine actually understands — is
//! pinned by a test against [`Scene::parse`], a typo can never ship an un-launchable mode tile.

use crate::Scene;

/// One selectable standing battle on the skirmish battlefield picker: a stable id, a display name +
/// one-line blurb for the tile, and the engine [`Scene`] token deployed on pick. All fields are
/// `&'static str`, so the type is `Copy` and the whole table is a `const`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GameMode {
    /// Stable id (also a tile key). ASCII.
    pub id: &'static str,
    /// Display name shown on the tile.
    pub name: &'static str,
    /// The engine scene token handed to [`Scene::parse`] at Deploy — must resolve (guarded by test).
    pub scene_token: &'static str,
    /// One-line teaser under the name.
    pub blurb: &'static str,
}

impl GameMode {
    /// The [`Scene`] this mode deploys into, or `None` if its token is unknown to [`Scene::parse`]
    /// (which the [`SHELL_GAME_MODES`] test forbids for any shipped mode, so in practice always
    /// `Some`). Pure — this is the battlefield picker's one real decision, unit-tested without a GPU.
    #[inline]
    pub fn scene(self) -> Option<Scene> {
        Scene::parse(self.scene_token)
    }
}

/// The standing battles the skirmish battlefield picker offers today, mirroring Android's
/// `shellGameModes`. Skirmish is the open fight against the scripted enemy commander; Seize Ground is
/// the take-and-hold objective map (the same battlefield campaign mission 1 is authored on — content
/// is mode-agnostic, D76). The list grows as more scenes land, and becomes the map-library manifest
/// when the D34 listing seam does.
pub const SHELL_GAME_MODES: &[GameMode] = &[
    GameMode {
        id: "skirmish",
        name: "Skirmish",
        scene_token: "skirmish",
        blurb: "Open battle against the enemy commander. Grow your camp, then go dark and fight.",
    },
    GameMode {
        id: "seize",
        name: "Seize Ground",
        scene_token: "seize",
        blurb: "Take and hold the objective before the enemy assault overruns it.",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_mode_resolves_to_a_real_scene() {
        // The load-bearing guard (D81): a mode's token must be one `Scene::parse` accepts, or the
        // tile would deploy into nothing. Mirrors `GameModeTest.kt`'s KNOWN_SCENE_TOKENS check.
        assert!(!SHELL_GAME_MODES.is_empty());
        for mode in SHELL_GAME_MODES {
            assert!(
                mode.scene().is_some(),
                "mode {:?} has an un-parseable scene token {:?}",
                mode.id,
                mode.scene_token
            );
        }
    }

    #[test]
    fn the_two_standing_modes_map_to_their_scenes() {
        // The exact Android parity: Skirmish → the open two-base fight, Seize Ground → the seize
        // mission scene.
        let skirmish = SHELL_GAME_MODES.iter().find(|m| m.id == "skirmish").unwrap();
        let seize = SHELL_GAME_MODES.iter().find(|m| m.id == "seize").unwrap();
        assert_eq!(skirmish.scene(), Some(Scene::Skirmish));
        assert_eq!(seize.scene(), Some(Scene::Mission1));
    }

    #[test]
    fn mode_ids_and_names_are_distinct_ascii() {
        for m in SHELL_GAME_MODES {
            assert!(m.id.is_ascii() && m.name.is_ascii() && m.blurb.is_ascii());
            assert!(!m.id.is_empty() && !m.name.is_empty() && !m.blurb.is_empty());
        }
        // No duplicate ids (each tile is uniquely keyed).
        for (i, a) in SHELL_GAME_MODES.iter().enumerate() {
            for b in &SHELL_GAME_MODES[i + 1..] {
                assert_ne!(a.id, b.id, "duplicate mode id {:?}", a.id);
            }
        }
    }
}
