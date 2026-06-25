package com.jaredhoward.goingdark.ui.theme

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

// "Going dark" palette: a near-black field, dim chrome, and a single amber accent — the game's
// directional-alert colour (invariant #6: alerts are a flash + audio). Deliberately low-key; the
// landing screen should read like a darkened command console, not a bright storefront menu.
private val Ink = Color(0xFF07090C) // background — almost black
private val Panel = Color(0xFF0F141A) // raised surface
private val Bone = Color(0xFFE7ECEF) // primary text
private val Ash = Color(0xFF8A949C) // secondary / muted text
private val Amber = Color(0xFFE0791F) // alert accent + primary action
private val OnAmber = Color(0xFF120A02)

private val GoingDarkColors = darkColorScheme(
    primary = Amber,
    onPrimary = OnAmber,
    background = Ink,
    onBackground = Bone,
    surface = Panel,
    onSurface = Bone,
    onSurfaceVariant = Ash,
    outline = Ash,
)

/** The app shell's Material 3 theme — a fixed dark scheme (the shell is always "dark"). */
@Composable
fun GoingDarkTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = GoingDarkColors, content = content)
}
