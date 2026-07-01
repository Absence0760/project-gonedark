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
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
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
import kotlin.math.roundToInt

/**
 * The Settings screen — Compose shell parity Tier 1 (`docs/plans/compose-shell-parity.md`, Settings
 * row), the native twin of the desktop egui Settings surface (`app/src/shell.rs::settings_screen`).
 * Pure presentation: it renders an immutable [SettingsState] and reports edits through [onChange]; it
 * carries no game state and never touches the sim (these are host prefs, not sim state).
 *
 * **Stateless / hoisted state.** Every control computes a new [SettingsState] (via `copy(...).clamp()`)
 * and hands it back through [onChange] — the host owns the value. This keeps the composable device-
 * agnostic and previewable without an Activity, exactly like [TitleScreen]; the real logic (bounds,
 * clamping, defaults, quality cycling) lives in the testable [SettingsState] seam, which the UI is
 * exempt from (the `BuildStamp.kt` / D32 pattern).
 */
@Composable
fun SettingsScreen(
    state: SettingsState,
    onChange: (SettingsState) -> Unit,
    onOpenLoadout: () -> Unit,
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
                text = "SETTINGS",
                color = MaterialTheme.colorScheme.onBackground,
                fontSize = 30.sp,
                letterSpacing = 8.sp,
                textAlign = TextAlign.Center,
            )

            Spacer(Modifier.height(24.dp))

            // The controls column, width-capped so sliders don't stretch across a wide landscape screen.
            Column(
                modifier = Modifier.widthIn(max = 440.dp).fillMaxWidth(),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                SectionLabel("AUDIO")
                SettingSlider(
                    label = "Master",
                    valuePct = state.masterPct,
                    min = 0,
                    max = SettingsState.GAIN_PCT_MAX,
                    valueText = "${state.masterPct}%",
                    onValue = { onChange(state.copy(masterPct = it).clamp()) },
                )
                SettingSlider(
                    label = "SFX",
                    valuePct = state.sfxPct,
                    min = 0,
                    max = SettingsState.GAIN_PCT_MAX,
                    valueText = "${state.sfxPct}%",
                    onValue = { onChange(state.copy(sfxPct = it).clamp()) },
                )
                SettingSlider(
                    label = "Music",
                    valuePct = state.musicPct,
                    min = 0,
                    max = SettingsState.GAIN_PCT_MAX,
                    valueText = "${state.musicPct}%",
                    onValue = { onChange(state.copy(musicPct = it).clamp()) },
                )

                Spacer(Modifier.height(8.dp))
                SectionLabel("CONTROLS")
                SettingSlider(
                    label = "Look sensitivity",
                    valuePct = state.sensX100,
                    min = SettingsState.SENS_MIN,
                    max = SettingsState.SENS_MAX,
                    valueText = sensitivityLabel(state.sensX100),
                    onValue = { onChange(state.copy(sensX100 = it).clamp()) },
                )

                Row(
                    modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        text = "Invert look Y",
                        color = MaterialTheme.colorScheme.onSurface,
                        fontSize = 15.sp,
                        modifier = Modifier.weight(1f),
                    )
                    Switch(
                        checked = state.invertLookY,
                        onCheckedChange = { onChange(state.copy(invertLookY = it).clamp()) },
                    )
                }

                Spacer(Modifier.height(8.dp))
                SectionLabel("DISPLAY")
                Row(
                    modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        text = "Quality",
                        color = MaterialTheme.colorScheme.onSurface,
                        fontSize = 15.sp,
                        modifier = Modifier.weight(1f),
                    )
                    OutlinedButton(
                        onClick = { onChange(state.copy(quality = state.quality.next()).clamp()) },
                        modifier = Modifier.widthIn(min = 140.dp),
                    ) {
                        Text(state.quality.label().uppercase(), letterSpacing = 2.sp)
                    }
                }

                Spacer(Modifier.height(8.dp))
                SectionLabel("LOADOUT")
                // The gunsmith is loadout customization, not a play gate (D81): it lives here, reached
                // on demand, and its edits persist for the next Deploy — tapping a play mode no longer
                // forces you through it.
                OutlinedButton(
                    onClick = onOpenLoadout,
                    modifier = Modifier.fillMaxWidth().height(50.dp),
                ) {
                    Text("GUNSMITH", letterSpacing = 2.sp)
                }
            }

            Spacer(Modifier.height(28.dp))

            // Bottom actions: RESET restores defaults, BACK returns to the title.
            Column(
                modifier = Modifier.widthIn(max = 360.dp).fillMaxWidth(),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                OutlinedButton(
                    onClick = { onChange(state.reset()) },
                    modifier = Modifier.fillMaxWidth().height(50.dp),
                ) {
                    Text("RESET", letterSpacing = 2.sp)
                }
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

/** An uppercase, letter-spaced section divider label (the TitleScreen tagline idiom). */
@Composable
private fun SectionLabel(text: String) {
    Text(
        text = text,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        fontSize = 12.sp,
        letterSpacing = 4.sp,
        modifier = Modifier.padding(bottom = 2.dp),
    )
}

/**
 * A labelled integer slider row: name + current value on top, the [Slider] below. The slider works in
 * `Float` space (Material3's only Slider type), rounding back to the nearest integer step on each edit;
 * the host's [SettingsState.clamp] is the authoritative re-bound.
 */
@Composable
private fun SettingSlider(
    label: String,
    valuePct: Int,
    min: Int,
    max: Int,
    valueText: String,
    onValue: (Int) -> Unit,
) {
    Column(modifier = Modifier.fillMaxWidth()) {
        Row(modifier = Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(
                text = label,
                color = MaterialTheme.colorScheme.onSurface,
                fontSize = 15.sp,
                modifier = Modifier.weight(1f),
            )
            Text(
                text = valueText,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 14.sp,
            )
        }
        Slider(
            value = valuePct.toFloat(),
            onValueChange = { onValue(it.roundToInt()) },
            valueRange = min.toFloat()..max.toFloat(),
        )
    }
}

/** Render a sensitivity ×100 integer as a `1.00×` multiplier label (the desktop reads it as a float). */
private fun sensitivityLabel(sensX100: Int): String {
    val whole = sensX100 / 100
    val frac = sensX100 % 100
    return "$whole.${frac.toString().padStart(2, '0')}×"
}

@Preview(showBackground = true, widthDp = 880, heightDp = 520)
@Composable
private fun SettingsScreenPreview() {
    GoingDarkTheme {
        SettingsScreen(
            state = SettingsState.defaults(),
            onChange = {},
            onOpenLoadout = {},
            onBack = {},
        )
    }
}
