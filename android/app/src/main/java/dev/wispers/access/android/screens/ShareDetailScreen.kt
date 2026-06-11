package dev.wispers.access.android.screens

import android.widget.Toast
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.wispers.access.android.addToHomescreen
import dev.wispers.access.android.proxy.ShareStatusTracker
import dev.wispers.access.android.storage.Share
import dev.wispers.access.android.storage.ShareId
import dev.wispers.access.android.storage.ShareRepository
import java.time.Duration
import java.time.Instant
import javax.inject.Inject
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch

@HiltViewModel
class ShareDetailViewModel @Inject constructor(
    savedStateHandle: SavedStateHandle,
    private val repo: ShareRepository,
    statusTracker: ShareStatusTracker,
) : ViewModel() {

    private val shareId: ShareId =
        ShareId(checkNotNull(savedStateHandle["shareId"]) { "shareId arg required" })

    val share: StateFlow<Share?> = repo.observeShare(shareId).stateIn(
        scope = viewModelScope,
        started = SharingStarted.WhileSubscribed(5_000),
        initialValue = null,
    )

    /** Serving-node availability; null = not known (yet). Shared with the list screen. */
    val online: StateFlow<Boolean?> = statusTracker.statuses
        .map { it[shareId] }
        .stateIn(
            scope = viewModelScope,
            started = SharingStarted.WhileSubscribed(5_000),
            initialValue = null,
        )

    private val _removed = MutableStateFlow(false)
    val removed: StateFlow<Boolean> = _removed.asStateFlow()

    fun onRemove() {
        viewModelScope.launch {
            repo.deleteShare(shareId)
            _removed.value = true
        }
    }

    private companion object {
        const val STATUS_REFRESH_MS = 30_000L
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ShareDetailScreen(
    onBack: () -> Unit,
    onOpenShare: (ShareId) -> Unit,
    viewModel: ShareDetailViewModel = hiltViewModel(),
) {
    val share by viewModel.share.collectAsState()
    val online by viewModel.online.collectAsState()
    val removed by viewModel.removed.collectAsState()
    var confirmRemove by remember { mutableStateOf(false) }
    val context = LocalContext.current

    LaunchedEffect(removed) {
        if (removed) onBack()
    }

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            TopAppBar(
                title = { Text("Share") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
                // The app bar defaults to surface (white) which clashes with the
                // background-colored page; blend it in like the list screen does.
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.background,
                ),
            )
        },
    ) { innerPadding ->
        val current = share
        if (current != null) {
            ShareDetailContent(
                share = current,
                online = online,
                contentPadding = innerPadding,
                onOpen = { onOpenShare(current.id) },
                onAddToHomescreen = {
                    if (!addToHomescreen(context, current.id, current.nickname, current.iconPng)) {
                        Toast.makeText(
                            context,
                            "Your launcher doesn't support adding shortcuts",
                            Toast.LENGTH_SHORT,
                        ).show()
                    }
                },
                onRemove = { confirmRemove = true },
            )
        }
    }

    if (confirmRemove) {
        AlertDialog(
            onDismissRequest = { confirmRemove = false },
            title = { Text("Remove share?") },
            text = {
                Text("This deletes the local share data. You'll need a new invitation code to rejoin.")
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        confirmRemove = false
                        viewModel.onRemove()
                    },
                ) {
                    Text("Remove", color = MaterialTheme.colorScheme.error)
                }
            },
            dismissButton = {
                TextButton(onClick = { confirmRemove = false }) {
                    // TextButton defaults to primary (light green) — illegible on the
                    // dialog surface; plain text color reads as the neutral action.
                    Text("Cancel", color = MaterialTheme.colorScheme.onSurface)
                }
            },
        )
    }
}

@Composable
private fun ShareDetailContent(
    share: Share,
    online: Boolean?,
    contentPadding: PaddingValues,
    onOpen: () -> Unit,
    onAddToHomescreen: () -> Unit,
    onRemove: () -> Unit,
) {
    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(contentPadding)
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        StatusRow(online = online)
        ShareAvatar(nickname = share.nickname, iconPng = share.iconPng, size = 64.dp)
        Text(
            text = share.nickname.ifBlank { "Untitled share" },
            style = MaterialTheme.typography.headlineLarge,
        )
        InfoCardsRow(share = share)
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(
                onClick = onOpen,
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text("Open share ↗")
            }
            OutlinedButton(
                onClick = onAddToHomescreen,
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text("Add to homescreen")
            }
            OutlinedButton(
                onClick = onRemove,
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text("Remove share", color = MaterialTheme.colorScheme.error)
            }
        }
    }
}

@Composable
private fun StatusRow(online: Boolean?) {
    Row(
        verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (online == null) {
            CircularProgressIndicator(
                modifier = Modifier.size(10.dp),
                strokeWidth = 1.5.dp,
                color = MaterialTheme.colorScheme.outline,
            )
        } else {
            androidx.compose.foundation.layout.Box(
                modifier = Modifier
                    .size(10.dp)
                    .background(
                        color = if (online) Color(0xFF34A853) else MaterialTheme.colorScheme.outline,
                        shape = CircleShape,
                    ),
            )
        }
        Text(
            text = when (online) {
                true -> "ONLINE"
                false -> "OFFLINE"
                null -> "CHECKING…"
            },
            style = MaterialTheme.typography.labelMedium,
        )
    }
}

@Composable
private fun InfoCardsRow(share: Share) {
    val now = Instant.now()
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        InfoCard(
            label = "LAST CONNECTED",
            value = formatRelative(share.lastConnectedAt, now),
            modifier = Modifier.weight(1f),
        )
        InfoCard(
            label = "JOINED",
            value = formatRelative(share.createdAt, now),
            modifier = Modifier.weight(1f),
        )
    }
}

@Composable
private fun InfoCard(label: String, value: String, modifier: Modifier = Modifier) {
    Card(modifier = modifier) {
        Column(
            modifier = Modifier.padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(label, style = MaterialTheme.typography.labelSmall)
            Text(value, style = MaterialTheme.typography.bodyLarge)
        }
    }
}

private fun formatRelative(instant: Instant?, now: Instant): String {
    if (instant == null) return "—"
    val diff = Duration.between(instant, now)
    return when {
        diff.isNegative -> "in the future"
        diff.seconds < 60 -> "just now"
        diff.toMinutes() < 60 -> "${diff.toMinutes()}m ago"
        diff.toHours() < 24 -> "${diff.toHours()}h ago"
        diff.toDays() < 30 -> "${diff.toDays()}d ago"
        else -> instant.atZone(java.time.ZoneId.systemDefault()).toLocalDate().toString()
    }
}
