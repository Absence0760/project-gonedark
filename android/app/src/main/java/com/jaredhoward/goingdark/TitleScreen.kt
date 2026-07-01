package com.jaredhoward.goingdark

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
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
 * The landing / title screen — D32 app-shell surface 1 ("Boot & title"). Pure presentation: the
 * title, a tagline, the top-level play-mode split, the utility/secondary actions, and a build/version
 * stamp. It carries **no** game state and never touches the sim; the play-mode buttons hand off to the
 * engine through the host (MainActivity), the one place the shell crosses into the shared core (via
 * the `core::shell` seam, D34).
 *
 * Mirrors the desktop egui title (`app/src/shell.rs`): the same top-level split — **CAMPAIGN / PvE /
 * PvP** — plus **SETTINGS / PROFILE / FIELD MANUAL** and **QUIT**. The click→route decision lives in
 * the pure [resolveTitleAction] seam (D79), so this composable is host-agnostic, previewable without
 * an Activity, and emits only callbacks. Behind the chrome sits the animated Compose-native
 * [TitleBackdrop] (D78 option 1) — a 2D motif, deliberately *not* the desktop's live 3D scene.
 *
 * Actions are passed in as callbacks so the screen stays decoupled from the host nav graph. PvE and
 * PvP both open the gunsmith and Deploy into Skirmish today (their mode divergence — PvP match setup —
 * is Q5/Phase-3 work the host owns, not this screen).
 */
@Composable
fun TitleScreen(
    versionStamp: String,
    onCampaign: () -> Unit,
    onPve: () -> Unit,
    onPvp: () -> Unit,
    onSettings: () -> Unit,
    onProfile: () -> Unit,
    onArmy: () -> Unit,
    onAbout: () -> Unit,
    onQuit: () -> Unit,
    modifier: Modifier = Modifier,
) {
    // The animated backdrop sits behind everything; the content Column stacks over it. A Box (not a
    // Surface) so the backdrop's own dark field shows through — the backdrop owns the background fill.
    Box(modifier = modifier.fillMaxSize()) {
        TitleBackdrop(Modifier.fillMaxSize())

        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(horizontal = 40.dp, vertical = 32.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Spacer(Modifier.weight(1.2f))

            Text(
                text = "GOING DARK",
                color = MaterialTheme.colorScheme.onBackground,
                fontSize = 46.sp,
                letterSpacing = 10.sp,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(10.dp))
            Text(
                text = "COMMAND · EMBODY",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
                letterSpacing = 5.sp,
                textAlign = TextAlign.Center,
            )

            Spacer(Modifier.weight(1f))

            // The action column, width-capped so buttons don't stretch across a wide landscape screen.
            Column(
                modifier = Modifier.widthIn(max = 360.dp).fillMaxWidth(),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                // The top-level play-mode split, mirroring the desktop title. CAMPAIGN is the one
                // filled call-to-action; PvE / PvP are neutral secondaries (their mode divergence is
                // future work — see resolveTitleAction).
                Button(
                    onClick = onCampaign,
                    modifier = Modifier.fillMaxWidth().height(54.dp),
                ) {
                    Text("CAMPAIGN", letterSpacing = 2.sp)
                }
                OutlinedButton(
                    onClick = onPve,
                    modifier = Modifier.fillMaxWidth().height(54.dp),
                ) {
                    Text("PvE", letterSpacing = 2.sp)
                }
                OutlinedButton(
                    onClick = onPvp,
                    modifier = Modifier.fillMaxWidth().height(54.dp),
                ) {
                    Text("PvP", letterSpacing = 2.sp)
                }

                Spacer(Modifier.height(6.dp))

                // The secondary / utility actions, quieter than the play modes.
                OutlinedButton(
                    onClick = onSettings,
                    modifier = Modifier.fillMaxWidth().height(54.dp),
                ) {
                    Text("SETTINGS", letterSpacing = 2.sp)
                }
                OutlinedButton(
                    onClick = onProfile,
                    modifier = Modifier.fillMaxWidth().height(54.dp),
                ) {
                    Text("PROFILE", letterSpacing = 2.sp)
                }
                // The army-select entry (US vs FR) — a pre-deploy pick fielded at every match start.
                // Mirrors the desktop title's ARMY utility chip (`app/src/shell.rs`).
                OutlinedButton(
                    onClick = onArmy,
                    modifier = Modifier.fillMaxWidth().height(54.dp),
                ) {
                    Text("ARMY", letterSpacing = 2.sp)
                }
                TextButton(
                    onClick = onAbout,
                    modifier = Modifier.fillMaxWidth().height(54.dp),
                ) {
                    Text("FIELD MANUAL", letterSpacing = 2.sp)
                }
                TextButton(
                    onClick = onQuit,
                    modifier = Modifier.fillMaxWidth().height(54.dp),
                ) {
                    Text("QUIT", letterSpacing = 2.sp)
                }
            }

            Spacer(Modifier.weight(1.2f))

            Text(
                text = versionStamp,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 12.sp,
            )
        }
    }
}

@Preview(showBackground = true, widthDp = 880, heightDp = 400)
@Composable
private fun TitleScreenPreview() {
    GoingDarkTheme {
        TitleScreen(
            versionStamp = buildStamp("dev", "0.0.0"),
            onCampaign = {},
            onPve = {},
            onPvp = {},
            onSettings = {},
            onProfile = {},
            onArmy = {},
            onAbout = {},
            onQuit = {},
        )
    }
}
