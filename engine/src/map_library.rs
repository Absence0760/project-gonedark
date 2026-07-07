//! The **map library** + the unified **battlefield table** (D102) — the presentation-safe
//! map-manifest listing `modes.md` §3 named as the owed D34 work, and the map-driven skirmish
//! boot behind it.
//!
//! Two layers:
//!  - [`MAP_LIBRARY`] — the shipped authored `*.map.ron` battlefields (D76), **embedded via
//!    `include_str!`** (the sanctioned D80-bridge delivery: content compiles into the binary/APK,
//!    so the same library exists on every platform with no filesystem or AAsset story yet — the
//!    D94/CT-D content-directory load replaces this when it lands). Each entry is validated by
//!    test: it parses, range-checks, references a buildable terrain, and carries the
//!    `player`/`enemy` spawn zones the skirmish recipe populates.
//!  - [`BATTLEFIELDS`] — everything the skirmish setup's battlefield picker lists: the standing
//!    battle scenes (the former `SHELL_GAME_MODES`, D81/D101) **plus** the library maps, one
//!    `const` table the shells render (the Kotlin twin is `Battlefield.kt`/`shellBattlefields`,
//!    D79 hand-mirrored).
//!
//! D34 rules hold throughout: this is **content, not sim state** — read-only presentation data,
//! integer-only, never `&mut` sim access, never a checksum surface. A picked library map reaches
//! the sim only through the validated [`MapSpec`] airlock (`MapSpec::apply`, D76) plus the
//! core-tested force recipe (`core::scenario::seed_positioned_skirmish`) at match start.

use crate::map_format::{MapSpec, SpawnZoneSpec};
use crate::Scene;
use gonedark_core::components::Vec2;
use gonedark_core::gunsmith::Loadout;
use gonedark_core::scenario::{seed_positioned_skirmish, Skirmish};
use gonedark_core::sim::Sim;

/// One authored library map: a stable id (the `.map.ron` filename stem — the same identity the
/// `ContentRegistry` derives) and the embedded RON source. Display name/blurb live on the
/// [`Battlefield`] entry, not the `MapSpec` (the format carries geometry, not marketing copy).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MapLibraryEntry {
    /// Stable id — the filename stem, matching `ContentRegistry::map_id_of`.
    pub id: &'static str,
    /// The embedded `.map.ron` text, parsed + validated through the D76 airlock at boot.
    pub source: &'static str,
}

/// The shipped authored battlefields (D76 `*.map.ron`, embedded — see the module docs). Ships
/// **Crossroads** (three posts across an open junction, `player`/`enemy` spawn zones, terrain 0)
/// and **Prokhorovka** (D116) — a large, *even* open-steppe skirmish map on the D80-baked
/// `Terrain::PROKHOROVKA_MAP_ID` (terrain 2): a `.map.ron` reaches baked terrain through the same
/// `Terrain::from_map_id` interim bridge Pointe du Hoc uses. Generated maps (CT-G) join when the
/// D77 content-addressed loader lands (`modes.md` §3 scopes them as "eventually").
pub const MAP_LIBRARY: &[MapLibraryEntry] = &[
    MapLibraryEntry {
        id: "crossroads",
        source: include_str!("../../maps/crossroads.map.ron"),
    },
    // Prokhorovka (Kursk) — a large, even open-steppe battlefield on the baked
    // `core::terrain` PROKHOROVKA_MAP_ID (terrain 2), spawns at opposite ends of the full field.
    MapLibraryEntry {
        id: "prokhorovka",
        source: include_str!("../../maps/prokhorovka.map.ron"),
    },
    // Pointe du Hoc (Normandy) — the D80-baked coastal assault on
    // `core::terrain` POINTE_DU_HOC_MAP_ID (terrain 1): sea wall, casemates and hedgerow scrub are
    // in the baked grid; this spec adds the posts, tactical cover, and south→north deploy zones.
    MapLibraryEntry {
        id: "pointe-du-hoc",
        source: include_str!("../../maps/pointe-du-hoc.map.ron"),
    },
    // Bocage — a dense north–south hedgerow maze authored entirely over the open playfield
    // (terrain 0), mirror-x symmetric, five posts. A deliberately claustrophobic contrast to the
    // open steppe and the small crossroads junction.
    MapLibraryEntry {
        id: "bocage",
        source: include_str!("../../maps/bocage.map.ron"),
    },
];

