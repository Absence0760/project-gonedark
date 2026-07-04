package com.jaredhoward.goingdark

/**
 * The **map card** data for the skirmish picker (`docs/modes.md` §3's "see what you're getting
 * into", shipped v1) — the Kotlin mirror of the engine's derived `engine::map_card::MapCard`
 * plus the desktop shell's pure card seams (`app/src/shell/skirmish.rs`:
 * `map_card_metric_lines` / `cell_sketch_rect` / `zone_outline_color`). Kotlin cannot parse the
 * RON the engine embeds, so the geometry in [mapCards] is copied **verbatim** from the map's
 * `*.map.ron` source (D79 hand-mirrored — exactly how [shellBattlefields] mirrors the Rust
 * `BATTLEFIELDS`), and the metric fields are the engine's pinned derived card
 * (`engine/src/map_card.rs`, `crossroads_card_is_internally_consistent_and_pinned`).
 * [MapCardTest] guards the mirror both ways: the pinned metrics must re-derive from the
 * mirrored geometry, and every card id must be engine-embedded ([KNOWN_MAP_IDS]).
 *
 * **No Android imports** on purpose (the [SkirmishSetup] / [CampaignModel.kt] pattern): every
 * decision here is JVM-tested; the Compose sketch in [SkirmishSetupScreen] is the exempt glue.
 * Presentation data only, never sim state (the engine card's D34 rule) — floats appear only in
 * the sketch mapping below, which is render-side by construction.
 */

/** The playfield's cell grid side (`core::flow_field::GRID`) — every card covers 128x128 cells. */
const val MAP_GRID = 128

/**
 * The cover-prop kinds a `*.map.ron` can field, in the engine's `COVER_KINDS` declaration
 * order — the order the sketch's colour key lists them in.
 */
enum class CoverKind { Crate, Tree, Rock, Barricade, Turret }

/** The sketch legend's kind label. ASCII, uppercase — the chip convention. */
fun CoverKind.label(): String = name.uppercase()

/** One integer playfield cell (the RON `CellRef`), `[0, MAP_GRID)` per axis. */
data class MapCell(val x: Int, val y: Int)

/** One authored cover prop: its kind on its cell (the RON `CoverPropSpec`). */
data class PropCell(val kind: CoverKind, val cell: MapCell)

/**
 * One spawn zone on the card: the authored name plus the **sorted** inclusive cell extent —
 * the engine's `SpawnZoneSummary` shape (RON corners need not be authored sorted; these are).
 */
data class ZoneRect(val name: String, val loX: Int, val loY: Int, val hiX: Int, val hiY: Int) {
    /** Cell width of the inclusive extent. */
    val width: Int get() = hiX - loX + 1

    /** Cell height of the inclusive extent. */
    val height: Int get() = hiY - loY + 1

    /** Cell count of the inclusive extent (`width * height`). */
    val cells: Int get() = width * height
}

/**
 * Which grid quadrant a cell falls in, split at [MAP_GRID]`/2` per axis — the engine's
 * `quadrant_of` order: `0` = low-x/low-y, `1` = high-x/low-y, `2` = low-x/high-y,
 * `3` = high-x/high-y (x or y >= 64 is "hi"). Cell-space only — the card makes no compass
 * claim (which way is "north" is the renderer's business).
 */
fun quadrantOf(x: Int, y: Int): Int =
    (if (y >= MAP_GRID / 2) 2 else 0) + (if (x >= MAP_GRID / 2) 1 else 0)

/**
 * The picker card for one library map: the mirrored geometry (control-point cells, cover props
 * with kinds, spawn-zone rects — verbatim from the `*.map.ron` source) beside the engine's
 * pinned derived metrics. [coveredCells] counts **occupied cells**, not props (two props on one
 * cell lay one cell of cover — the engine card deduplicates before it divides);
 * [coverPermille] is that count as integer permille of the [MAP_GRID]² field;
 * [quadrantCells] splits it per [quadrantOf]'s index order.
 */
