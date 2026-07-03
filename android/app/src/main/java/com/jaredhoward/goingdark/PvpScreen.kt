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
 * The **PvP staging** screen (D101) — the title's PvP door, the Compose twin of the desktop
 * `pvp_ui` (`app/src/shell/pvp.rs`). Pure presentation over the [pvpQueues] table: the three
 * queues in `modes.md` §5 build order (none joinable pre-net — the pure [queueJoinable] seam),
 * the §4a pre-queue identity line (the persisted army pick, read-only here), then BACK — the
 * screen's only live control. Honest by construction: the copy says the net layer is what's
 * missing, and nothing on the screen pretends otherwise.
 *
 * Stateless / hoisted, like the sibling shell screens — every action is a callback, so it is
 * device-agnostic and previewable without an Activity.
 */
@Composable
fun PvpScreen(
    playerArmy: Army,
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
                text = "PVP",
                color = MaterialTheme.colorScheme.onBackground,
                fontSize = 30.sp,
                letterSpacing = 8.sp,
                textAlign = TextAlign.Center,
            )

            Spacer(Modifier.height(12.dp))

            // Mirrors the desktop pvp_ui copy verbatim (minus its egui `--` dash convention).
            Text(
                text = "Live commanders over lockstep -- the divided-attention mind game against " +
                    "a human. The net layer lands in Phase 3; until it does, no queue is joinable.",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
                textAlign = TextAlign.Center,
                modifier = Modifier.widthIn(max = 440.dp).fillMaxWidth(),
            )

            Spacer(Modifier.height(24.dp))

            // The queue list, width-capped like every tile list in the shell.
            Column(
                modifier = Modifier.widthIn(max = 440.dp).fillMaxWidth(),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                for (queue in pvpQueues) {
                    QueueTile(queue)
                }
            }

            Spacer(Modifier.height(20.dp))

            // The §4a pre-queue identity line: read-only — the pick is edited on the army-select
            // screen (and the loadout in the gunsmith), exactly like the desktop card.
            Column(
                modifier = Modifier.widthIn(max = 440.dp).fillMaxWidth(),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Text(
                    text = "YOU QUEUE AS",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 11.sp,
                    letterSpacing = 2.sp,
                )
                Text(
                    text = playerArmy.label().uppercase(),
                    color = MaterialTheme.colorScheme.onSurface,
                    fontSize = 16.sp,
                    letterSpacing = 2.sp,
                )
                Text(
                    text = "Your army and gunsmith loadout travel into every queue. Change them " +
                        "under ARMY and Settings on the title.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 13.sp,
                )
            }

            Spacer(Modifier.height(28.dp))

            Column(
                modifier = Modifier.widthIn(max = 360.dp).fillMaxWidth(),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Button(
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
 * One queue tile: the queue name beside its build-order status label, over the one-line blurb.
 * Disabled whenever [queueJoinable] says so (today: always), so the row renders as information,
 * never as a dead button that swallows taps — the joinability decision is the pure seam, this is
 * the exempt Compose glue.
 */
@Composable
private fun QueueTile(queue: PvpQueue) {
    OutlinedButton(
        onClick = {},
        enabled = queueJoinable(queue),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(vertical = 6.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Row(modifier = Modifier.fillMaxWidth()) {
                Text(
                    text = queue.name.uppercase(),
                    color = MaterialTheme.colorScheme.onSurface,
                    fontSize = 16.sp,
                    letterSpacing = 2.sp,
                    modifier = Modifier.weight(1f),
                )
                Text(
                    text = queue.status,
                    // FIRST UP reads in the accent colour (the next thing that becomes real);
                    // the rest stay muted — the desktop chip-colour rule.
                    color = if (queue.status == "FIRST UP") {
                        MaterialTheme.colorScheme.primary
                    } else {
                        MaterialTheme.colorScheme.onSurfaceVariant
                    },
                    fontSize = 11.sp,
                    letterSpacing = 2.sp,
                )
            }
            Text(
                text = queue.blurb,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
            )
        }
    }
}

@Preview(showBackground = true, widthDp = 880, heightDp = 520)
@Composable
private fun PvpScreenPreview() {
    GoingDarkTheme {
        PvpScreen(
            playerArmy = Army.Us,
            onBack = {},
        )
    }
}