/// Parse + validate a library map by id — the one gate between the embedded text and a usable
/// [`MapSpec`] (the D76 float airlock; a `None` is an unknown id or — defensively, forbidden by
/// the library test — an entry that no longer validates). Parsing at boot keeps the entries as
/// plain committed text (git-diffable, sha-manifested) rather than pre-baked structs.
pub fn library_spec(id: &str) -> Option<MapSpec> {
    let entry = MAP_LIBRARY.iter().find(|e| e.id == id)?;
    MapSpec::load(entry.source).ok()
}

/// How a battlefield boots: a code-seeded standing **scene** (by `Scene::parse` token — the D81
/// `GameMode` convention) or an authored **library map** (by [`MAP_LIBRARY`] id, booted through
/// [`seed_map_skirmish`]). `&'static str` payloads keep the whole table `Copy`/`const`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BattlefieldKind {
    /// A standing battle scene; the token must resolve via `Scene::parse` (guarded by test).
    Scene(&'static str),
    /// An authored library map; the id must exist in [`MAP_LIBRARY`] (guarded by test).
    LibraryMap(&'static str),
}

/// One selectable battlefield on the skirmish setup: a stable id (the tile key), a display name +
/// one-line blurb, and how it boots. The unified successor of the D81 `GameMode` table — one list,
/// scenes and library maps side by side, so the picker (and its Kotlin twin) never special-cases.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Battlefield {
    /// Stable id (also a tile key). ASCII.
    pub id: &'static str,
    /// Display name shown on the tile.
    pub name: &'static str,
    /// One-line teaser under the name.
    pub blurb: &'static str,
    /// How a Deploy boots it.
    pub kind: BattlefieldKind,
}

impl Battlefield {
    /// The [`Scene`] a scene-kind battlefield deploys into (`None` for a library map, or — for a
    /// shipped entry, forbidden by test — an unparseable token). The `GameMode::scene` seam,
    /// carried over: the picker's one launch decision, unit-tested without a GPU.
    #[inline]
    pub fn scene(&self) -> Option<Scene> {
        match self.kind {
            BattlefieldKind::Scene(token) => Scene::parse(token),
            BattlefieldKind::LibraryMap(_) => None,
        }
    }

    /// The library-map id a map-kind battlefield boots (`None` for a scene).
    #[inline]
    pub fn library_map(&self) -> Option<&'static str> {
        match self.kind {
            BattlefieldKind::Scene(_) => None,
            BattlefieldKind::LibraryMap(id) => Some(id),
        }
    }
}

