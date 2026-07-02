//! Host-side `*.map.ron` battlefield format + its float-airlock loader (content-tooling CT-C).
//!
//! This is the **spatial half** of a scenario, factored out of `core::scenario` into a reusable,
//! designer-editable data file: which terrain a battlefield is on, where its control points sit,
//! what cover props are strewn across it, and the named spawn zones a mission drops forces into.
//! One `MapSpec` backs **many** missions (the Operations-hub replay model) and serves a PvP
//! skirmish (two human commanders, same ground) — the content format is mode-agnostic.
//!
//! ## Why it lives in `engine`, not `core`
//!
//! `core` carries no serde dependency and must never gain one (invariant #2). So — exactly like the
//! objective layer and the (parallel) mission format — the RON dependency and **all** validation
//! live host-side, in `engine`. `core::scenario::ScenarioBuilder` stays serde-free, fixed-point, and
//! deterministic; this module is the one place a text number becomes a sim number.
//!
//! ## The float airlock (invariant #1)
//!
//! Every numeric field in the schema is an **integer** — cells for positions, a `u16` map id for the
//! terrain reference. There is **no `f32`/`f64` anywhere in the type graph from file to sim**: a
//! float literal in a `.map.ron` fails to deserialize into an `i32`, and the determinism guard greps
//! this module the same as any sim code. Integers cross into `core` only through
//! [`Fixed::from_int`] (cell → world). `#[serde(deny_unknown_fields)]` rejects typos/unknown keys,
//! the loader range-validates every cell and spawn zone, and it **fails loud** (a returned
//! [`MapError`], never a silent clamp) on an out-of-bounds cell, an out-of-bounds or overlapping
//! spawn zone, or a dangling terrain reference — a bad file errors host-side, never silently
//! desyncs.
//!
//! ## Zero checksum surface (invariants #4/#7)
//!
//! Applying a map only calls the same `ScenarioBuilder`/`Terrain` primitives a hand seeder already
//! calls: control points are folded state, cover is static map data (never in the per-tick
//! checksum). The data file never enters the checksum — only the seeded `Sim` does, on the exact
//! same footing as a code-built scene. Two applies of the same map build a byte-identical `Sim`.

use std::fmt;

use gonedark_core::components::Vec2;
use gonedark_core::fixed::Fixed;
use gonedark_core::flow_field::GRID;
use gonedark_core::scenario::ScenarioBuilder;
use gonedark_core::terrain::{Cover, MapId, Terrain};

use serde::Deserialize;

/// A cell coordinate on the `GRID × GRID` playfield — the authoring unit for every position in the
/// format. **Integer only** (the airlock): a float literal here fails to deserialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellRef {
    pub x: i32,
    pub y: i32,
}

impl CellRef {
    /// Is this cell inside the `[0, GRID)²` playfield?
    #[inline]
    fn in_bounds(self) -> bool {
        self.x >= 0 && self.y >= 0 && (self.x as usize) < GRID && (self.y as usize) < GRID
    }

    /// World position at this cell's **centre** — the one point guaranteed to map back to this exact
    /// cell under [`Terrain::cell_of`] (`world → cell` is a clamped floor; cell `i` covers world
    /// `[-HALF_EXTENT + i, +1)`, so its centre `-HALF_EXTENT + i + 1/2` floors to `i`). Integer →
    /// `Fixed` only (invariant #1): `HALF_EXTENT == GRID/2` world units, so the centre is
    /// `Fixed::from_int(x - GRID/2) + 1/2`.
    #[inline]
    fn to_world_center(self) -> Vec2 {
        let half = (GRID / 2) as i32;
        let c = |v: i32| Fixed::from_int(v - half) + Fixed::from_ratio(1, 2);
        Vec2::new(c(self.x), c(self.y))
    }
}

