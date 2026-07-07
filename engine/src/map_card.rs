//! The **map card** — picker-preview metrics derived at runtime from a [`MapSpec`]
//! (`modes.md` §3's "see what you're getting into", shipped v1).
//!
//! The target model puts the baker's lint PNG + balance metrics in the picker; both stay
//! deferred behind the [D77](../decisions.md) content-hash loader (the [D102](../decisions.md)
//! "deferred presentation" note). This v1 needs neither: everything on the card is re-derivable
//! from the `MapSpec` the library already embeds — control-point count, cover-prop counts by
//! kind, spawn-zone summaries, and cover density as integer **permille** of the `GRID`² field
//! (with a per-quadrant breakdown, the asymmetry read at a glance).
//!
//! D34 rules hold: this is **presentation data, not sim state** — read-only, derived, never a
//! checksum surface. It is also deliberately **integer-only** (invariant #1 hygiene): the card
//! never touches the sim, but keeping it float-free means it can never be mistaken for — or
//! drift into — unsafe-to-sim data. Floats appear only where a shell *draws* the card.

use crate::map_format::{CoverPropKind, MapSpec};
use gonedark_core::flow_field::GRID;

/// Every cover-prop kind, in declaration order — the index space of
/// [`MapCard::prop_counts`] (and the order a shell legend lists them in).
pub const COVER_KINDS: [CoverPropKind; 5] = [
    CoverPropKind::Crate,
    CoverPropKind::Tree,
    CoverPropKind::Rock,
    CoverPropKind::Barricade,
    CoverPropKind::Turret,
];

/// A kind's slot in [`COVER_KINDS`] / [`MapCard::prop_counts`].
#[inline]
fn kind_index(kind: CoverPropKind) -> usize {
    match kind {
        CoverPropKind::Crate => 0,
        CoverPropKind::Tree => 1,
        CoverPropKind::Rock => 2,
        CoverPropKind::Barricade => 3,
        CoverPropKind::Turret => 4,
    }
}

/// Which grid quadrant a cell falls in, split at `GRID/2` per axis:
/// `0` = low-x/low-y, `1` = high-x/low-y, `2` = low-x/high-y, `3` = high-x/high-y.
/// Cell-space only — the card makes no compass claim (which way is "north" is the
/// renderer's business).
#[inline]
fn quadrant_of(x: i32, y: i32) -> usize {
    let half = (GRID / 2) as i32;
    (usize::from(y >= half) << 1) | usize::from(x >= half)
}

/// One spawn zone on the card: its name plus the sorted inclusive cell extent and area —
/// the summary form of a [`SpawnZoneSpec`](crate::map_format::SpawnZoneSpec) (whose corners
/// need not be authored sorted; these are).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnZoneSummary {
    /// The zone's authored name (`player` / `enemy` on every library map).
    pub name: String,
    /// Sorted inclusive low corner `(x, y)`.
    pub lo: (i32, i32),
    /// Sorted inclusive high corner `(x, y)`.
    pub hi: (i32, i32),
    /// Cell count of the inclusive extent (`width * height`).
    pub cells: u32,
}

/// The derived picker card for one map — every metric integer, every field re-derivable from
/// the spec (nothing here is authored, so the card can never drift from the map).
///
/// Density counts **occupied cells**, not props: two props authored onto the same cell lay one
/// cell of cover (exactly what [`MapSpec::apply`] lays), so the card deduplicates before it
/// divides. Assumes a **validated** spec ([`MapSpec::load`]) — like `apply`, it does not
/// re-range-check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapCard {
    /// How many neutral control points the map fields.
    pub control_points: u32,
    /// Cover-prop counts by kind, indexed parallel to [`COVER_KINDS`].
    pub prop_counts: [u32; COVER_KINDS.len()],
    /// Distinct cells occupied by cover (props deduplicated by cell).
    pub covered_cells: u32,
    /// Cover density: occupied cells as integer permille of the `GRID`² field.
    pub cover_permille: u32,
    /// Occupied cover cells per quadrant (see [`quadrant_of`] for the index order).
    pub quadrant_cells: [u32; 4],
    /// Per-quadrant cover density, permille of that quadrant's `(GRID/2)`² cells.
    pub quadrant_permille: [u32; 4],
    /// One summary per authored spawn zone, in authored order.
    pub spawn_zones: Vec<SpawnZoneSummary>,
}

