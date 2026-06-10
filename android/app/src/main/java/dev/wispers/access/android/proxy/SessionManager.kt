package dev.wispers.access.android.proxy

import dev.wispers.access.android.storage.ShareId
import dev.wispers.access.android.storage.ShareRepository
import dev.wispers.access.android.storage.restoreOrInitNode
import dev.wispers.connect.handles.Node
import dev.wispers.connect.handles.QuicConnection
import dev.wispers.connect.handles.QuicStream
import javax.inject.Inject
import javax.inject.Singleton
import kotlin.coroutines.coroutineContext
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout

/**
 * Per-share Node + QuicConnection cache. Equivalent of waclient's `StreamFactory`.
 *
 * Connections are established lazily on first stream request and reused across requests.
 * Stream opens get one retry — if the cached connection has died, we drop it from the
 * pool and reconnect on the second attempt. Opening a stream on a dead connection can
 * still succeed (it's local bookkeeping), so callers must report mid-request failures
 * via [invalidate] — otherwise the dead connection stays cached and every later request
 * fails the same way.
 */
@Singleton
class SessionManager @Inject constructor(
    private val repo: ShareRepository,
) {
    private val nodeMutex = Mutex()
    private val nodes = mutableMapOf<ShareId, Node>()

    private val connMutex = Mutex()
    private val connections = mutableMapOf<ShareId, QuicConnection>()

    /** A stream plus the connection it came from, so [invalidate] can evict exactly that connection. */
    class StreamLease internal constructor(
        val stream: QuicStream,
        internal val connection: QuicConnection,
    )

    // Connect/open can hang silently (e.g. the network changed under an idle
    // connection and packets blackhole), so each attempt gets a deadline.
    suspend fun openStream(shareId: ShareId): StreamLease = try {
        withTimeout(OPEN_TIMEOUT_MS) { tryOpen(shareId) }
    } catch (e: Exception) {
        coroutineContext.ensureActive()
        withTimeout(OPEN_TIMEOUT_MS) { tryOpen(shareId) }
    }

    /** Evicts and closes [lease]'s connection (unless already replaced by a newer one). */
    suspend fun invalidate(shareId: ShareId, lease: StreamLease) {
        evict(shareId, lease.connection)
    }

    /**
     * Drops and closes every cached connection. Called when the device's default
     * network changes: connections on the old network blackhole silently, so
     * reconnecting eagerly beats waiting for per-request timeouts to flush them.
     *
     * @return the affected share ids, for an optional [prewarm].
     */
    suspend fun evictAll(): List<ShareId> {
        val stale = connMutex.withLock {
            val all = connections.toMap()
            connections.clear()
            all
        }
        for (conn in stale.values) runCatching { conn.closeAsync() }
        return stale.keys.toList()
    }

    /**
     * Re-establishes connections for [shareIds] ahead of traffic, so the next
     * request doesn't pay the connection setup (ICE is the expensive part).
     * Failures are swallowed — the per-request path retries lazily anyway.
     */
    suspend fun prewarm(shareIds: List<ShareId>) {
        for (id in shareIds) {
            runCatching { getConnection(id, getNode(id)) }
        }
    }

    private suspend fun tryOpen(shareId: ShareId): StreamLease {
        val node = getNode(shareId)
        val conn = getConnection(shareId, node)
        return try {
            StreamLease(conn.openStream(), conn)
        } catch (e: Exception) {
            evict(shareId, conn)
            throw e
        }
    }

    // NonCancellable: eviction is triggered from catch blocks that may themselves be
    // running cancellation (open timeout) — it must complete anyway, or the dead
    // connection stays cached and every later request hits it.
    private suspend fun evict(shareId: ShareId, conn: QuicConnection) =
        withContext(NonCancellable) {
            connMutex.withLock {
                if (connections[shareId] === conn) connections.remove(shareId)
            }
            runCatching { conn.closeAsync() }
        }

    private suspend fun getNode(shareId: ShareId): Node = nodeMutex.withLock {
        nodes.getOrPut(shareId) {
            val storage = repo.storageFor(shareId)
            val (node, _) = storage.restoreOrInitNode()
            node
        }
    }

    private suspend fun getConnection(shareId: ShareId, node: Node): QuicConnection =
        connMutex.withLock {
            connections.getOrPut(shareId) { node.connectQuic(SERVING_NODE_NUMBER) }
        }

    private companion object {
        const val SERVING_NODE_NUMBER = 1
        const val OPEN_TIMEOUT_MS = 10_000L
    }
}
