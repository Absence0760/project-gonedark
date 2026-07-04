package com.jaredhoward.goingdark

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.jaredhoward.goingdark.ui.theme.GoingDarkTheme

/**
 * The **skirmish match-setup** screen (`docs/modes.md` §3) — the title's SKIRMISH door, the Compose
 * twin of the desktop `skirmish_setup_ui` (`app/src/shell/skirmish.rs`) and the close of parity
 * §12 item 6. Battlefield tiles, the two army cyclers, the opponent-tier cycler with the 4-pip
 * ladder, then DEPLOY / BACK — in the order a player thinks (map, army, opponent; the loadout
 * pointer notes the gunsmith carries in). A selected library map additionally shows its **map
 * card** (`modes.md` §3's picker preview, shipped v1): the Canvas sketch of the mirrored
 * [MapCard] geometry beside its metric lines and cover-kind colour key — the Compose twin of the
 * desktop `map_card_panel`; a scene tile shows the one-line no-card note instead. Pure
 * presentation over the hoisted [SkirmishSetup]: every control is a callback into the pure seam
 * at the host, so this is the exempt glue and [SkirmishSetupTest]/[MapCardTest] pin the
 * decisions it renders.
 */
@Composable
fun SkirmishSetupScreen(
    setup: SkirmishSetup,
    onChooseBattlefield: (Int) -> Unit,
    onCyclePlayerArmy: () -> Unit,
    onCycleEnemyArmy: () -> Unit,
    onCycleDifficulty: () -> Unit,
    onDeploy: () -> Unit,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Surface(modifier = modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 40.dp, vertical = 32.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(
                text = "SKIRMISH",
                color = MaterialTheme.colorScheme.onBackground,
                fontSize = 30.sp,
                letterSpacing = 8.sp,
                textAlign = TextAlign.Center,
            )

            Spacer(Modifier.height(12.dp))

            // Mirrors the desktop skirmish_setup_ui copy verbatim.
            Text(
                text = "Pick your battle: the sandbox match against the honest enemy commander. " +
                    "No gating, no stakes -- rehearse a battlefield, an army, or a tier.",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
                textAlign = TextAlign.Center,
                modifier = Modifier.widthIn(max = 440.dp).fillMaxWidth(),
            )

            Spacer(Modifier.height(20.dp))

            Column(
                modifier = Modifier.widthIn(max = 440.dp).fillMaxWidth(),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                SectionLabel("BATTLEFIELD")
                val selected = clampBattlefield(setup.battlefield)
                for ((i, battlefield) in shellBattlefields.withIndex()) {
                    BattlefieldTile(
                        battlefield = battlefield,
                        selected = i == selected,
                        onClick = { onChooseBattlefield(i) },
                    )
                }

                SectionLabel("MAP CARD")
                MapCardPanel(shellBattlefields[selected])

                Spacer(Modifier.height(6.dp))

                SectionLabel("FORCES")
                CycleRow("Your army", setup.playerArmy.label().uppercase(), onCyclePlayerArmy)
                CycleRow("Enemy army", setup.enemyArmy.label().uppercase(), onCycleEnemyArmy)
                Caption(
                    "Asymmetry is of flavour and feel, never of power. Your gunsmith loadout " +
                        "carries in -- edit it under Settings.",
                )

                Spacer(Modifier.height(6.dp))

                SectionLabel("OPPONENT")
                // The D83 tier — commander band + situation modifiers, exactly the campaign replay
                // vocabulary (a harder tier is a better commander, never an omniscient one).
                CycleRow("Difficulty", setup.difficulty.label().uppercase(), onCycleDifficulty)
                // The briefing's 4-pip ladder (Recruit -> Elite), so the cycle reads as "n of 4".
                Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    for (d in Difficulty.entries) {
                        val filled = d <= setup.difficulty
                        Box(
                            Modifier
                                .size(width = 30.dp, height = 4.dp)
                                .background(
                                    color = if (filled) {
                                        MaterialTheme.colorScheme.primary
                                    } else {
                                        MaterialTheme.colorScheme.outlineVariant
                                    },
                                    shape = RoundedCornerShape(2.dp),
                                ),
                        )
                    }
                }
                Caption(
                    "Difficulty reshapes the situation -- a sharper commander, a faster enemy " +
                        "reinforcement drip -- never the balance numbers.",
                )
            }

            Spacer(Modifier.height(28.dp))

            Column(
                modifier = Modifier.widthIn(max = 360.dp).fillMaxWidth(),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Button(
                    onClick = onDeploy,
                    modifier = Modifier.fillMaxWidth().height(54.dp),
                ) {
                    Text("DEPLOY", letterSpacing = 2.sp)
                }
                Spacer(Modifier.height(8.dp))
                // BACK stays the quiet exit — DEPLOY is the genuine primary action here (the
                // desktop's Primary/Tertiary emphasis split).
                TextButton(
                    onClick = onBack,
                    modifier = Modifier.fillMaxWidth().height(48.dp),
                ) {
                    Text("BACK", letterSpacing = 2.sp)
                }
            }
        }
    }
}