impl MapCard {
    /// Derive the card from a validated spec. Integer math only.
    pub fn derive(spec: &MapSpec) -> MapCard {
        let mut prop_counts = [0u32; COVER_KINDS.len()];
        // Dedupe occupied cells by sort (row-major order, like every grid walk here) — two
        // props on one cell are one cell of laid cover.
        let mut occupied: Vec<(i32, i32)> = Vec::with_capacity(spec.cover_props.len());
        for prop in &spec.cover_props {
            prop_counts[kind_index(prop.kind)] += 1;
            occupied.push((prop.cell.y, prop.cell.x));
        }
        occupied.sort_unstable();
        occupied.dedup();

        let mut quadrant_cells = [0u32; 4];
        for &(y, x) in &occupied {
            quadrant_cells[quadrant_of(x, y)] += 1;
        }

        let field = (GRID * GRID) as u32;
        let quadrant_field = field / 4;
        let covered_cells = occupied.len() as u32;
        let mut quadrant_permille = [0u32; 4];
        for (permille, &cells) in quadrant_permille.iter_mut().zip(&quadrant_cells) {
            *permille = cells * 1000 / quadrant_field;
        }

        let spawn_zones = spec
            .spawn_zones
            .iter()
            .map(|zone| {
                let (lo_x, lo_y, hi_x, hi_y) = zone.extent();
                SpawnZoneSummary {
                    name: zone.name.clone(),
                    lo: (lo_x, lo_y),
                    hi: (hi_x, hi_y),
                    cells: ((hi_x - lo_x + 1) * (hi_y - lo_y + 1)) as u32,
                }
            })
            .collect();

        MapCard {
            control_points: spec.control_points.len() as u32,
            prop_counts,
            covered_cells,
            cover_permille: covered_cells * 1000 / field,
            quadrant_cells,
            quadrant_permille,
            spawn_zones,
        }
    }

