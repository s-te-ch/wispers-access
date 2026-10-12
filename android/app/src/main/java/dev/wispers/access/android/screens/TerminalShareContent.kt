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
import dev.wispers.access.sdk.ShareState

/**
 * Explains why a share can't be used - what happened, and what remains to do.
 * For example, update the app, or remove the share. Shared between the detail
 * screen and the WebView activity (a pinned shortcut can still land there), so
 * the story reads the same everywhere.
 */
@Composable
fun TerminalShareExplanation(state: ShareState, modifier: Modifier = Modifier) {
    Card(modifier = modifier.fillMaxWidth()) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                text = when (state) {
                    ShareState.OUTDATED -> "This app is too old for this share"
                    else -> "This share is no longer available"
                },
                style = MaterialTheme.typography.titleMedium,
            )
            Text(
                text = when (state) {
                    ShareState.OUTDATED ->
                        "The share's host runs a newer version of Wispers Access than this app " +
                            "understands. Update the app to keep using the share."
                    ShareState.REMOVED ->
                        "The share was removed by its host and can't be reached anymore. " +
                            REMOVAL_HINT
                    ShareState.REVOKED ->
                        "This device's access was revoked by the share's host. " + REMOVAL_HINT
                    ShareState.LIVE -> "The share is available. " + REMOVAL_HINT
                },
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

private const val REMOVAL_HINT =
    "You can remove it from this device. Joining again needs a new invitation code."
