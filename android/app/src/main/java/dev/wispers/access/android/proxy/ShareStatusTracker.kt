package dev.wispers.access.android.proxy

import android.content.Context
import android.util.Log
import dagger.hilt.android.qualifiers.ApplicationContext
import dev.wispers.access.android.SdkHolder
import dev.wispers.access.android.ShareManager
import dev.wispers.access.android.demo.DemoMode
import dev.wispers.access.android.disableShortcuts
import dev.wispers.access.android.storage.ShareExtras
import dev.wispers.access.sdk.SdkException
import dev.wispers.access.sdk.Share
import dev.wispers.access.sdk.ShareId
import dev.wispers.access.sdk.ShareState
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
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/**
 * Single source of truth for per-share availability, shared by all screens so
 * they can never disagree. Absent key = not checked yet. A check is the SDK's
 * `refresh`, which reaches the host node over the share's transport: an
 * answer means online, a refusal offline.
 *
 * Shares are checked concurrently, each under its own deadline, so one host
 * node that blackholes can't wedge the others' status. A share the SDK
 * reports terminal drops out of polling for good; the UI renders the state
 * from the share itself from then on.
 *
 * Polls while anyone subscribes to [statuses] and goes quiet otherwise.
 */
@Singleton
class ShareStatusTracker @Inject constructor(
    private val manager: ShareManager,
    private val sdk: SdkHolder,
    private val extras: ShareExtras,
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
        // Demo roster: fixed statuses, no host to poll.
        if (DemoMode.active) {
            _statuses.value = DemoMode.statuses
            return
        }
        manager.shares.filterNotNull().collectLatest { shares ->
            while (true) {
                coroutineScope {
                    for (share in shares) {
                        // Terminal is forever; the share's own state renders.
                        if (share.state != ShareState.LIVE) continue
                        launch { checkShare(share) }
                    }
                }
                delay(REFRESH_MS)
            }
        }
    }

    private suspend fun checkShare(share: Share) {
        val client = sdk.client ?: return
        val availability = try {
            // `refresh` answers null when nothing changed, which is the healthy
            // case; only the deadline running out is unknown.
            val answer = withTimeoutOrNull(CHECK_TIMEOUT_MS) { Answer(client.refresh(share.id)) }
                ?: return publish(share.id, ShareAvailability.UNKNOWN)
            answer.changed?.state?.toAvailability() ?: ShareAvailability.ONLINE
        } catch (e: SdkException.HostNode) {
            ShareAvailability.OFFLINE
        } catch (e: SdkException) {
            Log.w(TAG, "checking share ${share.id} failed", e)
            ShareAvailability.UNKNOWN
        }
        when (availability) {
            ShareAvailability.ONLINE -> extras.markConnected(share.id)
            ShareAvailability.REMOVED, ShareAvailability.REVOKED -> {
                Log.i(TAG, "share ${share.id} is terminal: $availability")
                // A pinned shortcut can't be deleted, but disabling greys it
                // and shows the message on tap.
                disableShortcuts(context, share, SHORTCUT_DISABLED_MESSAGE)
            }
            else -> Unit
        }
        publish(share.id, availability)
    }

    /** The host node's answer to a refresh: the share if it changed, else null. */
    private class Answer(val changed: Share?)

    private fun publish(id: ShareId, availability: ShareAvailability) {
        _statuses.update { it + (id to availability) }
    }

    private companion object {
        const val TAG = "ShareStatusTracker"
        const val REFRESH_MS = 30_000L

        // Generous per-share deadline: a reachable host node answers in well
        // under a second; only a blackholing connect runs into this.
        const val CHECK_TIMEOUT_MS = 10_000L

        const val SHORTCUT_DISABLED_MESSAGE = "This app is no longer available"
    }
}