/// The cover-prop archetypes a battlefield may strew across its ground ([D50](../decisions.md)):
/// crate / tree / rock / barricade / turret. Each is authoring shorthand for a sim [`Cover`] level
/// on its cell — light concealment you can still fire through, or a sight-blocking solid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum CoverPropKind {
    /// Supply crate — [`Cover::Light`] (partial mitigation, sight passes).
    Crate,
    /// Tree / scrub — [`Cover::Light`] (concealment, sight passes).
    Tree,
    /// Boulder — [`Cover::Heavy`] (a solid; strong mitigation, blocks line of sight).
    Rock,
    /// Barricade / wall segment — [`Cover::Heavy`] (blocks line of sight).
    Barricade,
    /// Fortified gun emplacement — [`Cover::Heavy`] (a hard point; blocks line of sight).
    Turret,
}

impl CoverPropKind {
    /// The sim [`Cover`] level this prop lays on its cell. The airlock's semantic half: an authoring
    /// name becomes the integer cover the sim reads for damage mitigation + LoS.
    #[inline]
    pub fn cover(self) -> Cover {
        match self {
            CoverPropKind::Crate | CoverPropKind::Tree => Cover::Light,
            CoverPropKind::Rock | CoverPropKind::Barricade | CoverPropKind::Turret => Cover::Heavy,
        }
    }
}

/// One cover prop: a kind + the cell it sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverPropSpec {
    pub kind: CoverPropKind,
    pub cell: CellRef,
}

/// A named rectangular spawn zone — the region a mission drops a force into. Inclusive corners
/// (`min..=max` on each axis); `min` need not be sorted before `max` (the loader validates order).
/// Zones are carried by the map and consumed by a mission; the loader guarantees they are in bounds
/// and mutually non-overlapping, so a mission can populate one without colliding with another.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnZoneSpec {
    pub name: String,
    pub min: CellRef,
    pub max: CellRef,
}

impl SpawnZoneSpec {
    /// The inclusive `(lo_x, lo_y, hi_x, hi_y)` extent, corners sorted. Pure — the seam the overlap
    /// and bounds checks read.
    #[inline]
    fn extent(&self) -> (i32, i32, i32, i32) {
        let (lo_x, hi_x) = if self.min.x <= self.max.x {
            (self.min.x, self.max.x)
        } else {
            (self.max.x, self.min.x)
        };
        let (lo_y, hi_y) = if self.min.y <= self.max.y {
            (self.min.y, self.max.y)
        } else {
            (self.max.y, self.min.y)
        };
        (lo_x, lo_y, hi_x, hi_y)
    }

    /// Every cell inside this zone (inclusive), row-major — what a mission iterates to place a force.
    /// Deterministic order (`y` outer, `x` inner) so a mission that populates a zone spawns in a
    /// stable order (invariant #1: no per-tick fold, but a stable seed order keeps the scene
    /// reproducible).
    pub fn cells(&self) -> impl Iterator<Item = CellRef> {
        let (lo_x, lo_y, hi_x, hi_y) = self.extent();
        (lo_y..=hi_y).flat_map(move |y| (lo_x..=hi_x).map(move |x| CellRef { x, y }))
    }
}

/// Do two inclusive extents overlap on both axes? Pure helper for the loader's spawn-zone check.
#[inline]
fn extents_overlap(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> bool {
    let (a_lx, a_ly, a_hx, a_hy) = a;
    let (b_lx, b_ly, b_hx, b_hy) = b;
    a_lx <= b_hx && b_lx <= a_hx && a_ly <= b_hy && b_ly <= a_hy
}

/// The raw, deserialized battlefield — the **spatial half** of a scenario. `#[serde(deny_unknown_fields)]`
/// + all-integer fields make it the float airlock's front door. Deserializing it does **not** validate
/// it; call [`MapSpec::load`] (or [`MapSpec::validate`]) to range-check before use.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapSpec {
    /// Which static terrain this battlefield is on — a [`MapId`] into the `core::terrain` registry
    /// (the existing terrain-by-map-id, [D28](../decisions.md)). Placements are laid over it.
    pub terrain: MapId,
    /// Neutral control points to fight over, at authored cells.
    #[serde(default)]
    pub control_points: Vec<CellRef>,
    /// Cover props strewn across the ground (crate/tree/rock/barricade/turret), at authored cells.
    #[serde(default)]
    pub cover_props: Vec<CoverPropSpec>,
    /// Named spawn zones a mission drops forces into. Validated in-bounds + mutually non-overlapping.
    #[serde(default)]
    pub spawn_zones: Vec<SpawnZoneSpec>,
}

