package dev.wispers.access.android.proxy

import android.webkit.CookieManager
import dev.wispers.access.sdk.RequiredCookie
import java.security.SecureRandom

/**
 * Authenticates loopback-proxy requests as coming from this app's own web
 * views. The loopback port is reachable by every process on the device, so
 * the SDK's proxy demands a secret only our web views hold: a cookie set for
 * each app's `<app>.<share>.localhost` origin before its first load. The
 * secret is fresh per launch.
 */
class ProxyAuth {
    private val secret: String =
        ByteArray(16).also { SecureRandom().nextBytes(it) }.joinToString("") { "%02x".format(it) }

    /** What the SDK's proxy requires on every request. */
    val requiredCookie: RequiredCookie get() = RequiredCookie(COOKIE_NAME, secret)

    /** Installs the cookie for an app's origin, so its web view gets through. */
    fun install(baseUrl: String) {
        CookieManager.getInstance().setCookie(baseUrl, "$COOKIE_NAME=$secret")
    }

    private companion object {
        const val COOKIE_NAME = "__wispers_proxy_auth"
    }
}
