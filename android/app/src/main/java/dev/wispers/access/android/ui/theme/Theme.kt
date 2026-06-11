package dev.wispers.access.android.ui.theme

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

private val WispersAccessColors = lightColorScheme(
    primary = AccessPrimaryLight,
    // Dark forest on the light green: white fails contrast (~1.8:1) there.
    onPrimary = AccessPrimaryDark,
    primaryContainer = AccessPrimaryLight,
    onPrimaryContainer = AccessPrimaryDark,
    secondary = AccessPrimaryDark,
    onSecondary = Color.White,
    background = AccessBackground,
    onBackground = AccessOnSurface,
    surface = AccessSurface,
    onSurface = AccessOnSurface,
    surfaceVariant = AccessBackground,
    onSurfaceVariant = AccessOnSurfaceVariant,
    outline = AccessOutline,
)

@Composable
fun WispersAccessTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = WispersAccessColors,
        typography = Typography,
        content = content,
    )
}
