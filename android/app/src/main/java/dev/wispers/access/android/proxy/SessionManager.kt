package dev.wispers.access.android.proxy

import dev.wispers.access.android.storage.ShareId
import dev.wispers.access.android.storage.ShareRepository
import dev.wispers.access.android.storage.restoreOrInitNode
import dev.wispers.connect.handles.Node
import dev.wispers.connect.handles.QuicConnection
import dev.wispers.connect.handles.QuicStream
import javax.inject.Inject
import javax.inject.Singleton
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * Per-share Node + QuicConnection cache. Equivalent of waclient's `StreamFactory`.
 *
 * Connections are established lazily on first stream request and reused across requests.
 * Stream opens get one retry — if the cached connection has died, we drop it from the
 * pool and reconnect on the second attempt.
 */
@Singleton
class SessionManager @Inject constructor(
    private val repo: ShareRepository,
) {
    private val nodeMutex = Mutex()
    private val nodes = mutableMapOf<ShareId, Node>()

    private val connMutex = Mutex()
    private val connections = mutableMapOf<ShareId, QuicConnection>()

    suspend fun openStream(shareId: ShareId): QuicStream = try {
        tryOpen(shareId)
    } catch (_: Exception) {
        tryOpen(shareId)
    }

    private suspend fun tryOpen(shareId: ShareId): QuicStream {
        val node = getNode(shareId)
        val conn = getConnection(shareId, node)
        return try {
            conn.openStream()
        } catch (e: Exception) {
            connMutex.withLock { connections.remove(shareId) }
            throw e
        }
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
    }
}
