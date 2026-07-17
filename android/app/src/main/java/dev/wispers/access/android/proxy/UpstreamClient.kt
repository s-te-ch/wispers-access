package dev.wispers.access.android.proxy

import android.util.Log
import dev.wispers.access.android.storage.ShareId
import dev.wispers.connect.handles.QuicStream
import io.ktor.http.Headers
import io.ktor.http.HttpHeaders
import io.ktor.http.HttpStatusCode
import io.ktor.http.content.OutgoingContent
import io.ktor.server.application.ApplicationCall
import io.ktor.server.request.httpMethod
import io.ktor.server.request.receiveChannel
import io.ktor.server.request.uri
import io.ktor.server.response.respond
import io.ktor.server.response.respondBytesWriter
import io.ktor.utils.io.ByteReadChannel
import io.ktor.utils.io.ByteWriteChannel
import io.ktor.utils.io.readAvailable
import io.ktor.utils.io.readFully
import io.ktor.utils.io.writeFully
import java.io.IOException
import kotlin.coroutines.CoroutineContext
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeout

/**
 * Forwards a single HTTP request from an inbound Ktor call to upstream over a QUIC stream.
 *
 * - Outbound request: HTTP/1.1, hop-by-hop stripped. Body framing preserves the inbound
 *   intent: Content-Length passes through when known; chunked re-encoding only when the
 *   inbound length is unknown (browser sent chunked, or POST without Content-Length).
 * - Inbound response: HTTP/1.1; body is decoded per its framing (chunked / Content-Length
 *   / until-FIN), then re-emitted to Ktor with framing headers stripped so Ktor decides
 *   the wire framing for the browser.
 */