/// Every battlefield the skirmish setup offers, in display order: the standing battle scenes
/// first (the shipped defaults — the open skirmish is the picker's first tile and the fallback
/// battlefield), then the [`MAP_LIBRARY`] maps. Mirrors Android's `shellBattlefields` (D79).
pub const BATTLEFIELDS: &[Battlefield] = &[
    Battlefield {
        id: "skirmish",
        name: "Skirmish",
        blurb: "Open battle against the enemy commander. Grow your camp, then go dark and fight.",
        kind: BattlefieldKind::Scene("skirmish"),
    },
    Battlefield {
        id: "seize",
        name: "Seize Ground",
        blurb: "Take and hold the objective before the enemy assault overruns it.",
        kind: BattlefieldKind::Scene("seize"),
    },
    Battlefield {
        id: "crossroads",
        name: "Crossroads",
        blurb: "Three posts strung across an open junction. The library's first authored map.",
        kind: BattlefieldKind::LibraryMap("crossroads"),
    },
    Battlefield {
        id: "prokhorovka",
        name: "Prokhorovka",
        blurb: "The 1943 Kursk tank battle: a wide, even steppe. Deploy at opposite ends and cross.",
        kind: BattlefieldKind::LibraryMap("prokhorovka"),
    },
    Battlefield {
        id: "pointe-du-hoc",
        name: "Pointe du Hoc",
        blurb: "The Normandy clifftop battery. Land under the sea wall and fight north through the casemates.",
        kind: BattlefieldKind::LibraryMap("pointe-du-hoc"),
    },
    Battlefield {
        id: "bocage",
        name: "Bocage",
        blurb: "A hedgerow maze fought south to north. Weave the lanes; every wall breaks sight.",
        kind: BattlefieldKind::LibraryMap("bocage"),
    },
];

/// The spawn-zone names the skirmish recipe populates — every library battlefield must carry both
/// (guarded by the library test), exactly the names the shipped maps author.
pub const PLAYER_ZONE: &str = "player";
/// See [`PLAYER_ZONE`].
pub const ENEMY_ZONE: &str = "enemy";

/// A zone's centre cell as a world position — integer midpoint of the sorted extent, then the one
/// world point guaranteed to map back to that cell (`CellRef::to_world_center`). Deterministic
/// integer math only (invariant #1).
fn zone_center_world(zone: &SpawnZoneSpec) -> Vec2 {
    let (lo_x, lo_y, hi_x, hi_y) = zone.extent();
    crate::map_format::CellRef {
        x: (lo_x + hi_x) / 2,
        y: (lo_y + hi_y) / 2,
    }
    .to_world_center()
}

