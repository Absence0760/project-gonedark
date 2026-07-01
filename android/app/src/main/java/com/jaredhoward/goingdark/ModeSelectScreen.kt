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
 * The Pve/Pvp **mode / map select** screen (D81) — the lightweight picker a play-mode tap now opens
 * instead of the gunsmith. Pure presentation, modelled on [MissionSelectScreen]: it renders the
 * immutable [shellGameModes] as a titled list of tiles and reports a pick through [onPick]; the host
 * ([MainActivity]'s `Shell`) turns that into a Deploy with the mode's scene token + the persisted
 * loadout. Carries no game state and never touches the sim.
 *
 * Stateless / hoisted, like the sibling shell screens — every action is a callback, so it is
 * device-agnostic and previewable without an Activity.
 */
@Composable
fun ModeSelectScreen(
    modes: List<GameMode>,
    onPick: (GameMode) -> Unit,
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
                text = "SELECT MODE",
                color = MaterialTheme.colorScheme.onBackground,
                fontSize = 30.sp,
                letterSpacing = 8.sp,
                textAlign = TextAlign.Center,
            )

            Spacer(Modifier.height(12.dp))

            Text(
                text = "Pick a battle to deploy into. Your loadout is set in the gunsmith, under Settings.",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
                textAlign = TextAlign.Center,
                modifier = Modifier.widthIn(max = 440.dp).fillMaxWidth(),
            )

            Spacer(Modifier.height(24.dp))

            // The mode list, width-capped so tiles don't stretch across a wide landscape screen.
            Column(
                modifier = Modifier.widthIn(max = 440.dp).fillMaxWidth(),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                for (mode in modes) {
                    ModeTile(mode = mode, onClick = { onPick(mode) })
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

/** One mode tile: the mode name over its one-line blurb; tapping it deploys that mode. */
@Composable
private fun ModeTile(mode: GameMode, onClick: () -> Unit) {
    OutlinedButton(
        onClick = onClick,
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(vertical = 6.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(
                text = mode.name.uppercase(),
                color = MaterialTheme.colorScheme.onSurface,
                fontSize = 16.sp,
                letterSpacing = 2.sp,
            )
            Text(
                text = mode.blurb,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
            )
        }
    }
}

@Preview(showBackground = true, widthDp = 880, heightDp = 520)
@Composable
private fun ModeSelectScreenPreview() {
    GoingDarkTheme {
        ModeSelectScreen(
            modes = shellGameModes,
            onPick = {},
            onBack = {},
        )
    }
}
