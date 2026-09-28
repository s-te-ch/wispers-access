package dev.wispers.access.android.storage

import dev.wispers.access.android.BrowseKey
import dev.wispers.access.sdk.ShareId
import java.time.Instant
import javax.inject.Inject
import javax.inject.Singleton
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.withContext

/**
 * The UI-only state the SDK does not keep: when each share was last reached,
 * and the icons harvested per app. Keyed by the SDK's ids.
 */
@Singleton
class ShareExtras @Inject internal constructor(private val dao: ExtrasDao) {

    fun observeLastConnected(): Flow<Map<ShareId, Instant>> =
        dao.observeActivity().map { rows ->
            rows.associate { it.share to Instant.ofEpochMilli(it.lastConnectedAt) }
        }

    fun observeIcons(): Flow<Map<BrowseKey, ByteArray>> =
        dao.observeIcons().map { rows ->
            rows.associate { BrowseKey(it.share, it.app) to it.png }
        }

    suspend fun markConnected(share: ShareId, at: Instant = Instant.now()) =
        withContext(Dispatchers.IO) {
            dao.putActivity(ShareActivityEntity(share, at.toEpochMilli()))
        }

    /** The cached icon of an app, if any. */
    suspend fun icon(key: BrowseKey): ByteArray? = withContext(Dispatchers.IO) {
        dao.icon(key.shareId, key.appId)?.png
    }

    /** Stores a harvested icon if it out-ranks the cached one; true if it did. */
    suspend fun updateIcon(key: BrowseKey, png: ByteArray, rank: Int): Boolean =
        withContext(Dispatchers.IO) {
            if (rank <= (dao.iconRank(key.shareId, key.appId) ?: 0)) return@withContext false
            dao.putIcon(AppIconEntity(key.shareId, key.appId, png, rank))
            true
        }

    /** Drops everything kept for a share. */
    suspend fun forget(share: ShareId) = withContext(Dispatchers.IO) {
        dao.deleteActivity(share)
        dao.deleteIcons(share)
    }
}