internal class UpstreamClient(
    private val stream: QuicStream,
    private val rewriter: HeaderRewriter,
    private val shareId: ShareId,
) {
    /**
     * [suspectConnection] shortens the head deadline to a probe: the connection may
     * have died silently in the background, and the caller replays on a fresh one if
     * the probe fails. [onResponseHead] fires once the response head has arrived —
     * the connection is proven alive from that point on.
     */
    suspend fun forward(
        call: ApplicationCall,
        suspectConnection: Boolean,
        onResponseHead: () -> Unit,
    ) {
        val upgrade = isUpgradeRequest(call)
        val request = buildOutboundRequest(call, upgrade)
        val reader = StreamReader(stream)
        // Deadline on request + response head: a connection that died silently (e.g.
        // network handover while idle) blackholes packets and would otherwise hang
        // this read forever — no error, no eviction, no recovery. The body pipe (and
        // the upgraded relay) below are exempt so large/slow/long-lived transfers
        // aren't cut off.
        val deadline = if (suspectConnection) PROBE_TIMEOUT_MS else EXCHANGE_TIMEOUT_MS
        val response = withTimeout(deadline) {
            sendRequest(request, call.receiveChannel())
            readResponseHead(reader, upgrade)
        }
        onResponseHead()
        Log.d(TAG, "${request.method} ${request.target} → ${response.status}")

        if (upgrade && response.status == HttpStatusCode.SwitchingProtocols.value) {
            // Hand the browser socket and the QUIC stream to a raw bidirectional
            // relay for the socket's lifetime. respond() suspends here until either
            // side closes; ProxyServer closes the stream afterwards.
            call.respond(upgradeContent(response, reader))
            return
        }

        respond(call, response, reader)
        // Half-close our send side only after the full response cycle. Calling finish()
        // immediately after the request head confuses upstream hyper into reporting an
        // "incomplete message" — hyper's own client doesn't FIN until the exchange is
        // over, and we match that.
        runCatching { stream.finish() }
    }

    // ---- Outbound (request) ----

    private fun buildOutboundRequest(call: ApplicationCall, isUpgrade: Boolean): OutboundRequest {
        val method = call.request.httpMethod.value
        val target = call.request.uri
        // An Upgrade handshake is a bodyless GET; the post-101 bytes are relayed raw.
        val body = if (isUpgrade) RequestBody.None else detectRequestBody(call)

        val headers = mutableListOf<Header>().apply {
            for ((name, values) in call.request.headers.entries()) {
                val lower = name.lowercase()
                if (lower in FRAMING) continue
                // Preserve Connection/Upgrade for an upgrade — they carry the
                // handshake; strip the rest of hop-by-hop either way.
                if (lower in HOP_BY_HOP && !(isUpgrade && lower in UPGRADE_HEADERS)) continue
                for (v in values) add(Header(name, v))
            }
            if (isUpgrade) {
                // Keep the browser's Connection/Upgrade/Sec-WebSocket-* as sent, and
                // hold the stream open for the socket's lifetime (no Connection: close).
            } else {
                when (body) {
                    is RequestBody.Fixed -> add(Header("Content-Length", body.length.toString()))
                    RequestBody.Chunked -> add(Header("Transfer-Encoding", "chunked"))
                    RequestBody.None -> Unit
                }
                add(Header("Connection", "close"))
            }
        }
        rewriter.rewriteRequest(method, target, headers, shareId)
        return OutboundRequest(method, target, headers, body)
    }

    private fun detectRequestBody(call: ApplicationCall): RequestBody {
        val contentLength = call.request.headers["Content-Length"]?.toLongOrNull()
        if (contentLength != null) return RequestBody.Fixed(contentLength)
        val chunked = call.request.headers["Transfer-Encoding"]
            ?.lowercase()?.contains("chunked") == true
        val mightHaveBody = call.request.httpMethod.value.uppercase() in METHODS_WITH_BODY
        return if (chunked || mightHaveBody) RequestBody.Chunked else RequestBody.None
    }

    private suspend fun sendRequest(req: OutboundRequest, body: ByteReadChannel) {
        stream.write(serializeHead(req).toByteArray(Charsets.ISO_8859_1))
        when (val b = req.body) {
            RequestBody.None -> Unit
            RequestBody.Chunked -> sendChunkedBody(body)
            is RequestBody.Fixed -> sendFixedBody(body, b.length)
        }
        // Intentionally no finish() here. The request is framed by headers
        // (Content-Length or chunked terminator) or implicit zero body for
        // method-without-body; upstream knows the request is complete without
        // needing FIN. We FIN after the response, mirroring hyper's client.
    }

    private suspend fun sendFixedBody(src: ByteReadChannel, length: Long) {
        val buf = ByteArray(BUFFER_SIZE)
        var remaining = length
        while (remaining > 0) {
            val take = minOf(remaining, buf.size.toLong()).toInt()
            src.readFully(buf, 0, take)
            stream.write(buf.copyOfRange(0, take))
            remaining -= take
        }
    }

    private fun serializeHead(req: OutboundRequest): String = buildString {
        append(req.method).append(' ').append(req.target).append(" HTTP/1.1\r\n")
        for (h in req.headers) append(h.name).append(": ").append(h.value).append("\r\n")
        append("\r\n")
    }

    private suspend fun sendChunkedBody(src: ByteReadChannel) {
        val buf = ByteArray(BUFFER_SIZE)
        while (true) {
            val n = src.readAvailable(buf, 0, buf.size)
            if (n < 0) break
            if (n == 0) continue
            stream.write("${n.toString(16)}\r\n".toByteArray(Charsets.ISO_8859_1))
            stream.write(buf.copyOfRange(0, n))
            stream.write(CRLF)
        }
        stream.write(FINAL_CHUNK)
    }

    // ---- Inbound (response) ----

    private suspend fun readResponseHead(reader: StreamReader, isUpgrade: Boolean): ResponseHead {
        val statusLine = reader.readLine() ?: throw IOException("upstream closed before status line")
        val status = statusLine.split(' ', limit = 3).getOrNull(1)?.toIntOrNull()
            ?: throw IOException("malformed status line: $statusLine")

        val headers = mutableListOf<Header>()
        while (true) {
            val line = reader.readLine() ?: break
            if (line.isEmpty()) break
            val colon = line.indexOf(':')
            if (colon < 0) continue
            headers.add(Header(line.substring(0, colon).trim(), line.substring(colon + 1).trim()))
        }

        if (isUpgrade && status == HttpStatusCode.SwitchingProtocols.value) {
            // Preserve the handshake (Connection/Upgrade/Sec-WebSocket-*); drop only
            // non-handshake hop-by-hop and framing. No body follows — the bytes after
            // the head are the upgraded protocol, relayed raw. Framing is unused here.
            headers.removeAll {
                val l = it.name.lowercase()
                (l in HOP_BY_HOP && l !in UPGRADE_HEADERS) || l in FRAMING
            }
            rewriter.rewriteResponse(status, headers, shareId)
            return ResponseHead(status, headers, BodyFraming.UntilEof)
        }

        val framing = detectFraming(headers)
        headers.removeAll { it.name.lowercase() in HOP_BY_HOP || it.name.lowercase() in FRAMING }
        rewriter.rewriteResponse(status, headers, shareId)

        return ResponseHead(status, headers, framing)
    }

    private fun detectFraming(headers: List<Header>): BodyFraming {
        val chunked = headerValue(headers, "transfer-encoding")
            ?.lowercase()?.contains("chunked") == true
        if (chunked) return BodyFraming.Chunked
        val length = headerValue(headers, "content-length")?.toLongOrNull()
        if (length != null) return BodyFraming.Fixed(length)
        return BodyFraming.UntilEof
    }

    private suspend fun respond(call: ApplicationCall, head: ResponseHead, reader: StreamReader) {
        call.response.status(HttpStatusCode.fromValue(head.status))
        for (h in head.headers) call.response.headers.append(h.name, h.value)
        call.respondBytesWriter {
            when (val framing = head.framing) {
                BodyFraming.Chunked -> pipeChunked(reader, this)
                is BodyFraming.Fixed -> pipeFixed(reader, framing.length, this)
                BodyFraming.UntilEof -> pipeUntilEof(reader, this)
            }
        }
    }

    private suspend fun pipeFixed(reader: StreamReader, length: Long, sink: ByteWriteChannel) {
        var remaining = length
        while (remaining > 0) {
            val take = minOf(remaining, BUFFER_SIZE.toLong()).toInt()
            val chunk = reader.readExactly(take)
            sink.writeFully(chunk, 0, chunk.size)
            remaining -= chunk.size
        }
    }

    private suspend fun pipeUntilEof(reader: StreamReader, sink: ByteWriteChannel) {
        while (true) {
            val chunk = reader.readSome() ?: break
            sink.writeFully(chunk, 0, chunk.size)
        }
    }

    private suspend fun pipeChunked(reader: StreamReader, sink: ByteWriteChannel) {
        while (true) {
            val lengthLine = reader.readLine() ?: throw IOException("eof in chunked body")
            val size = lengthLine.substringBefore(';').trim().toInt(16)
            if (size == 0) {
                // Consume optional trailers + terminating empty line.
                while (true) {
                    val trailer = reader.readLine() ?: break
                    if (trailer.isEmpty()) break
                }
                return
            }
            sink.writeFully(reader.readExactly(size))
            val crlf = reader.readExactly(2)
            if (crlf[0] != 0x0D.toByte() || crlf[1] != 0x0A.toByte()) {
                throw IOException("malformed chunk trailer")
            }
        }
    }

    // ---- Upgrade (WebSocket and other HTTP/1.1 Upgrades) ----

    /**
     * Matches Ktor CIO's own upgrade trigger (`expectHttpUpgrade`): a `GET` with an
     * `Upgrade` header and a `Connection` header listing the `upgrade` token. We must
     * not respond with a [OutgoingContent.ProtocolUpgrade] unless this holds, or the
     * engine rejects it (it only arms the upgrade for such requests).
     */
    private fun isUpgradeRequest(call: ApplicationCall): Boolean {
        if (!call.request.httpMethod.value.equals("GET", ignoreCase = true)) return false
        if (call.request.headers["Upgrade"].isNullOrBlank()) return false
        val connection = call.request.headers["Connection"] ?: return false
        return connection.split(',').any { it.trim().equals("upgrade", ignoreCase = true) }
    }

    /**
     * A [OutgoingContent.ProtocolUpgrade] that replays the upstream 101 headers to the
     * browser, then relays raw bytes both ways for the socket's lifetime. The engine
     * writes the 101 line + [headers], calls [upgrade], and awaits the returned [Job].
     */
    private fun upgradeContent(head: ResponseHead, reader: StreamReader) =
        object : OutgoingContent.ProtocolUpgrade() {
            override val headers: Headers = Headers.build {
                for (h in head.headers) {
                    // Ktor treats `Upgrade` as engine-reserved and only lets a
                    // ProtocolUpgrade set it when the name matches HttpHeaders.Upgrade
                    // exactly (case-sensitive). Upstream (hyper) sends it lowercased,
                    // which would trip the "controlled by the engine" rejection — so
                    // canonicalise the handshake header names.
                    val name = when {
                        h.name.equals(HttpHeaders.Upgrade, ignoreCase = true) -> HttpHeaders.Upgrade
                        h.name.equals(HttpHeaders.Connection, ignoreCase = true) -> HttpHeaders.Connection
                        else -> h.name
                    }
                    append(name, h.value)
                }
            }

            override suspend fun upgrade(
                input: ByteReadChannel,
                output: ByteWriteChannel,
                engineContext: CoroutineContext,
                userContext: CoroutineContext,
            ): Job = CoroutineScope(engineContext).launch {
                // Two independent pumps: each ends at its own EOF (mirroring a TCP
                // half-close), so one direction closing doesn't tear down the other.
                // The relay ends only once both finish; an error in either cancels
                // both (the connection is gone). Reads and writes hit opposite halves
                // of the QUIC stream, so they run concurrently.
                //
                // This is a root coroutine and Ktor only join()s it (swallowing the
                // result), so a mid-socket failure would otherwise surface as an
                // uncaught exception. Catch it here; Ktor still closes the channels.
                try {
                    coroutineScope {
                        launch { pumpStreamToBrowser(reader, output) }
                        launch { pumpBrowserToStream(input) }
                    }
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    Log.d(TAG, "websocket relay ended: ${e.message}")
                }
            }
        }

    private suspend fun pumpBrowserToStream(input: ByteReadChannel) {
        val buf = ByteArray(BUFFER_SIZE)
        while (true) {
            val n = input.readAvailable(buf, 0, buf.size)
            if (n < 0) break // browser closed its write side
            if (n == 0) continue
            stream.write(buf.copyOfRange(0, n))
        }
        // Forward the half-close as a FIN so upstream sees end-of-input.
        runCatching { stream.finish() }
    }

    private suspend fun pumpStreamToBrowser(reader: StreamReader, output: ByteWriteChannel) {
        // readSome() hands back any bytes already buffered past the 101 head before
        // it reads fresh from the stream, so the post-handshake prefix isn't dropped.
        while (true) {
            val chunk = reader.readSome() ?: break // upstream sent FIN
            output.writeFully(chunk)
            output.flush()
        }
        runCatching { output.flushAndClose() }
    }

    // ---- helpers ----

    private fun headerValue(headers: List<Header>, name: String): String? =
        headers.firstOrNull { it.name.equals(name, ignoreCase = true) }?.value

    private data class OutboundRequest(
        val method: String,
        val target: String,
        val headers: List<Header>,
        val body: RequestBody,
    )

    private sealed interface RequestBody {
        data object None : RequestBody
        data object Chunked : RequestBody
        data class Fixed(val length: Long) : RequestBody
    }

    private data class ResponseHead(
        val status: Int,
        val headers: List<Header>,
        val framing: BodyFraming,
    )

    private sealed interface BodyFraming {
        data object Chunked : BodyFraming
        data class Fixed(val length: Long) : BodyFraming
        data object UntilEof : BodyFraming
    }

    private companion object {
        const val TAG = "UpstreamClient"
        const val BUFFER_SIZE = 8192
        const val EXCHANGE_TIMEOUT_MS = 15_000L

        // Probe deadline for a suspect connection: long enough for the path RTT
        // plus a couple of QUIC loss-recovery cycles, short enough that a dead
        // connection is detected (and the request replayed) before the user
        // gives up. Deliberately a bet that the upstream answers fast — a slow
        // upstream on a live-but-suspect connection loses the bet and pays an
        // unnecessary reconnect, once.
        const val PROBE_TIMEOUT_MS = 2_500L
        val CRLF = byteArrayOf(0x0D, 0x0A)
        val FINAL_CHUNK = "0\r\n\r\n".toByteArray(Charsets.ISO_8859_1)
        val HOP_BY_HOP = setOf(
            "connection",
            "keep-alive",
            "proxy-authenticate",
            "proxy-authorization",
            "te",
            "trailers",
            "upgrade",
        )

        // Hop-by-hop headers that carry an Upgrade handshake, so they're kept (not
        // stripped) on an upgrade exchange in both directions.
        val UPGRADE_HEADERS = setOf("connection", "upgrade")
        val FRAMING = setOf("content-length", "transfer-encoding")
        val METHODS_WITH_BODY = setOf("POST", "PUT", "PATCH", "DELETE")
    }
}