/// Seed a **skirmish on a library map**: the map's own battlefield (terrain, control points,
/// cover props — [`MapSpec::apply`], the D76 airlock's laying half) plus the shared skirmish
/// force recipe (`core::scenario::seed_positioned_skirmish`) dropped into the map's
/// `player`/`enemy` spawn zones. `None` only if a zone is missing — forbidden for shipped
/// entries by the library test, so in practice the defensive arm.
///
/// Deterministic end to end (invariant #1/#7): the spec is validated integer data, the zone
/// centres are integer midpoints, and both halves it composes are themselves seed-order-stable —
/// pinned by the double-seed checksum test below, the same floor every shipped mission meets.
pub fn seed_map_skirmish(
    sim: &mut Sim,
    spec: &MapSpec,
    player_loadout: Loadout,
) -> Option<Skirmish> {
    let player_zone = spec.spawn_zone(PLAYER_ZONE)?;
    let enemy_zone = spec.spawn_zone(ENEMY_ZONE)?;
    let player_pos = zone_center_world(player_zone);
    let enemy_pos = zone_center_world(enemy_zone);

    // Lay the battlefield first (terrain + posts + cover): posts spawn before the camps/troops,
    // preserving the canonical skirmish's neutral-entities-first spawn order.
    {
        let mut b = gonedark_core::scenario::ScenarioBuilder::new(sim);
        spec.apply(&mut b);
    }

    // Then the shared force recipe at the zone centres — income pace, US-vs-FR, camps, purse,
    // one posted troop each, the player's loadout (the core-tested half).
    Some(seed_positioned_skirmish(
        sim,
        player_pos,
        enemy_pos,
        player_loadout,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gonedark_core::components::Faction;
    use gonedark_core::scenario::{SKIRMISH_INCOME_PERIOD, SKIRMISH_START_PURSE};

    /// Count a faction's entities — the sanity floor a seeded map skirmish must field both sides of.
    fn faction_entity_count(sim: &Sim, faction: Faction) -> usize {
        (0..sim.world.capacity())
            .filter(|&i| sim.world.faction[i] == faction)
            .count()
    }

    #[test]
    fn every_battlefield_boots_something_real() {
        // The load-bearing guard (the D81 scene-token test, unified): a scene-kind entry's token
        // must parse, a map-kind entry's id must resolve to a validating library spec with both
        // spawn zones — so a shipped tile can never deploy into nothing.
        assert!(!BATTLEFIELDS.is_empty());
        for bf in BATTLEFIELDS {
            match bf.kind {
                BattlefieldKind::Scene(token) => {
                    assert!(
                        bf.scene().is_some(),
                        "battlefield {:?}: unparseable token {token:?}",
                        bf.id
                    );
                }
                BattlefieldKind::LibraryMap(id) => {
                    let spec = library_spec(id).unwrap_or_else(|| {
                        panic!("battlefield {:?}: no valid library map {id:?}", bf.id)
                    });
                    assert!(
                        spec.spawn_zone(PLAYER_ZONE).is_some(),
                        "{id:?} needs a player zone"
                    );
                    assert!(
                        spec.spawn_zone(ENEMY_ZONE).is_some(),
                        "{id:?} needs an enemy zone"
                    );
                }
            }
        }
    }

    #[test]
    fn the_first_battlefield_is_the_open_skirmish_fallback() {
        // Hosts fall back to BATTLEFIELDS[0] on a stale index / unknown map id; it must stay the
        // standing open skirmish so degradation is always a playable match.
        assert_eq!(BATTLEFIELDS[0].id, "skirmish");
        assert_eq!(BATTLEFIELDS[0].scene(), Some(Scene::Skirmish));
    }

    #[test]
    fn battlefield_and_library_ids_are_distinct_ascii() {
        for bf in BATTLEFIELDS {
            assert!(bf.id.is_ascii() && bf.name.is_ascii() && bf.blurb.is_ascii());
            assert!(!bf.id.is_empty() && !bf.name.is_empty() && !bf.blurb.is_empty());
        }
        for (i, a) in BATTLEFIELDS.iter().enumerate() {
            for b in &BATTLEFIELDS[i + 1..] {
                assert_ne!(a.id, b.id, "duplicate battlefield id {:?}", a.id);
            }
        }
        // Every library map is reachable from the picker (no orphan embeds), and ids are unique.
        for entry in MAP_LIBRARY {
            assert!(
                BATTLEFIELDS
                    .iter()
                    .any(|b| b.library_map() == Some(entry.id)),
                "library map {:?} has no battlefield tile",
                entry.id
            );
        }
    }

    #[test]
    fn every_library_map_seeds_a_playable_skirmish() {
        for entry in MAP_LIBRARY {
            let spec = library_spec(entry.id).expect("valid library map");
            let mut sim = Sim::new(0xD102);
            let s = seed_map_skirmish(&mut sim, &spec, Loadout::STANDARD)
                .expect("shipped library map seeds");
            // Both sides fielded (camp + troop each), the skirmish economy set — the same floor
            // the canonical skirmish gives the win-condition evaluator to work with.
            assert!(
                faction_entity_count(&sim, Faction::Player) >= 2,
                "{}: player side",
                entry.id
            );
            assert!(
                faction_entity_count(&sim, Faction::Enemy) >= 2,
                "{}: enemy side",
                entry.id
            );
            assert_ne!(s.player_base, s.enemy_base);
            assert_eq!(
                sim.resources.amounts[Faction::Player.index()],
                SKIRMISH_START_PURSE
            );
            assert_eq!(sim.income_period(), SKIRMISH_INCOME_PERIOD);
        }
    }

    #[test]
    fn pointe_du_hoc_is_a_selectable_baked_battlefield() {
        // Item-1 guard: the orphaned D80-baked Pointe du Hoc terrain is now a real, selectable
        // battlefield — it has a picker tile, its library map validates, it rides the baked
        // terrain (map id 1, not the open playfield), and it carries both deploy zones the
        // skirmish recipe needs.
        let bf = BATTLEFIELDS
            .iter()
            .find(|b| b.id == "pointe-du-hoc")
            .expect("Pointe du Hoc has a battlefield tile");
        assert_eq!(bf.library_map(), Some("pointe-du-hoc"));
        let spec = library_spec("pointe-du-hoc").expect("Pointe du Hoc library map validates");
        assert_eq!(
            spec.terrain,
            gonedark_core::terrain::Terrain::POINTE_DU_HOC_MAP_ID,
            "Pointe du Hoc rides its baked coastal terrain, not the open playfield",
        );
        assert!(spec.spawn_zone(PLAYER_ZONE).is_some() && spec.spawn_zone(ENEMY_ZONE).is_some());
        assert_eq!(
            spec.control_points.len(),
            3,
            "three posts up the assault axis"
        );

        // It seeds a real, playable skirmish: both sides fielded on the baked ground.
        let mut sim = Sim::new(0x9013);
        let s = seed_map_skirmish(&mut sim, &spec, Loadout::STANDARD).expect("seeds a skirmish");
        assert!(faction_entity_count(&sim, Faction::Player) >= 2);
        assert!(faction_entity_count(&sim, Faction::Enemy) >= 2);
        assert_ne!(s.player_base, s.enemy_base);
    }

    #[test]
    fn bocage_is_a_distinct_hedgerow_battlefield_with_solid_walls() {
        // Item-2 guard: the new authored bocage map is selectable, validates, and — unlike the
        // open-field samples — actually lays SOLID walls (its Heavy hedgerow props paint
        // Cover::Impassable), so it plays as a genuinely different, cover-dense layout.
        let bf = BATTLEFIELDS
            .iter()
            .find(|b| b.id == "bocage")
            .expect("Bocage has a battlefield tile");
        assert_eq!(bf.library_map(), Some("bocage"));
        let spec = library_spec("bocage").expect("Bocage library map validates");
        assert_eq!(spec.control_points.len(), 5, "a five-post diamond");
        assert!(spec.spawn_zone(PLAYER_ZONE).is_some() && spec.spawn_zone(ENEMY_ZONE).is_some());

        let mut sim = Sim::new(0xB0CA);
        seed_map_skirmish(&mut sim, &spec, Loadout::STANDARD).expect("seeds a skirmish");
        // The barricade footprints are solid (blocks movement) — the defining feature vs. the
        // open-field maps, and neither deploy-zone centre is ever walled in (loader + authoring).
        let solid_cells = spec
            .cover_props
            .iter()
            .filter(|p| p.kind.is_solid())
            .filter(|p| {
                sim.terrain
                    .cover_at_cell(p.cell.x, p.cell.y)
                    .blocks_movement()
            })
            .count();
        assert!(
            solid_cells >= 90,
            "the hedgerows lay solid walls, got {solid_cells}"
        );
    }

    #[test]
    fn map_skirmish_stays_bit_identical_over_ticks() {
        // The determinism floor every shipped scene meets (the content-lint double-seed pattern):
        // two identically-seeded map skirmishes fold identically at seed AND across live ticks.
        for entry in MAP_LIBRARY {
            let spec = library_spec(entry.id).expect("valid library map");
            let mut a = Sim::new(0xC0FFEE);
            let mut b = Sim::new(0xC0FFEE);
            seed_map_skirmish(&mut a, &spec, Loadout::STANDARD).unwrap();
            seed_map_skirmish(&mut b, &spec, Loadout::STANDARD).unwrap();
            assert_eq!(a.checksum(), b.checksum(), "{}: seed divergence", entry.id);
            for tick in 0..120 {
                a.step(&[]);
                b.step(&[]);
                assert_eq!(
                    a.checksum(),
                    b.checksum(),
                    "{}: divergence at tick {tick}",
                    entry.id
                );
            }
        }
    }
}
