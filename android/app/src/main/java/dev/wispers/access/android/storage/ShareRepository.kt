package dev.wispers.access.android.storage

import dev.wispers.connect.WispersConnect
import dev.wispers.connect.handles.Storage
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

    suspend fun getShare(id: ShareId): Share? = withContext(Dispatchers.IO) {
        dao.getShareInfo(id.value)?.toShare()
    }

    suspend fun createShare(nickname: String = ""): ShareId = withContext(Dispatchers.IO) {
        val id = ShareId(UUID.randomUUID().toString())
        dao.insert(
            ShareEntity(
                id = id.value,
                nickname = nickname,
                createdAt = Instant.now().toEpochMilli(),
                lastConnectedAt = null,
                rootKey = null,
                registration = null,
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

    suspend fun deleteShare(id: ShareId) = withContext(Dispatchers.IO) {
        dao.delete(id.value)
    }

    fun storageFor(id: ShareId): Storage =
        WispersConnect.createStorage(ShareNodeStorageCallbacks(dao, id.value))
}

private fun ShareInfoRow.toShare(): Share = Share(
    id = ShareId(id),
    nickname = nickname,
    createdAt = Instant.ofEpochMilli(createdAt),
    lastConnectedAt = lastConnectedAt?.let(Instant::ofEpochMilli),
)
