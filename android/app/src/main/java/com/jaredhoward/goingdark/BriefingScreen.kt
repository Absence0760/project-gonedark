package com.jaredhoward.goingdark

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
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
 * The campaign **briefing** screen — Compose shell parity Tier 2 (the campaign row of
 * `docs/plans/compose-shell-parity.md`), the native twin of the desktop egui briefing
 * (`app/src/shell.rs::briefing_ui`). Pure presentation: it shows the mission name + briefing copy, a
 * difficulty cycler, and DEPLOY / BACK, reporting every interaction through callbacks. It carries no
 * game state and never touches the sim.
 *
 * **Stateless / hoisted state**, exactly like [TitleScreen] / [SettingsScreen] / [MissionSelectScreen]:
 * the host owns the selected [difficulty] and advances it (via [Difficulty.next]) when
 * [onCycleDifficulty] fires — mirroring the desktop's pure `apply_briefing_action` seam, where
 * `CycleDifficulty` stays on screen and `Deploy`/`Back` are transitions. The chosen tier is recorded
 * against the campaign on a win; the host (MainActivity) owns that and the Briefing → gunsmith hop,
 * not this screen.
 */
@Composable
fun BriefingScreen(
    node: MissionNode,
    progress: NodeProgress,
    difficulty: Difficulty,
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
                text = node.name.uppercase(),
                color = MaterialTheme.colorScheme.onBackground,
                fontSize = 28.sp,
                letterSpacing = 6.sp,
                textAlign = TextAlign.Center,
            )

            Spacer(Modifier.height(20.dp))

            Column(
                modifier = Modifier.widthIn(max = 440.dp).fillMaxWidth(),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    text = "BRIEFING",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 12.sp,
                    letterSpacing = 4.sp,
                )
                Text(
                    text = node.briefing,
                    color = MaterialTheme.colorScheme.onSurface,
                    fontSize = 15.sp,
                )

                Spacer(Modifier.height(12.dp))

                // The difficulty cycler: label + a tap-to-cycle control (the desktop's
                // CycleDifficulty action — the host advances the selection via Difficulty.next()).
                Row(
                    modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        text = "Difficulty",
                        color = MaterialTheme.colorScheme.onSurface,
                        fontSize = 15.sp,
                        modifier = Modifier.weight(1f),
                    )
                    OutlinedButton(
                        onClick = onCycleDifficulty,
                        modifier = Modifier.widthIn(min = 160.dp),
                    ) {
                        Text(difficulty.label().uppercase(), letterSpacing = 2.sp)
                    }
                }

                // The clear-status line — driven by the node's derived progress, the native twin of
                // the desktop briefing_ui's status label ("Cleared at X -- replay…" / "Not yet
                // cleared." / "Locked.").
                Text(
                    text = clearStatusLine(progress),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 12.sp,
                )
            }

            Spacer(Modifier.height(28.dp))

            // Bottom actions: DEPLOY launches at the selected tier, BACK returns to the hub.
            Column(
                modifier = Modifier.widthIn(max = 360.dp).fillMaxWidth(),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Button(
                    onClick = onDeploy,
                    modifier = Modifier.fillMaxWidth().height(54.dp),
                ) {
                    Text("DEPLOY", letterSpacing = 2.sp)
                }
                OutlinedButton(
                    onClick = onBack,
                    modifier = Modifier.fillMaxWidth().height(50.dp),
                ) {
                    Text("BACK", letterSpacing = 2.sp)
                }
            }
        }
    }
}

/**
 * The clear-status copy for a node, mirroring the desktop `briefing_ui` status label **verbatim**
 * (the "--" is the ASCII dash the egui default font renders). Kept a pure function so it is covered
 * by [CampaignProgressTest] rather than trapped in the device-gated composable.
 */
fun clearStatusLine(progress: NodeProgress): String = when (progress) {
    is NodeProgress.Cleared ->
        "Cleared at ${progress.best.label()} -- replay to raise your best."
    NodeProgress.Available -> "Not yet cleared."
    NodeProgress.Locked -> "Locked."
}

@Preview(showBackground = true, widthDp = 880, heightDp = 520)
@Composable
private fun BriefingScreenPreview() {
    GoingDarkTheme {
        BriefingScreen(
            node = campaignNodes.first(),
            progress = NodeProgress.Available,
            difficulty = Difficulty.Recruit,
            onCycleDifficulty = {},
            onDeploy = {},
            onBack = {},
        )
    }
}
