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
import androidx.compose.material3.Button
import androidx.compose.material3.Card
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
 * The pre-match **gunsmith / loadout** screen — Compose shell parity Tier 2
 * (`docs/plans/compose-shell-parity.md` §5), the native re-author of desktop's `engine::loadout_ui`
 * editor. Pure presentation: it shows the three weapon slots and lets the player cycle each one's
 * option. It carries **no** game state and never touches the sim — the chosen [LoadoutSelection]
 * indices are handed back to the host, which packs them into the `opt`/`bar`/`mag` fields of a
 * [LaunchConfig] at Deploy (the engine already consumes that across the launch seam).
 *
 * Stateless: the current [selection] and all actions are passed in, so the screen is host-agnostic
 * and previewable without an Activity (the `TitleScreen.kt` pattern). [onChange] is invoked with the
 * new selection whenever a slot is cycled; the host owns the state (persisted for the next Deploy).
 *
 * **Customization only, not a play gate (D81):** the gunsmith is reached from Settings and has no
 * Deploy — edits persist and apply to the next match you launch from the mode/mission flow. It is no
 * longer a mandatory step in front of PvE/PvP.
 */
@Composable
fun GunsmithScreen(
    selection: LoadoutSelection,
    onChange: (LoadoutSelection) -> Unit,
    onReset: () -> Unit,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Surface(modifier = modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(horizontal = 40.dp, vertical = 32.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(
                text = "GUNSMITH",
                color = MaterialTheme.colorScheme.onBackground,
                fontSize = 34.sp,
                letterSpacing = 8.sp,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(6.dp))
            Text(
                text = "SIDEGRADE · NEVER A TIER",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 12.sp,
                letterSpacing = 4.sp,
                textAlign = TextAlign.Center,
            )

            Spacer(Modifier.height(28.dp))

            // The slot column, width-capped so rows don't stretch across a wide landscape screen.
            Column(
                modifier = Modifier.widthIn(max = 460.dp).fillMaxWidth(),
                verticalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                for (slot in Slot.entries) {
                    SlotRow(
                        slot = slot,
                        selection = selection,
                        onChange = onChange,
                    )
                }
            }

            Spacer(Modifier.weight(1f))

            Column(
                modifier = Modifier.widthIn(max = 360.dp).fillMaxWidth(),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                // RESET returns every slot to the neutral all-Standard baseline — mirrors the desktop
                // gunsmith's RESET action (LoadoutAction::Reset). DONE saves-and-returns to Settings
                // (the edits are already persisted via onChange); there is no Deploy here (D81).
                OutlinedButton(
                    onClick = onReset,
                    modifier = Modifier.fillMaxWidth().height(54.dp),
                ) {
                    Text("RESET", letterSpacing = 2.sp)
                }
                Button(
                    onClick = onBack,
                    modifier = Modifier.fillMaxWidth().height(54.dp),
                ) {
                    Text("DONE", letterSpacing = 2.sp)
                }
            }
        }
    }
}

/**
 * One slot row: the slot name, its trade-axis hint, the currently-selected option label, and prev/next
 * controls that cycle the slot via [LoadoutSelection.cycle].
 */
@Composable
private fun SlotRow(
    slot: Slot,
    selection: LoadoutSelection,
    onChange: (LoadoutSelection) -> Unit,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 18.dp, vertical = 14.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    text = slot.name.uppercase(),
                    color = MaterialTheme.colorScheme.onSurface,
                    fontSize = 15.sp,
                    letterSpacing = 3.sp,
                )
                Text(
                    text = LoadoutSelection.tradeHint(slot),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 12.sp,
                )
            }

            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                OutlinedButton(onClick = { onChange(selection.cycle(slot, forward = false)) }) {
                    Text("‹")
                }
                Text(
                    text = LoadoutSelection.label(slot, selection.index(slot)),
                    color = MaterialTheme.colorScheme.onSurface,
                    fontSize = 18.sp,
                    textAlign = TextAlign.Center,
                )
                OutlinedButton(onClick = { onChange(selection.cycle(slot, forward = true)) }) {
                    Text("›")
                }
            }
        }
    }
}

@Preview(showBackground = true, widthDp = 880, heightDp = 480)
@Composable
private fun GunsmithScreenPreview() {
    GoingDarkTheme {
        GunsmithScreen(
            selection = LoadoutSelection(optic = 1, barrel = 2, magazine = 0),
            onChange = {},
            onReset = {},
            onBack = {},
        )
    }
}
