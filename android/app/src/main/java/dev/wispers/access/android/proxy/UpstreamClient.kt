package dev.wispers.access.android.proxy

import android.util.Log
import dev.wispers.access.android.storage.ShareId
import dev.wispers.connect.handles.QuicStream
import io.ktor.http.HttpStatusCode
import io.ktor.server.application.ApplicationCall
import io.ktor.server.request.httpMethod
import io.ktor.server.request.receiveChannel
import io.ktor.server.request.uri
import io.ktor.server.response.respondBytesWriter
import io.ktor.utils.io.ByteReadChannel
import io.ktor.utils.io.ByteWriteChannel
import io.ktor.utils.io.readAvailable
import io.ktor.utils.io.readFully
import io.ktor.utils.io.writeFully
import java.io.IOException

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
    suspend fun forward(call: ApplicationCall) {
        val request = buildOutboundRequest(call)
        Log.d(TAG, "→ ${request.method} ${request.target} body=${request.body::class.simpleName}")
        sendRequest(request, call.receiveChannel())
        val reader = StreamReader(stream)
        val response = try {
            readResponseHead(reader)
        } catch (e: Exception) {
            Log.w(TAG, "← read response head failed: ${e.message}")
            throw e
        }
        Log.d(TAG, "← ${response.status} framing=${response.framing::class.simpleName}")
        respond(call, response, reader)
        // Half-close our send side only after the full response cycle. Calling finish()
        // immediately after the request head was observed to confuse the upstream's
        // hyper into reporting an "incomplete message" — hyper's own client doesn't FIN
        // until the exchange is over, and we match that.
        runCatching { stream.finish() }
    }

    // ---- Outbound (request) ----

    private fun buildOutboundRequest(call: ApplicationCall): OutboundRequest {
        val method = call.request.httpMethod.value
        val target = call.request.uri
        val body = detectRequestBody(call)

        val headers = mutableListOf<Header>().apply {
            for ((name, values) in call.request.headers.entries()) {
                val lower = name.lowercase()
                if (lower in HOP_BY_HOP || lower in FRAMING) continue
                for (v in values) add(Header(name, v))
            }
            when (body) {
                is RequestBody.Fixed -> add(Header("Content-Length", body.length.toString()))
                RequestBody.Chunked -> add(Header("Transfer-Encoding", "chunked"))
                RequestBody.None -> Unit
            }
            add(Header("Connection", "close"))
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
        val headBytes = serializeHead(req).toByteArray(Charsets.ISO_8859_1)
        Log.d(TAG, "→ head bytes=${headBytes.size} last8=${hexTail(headBytes, 8)}")
        stream.write(headBytes)
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

    private fun hexTail(b: ByteArray, n: Int): String =
        b.takeLast(n).joinToString(" ") { "%02x".format(it) }

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

    private suspend fun readResponseHead(reader: StreamReader): ResponseHead {
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
