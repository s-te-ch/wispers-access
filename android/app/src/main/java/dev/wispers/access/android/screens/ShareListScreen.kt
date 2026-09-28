package dev.wispers.access.android.screens

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.wispers.access.android.BrowseKey
import dev.wispers.access.android.R
import dev.wispers.access.android.ShareManager
import dev.wispers.access.android.demo.DemoMode
import dev.wispers.access.android.proxy.ShareAvailability
import dev.wispers.access.android.proxy.ShareStatusTracker
import dev.wispers.access.android.proxy.toAvailability
import dev.wispers.access.android.storage.ShareExtras
import dev.wispers.access.android.ui.theme.AccessOnSurfaceMedium
import dev.wispers.access.sdk.Share
import dev.wispers.access.sdk.ShareId
import dev.wispers.access.sdk.ShareState
import dev.wispers.access.sdk.SharedApp
import java.time.Duration
import java.time.Instant
import javax.inject.Inject
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.stateIn

@HiltViewModel
class ShareListViewModel @Inject constructor(
    manager: ShareManager,
    statusTracker: ShareStatusTracker,
    extras: ShareExtras,
) : ViewModel() {

    /** Null until the first load, so "loading" isn't rendered as "no shares". */
    val shares: StateFlow<List<Share>?> = manager.shares

    /** Per-share availability; absent key = not checked yet. */
    val availability: StateFlow<Map<ShareId, ShareAvailability>> = statusTracker.statuses

    val lastConnected: StateFlow<Map<ShareId, Instant>> =
        (if (DemoMode.active) flowOf(DemoMode.lastConnected) else extras.observeLastConnected())
            .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), emptyMap())

    val icons: StateFlow<Map<BrowseKey, ByteArray>> =
        (if (DemoMode.active) flowOf(DemoMode.icons) else extras.observeIcons())
            .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), emptyMap())
}

/**
 * The roster: the apps shared with you, grouped by the share they come from.
 * A share's header carries its name and status and leads to its detail
 * screen; each app underneath is a card that opens in one tap.
 */
@Composable
fun ShareListScreen(
    onAddClick: () -> Unit,
    onShareClick: (Share) -> Unit,
    onAppClick: (BrowseKey) -> Unit,
    viewModel: ShareListViewModel = hiltViewModel(),
) {
    // Plain value read (not `by`) so the null check below smart-casts.
    val shares = viewModel.shares.collectAsState().value
    val availability by viewModel.availability.collectAsState()
    val lastConnected by viewModel.lastConnected.collectAsState()
    val icons by viewModel.icons.collectAsState()

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        floatingActionButton = {
            FloatingActionButton(
                onClick = onAddClick,
                containerColor = MaterialTheme.colorScheme.primary,
                contentColor = MaterialTheme.colorScheme.onPrimary,
                shape = CircleShape,
            ) {
                Icon(Icons.Filled.Add, contentDescription = "Add a share")
            }
        },
    ) { innerPadding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(innerPadding)
                .padding(horizontal = 24.dp),
        ) {
            Spacer(Modifier.height(24.dp))
            BrandHeader()
            Spacer(Modifier.height(40.dp))
            SectionHeader(count = shares?.sumOf { it.apps.size })
            Spacer(Modifier.height(12.dp))
            when {
                shares == null -> LoadingShareList()
                shares.isEmpty() -> EmptyShareList()
                else -> ShareSections(
                    shares = shares,
                    availability = availability,
                    lastConnected = lastConnected,
                    icons = icons,
                    onShareClick = onShareClick,
                    onAppClick = onAppClick,
                )
            }
        }
    }
}

@Composable
private fun BrandHeader() {
    Box(
        modifier = Modifier.fillMaxWidth(),
        contentAlignment = Alignment.Center,
    ) {
        Image(
            painter = painterResource(R.drawable.wispers_access_logo),
            contentDescription = "Wispers Access",
            modifier = Modifier.height(48.dp),
        )
    }
}

