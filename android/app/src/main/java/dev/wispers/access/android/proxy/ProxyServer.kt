package dev.wispers.access.android.proxy

import android.util.Log
import dev.wispers.access.android.storage.ShareId
import io.ktor.http.ContentType
import io.ktor.http.HttpStatusCode
import io.ktor.server.application.ApplicationCall
import io.ktor.server.application.ApplicationCallPipeline
import io.ktor.server.application.call
import io.ktor.server.cio.CIO
import io.ktor.server.engine.EmbeddedServer
import io.ktor.server.engine.embeddedServer
import io.ktor.server.request.header
import io.ktor.server.response.respond
import io.ktor.server.response.respondText
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

        val lease = try {
            sessionManager.openStream(shareId)
        } catch (e: Exception) {
            Log.w(TAG, "openStream failed: ${e.message}")
            return respondErrorPage(call, e.message)
        }

        try {
            UpstreamClient(lease.stream, rewriter, shareId).forward(call)
        } catch (e: Exception) {
            Log.w(TAG, "proxy error: ${e.message}")
            // A failure mid-request usually means the connection died under us
            // (stream opens still succeed on a dead connection) — evict it so the
            // next request reconnects instead of hitting the same corpse.
            sessionManager.invalidate(shareId, lease)
            // If we haven't responded yet, surface the error page; if we have, this is a no-op.
            runCatching { respondErrorPage(call, e.message) }
        } finally {
            runCatching { lease.stream.close() }
        }
    }

    private suspend fun respondErrorPage(call: ApplicationCall, message: String?) {
        val safe = (message ?: "unknown error")
            .replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
        call.respondText(
            """
            <!doctype html>
            <html>
            <head>
            <meta name="viewport" content="width=device-width, initial-scale=1">
            <title>Connection problem</title>
            <style>
              /* Matches the app theme (ui/theme/Color.kt): cream background,
                 light-green primary with dark-forest text. */
              body { font-family: sans-serif; margin: 0; min-height: 100vh; display: flex;
                     align-items: center; justify-content: center; background: #f7f7f2; color: #1a1a1a; }
              main { text-align: center; padding: 2rem; max-width: 28rem; }
              p { color: #6b6b68; font-size: 0.85rem; overflow-wrap: anywhere; }
              button { font-size: 1rem; padding: 0.75rem 2.5rem; border: none; border-radius: 1.5rem;
                       background: #a1d283; color: #4a6b36; font-weight: 600; }
            </style>
            </head>
            <body>
            <main>
              <h2>Connection problem</h2>
              <p>$safe</p>
              <button onclick="location.reload()">Retry</button>
            </main>
            </body>
            </html>
            """.trimIndent(),
            ContentType.Text.Html,
            HttpStatusCode.BadGateway,
        )
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