    /// This card's count of one prop kind (the [`COVER_KINDS`]-indexed array, by name).
    #[inline]
    pub fn prop_count(&self, kind: CoverPropKind) -> u32 {
        self.prop_counts[kind_index(kind)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_format::{CellRef, CoverPropSpec, SpawnZoneSpec};
    use crate::map_library::library_spec;

    /// A bare spec (terrain only) to build fixtures on.
    fn empty_spec() -> MapSpec {
        MapSpec::load("MapSpec(terrain: 0)").unwrap()
    }

    fn prop(kind: CoverPropKind, x: i32, y: i32) -> CoverPropSpec {
        CoverPropSpec {
            kind,
            cell: CellRef { x, y },
        }
    }

    #[test]
    fn empty_map_derives_an_all_zero_card() {
        let card = MapCard::derive(&empty_spec());
        assert_eq!(card.control_points, 0);
        assert_eq!(card.prop_counts, [0; 5]);
        assert_eq!(card.covered_cells, 0);
        assert_eq!(card.cover_permille, 0);
        assert_eq!(card.quadrant_cells, [0; 4]);
        assert_eq!(card.quadrant_permille, [0; 4]);
        assert!(card.spawn_zones.is_empty());
    }

    #[test]
    fn counts_tally_by_kind_and_cells_dedupe() {
        let mut spec = empty_spec();
        spec.control_points = vec![CellRef { x: 64, y: 64 }, CellRef { x: 10, y: 10 }];
        spec.cover_props = vec![
            prop(CoverPropKind::Crate, 1, 1),
            prop(CoverPropKind::Crate, 1, 1), // same cell: two props, one covered cell
            prop(CoverPropKind::Tree, 100, 100),
            prop(CoverPropKind::Rock, 2, 120),
        ];
        let card = MapCard::derive(&spec);
        assert_eq!(card.control_points, 2);
        assert_eq!(card.prop_counts, [2, 1, 1, 0, 0]);
        assert_eq!(card.prop_count(CoverPropKind::Crate), 2);
        assert_eq!(card.prop_count(CoverPropKind::Turret), 0);
        assert_eq!(card.covered_cells, 3);
        // (1,1) low/low, (100,100) high/high, (2,120) low-x/high-y.
        assert_eq!(card.quadrant_cells, [1, 0, 1, 1]);
        assert_eq!(card.quadrant_cells.iter().sum::<u32>(), card.covered_cells);
    }

    #[test]
    fn density_permille_is_exact_integer_math() {
        // Fill quadrant 0 (low-x/low-y) wall to wall: (GRID/2)^2 covered cells is a quarter of
        // the field — 250 permille overall, 1000 permille in that quadrant, 0 elsewhere.
        let half = (GRID / 2) as i32;
        let mut spec = empty_spec();
        spec.cover_props = (0..half)
            .flat_map(|y| (0..half).map(move |x| prop(CoverPropKind::Rock, x, y)))
            .collect();
        let card = MapCard::derive(&spec);
        assert_eq!(card.covered_cells, (GRID * GRID / 4) as u32);
        assert_eq!(card.cover_permille, 250);
        assert_eq!(card.quadrant_cells, [(GRID * GRID / 4) as u32, 0, 0, 0]);
        assert_eq!(card.quadrant_permille, [1000, 0, 0, 0]);
    }

    #[test]
    fn zone_summaries_sort_corners_and_count_cells() {
        let mut spec = empty_spec();
        // Corners deliberately unsorted — the summary reports the sorted extent.
        spec.spawn_zones = vec![SpawnZoneSpec {
            name: "z".into(),
            min: CellRef { x: 14, y: 60 },
            max: CellRef { x: 8, y: 68 },
        }];
        let card = MapCard::derive(&spec);
        assert_eq!(card.spawn_zones.len(), 1);
        let z = &card.spawn_zones[0];
        assert_eq!(z.name, "z");
        assert_eq!(z.lo, (8, 60));
        assert_eq!(z.hi, (14, 68));
        assert_eq!(z.cells, 7 * 9);
    }

    #[test]
    fn crossroads_card_is_internally_consistent_and_pinned() {
        let spec = library_spec("crossroads").expect("shipped library map");
        let card = MapCard::derive(&spec);

        // Internal consistency: every count re-derives from the spec's own vec lengths, and
        // the quadrant split partitions the covered cells.
        assert_eq!(card.control_points as usize, spec.control_points.len());
        assert_eq!(
            card.prop_counts.iter().sum::<u32>() as usize,
            spec.cover_props.len()
        );
        assert!(card.covered_cells as usize <= spec.cover_props.len());
        assert_eq!(card.quadrant_cells.iter().sum::<u32>(), card.covered_cells);
        assert_eq!(card.spawn_zones.len(), spec.spawn_zones.len());
        for (summary, zone) in card.spawn_zones.iter().zip(&spec.spawn_zones) {
            assert_eq!(summary.name, zone.name);
            let (lo_x, lo_y, hi_x, hi_y) = zone.extent();
            assert_eq!(summary.lo, (lo_x, lo_y));
            assert_eq!(summary.hi, (hi_x, hi_y));
            assert_eq!(summary.cells as usize, zone.cells().count());
        }

        // Pinned exact values (the Kotlin twin mirrors these verbatim — D79): three posts, six
        // props on six distinct cells (2 crate / 1 rock / 2 barricade / 1 turret), density
        // rounding to 0 permille of the 16384-cell field, and the two 7x9 deploy zones.
        assert_eq!(card.control_points, 3);
        assert_eq!(card.prop_counts, [2, 0, 1, 2, 1]);
        assert_eq!(card.covered_cells, 6);
        assert_eq!(card.cover_permille, 0);
        assert_eq!(card.quadrant_cells, [1, 2, 1, 2]);
        assert_eq!(card.quadrant_permille, [0, 0, 0, 0]);
        assert_eq!(card.spawn_zones.len(), 2);
        assert_eq!(card.spawn_zones[0].name, "player");
        assert_eq!(card.spawn_zones[0].lo, (8, 60));
        assert_eq!(card.spawn_zones[0].hi, (14, 68));
        assert_eq!(card.spawn_zones[0].cells, 63);
        assert_eq!(card.spawn_zones[1].name, "enemy");
        assert_eq!(card.spawn_zones[1].lo, (114, 60));
        assert_eq!(card.spawn_zones[1].hi, (120, 68));
        assert_eq!(card.spawn_zones[1].cells, 63);
    }
}
