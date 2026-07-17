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
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FloatingActionButton
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
import dev.wispers.access.android.R
import dev.wispers.access.android.proxy.ShareAvailability
import dev.wispers.access.android.proxy.ShareStatusTracker
import dev.wispers.access.android.proxy.toAvailability
import dev.wispers.access.android.storage.Share
import dev.wispers.access.android.storage.ShareId
import dev.wispers.access.android.storage.ShareRepository
import dev.wispers.access.android.ui.theme.AccessOnSurfaceMedium
import java.time.Duration
import java.time.Instant
import javax.inject.Inject
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.stateIn

@HiltViewModel
class ShareListViewModel @Inject constructor(
    repo: ShareRepository,
    statusTracker: ShareStatusTracker,
) : ViewModel() {

    val shares: StateFlow<List<Share>> = repo.observeShares()
        .stateIn(
            scope = viewModelScope,
            started = SharingStarted.WhileSubscribed(5_000),
            initialValue = emptyList(),
        )

    /** Per-share availability; absent key = not checked yet. */
    val availability: StateFlow<Map<ShareId, ShareAvailability>> = statusTracker.statuses
}

@Composable
fun ShareListScreen(
    onAddClick: () -> Unit,
    onShareClick: (ShareId) -> Unit,
    viewModel: ShareListViewModel = hiltViewModel(),
) {
    val shares by viewModel.shares.collectAsState()
    val availability by viewModel.availability.collectAsState()

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        floatingActionButton = {
            FloatingActionButton(
                onClick = onAddClick,
                containerColor = MaterialTheme.colorScheme.primary,
                contentColor = MaterialTheme.colorScheme.onPrimary,
                shape = CircleShape,
            ) {
                Icon(Icons.Filled.Add, contentDescription = "Add share")
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
            SectionHeader(count = shares.size)
            Spacer(Modifier.height(12.dp))
            if (shares.isEmpty()) {
                EmptyShareList()
            } else {
                ShareList(shares = shares, availability = availability, onShareClick = onShareClick)
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
private fun SectionHeader(count: Int) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = "YOUR SHARES",
            style = MaterialTheme.typography.labelMedium.copy(letterSpacing = 1.5.sp),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(
            text = count.toString(),
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun ShareList(
    shares: List<Share>,
    availability: Map<ShareId, ShareAvailability>,
    onShareClick: (ShareId) -> Unit,
) {
    LazyColumn(
        modifier = Modifier.fillMaxSize(),
        verticalArrangement = Arrangement.spacedBy(8.dp),
        contentPadding = PaddingValues(bottom = 96.dp),
    ) {
        items(shares, key = { it.id.value }) { share ->
            ShareCard(
                share = share,
                // The persisted terminal state wins over (and outlives) live checks.
                availability = share.terminalState?.toAvailability() ?: availability[share.id],
                onClick = { onShareClick(share.id) },
            )
        }
    }
}

@Composable
private fun ShareCard(share: Share, availability: ShareAvailability?, onClick: () -> Unit) {
    val terminal =
        availability == ShareAvailability.REMOVED || availability == ShareAvailability.REVOKED
    Card(
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        elevation = CardDefaults.cardElevation(defaultElevation = 0.dp),
        shape = RoundedCornerShape(16.dp),
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 20.dp, vertical = 16.dp)
                .alpha(if (terminal) 0.6f else 1f),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            ShareAvatar(nickname = share.nickname, iconPng = share.iconPng)
            Spacer(Modifier.width(16.dp))
            Column(modifier = Modifier.weight(1f)) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    StatusDot(availability = availability)
                    Text(
                        text = statusLine(availability, share.lastConnectedAt),
                        style = MaterialTheme.typography.labelMedium.copy(letterSpacing = 1.sp),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Spacer(Modifier.height(4.dp))
                Text(
                    text = share.nickname.ifBlank { "Untitled share" },
                    style = MaterialTheme.typography.headlineSmall,
                    color = AccessOnSurfaceMedium,
                )
            }
            Icon(
                imageVector = Icons.AutoMirrored.Filled.KeyboardArrowRight,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
            )
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
private fun EmptyShareList() {
    Box(
        modifier = Modifier.fillMaxSize(),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = "No shares yet. Tap + to add one.",
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
