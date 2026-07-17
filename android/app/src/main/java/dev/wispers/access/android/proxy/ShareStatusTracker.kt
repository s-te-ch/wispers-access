package dev.wispers.access.android.proxy

import android.content.Context
import android.util.Log
import dagger.hilt.android.qualifiers.ApplicationContext
import dev.wispers.access.android.disableShortcut
import dev.wispers.access.android.storage.Share
import dev.wispers.access.android.storage.ShareId
import dev.wispers.access.android.storage.ShareRepository
import dev.wispers.access.android.storage.ShareTerminalState
import javax.inject.Inject
import javax.inject.Singleton
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/**
 * Single source of truth for per-share availability, shared by all screens so
 * they can never disagree. Absent key = not checked yet.
 *
 * Shares are checked concurrently, each under its own deadline, and results are
 * published per share — one hub that blackholes (an unreachable self-hosted
 * backend hangs the connect for minutes) must not wedge the others' status.
 *
 * A check that comes back terminal (REMOVED/REVOKED) is persisted to the share
 * row and the share drops out of polling for good; the UI renders the persisted
 * state from then on, hub or no hub.
 *
 * Polls while anyone subscribes to [statuses] and goes quiet otherwise, so
 * browsing a share or backgrounding the app doesn't keep hitting the hub.
 */
@Singleton
class ShareStatusTracker @Inject constructor(
    private val repo: ShareRepository,
    private val sessionManager: SessionManager,
    @param:ApplicationContext private val context: Context,
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    private val _statuses = MutableStateFlow<Map<ShareId, ShareAvailability>>(emptyMap())
    val statuses: StateFlow<Map<ShareId, ShareAvailability>> = _statuses.asStateFlow()

    init {
        scope.launch {
            _statuses.subscriptionCount
                .map { it > 0 }
                .distinctUntilChanged()
                .collectLatest { active ->
                    if (active) pollLoop()
                }
        }
    }

    private suspend fun pollLoop() {
        repo.observeShares().collectLatest { shares ->
            while (true) {
                coroutineScope {
                    for (share in shares) {
                        // Terminal is forever; the persisted state renders directly.
                        if (share.terminalState != null) continue
                        launch { checkShare(share) }
                    }
                }
                delay(REFRESH_MS)
            }
        }
    }

    private suspend fun checkShare(share: Share) {
        val availability = withTimeoutOrNull(CHECK_TIMEOUT_MS) {
            sessionManager.checkAvailability(share.id)
        } ?: ShareAvailability.UNKNOWN
        when (availability) {
            ShareAvailability.REMOVED -> markTerminal(share, ShareTerminalState.REMOVED)
            ShareAvailability.REVOKED -> markTerminal(share, ShareTerminalState.REVOKED)
            else -> Unit
        }
        _statuses.update { it + (share.id to availability) }
    }

    private suspend fun markTerminal(share: Share, state: ShareTerminalState) {
        Log.i(TAG, "share ${share.id.value} is terminal: $state")
        repo.markTerminal(share.id, state)
        // A pinned shortcut can't be deleted, but disabling greys it and shows
        // the message on tap — better than launching a dead WebView.
        disableShortcut(context, share.id, SHORTCUT_DISABLED_MESSAGE)
    }

    private companion object {
        const val TAG = "ShareStatusTracker"
        const val REFRESH_MS = 30_000L

        // Generous per-share deadline: a healthy hub answers in well under a
        // second; only a blackholing connect runs into this.
        const val CHECK_TIMEOUT_MS = 10_000L

        const val SHORTCUT_DISABLED_MESSAGE = "This share is no longer available"
    }
}
