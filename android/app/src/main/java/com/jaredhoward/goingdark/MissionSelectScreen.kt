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
 * The campaign **mission-select** screen — Compose shell parity Tier 2 (the campaign row of
 * `docs/plans/compose-shell-parity.md`), the native twin of the desktop egui mission-select
 * (`app/src/shell.rs::mission_select_ui` / `mission_tile`). Pure presentation: it renders an
 * immutable list of [MissionNode]s as a titled "OPERATIONS" list of tiles and reports a tap through
 * [onOpenNode]; it carries no game state and never touches the sim (the campaign model is host-side
 * data, never checksummed).
 *
 * **Stateless / hoisted state**, exactly like [TitleScreen] / [SettingsScreen]: every action is a
 * callback, so the composable is device-agnostic and previewable without an Activity. The
 * Campaign → MissionSelect → Briefing → gunsmith routing is the host's (MainActivity's) job, not
 * this screen's. Today the model ships one playable node; this list grows with the Rust campaign.
 */
@Composable
fun MissionSelectScreen(
    campaign: CampaignProgress,
    onOpenNode: (MissionNode) -> Unit,
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
                text = "OPERATIONS",
                color = MaterialTheme.colorScheme.onBackground,
                fontSize = 30.sp,
                letterSpacing = 8.sp,
                textAlign = TextAlign.Center,
            )

            Spacer(Modifier.height(12.dp))

            // The instructional subtitle — mirrors desktop's mission_select_ui copy verbatim.
            Text(
                text = "Clear an operation to open the next. A cleared operation can be replayed at " +
                    "a higher tier.",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
                textAlign = TextAlign.Center,
                modifier = Modifier.widthIn(max = 440.dp).fillMaxWidth(),
            )

            Spacer(Modifier.height(24.dp))

            // The mission list, width-capped so tiles don't stretch across a wide landscape screen.
            Column(
                modifier = Modifier.widthIn(max = 440.dp).fillMaxWidth(),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                for (node in campaign.nodes) {
                    val progress = campaign.progress(node.id)
                    MissionTile(
                        node = node,
                        progress = progress,
                        // A locked tile can't launch (the pure isPlayable gate, mirroring desktop's
                        // playable_node) — the tap is only wired for a playable node.
                        onClick = { if (progress.isPlayable) onOpenNode(node) },
                    )
                }
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
 * One mission tile: a status pill (LOCKED / AVAILABLE / CLEARED·tier) over the operation name and a
 * one-line teaser drawn from the briefing copy. Tapping a **playable** tile opens the briefing (the
 * desktop's `mission_tile` → `OpenNode`); a **locked** tile is `enabled = false` and un-tappable and
 * its text is dimmed — mirroring the desktop, which disables the button on a locked node. An
 * [OutlinedButton] so it reads as a quiet, tappable row in the list (the screens' established idiom).
 */
@Composable
private fun MissionTile(node: MissionNode, progress: NodeProgress, onClick: () -> Unit) {
    OutlinedButton(
        onClick = onClick,
        enabled = progress.isPlayable,
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(vertical = 6.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            StatusPill(progress)
            Text(
                text = node.name.uppercase(),
                // A locked tile dims its title (desktop uses MUTED for a non-playable node).
                color = if (progress.isPlayable) {
                    MaterialTheme.colorScheme.onSurface
                } else {
                    MaterialTheme.colorScheme.onSurfaceVariant
                },
                fontSize = 16.sp,
                letterSpacing = 2.sp,
            )
            Text(
                text = teaser(node.briefing),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
            )
        }
    }
}

/**
 * The per-node status pill — LOCKED / AVAILABLE / CLEARED · <tier> — the native twin of the desktop
 * `mission_tile`'s status label. The cleared pill names the best tier reached (the replay surface).
 */
@Composable
private fun StatusPill(progress: NodeProgress) {
    val text = when (progress) {
        NodeProgress.Locked -> "LOCKED"
        NodeProgress.Available -> "AVAILABLE"
        is NodeProgress.Cleared -> "CLEARED · ${progress.best.label().uppercase()}"
    }
    val color = when (progress) {
        NodeProgress.Locked -> MaterialTheme.colorScheme.onSurfaceVariant
        NodeProgress.Available -> MaterialTheme.colorScheme.primary
        is NodeProgress.Cleared -> MaterialTheme.colorScheme.onSurfaceVariant
    }
    Text(text = text, color = color, fontSize = 11.sp, letterSpacing = 2.sp)
}

/** First sentence (or a trimmed lead-in) of the briefing, for the tile's one-line teaser. */
private fun teaser(briefing: String): String {
    val firstSentence = briefing.substringBefore('.').trim()
    return if (firstSentence.isEmpty()) briefing.trim() else "$firstSentence."
}

@Preview(showBackground = true, widthDp = 880, heightDp = 520)
@Composable
private fun MissionSelectScreenPreview() {
    GoingDarkTheme {
        MissionSelectScreen(
            campaign = CampaignProgress(),
            onOpenNode = {},
            onBack = {},
        )
    }
}