/** The small letter-spaced section heading (BATTLEFIELD / FORCES / OPPONENT). */
@Composable
private fun SectionLabel(text: String) {
    Text(
        text = text,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        fontSize = 11.sp,
        letterSpacing = 2.sp,
    )
}

/** A muted explanatory caption under a control group. */
@Composable
private fun Caption(text: String) {
    Text(
        text = text,
        color = MaterialTheme.colorScheme.outline,
        fontSize = 12.sp,
    )
}

/**
 * One battlefield tile: the battle name (accented when picked, with a SELECTED trailing label —
 * legible beyond colour alone) over its one-line blurb; a library-map entry wears a muted
 * MAP LIBRARY label (the D102 manifest entries beside the standing battles — informational, never
 * a second tap target). Tapping any tile picks it; the clamping decision is the pure
 * [clampBattlefield] seam.
 */
@Composable
private fun BattlefieldTile(battlefield: Battlefield, selected: Boolean, onClick: () -> Unit) {
    OutlinedButton(
        onClick = onClick,
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(vertical = 6.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Row(modifier = Modifier.fillMaxWidth()) {
                Text(
                    text = battlefield.name.uppercase(),
                    color = if (selected) {
                        MaterialTheme.colorScheme.primary
                    } else {
                        MaterialTheme.colorScheme.onSurface
                    },
                    fontSize = 16.sp,
                    letterSpacing = 2.sp,
                    modifier = Modifier.weight(1f),
                )
                if (battlefield.mapId != null) {
                    Text(
                        text = "MAP LIBRARY",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        fontSize = 11.sp,
                        letterSpacing = 2.sp,
                    )
                }
                if (selected) {
                    Text(
                        text = if (battlefield.mapId != null) "  SELECTED" else "SELECTED",
                        color = MaterialTheme.colorScheme.primary,
                        fontSize = 11.sp,
                        letterSpacing = 2.sp,
                    )
                }
            }
            Text(
                text = battlefield.blurb,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
            )
        }
    }
}

/** A two-column setup row — label flush-left, the cycling value as a tappable chip flush-right. */
@Composable
private fun CycleRow(label: String, value: String, onCycle: () -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = label,
            color = MaterialTheme.colorScheme.onSurface,
            fontSize = 14.sp,
            modifier = Modifier.weight(1f),
        )
        OutlinedButton(onClick = onCycle) {
            Text(value, letterSpacing = 2.sp, fontSize = 13.sp)
        }
    }
}

/**
 * The selected battlefield's map card — the Compose twin of the desktop `map_card_panel`
 * (`modes.md` §3 picker preview, shipped v1): for a library map, the [MapCardSketch] beside the
 * mirrored [MapCard] metric lines and the cover-kind colour key; for a code-seeded scene (or —
 * defensively, forbidden by [MapCardTest] — a map id without a card), the desktop's one-line
 * note. Glue — every decision it renders (the metric lines, the cell mapping, the zone hues,
 * the mirrored geometry itself) is the pure [mapCards] seam.
 */
