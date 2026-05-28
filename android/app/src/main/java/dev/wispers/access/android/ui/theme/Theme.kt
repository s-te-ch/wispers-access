package dev.wispers.access.android.ui.theme

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

private val WispersAccessColors = lightColorScheme(
    primary = AccessPrimaryLight,
    onPrimary = Color.White,
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
