package dev.wispers.access.android.proxy

import android.util.Log
import dev.wispers.access.android.BrowseKey
import dev.wispers.access.android.SdkHolder
import dev.wispers.access.sdk.HostRoutedProxy
import javax.inject.Inject
import javax.inject.Singleton
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/**
 * The SDK's host-routed proxy for the whole app, on the fixed port the
 * pre-SDK app used, started at process start and guarded by [ProxyAuth].
 * Every app is served at `http://<app>.<share>.localhost:<port>/`, so each
 * keeps its own origin in the WebView.
 */
@Singleton
class ProxyHolder @Inject constructor(
    private val sdk: SdkHolder,
) {
    val auth = ProxyAuth()

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val proxy = CompletableDeferred<HostRoutedProxy>()

    fun start() {
        val client = sdk.client ?: return
        scope.launch {
            try {
                proxy.complete(client.startHostRoutedProxy(FIXED_PORT.toUShort(), auth.requiredCookie))
                Log.i(TAG, "Proxy listening on port $FIXED_PORT")
            } catch (e: Exception) {
                Log.e(TAG, "could not start the proxy", e)
                proxy.completeExceptionally(e)
            }
        }
    }

    /** The URL to open for an app, with the auth cookie installed for it. */
    suspend fun baseUrl(key: BrowseKey): String {
        val base = proxy.await().baseUrl(key.shareId, key.appId)
        auth.install(base)
        return base
    }

    private companion object {
        const val TAG = "ProxyHolder"
        const val FIXED_PORT = 10774
    }
}
