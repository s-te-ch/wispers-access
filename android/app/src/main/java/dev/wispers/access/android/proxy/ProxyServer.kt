package dev.wispers.access.android.proxy

import android.util.Log
import dev.wispers.access.android.storage.ShareId
import io.ktor.http.HttpStatusCode
import io.ktor.server.application.ApplicationCall
import io.ktor.server.application.ApplicationCallPipeline
import io.ktor.server.application.call
import io.ktor.server.cio.CIO
import io.ktor.server.engine.EmbeddedServer
import io.ktor.server.engine.embeddedServer
import io.ktor.server.request.header
import io.ktor.server.response.respond
import javax.inject.Inject
import javax.inject.Singleton

/**
 * Local HTTP/1 proxy server. Equivalent of waclient's `serve()` loop.
 *
 * Binds Ktor (CIO engine) to 127.0.0.1:[FIXED_PORT]. Every incoming HTTP request is
 * routed by the Host header's leading subdomain (`<shareId>.localhost`) to the
 * corresponding share, and forwarded through a freshly-opened QUIC stream by
 * [UpstreamClient]. Header rewrites in both directions go through [HeaderRewriter].
 */
@Singleton
class ProxyServer @Inject constructor(
    private val sessionManager: SessionManager,
    private val rewriter: HeaderRewriter,
) {
    val port: Int = FIXED_PORT

    @Volatile
    private var server: EmbeddedServer<*, *>? = null

    @Synchronized
    fun start() {
        if (server != null) return
        server = embeddedServer(CIO, host = "127.0.0.1", port = FIXED_PORT) {
            intercept(ApplicationCallPipeline.Call) {
                handleProxy(call)
                finish()
            }
        }.also { it.start(wait = false) }
        Log.i(TAG, "Proxy listening on 127.0.0.1:$FIXED_PORT")
    }

    private suspend fun handleProxy(call: ApplicationCall) {
        val host = call.request.header("Host")
            ?: return call.respond(HttpStatusCode.BadRequest, "missing Host header")
        val shareId = parseShareFromHost(host)
            ?: return call.respond(HttpStatusCode.NotFound, "unknown host")

        val stream = try {
            sessionManager.openStream(shareId)
        } catch (e: Exception) {
            Log.w(TAG, "openStream failed: ${e.message}")
            return call.respond(HttpStatusCode.BadGateway, "Wispers Access server unavailable")
        }

        try {
            UpstreamClient(stream, rewriter, shareId).forward(call)
        } catch (e: Exception) {
            Log.w(TAG, "proxy error: ${e.message}")
            // If we haven't responded yet, surface as 502; if we have, this is a no-op.
            runCatching { call.respond(HttpStatusCode.BadGateway, e.message ?: "proxy error") }
        } finally {
            runCatching { stream.close() }
        }
    }

    private fun parseShareFromHost(host: String): ShareId? {
        val noPort = host.substringBefore(':').lowercase()
        val dot = noPort.indexOf('.')
        if (dot < 0) return null
        val tail = noPort.substring(dot + 1)
        if (tail != "localhost") return null
        return ShareId(noPort.substring(0, dot))
    }

    private companion object {
        const val FIXED_PORT = 10774
        const val TAG = "ProxyServer"
    }
}
