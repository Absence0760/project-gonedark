package com.jaredhoward.goingdark

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.tooling.preview.Preview
import com.jaredhoward.goingdark.ui.theme.GoingDarkTheme

/**
 * The title screen's animated backdrop — a **Compose-native** drifting motif behind the title chrome,
 * locked as **D78 option 1**: a tasteful dark gradient + slow-moving vector motif + a pulsing horizon
 * glow, all drawn in pure Compose (`Canvas`/`drawBehind` + `androidx.compose.animation.core`). It is
 * deliberately **not** a wgpu/3D surface — bringing the desktop's live `render::title_backdrop`
 * scene to Compose would mean a second render surface in the shell process, which D78 declines to pay
 * for; this buys ~80% of the perceived polish at a fraction of the cost and keeps the title a pure
 * Compose surface. Desktop's 3D scene and this 2D motif are *meant* to look different (not a parity
 * bug).
 *
 * No external assets, no images, no fonts — everything is drawn from primitives, so it adds nothing
 * to the APK and can never fail to load. Pure presentation glue (a `@Composable` with no testable
 * decision logic), so it carries no unit test, exactly like the rest of the Compose chrome.
 */

// The "going dark" palette, mirrored from `ui/theme/Theme.kt` (which mirrors the canonical renderer
// palette in `render/src/theme.rs`). Kept here as locals so the backdrop draws independently of the
// MaterialTheme color slots — it sits *behind* the themed chrome and owns its own field.
private val Ink = Color(0xFF07090C) // near-black field — top of the gradient
private val DeepInk = Color(0xFF04060A) // a touch darker toward the floor
private val Horizon = Color(0xFF0F141A) // the panel hue, faint band at the horizon
private val Amber = Color(0xFFE0791F) // the lone alert accent — the horizon glow
private val Line = Color(0xFF1B2531) // raised-panel hue — the drifting motif lines

/**
 * Draw the animated title backdrop, filling [modifier]'s bounds. Two infinite animations drive it: a
 * slow horizontal **drift** that slides a field of faint diagonal lines, and a gentle **pulse** that
 * breathes the amber horizon glow. Both loop forever via `rememberInfiniteTransition`; nothing here
 * touches the sim or any engine surface.
 */
@Composable
fun TitleBackdrop(modifier: Modifier = Modifier) {
    val transition = rememberInfiniteTransition(label = "title-backdrop")

    // The drift phase, 0f→1f over 18s, restarting — slides the diagonal motif sideways. Slow and
    // linear so it reads as a calm, continuous current rather than a bounce.
    val drift by transition.animateFloat(
        initialValue = 0f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 18_000, easing = LinearEasing),
            repeatMode = RepeatMode.Restart,
        ),
        label = "drift",
    )

    // The horizon-glow pulse, 0f→1f and back over 6s — a subtle "breathing" amber band so the field
    // is never fully static even when the eye isn't tracking the drift.
    val pulse by transition.animateFloat(
        initialValue = 0f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 6_000, easing = LinearEasing),
            repeatMode = RepeatMode.Reverse,
        ),
        label = "pulse",
    )

    Canvas(modifier = modifier.fillMaxSize()) {
        val w = size.width
        val h = size.height

        // 1) Base vertical gradient — near-black at the top, a faint panel-hued band at the horizon
        //    (~78% down), then darker again at the floor. The whole field stays dark enough to sit
        //    behind text without hurting legibility.
        drawRect(
            brush = Brush.verticalGradient(
                0.0f to Ink,
                0.55f to Ink,
                0.78f to Horizon,
                1.0f to DeepInk,
            ),
        )

        // 2) The drifting diagonal motif — a field of parallel lines sloping up-left, sliding right
        //    with `drift`. Spacing is generous and the colour is dim, so it reads as faint structure
        //    (a darkened tac-map grid), not a loud pattern. Each line is drawn from the bottom edge to
        //    the top edge; the +/- spacing margins keep the field covered as the phase wraps.
        val spacing = 120f
        val slope = 0.45f // up-left lean: x decreases as y decreases
        val phase = drift * spacing // slide one full gap over the loop
        var x = -h * slope - spacing + phase
        while (x < w + spacing) {
            // Lines fade toward the edges so the motif concentrates around the centre.
            val centred = 1f - (kotlin.math.abs((x / w) - 0.5f) * 1.6f).coerceIn(0f, 1f)
            drawLine(
                color = Line.copy(alpha = 0.18f + 0.22f * centred),
                start = Offset(x, h),
                end = Offset(x + h * slope, 0f),
                strokeWidth = 1.5f,
            )
            x += spacing
        }

        // 3) The pulsing horizon glow — a soft amber radial seated just below the horizon band,
        //    breathing with `pulse`. Low alpha so it's an ember on the skyline, never a spotlight.
        val glowCenter = Offset(w * 0.5f, h * 0.82f)
        val glowRadius = w * (0.55f + 0.08f * pulse)
        val glowAlpha = 0.10f + 0.10f * pulse
        drawRect(
            brush = Brush.radialGradient(
                colors = listOf(Amber.copy(alpha = glowAlpha), Color.Transparent),
                center = glowCenter,
                radius = glowRadius,
            ),
        )
    }
}

@Preview(showBackground = true, widthDp = 880, heightDp = 400)
@Composable
private fun TitleBackdropPreview() {
    GoingDarkTheme {
        TitleBackdrop(Modifier.fillMaxSize())
    }
}