@Composable
private fun SectionHeader(count: Int?) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = "SHARED WITH YOU",
            style = MaterialTheme.typography.labelMedium.copy(letterSpacing = 1.5.sp),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(
            text = count?.toString() ?: "",
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

/** One row of the list: a share's header or one of its app cards. */
private sealed interface RosterRow {
    val key: String

    data class Header(val share: Share) : RosterRow {
        override val key get() = "share:${share.id}"
    }

    data class AppRow(val share: Share, val app: SharedApp) : RosterRow {
        override val key get() = "app:${share.id}/${app.id}"
    }

    data class NoApps(val share: Share) : RosterRow {
        override val key get() = "no-apps:${share.id}"
    }
}

@Composable
private fun ShareSections(
    shares: List<Share>,
    availability: Map<ShareId, ShareAvailability>,
    lastConnected: Map<ShareId, Instant>,
    icons: Map<BrowseKey, ByteArray>,
    onShareClick: (Share) -> Unit,
    onAppClick: (BrowseKey) -> Unit,
) {
    val rows = shares.flatMap { share ->
        listOf(RosterRow.Header(share)) +
            if (share.apps.isEmpty()) listOf(RosterRow.NoApps(share))
            else share.apps.map { RosterRow.AppRow(share, it) }
    }
    LazyColumn(
        modifier = Modifier.fillMaxSize(),
        verticalArrangement = Arrangement.spacedBy(8.dp),
        contentPadding = PaddingValues(bottom = 96.dp),
    ) {
        items(rows, key = { it.key }) { row ->
            when (row) {
                is RosterRow.Header -> ShareHeader(
                    share = row.share,
                    // The share's own terminal state wins over (and outlives) live checks.
                    availability = row.share.state.toAvailability() ?: availability[row.share.id],
                    lastConnected = lastConnected[row.share.id],
                    onClick = { onShareClick(row.share) },
                )
                is RosterRow.AppRow -> {
                    val key = BrowseKey(row.share.id, row.app.id)
                    val live = row.share.state == ShareState.LIVE
                    AppCard(
                        app = row.app,
                        iconPng = icons[key],
                        enabled = live,
                        // A terminal share's apps lead to the explanation instead.
                        onClick = { if (live) onAppClick(key) else onShareClick(row.share) },
                    )
                }
                is RosterRow.NoApps -> Text(
                    text = "No apps shared yet.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 20.dp, vertical = 12.dp),
                )
            }
        }
    }
}

/** A share's line above its apps: status, name, and the ⓘ that leads to its detail screen. */
@Composable
private fun ShareHeader(
    share: Share,
    availability: ShareAvailability?,
    lastConnected: Instant?,
    onClick: () -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(horizontal = 8.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(modifier = Modifier.weight(1f)) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                StatusDot(availability = availability)
                Text(
                    text = statusLine(availability, lastConnected),
                    style = MaterialTheme.typography.labelMedium.copy(letterSpacing = 1.sp),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Spacer(Modifier.height(4.dp))
            Text(
                text = share.name.ifBlank { "Untitled share" },
                style = MaterialTheme.typography.headlineSmall,
                color = AccessOnSurfaceMedium,
            )
        }
        Icon(
            imageVector = Icons.Outlined.Info,
            contentDescription = "Share details",
            tint = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

/**
 * One app's card: its icon (harvested while browsing, else a letter tile) and
 * name; tapping the row opens the app. An optional [footer] sits inside the
 * card under a divider, for actions that belong to this app, and is not part
 * of the tap target.
 */
@Composable
fun AppCard(
    app: SharedApp,
    iconPng: ByteArray?,
    enabled: Boolean,
    onClick: () -> Unit,
    footer: (@Composable () -> Unit)? = null,
) {
    val name = app.name.ifBlank { app.id }
    Card(
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        elevation = CardDefaults.cardElevation(defaultElevation = 0.dp),
        shape = RoundedCornerShape(16.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .clickable(onClick = onClick)
                .padding(horizontal = 20.dp, vertical = 16.dp)
                .alpha(if (enabled) 1f else 0.6f),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Avatar(nickname = name, iconPng = iconPng)
            Spacer(Modifier.width(16.dp))
            Text(
                text = name,
                style = MaterialTheme.typography.headlineSmall,
                color = AccessOnSurfaceMedium,
                modifier = Modifier.weight(1f),
            )
            Icon(
                imageVector = Icons.AutoMirrored.Filled.KeyboardArrowRight,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        if (footer != null) {
            HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 8.dp, vertical = 4.dp),
                horizontalArrangement = Arrangement.End,
            ) {
                footer()
            }
        }
    }
}

@Composable
private fun StatusDot(availability: ShareAvailability?) {
    if (availability == null) {
        CircularProgressIndicator(
            modifier = Modifier.size(8.dp),
            strokeWidth = 1.dp,
            color = MaterialTheme.colorScheme.outline,
        )
    } else {
        Box(
            modifier = Modifier
                .size(8.dp)
                .background(
                    color = when (availability) {
                        ShareAvailability.ONLINE -> MaterialTheme.colorScheme.primary
                        ShareAvailability.REMOVED, ShareAvailability.REVOKED ->
                            MaterialTheme.colorScheme.error
                        else -> MaterialTheme.colorScheme.outline
                    },
                    shape = CircleShape,
                ),
        )
    }
}

@Composable
private fun LoadingShareList() {
    Box(
        modifier = Modifier.fillMaxSize(),
        contentAlignment = Alignment.Center,
    ) {
        CircularProgressIndicator(
            modifier = Modifier.size(32.dp),
            strokeWidth = 3.dp,
            color = MaterialTheme.colorScheme.primary,
        )
    }
}

@Composable
private fun EmptyShareList() {
    Box(
        modifier = Modifier.fillMaxSize(),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = "No apps yet. Tap + to add a share.",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

private fun statusLine(availability: ShareAvailability?, lastConnected: Instant?): String {
    val status = when (availability) {
        ShareAvailability.ONLINE -> "ONLINE"
        ShareAvailability.OFFLINE -> "OFFLINE"
        ShareAvailability.UNKNOWN -> "UNKNOWN"
        ShareAvailability.REMOVED, ShareAvailability.REVOKED -> return "NO LONGER SHARED"
        null -> "CHECKING"
    }
    return "$status · LAST ${formatLastConnectedShort(lastConnected)}"
}

private fun formatLastConnectedShort(instant: Instant?): String {
    if (instant == null) return "NEVER"
    val diff = Duration.between(instant, Instant.now())
    return when {
        diff.seconds < 60 -> "JUST NOW"
        diff.toMinutes() < 60 -> "${diff.toMinutes()}M AGO"
        diff.toHours() < 24 -> "${diff.toHours()}H AGO"
        diff.toDays() < 7 -> "${diff.toDays()}D AGO"
        else -> "${diff.toDays() / 7}W AGO"
    }
}
