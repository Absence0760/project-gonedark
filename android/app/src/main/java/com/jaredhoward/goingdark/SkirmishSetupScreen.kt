package com.jaredhoward.goingdark

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
 * pointer notes the gunsmith carries in). Pure presentation over the hoisted [SkirmishSetup]:
 * every control is a callback into the pure seam at the host, so this is the exempt glue and
 * [SkirmishSetupTest] pins the decisions it renders.
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