/** Buffered reader over a QuicStream that supports readLine, readExactly, readSome. */
internal class StreamReader(private val stream: QuicStream) {
    private var buf: ByteArray = ByteArray(0)
    private var pos: Int = 0
    private var eof: Boolean = false

    suspend fun readLine(): String? {
        while (true) {
            val end = findCrlf()
            if (end >= 0) {
                val line = String(buf, pos, end - pos, Charsets.ISO_8859_1)
                pos = end + 2
                return line
            }
            if (!fill()) {
                return if (pos < buf.size) String(buf, pos, buf.size - pos, Charsets.ISO_8859_1) else null
            }
        }
    }

    suspend fun readExactly(n: Int): ByteArray {
        while (buf.size - pos < n) {
            if (!fill()) throw IOException("eof reading $n bytes")
        }
        val out = buf.copyOfRange(pos, pos + n)
        pos += n
        return out
    }

    suspend fun readSome(): ByteArray? {
        if (pos >= buf.size) {
            if (!fill()) return null
        }
        val out = buf.copyOfRange(pos, buf.size)
        pos = buf.size
        return out
    }

    private fun findCrlf(): Int {
        for (i in pos..buf.size - 2) {
            if (buf[i] == 0x0D.toByte() && buf[i + 1] == 0x0A.toByte()) return i
        }
        return -1
    }

    private suspend fun fill(): Boolean {
        if (eof) return false
        val chunk = stream.read(8192)
        if (chunk.isEmpty()) {
            eof = true
            return false
        }
        val tail = buf.size - pos
        val merged = ByteArray(tail + chunk.size)
        System.arraycopy(buf, pos, merged, 0, tail)
        System.arraycopy(chunk, 0, merged, tail, chunk.size)
        buf = merged
        pos = 0
        return true
    }
}
