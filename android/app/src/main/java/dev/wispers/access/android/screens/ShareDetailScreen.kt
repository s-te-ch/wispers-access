package dev.wispers.access.android.screens

import android.content.Context
import android.widget.Toast
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.AlertDialog
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import dagger.hilt.android.qualifiers.ApplicationContext
import dev.wispers.access.android.BrowseKey
import dev.wispers.access.android.ShareManager
import dev.wispers.access.android.addToHomescreen
import dev.wispers.access.android.demo.DemoMode
import dev.wispers.access.android.disableShortcuts
import dev.wispers.access.android.proxy.ShareAvailability
import dev.wispers.access.android.proxy.ShareStatusTracker
import dev.wispers.access.android.proxy.toAvailability
import dev.wispers.access.android.storage.ShareExtras
import dev.wispers.access.sdk.Share
import dev.wispers.access.sdk.ShareId
import dev.wispers.access.sdk.ShareState
import dev.wispers.access.sdk.SharedApp
import java.time.Duration
import java.time.Instant
import javax.inject.Inject
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch

@HiltViewModel
class ShareDetailViewModel @Inject constructor(
    savedStateHandle: SavedStateHandle,
    private val manager: ShareManager,
    private val extras: ShareExtras,
    statusTracker: ShareStatusTracker,
    @param:ApplicationContext private val appContext: Context,
) : ViewModel() {

    private val shareId: ShareId = checkNotNull(savedStateHandle["shareId"]) { "shareId arg required" }

    val share: StateFlow<Share?> = manager.shares
        .map { shares -> shares?.firstOrNull { it.id == shareId } }
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    /**
     * Availability; null = not known (yet). Shared with the list screen. The
     * share's own terminal state wins over live checks.
     */
    val availability: StateFlow<ShareAvailability?> =
        combine(share, statusTracker.statuses) { share, statuses ->
            share?.state?.toAvailability() ?: statuses[shareId]
        }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    val lastConnected: StateFlow<Instant?> =
        (if (DemoMode.active) flowOf(DemoMode.lastConnected) else extras.observeLastConnected())
            .map { it[shareId] }
            .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    val icons: StateFlow<Map<BrowseKey, ByteArray>> =
        (if (DemoMode.active) flowOf(DemoMode.icons) else extras.observeIcons())
            .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), emptyMap())

    private val _removed = MutableStateFlow(false)
    val removed: StateFlow<Boolean> = _removed.asStateFlow()

    fun onRemove() {
        val share = share.value ?: return
        viewModelScope.launch {
            disableShortcuts(appContext, share, "This app was removed")
            manager.leave(share.id)
            _removed.value = true
        }
    }

    /** Pins a home-screen shortcut for one app; false if the launcher can't. */
    suspend fun addToHomescreen(share: Share, app: SharedApp): Boolean =
        addToHomescreen(appContext, share, app, extras.icon(BrowseKey(share.id, app.id)))
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ShareDetailScreen(
    onBack: () -> Unit,
    onOpenApp: (BrowseKey) -> Unit,
    viewModel: ShareDetailViewModel = hiltViewModel(),
) {
    val share by viewModel.share.collectAsState()
    val availability by viewModel.availability.collectAsState()
    val lastConnected by viewModel.lastConnected.collectAsState()
    val icons by viewModel.icons.collectAsState()
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
                title = { Text("Shared with you") },
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
                availability = availability,
                lastConnected = lastConnected,
                icons = icons,
                contentPadding = innerPadding,
                onOpenApp = onOpenApp,
                onAddToHomescreen = { app ->
                    viewModel.viewModelScope.launch {
                        if (!viewModel.addToHomescreen(current, app)) {
                            Toast.makeText(
                                context,
                                "Your launcher doesn't support adding shortcuts",
                                Toast.LENGTH_SHORT,
                            ).show()
                        }
                    }
                },
                onRemove = { confirmRemove = true },
            )
        }
    }

    if (confirmRemove) {
        AlertDialog(
            onDismissRequest = { confirmRemove = false },
            // Name the thing being removed so this can't read as uninstalling the app itself.
            title = {
                val name = share?.name?.takeIf { it.isNotBlank() } ?: "this share"
                Text("Remove $name?")
            },
            text = {
                Text("This device's access will be removed on the host. You'll need a new invitation code to rejoin.")
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
    availability: ShareAvailability?,
    lastConnected: Instant?,
    icons: Map<BrowseKey, ByteArray>,
    contentPadding: PaddingValues,
    onOpenApp: (BrowseKey) -> Unit,
    onAddToHomescreen: (SharedApp) -> Unit,
    onRemove: () -> Unit,
) {
    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(contentPadding)
            .verticalScroll(rememberScrollState())
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        StatusRow(availability = availability)
        val firstIcon = share.apps.firstNotNullOfOrNull { icons[BrowseKey(share.id, it.id)] }
        Avatar(nickname = share.name, iconPng = firstIcon, size = 64.dp)
        Text(
            text = share.name.ifBlank { "Untitled share" },
            style = MaterialTheme.typography.headlineLarge,
        )
        InfoCardsRow(share = share, lastConnected = lastConnected)
        if (share.state != ShareState.LIVE) {
            TerminalShareExplanation(state = share.state)
        } else {
            Text(
                text = "APPS",
                style = MaterialTheme.typography.labelMedium.copy(letterSpacing = 1.5.sp),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (share.apps.isEmpty()) {
                Text(
                    "No apps shared yet. They appear here once the host adds some.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            for (app in share.apps) {
                val key = BrowseKey(share.id, app.id)
                AppCard(
                    app = app,
                    iconPng = icons[key],
                    enabled = true,
                    onClick = { onOpenApp(key) },
                    footer = {
                        TextButton(onClick = { onAddToHomescreen(app) }) {
                            Text("Add to homescreen")
                        }
                    },
                )
            }
            Text(
                "When the dialog appears, drag the icon onto your home screen. " +
                    "(On some phones the “Add automatically” button does nothing.)",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        OutlinedButton(
            onClick = onRemove,
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text("Remove from this device", color = MaterialTheme.colorScheme.error)
        }
    }
}

@Composable
private fun StatusRow(availability: ShareAvailability?) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (availability == null) {
            CircularProgressIndicator(
                modifier = Modifier.size(10.dp),
                strokeWidth = 1.5.dp,
                color = MaterialTheme.colorScheme.outline,
            )
        } else {
            Box(
                modifier = Modifier
                    .size(10.dp)
                    .background(
                        color = when (availability) {
                            ShareAvailability.ONLINE -> Color(0xFF34A853)
                            ShareAvailability.REMOVED, ShareAvailability.REVOKED ->
                                MaterialTheme.colorScheme.error
                            else -> MaterialTheme.colorScheme.outline
                        },
                        shape = CircleShape,
                    ),
            )
        }
        Text(
            text = when (availability) {
                ShareAvailability.ONLINE -> "ONLINE"
                ShareAvailability.OFFLINE -> "OFFLINE"
                ShareAvailability.UNKNOWN -> "UNKNOWN"
                ShareAvailability.REMOVED, ShareAvailability.REVOKED -> "NO LONGER AVAILABLE"
                null -> "CHECKING…"
            },
            style = MaterialTheme.typography.labelMedium,
        )
    }
}

@Composable
private fun InfoCardsRow(share: Share, lastConnected: Instant?) {
    val now = Instant.now()
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        InfoCard(
            label = "LAST CONNECTED",
            value = formatRelative(lastConnected, now),
            modifier = Modifier.weight(1f),
        )
        InfoCard(
            label = "JOINED",
            value = formatRelative(share.joinedAt, now),
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
