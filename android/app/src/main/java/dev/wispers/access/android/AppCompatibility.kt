package dev.wispers.access.android

import dev.wispers.access.android.proxy.proxiedHostPorts
import dev.wispers.access.android.proxy.webViewSupportsForwardProxy
import dev.wispers.access.sdk.AppProxyMode
import dev.wispers.access.sdk.SharedApp

/**
 * Returns the reason why this device can't open the given app, or null if it
   _can_.
 */
fun whyUnavailable(app: SharedApp): String? = when (app.mode) {
    AppProxyMode.REVERSE_PROXY -> null
    AppProxyMode.FORWARD_PROXY -> when {
        app.startUrl == null || proxiedHostPorts(app) == null -> NEEDS_NEWER_APP
        !webViewSupportsForwardProxy -> "Needs a newer Android System WebView"
        else -> null
    }
    AppProxyMode.UNKNOWN -> NEEDS_NEWER_APP
}

private const val NEEDS_NEWER_APP = "Needs a newer version of Wispers Access"
