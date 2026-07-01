package com.jaredhoward.goingdark

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.jaredhoward.goingdark.ui.theme.GoingDarkTheme

/**
 * The **army-select** screen (US vs French) — the native Compose twin of the desktop egui
 * `army_select_ui` (`app/src/shell.rs`). A pre-deploy pick fielded at every match start via the shared
 * `Game::select_army` seam; it persists across launches. Pure presentation, modelled on
 * [ModeSelectScreen]: it renders the two combatant rosters ([Army.SELECTABLE]) as cards, highlights the
 * current [selected] pick, and reports edits through [onChoose] / [onConfirm]; the host ([MainActivity]'s
 * `Shell`) records the pick and threads it into the launch wire. Carries no game state, never the sim.
 *
 * Choosing an army is an in-place edit (stays on-screen so the two identities can be compared, mirroring
 * the desktop); CONFIRM commits and returns to the title. Stateless / hoisted — every action is a
 * callback, so it is device-agnostic and previewable without an Activity.
 */
@Composable
fun ArmySelectScreen(
    selected: Army,
    onChoose: (Army) -> Unit,
    onConfirm: () -> Unit,
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
                text = "SELECT ARMY",
                color = MaterialTheme.colorScheme.onBackground,
                fontSize = 30.sp,
                letterSpacing = 8.sp,
                textAlign = TextAlign.Center,
            )

            Spacer(Modifier.height(12.dp))

            Text(
                text = "The real-army roster you deploy as. Asymmetry is of feel, never of power.",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
                textAlign = TextAlign.Center,
                modifier = Modifier.widthIn(max = 440.dp).fillMaxWidth(),
            )

            Spacer(Modifier.height(24.dp))

            // The two roster cards, width-capped so they don't stretch across a wide landscape screen.
            Column(
                modifier = Modifier.widthIn(max = 440.dp).fillMaxWidth(),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                for (army in Army.SELECTABLE) {
                    ArmyCard(army = army, selected = army == selected, onClick = { onChoose(army) })
                }
            }

            Spacer(Modifier.height(28.dp))

            Column(
                modifier = Modifier.widthIn(max = 360.dp).fillMaxWidth(),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Button(
                    onClick = onConfirm,
                    modifier = Modifier.fillMaxWidth().height(54.dp),
                ) {
                    Text("CONFIRM", letterSpacing = 2.sp)
                }
                OutlinedButton(
                    onClick = onBack,
                    modifier = Modifier.fillMaxWidth().height(54.dp),
                ) {
                    Text("BACK", letterSpacing = 2.sp)
                }
            }
        }
    }
}

/**
 * One army card: the roster name over its one-line identity blurb. The current pick is drawn as a
 * filled [Button] (so the selection reads without relying on colour alone — a CVD-safe tell, matching
 * the desktop card's selected state); the others are neutral [OutlinedButton]s.
 */
@Composable
private fun ArmyCard(army: Army, selected: Boolean, onClick: () -> Unit) {
    val content: @Composable () -> Unit = {
        Column(
            modifier = Modifier.fillMaxWidth().padding(vertical = 6.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(
                text = if (selected) "${army.label().uppercase()}  ✓" else army.label().uppercase(),
                fontSize = 16.sp,
                letterSpacing = 2.sp,
            )
            Text(
                text = army.flavor(),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
            )
        }
    }
    if (selected) {
        Button(onClick = onClick, modifier = Modifier.fillMaxWidth()) { content() }
    } else {
        OutlinedButton(
            onClick = onClick,
            modifier = Modifier.fillMaxWidth(),
            colors = ButtonDefaults.outlinedButtonColors(
                contentColor = MaterialTheme.colorScheme.onSurface,
            ),
        ) { content() }
    }
}

@Preview(showBackground = true, widthDp = 880, heightDp = 520)
@Composable
private fun ArmySelectScreenPreview() {
    GoingDarkTheme {
        ArmySelectScreen(
            selected = Army.Us,
            onChoose = {},
            onConfirm = {},
            onBack = {},
        )
    }
}
