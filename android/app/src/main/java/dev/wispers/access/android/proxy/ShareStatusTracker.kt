package dev.wispers.access.android.proxy

import dev.wispers.access.android.storage.ShareId
import dev.wispers.access.android.storage.ShareRepository
import javax.inject.Inject
import javax.inject.Singleton
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

/**
 * Single source of truth for per-share serving-node availability, shared by all
 * screens so they can never disagree. Absent key = not checked yet; null value =
 * status unknown (hub unreachable, typically this device is offline).
 *
 * Polls while anyone subscribes to [statuses] and goes quiet otherwise, so
 * browsing a share or backgrounding the app doesn't keep hitting the hub.
 */
@Singleton
class ShareStatusTracker @Inject constructor(
    private val repo: ShareRepository,
    private val sessionManager: SessionManager,
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    private val _statuses = MutableStateFlow<Map<ShareId, Boolean?>>(emptyMap())
    val statuses: StateFlow<Map<ShareId, Boolean?>> = _statuses.asStateFlow()

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
                _statuses.value =
                    shares.associate { it.id to sessionManager.isServingNodeOnline(it.id) }
                delay(REFRESH_MS)
            }
        }
    }

    private companion object {
        const val REFRESH_MS = 30_000L
    }
}