/// Everything that can make a `.map.ron` invalid — each a **loud** load-time failure, never a silent
/// clamp or desync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapError {
    /// The RON text failed to parse (bad syntax, an unknown field, or a **float literal** where an
    /// integer cell is required — the airlock rejecting a float).
    Parse(String),
    /// The terrain map id names a map this build can't rebuild ([`Terrain::from_map_id`] returned
    /// `None`) — a dangling reference.
    UnknownTerrain(MapId),
    /// A control point sits outside the `[0, GRID)²` playfield.
    ControlPointOutOfBounds { index: usize, cell: CellRef },
    /// A cover prop sits outside the playfield.
    CoverPropOutOfBounds { index: usize, cell: CellRef },
    /// A spawn zone corner sits outside the playfield.
    SpawnZoneOutOfBounds { name: String },
    /// Two spawn zones overlap — a mission populating one would collide with the other.
    SpawnZonesOverlap { a: String, b: String },
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MapError::Parse(e) => write!(f, "map parse error: {e}"),
            MapError::UnknownTerrain(id) => {
                write!(f, "unknown terrain map id {id} (no such map in the registry)")
            }
            MapError::ControlPointOutOfBounds { index, cell } => write!(
                f,
                "control point #{index} at cell ({}, {}) is outside the 0..{GRID} playfield",
                cell.x, cell.y
            ),
            MapError::CoverPropOutOfBounds { index, cell } => write!(
                f,
                "cover prop #{index} at cell ({}, {}) is outside the 0..{GRID} playfield",
                cell.x, cell.y
            ),
            MapError::SpawnZoneOutOfBounds { name } => {
                write!(f, "spawn zone {name:?} extends outside the 0..{GRID} playfield")
            }
            MapError::SpawnZonesOverlap { a, b } => {
                write!(f, "spawn zones {a:?} and {b:?} overlap")
            }
        }
    }
}

impl std::error::Error for MapError {}

impl MapSpec {
    /// Parse a `.map.ron` string into a raw `MapSpec` **without** validating it — the airlock's first
    /// gate. `deny_unknown_fields` + the all-integer type graph make RON reject unknown keys and
    /// float literals here; a syntax/type error becomes [`MapError::Parse`].
    pub fn parse(ron: &str) -> Result<MapSpec, MapError> {
        ron::from_str::<MapSpec>(ron).map_err(|e| MapError::Parse(e.to_string()))
    }

    /// Range-validate a parsed map, **failing loud** on the first problem: a dangling terrain ref,
    /// any out-of-bounds control point / cover prop, an out-of-bounds spawn zone, or two overlapping
    /// spawn zones. Pure over `core` (no I/O); the seam the loader and the content lint both call.
    pub fn validate(&self) -> Result<(), MapError> {
        // Terrain reference must resolve to a buildable map (never silently rebuild the wrong ground).
        if Terrain::from_map_id(self.terrain).is_none() {
            return Err(MapError::UnknownTerrain(self.terrain));
        }

        for (index, &cell) in self.control_points.iter().enumerate() {
            if !cell.in_bounds() {
                return Err(MapError::ControlPointOutOfBounds { index, cell });
            }
        }

        for (index, prop) in self.cover_props.iter().enumerate() {
            if !prop.cell.in_bounds() {
                return Err(MapError::CoverPropOutOfBounds {
                    index,
                    cell: prop.cell,
                });
            }
        }

        // Each spawn zone in bounds (both corners), then no pair may overlap.
        for zone in &self.spawn_zones {
            let (lo_x, lo_y, hi_x, hi_y) = zone.extent();
            let lo_ok = CellRef { x: lo_x, y: lo_y }.in_bounds();
            let hi_ok = CellRef { x: hi_x, y: hi_y }.in_bounds();
            if !lo_ok || !hi_ok {
                return Err(MapError::SpawnZoneOutOfBounds {
                    name: zone.name.clone(),
                });
            }
        }
        for i in 0..self.spawn_zones.len() {
            for j in (i + 1)..self.spawn_zones.len() {
                if extents_overlap(self.spawn_zones[i].extent(), self.spawn_zones[j].extent()) {
                    return Err(MapError::SpawnZonesOverlap {
                        a: self.spawn_zones[i].name.clone(),
                        b: self.spawn_zones[j].name.clone(),
                    });
                }
            }
        }

        Ok(())
    }