@Composable
private fun MapCardPanel(battlefield: Battlefield) {
    val card = battlefield.mapId?.let { mapCards[it] }
    if (battlefield.mapId == null) {
        Caption("Code-seeded scene -- no map card.")
        return
    }
    if (card == null) {
        Caption("Map unavailable.")
        return
    }
    Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        MapCardSketch(card)
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            for (line in mapCardMetricLines(card)) {
                Text(
                    text = line,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 12.sp,
                )
            }
            Spacer(Modifier.height(4.dp))
            // The sketch's colour key — only the kinds this map actually fields.
            for (kind in CoverKind.entries) {
                val count = card.props.count { it.kind == kind }
                if (count == 0) continue
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Box(
                        Modifier
                            .size(10.dp)
                            .background(coverKindColor(kind), RoundedCornerShape(2.dp)),
                    )
                    Text(
                        text = "${kind.label()} x$count",
                        color = MaterialTheme.colorScheme.outline,
                        fontSize = 12.sp,
                    )
                }
            }
        }
    }
}

/** The sketch's side: 128 cells at ~1.3dp each — a schematic, not a minimap (the desktop's 168pt). */
private val SketchSide = 168.dp

/** A cover kind's sketch/legend swatch — the drift-guarded mirror lives in [coverKindArgb]. */
private fun coverKindColor(kind: CoverKind): Color = Color(coverKindArgb(kind))

/**
 * Draw the map-card sketch: the grid-bounds rect (ink ground, hairline rim), spawn zones as
 * outlines — the player zone in the scheme's primary (the shell's selected accent), the enemy's
 * in its error hue, any other authored name muted — cover props as kind-coloured filled cells
 * (inflated a pixel so a single cell stays visible at this scale), control points as primary
 * dot markers. Glue (needs a `DrawScope`) — the cell/zone mapping and the hue *decisions* are
 * the pure MapCard.kt seams ([cellSketchRect] / [zoneSketchRect] / [zoneHue]).
 */
@Composable
private fun MapCardSketch(card: MapCard) {
    val ground = MaterialTheme.colorScheme.background
    val rim = MaterialTheme.colorScheme.outlineVariant
    val playerHue = MaterialTheme.colorScheme.primary
    val enemyHue = MaterialTheme.colorScheme.error
    val otherHue = MaterialTheme.colorScheme.outline
    val postHue = MaterialTheme.colorScheme.primary
    Canvas(Modifier.size(SketchSide)) {
        val hairline = 1.dp.toPx()
        val corner = CornerRadius(4.dp.toPx())
        drawRoundRect(color = ground, cornerRadius = corner)
        drawRoundRect(color = rim, cornerRadius = corner, style = Stroke(hairline))
        for (zone in card.spawnZones) {
            val r = zoneSketchRect(size.width, size.height, zone)
            val hue = when (zoneHue(zone.name)) {
                ZoneHue.Player -> playerHue
                ZoneHue.Enemy -> enemyHue
                ZoneHue.Other -> otherHue
            }
            drawRect(
                color = hue,
                topLeft = Offset(r.left, r.top),
                size = Size(r.width, r.height),
                style = Stroke(hairline),
            )
        }
        for (prop in card.props) {
            val cell = cellSketchRect(size.width, size.height, prop.cell.x, prop.cell.y)
                .inflate(1.dp.toPx())
            drawRect(
                color = coverKindColor(prop.kind),
                topLeft = Offset(cell.left, cell.top),
                size = Size(cell.width, cell.height),
            )
        }
        for (cp in card.controlPoints) {
            val cell = cellSketchRect(size.width, size.height, cp.x, cp.y)
            drawCircle(
                color = postHue,
                radius = 3.dp.toPx(),
                center = Offset(cell.centerX, cell.centerY),
            )
        }
    }
}

@Preview(showBackground = true, widthDp = 880, heightDp = 720)
@Composable
private fun SkirmishSetupScreenPreview() {
    GoingDarkTheme {
        SkirmishSetupScreen(
            setup = SkirmishSetup(),
            onChooseBattlefield = {},
            onCyclePlayerArmy = {},
            onCycleEnemyArmy = {},
            onCycleDifficulty = {},
            onDeploy = {},
            onBack = {},
        )
    }
}
