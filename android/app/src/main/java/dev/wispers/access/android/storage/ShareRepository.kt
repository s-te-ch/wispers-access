package dev.wispers.access.android.storage

import dev.wispers.connect.WispersConnect
import dev.wispers.connect.handles.Node
import dev.wispers.connect.handles.Storage
import dev.wispers.connect.types.NodeState
import java.time.Instant
import java.util.UUID
import javax.inject.Inject
import javax.inject.Singleton
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.withContext

@Singleton
class ShareRepository @Inject internal constructor(
    private val dao: ShareDao,
) {

    fun observeShares(): Flow<List<Share>> =
        dao.observeShareInfos().map { rows -> rows.map(ShareInfoRow::toShare) }

    fun observeShare(id: ShareId): Flow<Share?> =
        dao.observeShareInfo(id.value).map { it?.toShare() }

    suspend fun getShare(id: ShareId): Share? = withContext(Dispatchers.IO) {
        dao.getShareInfo(id.value)?.toShare()
    }

    suspend fun createShare(nickname: String = "", backend: String? = null): ShareId =
        withContext(Dispatchers.IO) {
            val id = ShareId(UUID.randomUUID().toString())
            dao.insert(
                ShareEntity(
                    id = id.value,
                    nickname = nickname,
                    createdAt = Instant.now().toEpochMilli(),
                    lastConnectedAt = null,
                    rootKey = null,
                    registration = null,
                    backend = backend,
                )
            )
            id
        }

    suspend fun setNickname(id: ShareId, nickname: String) = withContext(Dispatchers.IO) {
        dao.setNickname(id.value, nickname)
    }

    suspend fun markConnected(id: ShareId, at: Instant = Instant.now()) =
        withContext(Dispatchers.IO) {
            dao.setLastConnectedAt(id.value, at.toEpochMilli())
        }

    /**
     * Records that the hub definitively rejected this share's node. One-way:
     * a terminal share never returns to live polling.
     */
    suspend fun markTerminal(id: ShareId, state: ShareTerminalState) =
        withContext(Dispatchers.IO) {
            dao.setTerminalState(id.value, state.name)
        }

    /** Ladder rung of the currently-cached icon (0 if none). */
    suspend fun currentIconRank(id: ShareId): Int = withContext(Dispatchers.IO) {
        dao.getIconRank(id.value) ?: 0
    }

    suspend fun updateIcon(id: ShareId, png: ByteArray, rank: Int) = withContext(Dispatchers.IO) {
        dao.setIcon(id.value, png, rank)
    }

    suspend fun deleteShare(id: ShareId) = withContext(Dispatchers.IO) {
        dao.delete(id.value)
    }

    /**
     * A [Storage] handle for the share, already pointed at the share's
     * self-hosted hub if it has one.
     */
    suspend fun storageFor(id: ShareId): Storage = withContext(Dispatchers.IO) {
        val storage = WispersConnect.createStorage(ShareNodeStorageCallbacks(dao, id.value))
        dao.getBackend(id.value)?.let { storage.overrideHubAddr(it) }
        storage
    }
}

/**
 * Subject-correct rename of the library's [Storage.restoreOrInit]: a Storage doesn't get
 * restored, the Node it holds does.
 */
suspend fun Storage.restoreOrInitNode(): Pair<Node, NodeState> = restoreOrInit()

private fun ShareInfoRow.toShare(): Share = Share(
    id = ShareId(id),
    nickname = nickname,
    createdAt = Instant.ofEpochMilli(createdAt),
    lastConnectedAt = lastConnectedAt?.let(Instant::ofEpochMilli),
    iconPng = iconPng,
    terminalState = terminalState?.let(ShareTerminalState::valueOf),
)
