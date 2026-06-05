package dev.wispers.access.android.screens

import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import dev.wispers.access.android.shareLetterTile

/**
 * The share's icon as a rounded tile. Currently always the generated sage letter
 * tile; Phase 2 will swap in a cached site icon when one has been discovered.
 */
@Composable
fun ShareAvatar(nickname: String, modifier: Modifier = Modifier, size: Dp = 48.dp) {
    val context = LocalContext.current
    val bitmap = remember(nickname) { shareLetterTile(context, nickname).asImageBitmap() }
    Image(
        bitmap = bitmap,
        contentDescription = null,
        modifier = modifier
            .size(size)
            .clip(RoundedCornerShape(size / 3)),
    )
}
