package dev.wispers.access.android.proxy

import android.util.Log
import androidx.webkit.ProfileStore
import androidx.webkit.ProxyConfig
import androidx.webkit.ProxyController
import androidx.webkit.WebViewFeature
import dev.wispers.access.android.SdkHolder
import dev.wispers.access.android.ShareManager
import dev.wispers.access.sdk.ForwardProxy
import dev.wispers.access.sdk.ProxyCredential
import dev.wispers.access.sdk.ShareId
import dev.wispers.access.sdk.SharedApp
import java.net.URI
import javax.inject.Inject
import javax.inject.Singleton
import kotlin.coroutines.resume
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext

/**
 * Holder for the SDK's forward proxy and the WebView proxy override that sends
 * their origins to it. The override is process-wide, so it lists the proxied
 * origins of every share.
 */
@Singleton
class ForwardProxyHolder @Inject constructor(
    private val sdk: SdkHolder,
    private val manager: ShareManager,
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val proxy = CompletableDeferred<ForwardProxy>()

    /** The `host:port`s the override sends to the proxy, null until first set. */
    private val routed = MutableStateFlow<Set<String>?>(null)

    fun start() {
        if (!webViewSupportsForwardProxy) return
        val client = sdk.client ?: return
        scope.launch {
            val started = try {
                client.startForwardProxy(0u)
            } catch (e: Exception) {
                Log.e(TAG, "could not start the forward proxy", e)
                proxy.completeExceptionally(e)
                return@launch
            }
            proxy.complete(started)
            Log.i(TAG, "Forward proxy listening on port ${started.port()}")
            manager.shares.filterNotNull()
                .map { shares -> shares.flatMap { it.apps }.flatMap { proxiedHostPorts(it).orEmpty() }.toSet() }
                .distinctUntilChanged()
                .collect { hostPorts ->
                    route(started.port(), hostPorts)
                    routed.value = hostPorts
                }
        }
    }

    /**
     * What the WebView of a forward-proxy app needs to go through the proxy,
     * once the override sends the app's origins to it.
     */
    suspend fun webViewProxyParams(shareId: ShareId, app: SharedApp): WebViewProxyParams {
        val started = proxy.await()
        val hostPorts = proxiedHostPorts(app).orEmpty()
        routed.first { it != null && it.containsAll(hostPorts) }
        return WebViewProxyParams(
            profile = profileName(shareId),
            credential = started.credential(shareId),
            realm = started.realm(),
        )
    }

    /**
     * Deletes the WebView profile of a share that was left, with its cookies
     * and storage. On the main thread, as WebView wants.
     */
    fun forget(shareId: ShareId) {
        if (!webViewSupportsForwardProxy) return
        // Throws if the share never had a profile, or a WebView of it is
        // still open; either way there's nothing to delete now.
        runCatching { ProfileStore.getInstance().deleteProfile(profileName(shareId)) }
    }

    /** What a forward-proxy app's WebView needs to go through the proxy. */
    data class WebViewProxyParams(
        /**
         * The share's WebView profile. WebView keeps proxy credentials per
         * profile, so this is what keeps one share's credential out of
         * another share's WebViews. Cookies and storage are per profile too.
         */
        val profile: String,
        /** What the WebView answers the proxy's 407 with. */
        val credential: ProxyCredential,
        /** The proxy's realm, see [isProxyChallenge]. */
        val realm: String,
    ) {
        /** Whether a challenge is the proxy's, so the credential answers it. */
        fun isProxyChallenge(host: String?, realm: String?): Boolean =
            host == PROXY_HOST && realm == this.realm
    }

    /** Points the override at the proxy for `hostPorts`, or clears it if none. */
    private suspend fun route(port: UShort, hostPorts: Set<String>) = withContext(Dispatchers.Main) {
        suspendCancellableCoroutine { applied ->
            val controller = ProxyController.getInstance()
            if (hostPorts.isEmpty()) {
                controller.clearProxyOverride(Runnable::run) { applied.resume(Unit) }
                return@suspendCancellableCoroutine
            }
            val config = ProxyConfig.Builder()
                .addProxyRule("$PROXY_HOST:$port")
                // Loopback goes direct by default, but a dev setup's
                // `localhost` origins are the host node's.
                .removeImplicitRules()
                .apply { hostPorts.forEach { addBypassRule(it) } }
                .setReverseBypassEnabled(true)
                .build()
            controller.setProxyOverride(config, Runnable::run) { applied.resume(Unit) }
        }
        Log.i(TAG, "Forward proxy routes ${hostPorts.ifEmpty { "nothing" }}")
    }

    private fun profileName(shareId: ShareId) = "share-$shareId"

    private companion object {
        const val TAG = "ForwardProxyHolder"
        const val PROXY_HOST = "127.0.0.1"
    }
}

/**
 * Whether this device's WebView can open forward-proxy apps: it needs a proxy
 * override with reverse bypass, and profiles (WebView 123 and later).
 */
val webViewSupportsForwardProxy: Boolean by lazy {
    listOf(
        WebViewFeature.PROXY_OVERRIDE,
        WebViewFeature.PROXY_OVERRIDE_REVERSE_BYPASS,
        WebViewFeature.MULTI_PROFILE,
    ).all(WebViewFeature::isFeatureSupported)
}

/**
 * The app's proxied origins as `host:port`, the form the override lists them
 * in, so `ws://` URLs to the same port match as well. Null if one of them
 * doesn't parse, which makes the app one this version can't open.
 */
fun proxiedHostPorts(app: SharedApp): List<String>? = app.proxiedOrigins.map { origin ->
    val uri = runCatching { URI(origin) }.getOrNull() ?: return null
    val host = uri.host ?: return null
    val port = when {
        uri.port != -1 -> uri.port
        uri.scheme == "https" -> 443
        uri.scheme == "http" -> 80
        else -> return null
    }
    "$host:$port"
}
