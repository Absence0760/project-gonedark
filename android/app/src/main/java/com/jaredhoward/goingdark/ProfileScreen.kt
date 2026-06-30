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
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.jaredhoward.goingdark.ui.theme.GoingDarkTheme

/**
 * The Profile screen — the Android Compose mirror of the desktop egui `profile_ui` (`app/src/shell.rs`).
 * Pure presentation: a callsign field, a faction picker, the read-only lifetime record, and BACK.
 * It carries **no** game state and never touches the sim.
 *
 * Stateless: the committed [ProfileState] and all mutations flow through the host via [onChange] /
 * [onBack] callbacks, so the screen is host-agnostic and previewable without an Activity (matching
 * TitleScreen.kt). All real validation lives in the pure ProfileLogic.kt seam ([sanitizeCallsign],
 * [winRatePct]); this file is device-gated chrome (D32). The callsign is sanitised on commit — when
 * the field loses focus would be ideal, but to stay dependency-light it commits on BACK (mirroring
 * the Rust `ProfileAction::Back` sanitise-on-the-way-out), while the in-field draft is local edit
 * state.
 */
@Composable
fun ProfileScreen(
    state: ProfileState,
    onChange: (ProfileState) -> Unit,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    // Local draft for the text field so the user can type freely; sanitised into committed state on
    // BACK (and whenever a discrete control commits). Keyed on the incoming callsign so an external
    // change (e.g. a host reset) re-seeds the draft.
    var callsignDraft by remember(state.callsign) { mutableStateOf(state.callsign) }

    val rate = winRatePct(state.wins, state.matchesPlayed)?.let { "$it%" } ?: "--"

    Surface(modifier = modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(horizontal = 40.dp, vertical = 32.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Spacer(Modifier.weight(0.6f))

            Text(
                text = "PROFILE",
                color = MaterialTheme.colorScheme.onBackground,
                fontSize = 38.sp,
                letterSpacing = 8.sp,
                textAlign = TextAlign.Center,
            )

            Spacer(Modifier.weight(0.6f))

            Column(
                modifier = Modifier.widthIn(max = 420.dp).fillMaxWidth(),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                Text(
                    text = "IDENTITY",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 12.sp,
                    letterSpacing = 3.sp,
                    modifier = Modifier.fillMaxWidth(),
                    textAlign = TextAlign.Start,
                )

                OutlinedTextField(
                    value = callsignDraft,
                    onValueChange = { raw ->
                        // Hard-cap the in-field length so a name can't grow past the budget while typing;
                        // full trim/empty→default sanitisation runs on commit via sanitizeCallsign.
                        callsignDraft = raw.take(CALLSIGN_MAX)
                    },
                    singleLine = true,
                    label = { Text("Callsign") },
                    modifier = Modifier.fillMaxWidth(),
                )

                // Faction picker — tap to cycle US Army <-> French Army (mirrors ProfileAction::CycleFaction).
                OutlinedButton(
                    onClick = {
                        onChange(
                            state.copy(
                                callsign = sanitizeCallsign(callsignDraft),
                                faction = state.faction.next(),
                            ),
                        )
                    },
                    modifier = Modifier.fillMaxWidth().height(52.dp),
                ) {
                    Text("FACTION · ${state.faction.label()}", letterSpacing = 1.sp)
                }

                Spacer(Modifier.height(2.dp))

                Text(
                    text = "RECORD",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 12.sp,
                    letterSpacing = 3.sp,
                    modifier = Modifier.fillMaxWidth(),
                    textAlign = TextAlign.Start,
                )
                Text(
                    text = "Matches ${state.matchesPlayed}   ·   Wins ${state.wins}   ·   Win rate $rate",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 14.sp,
                    modifier = Modifier.fillMaxWidth(),
                    textAlign = TextAlign.Start,
                )

                // Zero the lifetime record (mirrors desktop ProfileAction::ResetStats). Commits the
                // in-field callsign too, like the faction cycle, so an unsaved draft isn't lost.
                OutlinedButton(
                    onClick = {
                        onChange(
                            state
                                .copy(callsign = sanitizeCallsign(callsignDraft))
                                .resetRecord(),
                        )
                    },
                    modifier = Modifier.fillMaxWidth().height(52.dp),
                ) {
                    Text("RESET RECORD", letterSpacing = 1.sp)
                }
            }

            Spacer(Modifier.weight(1f))

            Button(
                onClick = {
                    // Commit the sanitised callsign on the way out, then hand back to the host.
                    onChange(state.copy(callsign = sanitizeCallsign(callsignDraft)))
                    onBack()
                },
                modifier = Modifier.widthIn(max = 420.dp).fillMaxWidth().height(54.dp),
            ) {
                Text("BACK", letterSpacing = 2.sp)
            }

            Spacer(Modifier.weight(0.6f))
        }
    }
}

@Preview(showBackground = true, widthDp = 880, heightDp = 400)
@Composable
private fun ProfileScreenPreview() {
    GoingDarkTheme {
        ProfileScreen(
            state = ProfileState(
                callsign = "Reaper",
                faction = FactionPref.FrenchArmy,
                matchesPlayed = 12,
                wins = 7,
            ),
            onChange = {},
            onBack = {},
        )
    }
}