data class MapCard(
    /** The neutral control-point cells, in authored order. */
    val controlPoints: List<MapCell>,
    /** The authored cover props, in authored order. */
    val props: List<PropCell>,
    /** Distinct cells occupied by cover (props deduplicated by cell). */
    val coveredCells: Int,
    /** Cover density: occupied cells as integer permille of the [MAP_GRID]² field. */
    val coverPermille: Int,
    /** Occupied cover cells per quadrant (see [quadrantOf] for the index order); size 4. */
    val quadrantCells: List<Int>,
    /** One rect per authored spawn zone, in authored order. */
    val spawnZones: List<ZoneRect>,
)

/** The player deploy zone's authored name on every library map (the Rust `PLAYER_ZONE`). */
const val PLAYER_ZONE = "player"

/** The enemy deploy zone's authored name on every library map (the Rust `ENEMY_ZONE`). */
const val ENEMY_ZONE = "enemy"

/**
 * A spawn zone's outline hue on the sketch — the decision half of the desktop
 * `zone_outline_color`, kept pure (the Compose glue maps each case onto a Material scheme
 * slot). [Player] and [Enemy] are the two faction reads; any other authored name falls back
 * to the muted [Other].
 */
enum class ZoneHue { Player, Enemy, Other }

/** Pick a zone's [ZoneHue] by its authored name — mirrors the desktop `zone_outline_color`. */
fun zoneHue(name: String): ZoneHue = when (name) {
    PLAYER_ZONE -> ZoneHue.Player
    ENEMY_ZONE -> ZoneHue.Enemy
    else -> ZoneHue.Other
}

/**
 * A cover kind's sketch/legend swatch as ARGB — hand-mirrored from the canonical renderer
 * palette (`render/src/theme.rs` DATA_RESOURCE / DATA_TERRITORY / NEUTRAL / BONE /
 * ALERT_WARN, the same hues the desktop card's `prop_kind_color` picks; the TitleBackdrop
 * convention for hues the Material scheme has no slot for). Kept here, off the Compose glue,
 * so the mirror is drift-guarded like every other hand-mirrored value (pairwise-distinct +
 * pinned hex in MapCardTest).
 */
fun coverKindArgb(kind: CoverKind): Long = when (kind) {
    CoverKind.Crate -> 0xFFF2BD52 // DATA_RESOURCE — supply amber
    CoverKind.Tree -> 0xFF85D175 // DATA_TERRITORY — territory green
    CoverKind.Rock -> 0xFF8C8F9E // NEUTRAL — stone grey
    CoverKind.Barricade -> 0xFFE7ECEF // BONE — the built barricade
    CoverKind.Turret -> 0xFFFF9E26 // ALERT_WARN — the turret hard point
}

/**
 * Format a [MapCard]'s metrics as the card's caption lines — control points, cover (props,
 * occupied cells, density as `n/1000` of the field), the per-quadrant cell breakdown (the
 * asymmetry read), and the spawn zones with their cell extents. Pure formatting, ASCII only —
 * mirrors the desktop `map_card_metric_lines` output **verbatim** ([MapCardTest] pins the
 * crossroads lines against the desktop test's strings).
 */
fun mapCardMetricLines(card: MapCard): List<String> {
    val lines = mutableListOf(
        "Control points: ${card.controlPoints.size}",
        "Cover: ${card.props.size} props on ${card.coveredCells} cells -- " +
            "${card.coverPermille}/1000 of the field",
        "Cover by quadrant (cells): ${card.quadrantCells.joinToString(" / ")}",
    )
    lines += if (card.spawnZones.isEmpty()) {
        "Spawn zones: none"
    } else {
        "Spawn zones: ${card.spawnZones.size} -- " +
            card.spawnZones.joinToString(", ") { "${it.name} ${it.width}x${it.height}" }
    }
    return lines
}

/**
 * A panel-local rectangle on the sketch canvas, in pixels — plain floats so the seam carries no
 * Android import (the Compose glue turns it into `Offset`/`Size` at draw time). Origin is the
 * panel's top-left corner.
 */
