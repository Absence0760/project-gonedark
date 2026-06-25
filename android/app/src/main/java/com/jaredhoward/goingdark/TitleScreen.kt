package com.jaredhoward.goingdark

import androidx.compose.foundation.layout.Arrangement
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
 * The landing / title screen — D32 app-shell surface 1 ("Boot & title"). Pure presentation: the
 * title, a tagline, the three top-level actions, and a build/version stamp. It carries **no** game
 * state and never touches the sim; "Start" hands off to the engine through the host (MainActivity),
 * the one place the shell crosses into the shared core (via the `core::shell` seam, D34).
 *
 * Actions are passed in as callbacks so the screen is host-agnostic and previewable without an
 * Activity. The menu behind Start is intentionally lean: **Settings** is a placeholder until the
 * Settings surface lands, and the deeper menu (match setup, lobby) is Q5/Phase-3-blocked
 * (phase-4-plan §2).
 */
@Composable
fun TitleScreen(
    versionStamp: String,
    onStart: () -> Unit,
    onSettings: () -> Unit,
    onQuit: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Surface(modifier = modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
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
                Button(
                    onClick = onStart,
                    modifier = Modifier.fillMaxWidth().height(54.dp),
                ) {
                    Text("START", letterSpacing = 2.sp)
                }
                OutlinedButton(
                    onClick = onSettings,
                    modifier = Modifier.fillMaxWidth().height(54.dp),
                ) {
                    Text("SETTINGS", letterSpacing = 2.sp)
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
            onStart = {},
            onSettings = {},
            onQuit = {},
        )
    }
}
