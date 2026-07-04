package com.jaredhoward.goingdark

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for the map-card mirror (MapCard.kt) — the Kotlin twin of the desktop card
 * seams' tests (`app/src/shell/tests.rs`, the "skirmish map card" block) plus the D79 mirror
 * guards: every pinned metric must re-derive from the mirrored geometry (so the two halves of a
 * card can never drift apart), and every card id must be engine-embedded ([KNOWN_MAP_IDS]).
 * The Compose card consuming this is device-gated chrome and exempt; every decision it renders
 * is pinned here off-device.
 */
class MapCardTest {

    // ---- The metric lines (mirror the desktop `map_card_metric_lines` verbatim) --------------

    @Test
    fun crossroads_metric_lines_read_the_pinned_desktop_card() {
        // The full formatted card for the one shipped library map — pinned VERBATIM against the
        // desktop `crossroads_metric_lines_read_the_pinned_card` strings (D79: byte-identical
        // lines on both shells).
        assertEquals(
            listOf(
                "Control points: 3",
                "Cover: 6 props on 6 cells -- 0/1000 of the field",
                "Cover by quadrant (cells): 1 / 2 / 1 / 2",
                "Spawn zones: 2 -- player 7x9, enemy 7x9",
            ),
            mapCardMetricLines(mapCards.getValue("crossroads")),
        )
    }

    @Test
    fun metric_lines_handle_a_zoneless_card() {
        // A minimal card still formats fully — the zone line says so explicitly rather than
        // trailing an empty list (mirrors the desktop `metric_lines_handle_a_zoneless_card`).
        val bare = MapCard(
            controlPoints = emptyList(),
            props = emptyList(),
            coveredCells = 0,
            coverPermille = 0,
            quadrantCells = listOf(0, 0, 0, 0),
            spawnZones = emptyList(),
        )
        val lines = mapCardMetricLines(bare)
        assertEquals("Control points: 0", lines[0])
        assertEquals("Cover: 0 props on 0 cells -- 0/1000 of the field", lines[1])
        assertEquals("Spawn zones: none", lines[3])
    }

    // ---- The mirror guards (the pinned constants must cohere with the mirrored geometry) -----

    @Test
    fun every_card_re_derives_its_pinned_metrics_from_the_mirrored_geometry() {
        // The D79 drift guard: a card carries the geometry (mirrored from the *.map.ron) AND the
        // engine's pinned derived metrics — if someone edits the RON mirror without re-pinning
        // (or vice versa), the halves disagree and this fails. Same integer math as
        // `MapCard::derive`: covered cells dedupe props by cell, the quadrant split partitions
        // them, density is integer permille of the MAP_GRID^2 field.
        assertTrue(mapCards.isNotEmpty())
        for ((id, card) in mapCards) {
            val distinct = card.props.map { it.cell }.toSet()
            assertEquals("covered cells of $id", distinct.size, card.coveredCells)
            assertEquals(
                "cover permille of $id",
                card.coveredCells * 1000 / (MAP_GRID * MAP_GRID),
                card.coverPermille,
            )
            assertEquals("quadrant split arity of $id", 4, card.quadrantCells.size)
            val quads = IntArray(4)
            for (cell in distinct) quads[quadrantOf(cell.x, cell.y)]++
            assertEquals("quadrant split of $id", quads.toList(), card.quadrantCells)
            assertEquals(
                "quadrants partition the covered cells of $id",
                card.coveredCells,
                card.quadrantCells.sum(),
            )
            // Per-kind counts partition the prop list (the legend can't over- or under-count).
            assertEquals(
                "kind counts of $id",
                card.props.size,
                CoverKind.entries.sumOf { kind -> card.props.count { it.kind == kind } },
            )
        }
    }