    /// Parse **and** validate a `.map.ron` string — the one call a host uses to turn text into a
    /// map it can trust. Equivalent to [`parse`](Self::parse) then [`validate`](Self::validate).
    pub fn load(ron: &str) -> Result<MapSpec, MapError> {
        let spec = MapSpec::parse(ron)?;
        spec.validate()?;
        Ok(spec)
    }

    /// Look up a spawn zone by name (for a mission that populates it).
    pub fn spawn_zone(&self, name: &str) -> Option<&SpawnZoneSpec> {
        self.spawn_zones.iter().find(|z| z.name == name)
    }

    /// Lay this map's spatial data onto `builder`: load the referenced terrain, then place control
    /// points at their authored cells and cover props (as [`Cover`]) at theirs. Spawn zones are
    /// *carried*, not laid — a mission populates them. Assumes the map is already
    /// [`validate`](Self::validate)d (call [`load`](Self::load) first); a still-dangling terrain ref
    /// is a programmer error and panics rather than silently building the wrong ground.
    ///
    /// Only calls the same `ScenarioBuilder`/`Terrain` primitives a hand seeder calls, so the seeded
    /// `Sim` is on the exact same footing as a code-built scene — no new checksum surface (#7), and
    /// two applies of the same map build a byte-identical `Sim`.
    pub fn apply(&self, builder: &mut ScenarioBuilder) {
        assert!(
            builder.sim_mut().load_map(self.terrain),
            "map applied with an unvalidated/unknown terrain id {} (call MapSpec::load first)",
            self.terrain
        );

        for &cp in &self.control_points {
            builder.control_point(cp.to_world_center());
        }

        // Cover props are static map data (not in the checksum): set the cell's Cover directly, the
        // same primitive the hand seeders' terrain builders use.
        for prop in &self.cover_props {
            builder
                .sim_mut()
                .terrain
                .set_cover(prop.cell.x, prop.cell.y, prop.kind.cover());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gonedark_core::sim::Sim;

    /// A small, valid sample exercising every section: a known terrain, two control points, a few
    /// cover props (light + heavy), and two non-overlapping named spawn zones.
    const SAMPLE: &str = r#"
MapSpec(
    terrain: 1,
    control_points: [
        CellRef(x: 64, y: 64),
        CellRef(x: 30, y: 90),
    ],
    cover_props: [
        CoverPropSpec(kind: Crate,     cell: CellRef(x: 40, y: 40)),
        CoverPropSpec(kind: Barricade, cell: CellRef(x: 41, y: 40)),
        CoverPropSpec(kind: Tree,      cell: CellRef(x: 42, y: 40)),
    ],
    spawn_zones: [
        SpawnZoneSpec(name: "player", min: CellRef(x: 10, y: 10), max: CellRef(x: 14, y: 14)),
        SpawnZoneSpec(name: "enemy",  min: CellRef(x: 100, y: 100), max: CellRef(x: 110, y: 110)),
    ],
)
"#;

    #[test]
    fn sample_loads_and_validates() {
        let m = MapSpec::load(SAMPLE).expect("sample map is valid");
        assert_eq!(m.terrain, 1);
        assert_eq!(m.control_points.len(), 2);
        assert_eq!(m.cover_props.len(), 3);
        assert_eq!(m.spawn_zones.len(), 2);
        assert!(m.spawn_zone("player").is_some());
        assert!(m.spawn_zone("missing").is_none());
    }

    #[test]
    fn round_trips_from_string() {
        // Load-from-string is the round trip for a read-only format: parse → validate → the same
        // logical map every time, deterministically.
        let a = MapSpec::load(SAMPLE).unwrap();
        let b = MapSpec::load(SAMPLE).unwrap();
        assert_eq!(a.terrain, b.terrain);
        assert_eq!(a.control_points, b.control_points);
        assert_eq!(a.cover_props, b.cover_props);
        assert_eq!(a.spawn_zones.len(), b.spawn_zones.len());
    }

    #[test]
    fn cover_prop_kinds_map_to_the_right_cover() {
        assert_eq!(CoverPropKind::Crate.cover(), Cover::Light);
        assert_eq!(CoverPropKind::Tree.cover(), Cover::Light);
        assert_eq!(CoverPropKind::Rock.cover(), Cover::Heavy);
        assert_eq!(CoverPropKind::Barricade.cover(), Cover::Heavy);
        assert_eq!(CoverPropKind::Turret.cover(), Cover::Heavy);
    }

    #[test]
    fn placements_land_at_authored_cells() {
        let m = MapSpec::load(SAMPLE).unwrap();
        let mut sim = Sim::new(1);
        {
            let mut b = ScenarioBuilder::new(&mut sim);
            m.apply(&mut b);
        }

        // Control points landed at exactly their authored cells (world centre floors back to cell).
        assert_eq!(sim.territory.points.len(), 2);
        for (cp, authored) in sim.territory.points.iter().zip(&m.control_points) {
            assert_eq!(sim.terrain.cell_of(cp.pos), (authored.x, authored.y));
        }

        // Cover props landed at exactly their authored cells, with the mapped cover.
        for prop in &m.cover_props {
            assert_eq!(
                sim.terrain.cover_at_cell(prop.cell.x, prop.cell.y),
                prop.kind.cover(),
                "prop {:?} at ({}, {})",
                prop.kind,
                prop.cell.x,
                prop.cell.y
            );
        }
        // `apply` asserts `load_map(terrain)` succeeded, so reaching here proves the referenced
        // terrain was loaded (a bad id would have panicked at apply time).
    }

    #[test]
    fn same_map_applied_twice_is_byte_identical() {
        let m = MapSpec::load(SAMPLE).unwrap();

        let build = || {
            let mut sim = Sim::new(7);
            {
                let mut b = ScenarioBuilder::new(&mut sim);
                m.apply(&mut b);
            }
            sim
        };
        let s1 = build();
        let s2 = build();

        // Folded spatial state (control points, map id, units) is bit-identical.
        assert_eq!(s1.checksum(), s2.checksum());

        // Static terrain (not in the checksum) is also cell-for-cell identical.
        for cy in 0..GRID as i32 {
            for cx in 0..GRID as i32 {
                assert_eq!(
                    s1.terrain.cover_at_cell(cx, cy),
                    s2.terrain.cover_at_cell(cx, cy),
                    "terrain differs at ({cx}, {cy})"
                );
            }
        }
    }

    #[test]
    fn float_literal_is_rejected() {
        // The airlock: a fractional coordinate can't deserialize into an i32 cell.
        let ron = r#"MapSpec(terrain: 0, control_points: [CellRef(x: 1.5, y: 2)])"#;
        let err = MapSpec::load(ron).unwrap_err();
        assert!(matches!(err, MapError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn unknown_field_is_rejected() {
        let ron = r#"MapSpec(terrain: 0, bogus: 3)"#;
        let err = MapSpec::load(ron).unwrap_err();
        assert!(matches!(err, MapError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn unknown_terrain_ref_is_rejected() {
        let ron = r#"MapSpec(terrain: 9999)"#;
        let err = MapSpec::load(ron).unwrap_err();
        assert_eq!(err, MapError::UnknownTerrain(9999));
    }

    #[test]
    fn out_of_bounds_control_point_is_rejected() {
        let ron = r#"MapSpec(terrain: 0, control_points: [CellRef(x: 999, y: 0)])"#;
        let err = MapSpec::load(ron).unwrap_err();
        assert!(
            matches!(err, MapError::ControlPointOutOfBounds { index: 0, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn out_of_bounds_cover_prop_is_rejected() {
        let ron =
            r#"MapSpec(terrain: 0, cover_props: [CoverPropSpec(kind: Rock, cell: CellRef(x: 0, y: -1))])"#;
        let err = MapSpec::load(ron).unwrap_err();
        assert!(
            matches!(err, MapError::CoverPropOutOfBounds { index: 0, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn out_of_bounds_spawn_zone_is_rejected() {
        let ron = r#"MapSpec(terrain: 0, spawn_zones: [
            SpawnZoneSpec(name: "oob", min: CellRef(x: 120, y: 120), max: CellRef(x: 200, y: 130)),
        ])"#;
        let err = MapSpec::load(ron).unwrap_err();
        assert!(
            matches!(err, MapError::SpawnZoneOutOfBounds { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn overlapping_spawn_zones_are_rejected() {
        let ron = r#"MapSpec(terrain: 0, spawn_zones: [
            SpawnZoneSpec(name: "a", min: CellRef(x: 10, y: 10), max: CellRef(x: 20, y: 20)),
            SpawnZoneSpec(name: "b", min: CellRef(x: 15, y: 15), max: CellRef(x: 25, y: 25)),
        ])"#;
        let err = MapSpec::load(ron).unwrap_err();
        assert!(matches!(err, MapError::SpawnZonesOverlap { .. }), "got {err:?}");
    }

    #[test]
    fn spawn_zone_cells_enumerate_in_stable_row_major_order() {
        let z = SpawnZoneSpec {
            name: "z".into(),
            // Corners deliberately unsorted — extent() sorts them.
            min: CellRef { x: 3, y: 6 },
            max: CellRef { x: 1, y: 5 },
        };
        let cells: Vec<_> = z.cells().collect();
        assert_eq!(
            cells,
            vec![
                CellRef { x: 1, y: 5 },
                CellRef { x: 2, y: 5 },
                CellRef { x: 3, y: 5 },
                CellRef { x: 1, y: 6 },
                CellRef { x: 2, y: 6 },
                CellRef { x: 3, y: 6 },
            ]
        );
    }

    /// The shipped sample under `maps/` — embedded so the test suite guards it (it loads,
    /// validates, and applies to a real `Sim`). Any edit that breaks the file breaks CI.
    const SHIPPED_CROSSROADS: &str = include_str!("../../maps/crossroads.map.ron");

    #[test]
    fn shipped_sample_map_loads_validates_and_applies() {
        let m = MapSpec::load(SHIPPED_CROSSROADS).expect("shipped crossroads.map.ron is valid");
        assert_eq!(m.control_points.len(), 3);
        assert!(!m.cover_props.is_empty());
        assert!(m.spawn_zone("player").is_some());
        assert!(m.spawn_zone("enemy").is_some());

        let mut sim = Sim::new(3);
        {
            let mut b = ScenarioBuilder::new(&mut sim);
            m.apply(&mut b);
        }
        assert_eq!(sim.territory.points.len(), 3);
        for prop in &m.cover_props {
            assert_eq!(
                sim.terrain.cover_at_cell(prop.cell.x, prop.cell.y),
                prop.kind.cover()
            );
        }
    }

    #[test]
    fn empty_sections_default_to_empty() {
        // A minimal map (terrain only) is valid — the optional sections default to empty.
        let m = MapSpec::load("MapSpec(terrain: 0)").unwrap();
        assert!(m.control_points.is_empty());
        assert!(m.cover_props.is_empty());
        assert!(m.spawn_zones.is_empty());
    }
}
