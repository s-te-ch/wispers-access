package dev.wispers.access.android

import android.util.Log
import dev.wispers.access.android.demo.DemoMode
import dev.wispers.access.android.storage.ShareExtras
import dev.wispers.access.sdk.Client
import dev.wispers.access.sdk.Observer
import dev.wispers.access.sdk.Share
import dev.wispers.access.sdk.ShareId
import javax.inject.Inject
import javax.inject.Singleton
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * The shares as the SDK's store has them, published for the UI and reloaded
 * whenever the SDK reports a change, plus the join and leave the screens
 * call. In demo mode the roster is fixed and there is no client.
 */
@Singleton
class ShareManager @Inject constructor(
    private val sdk: SdkHolder,
    private val extras: ShareExtras,
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    /** Null until the first load, so "loading" isn't rendered as "no shares". */
    private val _shares = MutableStateFlow<List<Share>?>(null)
    val shares: StateFlow<List<Share>?> = _shares.asStateFlow()

    init {
        sdk.onShareChanged = { reload() }
        reload()
    }

    fun share(id: ShareId): Share? = _shares.value?.firstOrNull { it.id == id }

    /** Joins a share from an invite code; the SDK rolls a failed join back. */
    suspend fun join(inviteCode: String): Share {
        val client = sdk.client ?: throw IllegalStateException("Not available in demo mode.")
        val share = client.join(inviteCode.trim())
        reload()
        extras.markConnected(share.id)
        return share
    }

    /**
     * Leaves a share: drops it from the roster at once, then lets the SDK
     * tell the host node where it can and forget the share and its secrets
     * either way.
     */
    suspend fun leave(id: ShareId) {
        _shares.value = _shares.value?.filterNot { it.id == id }
        extras.forget(id)
        val client = sdk.client ?: return
        try {
            client.leave(id)
        } catch (e: Exception) {
            Log.w(TAG, "leaving share $id failed", e)
        }
        reload()
    }

    /** Re-reads the roster from the SDK's store. */
    fun reload() {
        DemoMode.shares?.let {
            _shares.value = it
            return
        }
        val client = sdk.client ?: return
        scope.launch {
            _shares.value = withContext(Dispatchers.IO) {
                try {
                    client.shares()
                } catch (e: Exception) {
                    Log.e(TAG, "reading shares failed", e)
                    _shares.value ?: emptyList()
                }
            }
        }
    }

    private companion object {
        const val TAG = "ShareManager"
    }
}

/**
 * The SDK client, created once with the app's secret store and this relay as
 * its observer; the relay is wired to the manager once there is one. `client`
 * is null in demo mode, where nothing should reach a host node.
 */
@Singleton
class SdkHolder @Inject internal constructor() : Observer {
    var client: Client? = null
        internal set

    @Volatile
    var onShareChanged: () -> Unit = {}

    override fun onShareChanged(share: Share) {
        onShareChanged.invoke()
    }
}
