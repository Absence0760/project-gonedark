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
 * The About / field-manual screen — the Compose port of desktop's egui About surface
 * (`app/src/shell.rs` `draw_about`, parity plan Tier 1, About row). Pure presentation: a banner,
 * the one-paragraph concept blurb, the grouped keymap from the [`fieldManualSections`] data seam,
 * and a BACK button. Carries **no** game state and never touches the sim.
 *
 * Stateless and host-agnostic: BACK is a callback so the screen is previewable without an Activity.
 * Content lives in `FieldManual.kt` (pure, tested); this file is the device-gated chrome (D32).
 */
@Composable
fun AboutScreen(
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
                text = "FIELD MANUAL",
                color = MaterialTheme.colorScheme.onBackground,
                fontSize = 32.sp,
                letterSpacing = 7.sp,
                textAlign = TextAlign.Center,
            )

            Spacer(Modifier.height(20.dp))

            // The readable body column, width-capped so prose/rows don't stretch across a wide screen.
            Column(
                modifier = Modifier.widthIn(max = 520.dp).fillMaxWidth(),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Text(
                    text = FIELD_MANUAL_BLURB,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 14.sp,
                    textAlign = TextAlign.Center,
                )

                Spacer(Modifier.height(24.dp))

                for (section in fieldManualSections) {
                    Text(
                        text = section.title,
                        color = MaterialTheme.colorScheme.primary,
                        fontSize = 13.sp,
                        letterSpacing = 3.sp,
                        modifier = Modifier.fillMaxWidth().padding(bottom = 6.dp),
                    )
                    for (row in section.rows) {
                        Row(
                            modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
                            horizontalArrangement = Arrangement.spacedBy(16.dp),
                            verticalAlignment = Alignment.Top,
                        ) {
                            Text(
                                text = row.binding,
                                color = MaterialTheme.colorScheme.onBackground,
                                fontSize = 13.sp,
                                modifier = Modifier.widthIn(min = 120.dp, max = 180.dp).weight(0.4f),
                            )
                            Text(
                                text = row.action,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                fontSize = 13.sp,
                                modifier = Modifier.weight(0.6f),
                            )
                        }
                    }
                    Spacer(Modifier.height(18.dp))
                }

                Button(
                    onClick = onBack,
                    modifier = Modifier.fillMaxWidth().height(54.dp),
                ) {
                    Text("BACK", letterSpacing = 2.sp)
                }
            }

            Spacer(Modifier.height(12.dp))
        }
    }
}

@Preview(showBackground = true, widthDp = 880, heightDp = 600)
@Composable
private fun AboutScreenPreview() {
    GoingDarkTheme {
        AboutScreen(onBack = {})
    }
}
