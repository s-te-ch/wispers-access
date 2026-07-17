package dev.wispers.access.android.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.wispers.access.android.storage.ShareTerminalState

/**
 * Explains a terminal share: what happened and that only removal remains.
 * Shared between the detail screen and the WebView activity (a pinned shortcut
 * can still land there), so the story reads the same everywhere.
 */
@Composable
fun TerminalShareExplanation(state: ShareTerminalState, modifier: Modifier = Modifier) {
    Card(modifier = modifier.fillMaxWidth()) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                text = "This share is no longer available",
                style = MaterialTheme.typography.titleMedium,
            )
            Text(
                text = when (state) {
                    ShareTerminalState.REMOVED ->
                        "The share was removed by its owner and can't be reached anymore. "
                    ShareTerminalState.REVOKED ->
                        "This device's access to the share was revoked by its owner. "
                } + "You can remove it from this device; joining again needs a new invitation code.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}