    @Test
    fun every_card_cell_is_on_the_playfield_and_every_zone_extent_is_sorted() {
        for ((id, card) in mapCards) {
            for (cell in card.controlPoints + card.props.map { it.cell }) {
                assertTrue("cell $cell of $id in [0, $MAP_GRID)", cell.x in 0 until MAP_GRID)
                assertTrue("cell $cell of $id in [0, $MAP_GRID)", cell.y in 0 until MAP_GRID)
            }
            for (zone in card.spawnZones) {
                assertTrue("zone ${zone.name} of $id is sorted", zone.loX <= zone.hiX)
                assertTrue("zone ${zone.name} of $id is sorted", zone.loY <= zone.hiY)
                assertTrue("zone ${zone.name} of $id in range", zone.loX in 0 until MAP_GRID)
                assertTrue("zone ${zone.name} of $id in range", zone.hiX in 0 until MAP_GRID)
                assertTrue("zone ${zone.name} of $id in range", zone.loY in 0 until MAP_GRID)
                assertTrue("zone ${zone.name} of $id in range", zone.hiY in 0 until MAP_GRID)
                assertEquals(
                    "zone ${zone.name} of $id extent",
                    zone.width * zone.height,
                    zone.cells,
                )
                assertTrue("zone ${zone.name} of $id is non-empty", zone.cells > 0)
            }
        }
    }

    @Test
    fun card_ids_are_engine_embedded_and_every_library_tile_has_a_card() {
        // A card for a map the engine doesn't embed is a lie — every card id must be in
        // KNOWN_MAP_IDS (the D79 twin of the engine's library test)…
        for (id in mapCards.keys) {
            assertTrue("card id $id must be engine-embedded", id in KNOWN_MAP_IDS)
        }
        // …and every shipped library-map tile must find its card, so the picker never renders
        // the defensive "Map unavailable." fallback for a battlefield we actually ship.
        for (bf in shellBattlefields) {
            bf.mapId?.let {
                assertTrue("library tile ${bf.id} has no map card", it in mapCards)
            }
        }
    }

    @Test
    fun crossroads_card_pins_the_engine_values() {
        // The engine's pinned crossroads card (engine/src/map_card.rs), mirrored number for
        // number: three posts, six props on six distinct cells (2 crate / 1 rock / 2 barricade /
        // 1 turret), and the two 7x9 deploy zones in authored order.
        val card = mapCards.getValue("crossroads")
        assertEquals(3, card.controlPoints.size)
        assertEquals(
            listOf(2, 0, 1, 2, 1),
            CoverKind.entries.map { kind -> card.props.count { it.kind == kind } },
        )
        assertEquals(6, card.coveredCells)
        assertEquals(0, card.coverPermille)
        assertEquals(listOf(1, 2, 1, 2), card.quadrantCells)
        assertEquals(2, card.spawnZones.size)
        val player = card.spawnZones[0]
        assertEquals("player", player.name)
        assertEquals(8 to 60, player.loX to player.loY)
        assertEquals(14 to 68, player.hiX to player.hiY)
        assertEquals(63, player.cells)
        val enemy = card.spawnZones[1]
        assertEquals("enemy", enemy.name)
        assertEquals(114 to 60, enemy.loX to enemy.loY)
        assertEquals(120 to 68, enemy.hiX to enemy.hiY)
        assertEquals(63, enemy.cells)
    }

    // ---- The sketch mapping (mirror the desktop `cell_sketch_rect` tests) --------------------

    @Test
    fun sketch_cell_mapping_covers_the_panel_exactly() {
        // The 128-cell grid tiles the panel edge to edge: cell (0,0) starts at the panel origin,
        // the last cell ends at its far corner, and each cell is an even 1/128 slice.
        val first = cellSketchRect(256f, 256f, 0, 0)
        assertEquals(0f, first.left)
        assertEquals(0f, first.top)
        assertEquals(2f, first.width)
        assertEquals(2f, first.height)
        val last = cellSketchRect(256f, 256f, 127, 127)
        assertEquals(256f, last.right)
        assertEquals(256f, last.bottom)
    }

    @Test
    fun sketch_centre_cell_starts_at_the_panel_midpoint() {
        // Cell (64, 64) — the playfield centre cell — begins exactly at the panel's midpoint
        // (the grid splits at MAP_GRID/2, same as the card's quadrants).
        val centre = cellSketchRect(256f, 256f, 64, 64)
        assertEquals(128f, centre.left)
        assertEquals(128f, centre.top)
    }

    @Test
    fun sketch_mapping_scales_each_axis_of_a_non_square_panel() {
        // Each axis scales independently — a 128x64 panel stretches the field, it never
        // letterboxes or clips.
        val r = cellSketchRect(128f, 64f, 1, 1)
        assertEquals(1f, r.left)
        assertEquals(0.5f, r.top)
        assertEquals(1f, r.width)
        assertEquals(0.5f, r.height)
        val last = cellSketchRect(128f, 64f, 127, 127)
        assertEquals(128f, last.right)
        assertEquals(64f, last.bottom)
    }