data class SketchRect(val left: Float, val top: Float, val width: Float, val height: Float) {
    val right: Float get() = left + width
    val bottom: Float get() = top + height
    val centerX: Float get() = left + width / 2f
    val centerY: Float get() = top + height / 2f

    /** The rect grown by [by] pixels on every side — the sketch's single-cell inflation. */
    fun inflate(by: Float): SketchRect =
        SketchRect(left - by, top - by, width + 2f * by, height + 2f * by)
}

/**
 * The panel-local rect of one playfield cell inside a sketch panel of the given pixel size: a
 * linear map of the [MAP_GRID]x[MAP_GRID] cell space onto the panel (x right, y down — cell
 * `(0, 0)` is the panel's top-left corner; the card makes no compass claim). Each axis scales
 * independently, so a non-square panel simply stretches the field. Mirrors the desktop
 * `cell_sketch_rect` — the sketch's one piece of math, unit-tested the same way (corners, the
 * centre cell, a non-square panel).
 */
fun cellSketchRect(panelWidth: Float, panelHeight: Float, x: Int, y: Int): SketchRect {
    val cellW = panelWidth / MAP_GRID
    val cellH = panelHeight / MAP_GRID
    return SketchRect(x * cellW, y * cellH, cellW, cellH)
}

/**
 * The panel-local rect of a whole spawn zone: the union of its lo and hi corner cells (the
 * inclusive extent, so the outline encloses every deployable cell) — exactly how the desktop
 * sketch frames a zone.
 */
fun zoneSketchRect(panelWidth: Float, panelHeight: Float, zone: ZoneRect): SketchRect {
    val lo = cellSketchRect(panelWidth, panelHeight, zone.loX, zone.loY)
    val hi = cellSketchRect(panelWidth, panelHeight, zone.hiX, zone.hiY)
    return SketchRect(lo.left, lo.top, hi.right - lo.left, hi.bottom - lo.top)
}

/**
 * The per-map cards, keyed by library-map id — one entry per shipped [KNOWN_MAP_IDS] map, so a
 * second library map is an **added entry, never new code**. Geometry is mirrored verbatim from
 * the map's `*.map.ron` source; the metric fields are the engine's pinned derived card.
 * [MapCardTest] re-derives every metric from the geometry so the two halves can never drift
 * apart, and pins the crossroads values against the engine's own pinned card.
 */
val mapCards: Map<String, MapCard> = mapOf(
    // maps/crossroads.map.ron — three posts strung about the centre junction (symmetric about
    // y = 64), a light crate nest + barricades/a boulder bracketing the centre post, and the
    // two opposing 7x9 deploy zones (west vs east). Pinned engine card:
    // engine/src/map_card.rs `crossroads_card_is_internally_consistent_and_pinned`.
    "crossroads" to MapCard(
        controlPoints = listOf(
            MapCell(x = 64, y = 64),
            MapCell(x = 64, y = 92),
            MapCell(x = 64, y = 36),
        ),
        props = listOf(
            PropCell(CoverKind.Crate, MapCell(x = 61, y = 66)),
            PropCell(CoverKind.Crate, MapCell(x = 67, y = 66)),
            PropCell(CoverKind.Barricade, MapCell(x = 61, y = 62)),
            PropCell(CoverKind.Barricade, MapCell(x = 67, y = 62)),
            PropCell(CoverKind.Rock, MapCell(x = 64, y = 70)),
            PropCell(CoverKind.Turret, MapCell(x = 64, y = 58)),
        ),
        coveredCells = 6,
        coverPermille = 0,
        quadrantCells = listOf(1, 2, 1, 2),
        spawnZones = listOf(
            ZoneRect(PLAYER_ZONE, loX = 8, loY = 60, hiX = 14, hiY = 68),
            ZoneRect(ENEMY_ZONE, loX = 114, loY = 60, hiX = 120, hiY = 68),
        ),
    ),
)