    @Test
    fun zone_sketch_rect_unions_the_corner_cells() {
        // A zone's outline is the union of its lo and hi corner cells — the INCLUSIVE extent, so
        // the player 7x9 zone spans 7 cells wide even though hi-lo is 6.
        val zone = ZoneRect("player", loX = 8, loY = 60, hiX = 14, hiY = 68)
        val r = zoneSketchRect(256f, 256f, zone)
        assertEquals(16f, r.left) // 8 * 2px
        assertEquals(120f, r.top) // 60 * 2px
        assertEquals(14f, r.width) // 7 cells * 2px
        assertEquals(18f, r.height) // 9 cells * 2px
    }

    @Test
    fun sketch_rect_inflate_grows_every_side() {
        // The single-cell inflation the sketch applies so a one-cell prop stays visible.
        val r = SketchRect(10f, 20f, 2f, 2f).inflate(1f)
        assertEquals(9f, r.left)
        assertEquals(19f, r.top)
        assertEquals(4f, r.width)
        assertEquals(4f, r.height)
        assertEquals(11f, r.centerX) // inflation never moves the centre
        assertEquals(21f, r.centerY)
    }

    // ---- The quadrant split and hue/label picks ----------------------------------------------

    @Test
    fun quadrant_split_matches_the_desktop_order() {
        // The engine's `quadrant_of` order — 0 lo/lo, 1 hi/lo, 2 lo/hi, 3 hi/hi — with the
        // split boundary itself (x or y = 64) counting as "hi".
        assertEquals(0, quadrantOf(0, 0))
        assertEquals(1, quadrantOf(127, 0))
        assertEquals(2, quadrantOf(0, 127))
        assertEquals(3, quadrantOf(127, 127))
        assertEquals(0, quadrantOf(63, 63))
        assertEquals(3, quadrantOf(64, 64))
        assertEquals(1, quadrantOf(64, 63))
        assertEquals(2, quadrantOf(63, 64))
    }

    @Test
    fun zone_hues_pick_the_faction_reads() {
        // The player/enemy deploy zones read in the two faction hues (blue-vs-red on desktop,
        // primary-vs-error on this shell); any other authored name falls back to the muted hue.
        assertEquals(ZoneHue.Player, zoneHue(PLAYER_ZONE))
        assertEquals(ZoneHue.Enemy, zoneHue(ENEMY_ZONE))
        assertEquals(ZoneHue.Other, zoneHue("flank"))
        assertNotEquals(zoneHue(PLAYER_ZONE), zoneHue(ENEMY_ZONE))
    }

    @Test
    fun cover_kind_labels_mirror_the_desktop_legend() {
        // The legend chips, pinned against the desktop `prop_kind_label` strings — and pairwise
        // distinct ASCII, so the colour key is only ever honest.
        assertEquals(
            listOf("CRATE", "TREE", "ROCK", "BARRICADE", "TURRET"),
            CoverKind.entries.map { it.label() },
        )
        assertEquals(
            CoverKind.entries.size,
            CoverKind.entries.map { it.label() }.toSet().size,
        )
        for (kind in CoverKind.entries) {
            assertTrue(kind.label().all { it.code in 32..126 })
        }
    }

    @Test
    fun cover_kind_swatches_are_pairwise_distinct_and_pin_the_renderer_palette() {
        // The Android twin of the desktop `prop_kind_swatches_are_pairwise_distinct` test, plus
        // the D79 mirror pin: these hex values are the `render/src/theme.rs` constants
        // (DATA_RESOURCE / DATA_TERRITORY / NEUTRAL / BONE / ALERT_WARN) through the same rgb8
        // rounding the desktop card uses — if the renderer palette moves, this fails instead of
        // the two shells silently drifting apart.
        assertEquals(0xFFF2BD52, coverKindArgb(CoverKind.Crate))
        assertEquals(0xFF85D175, coverKindArgb(CoverKind.Tree))
        assertEquals(0xFF8C8F9E, coverKindArgb(CoverKind.Rock))
        assertEquals(0xFFE7ECEF, coverKindArgb(CoverKind.Barricade))
        assertEquals(0xFFFF9E26, coverKindArgb(CoverKind.Turret))
        assertEquals(
            CoverKind.entries.size,
            CoverKind.entries.map { coverKindArgb(it) }.toSet().size,
        )
    }
}
