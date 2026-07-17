package dev.wispers.access.android.storage

import androidx.room.Dao
import androidx.room.Insert
import androidx.room.Query
import kotlinx.coroutines.flow.Flow

@Dao
internal interface ShareDao {

    @Query(
        "SELECT id, nickname, createdAt, lastConnectedAt, iconPng, terminalState FROM shares " +
            "ORDER BY lastConnectedAt DESC, createdAt DESC"
    )
    fun observeShareInfos(): Flow<List<ShareInfoRow>>

    @Query(
        "SELECT id, nickname, createdAt, lastConnectedAt, iconPng, terminalState FROM shares " +
            "WHERE id = :id"
    )
    fun getShareInfo(id: String): ShareInfoRow?

    @Query(
        "SELECT id, nickname, createdAt, lastConnectedAt, iconPng, terminalState FROM shares " +
            "WHERE id = :id"
    )
    fun observeShareInfo(id: String): Flow<ShareInfoRow?>

    @Query("SELECT iconRank FROM shares WHERE id = :id")
    fun getIconRank(id: String): Int?

    @Query("UPDATE shares SET iconPng = :png, iconRank = :rank WHERE id = :id")
    fun setIcon(id: String, png: ByteArray, rank: Int)

    @Insert
    fun insert(entity: ShareEntity)

    @Query("UPDATE shares SET nickname = :nickname WHERE id = :id")
    fun setNickname(id: String, nickname: String)

    @Query("UPDATE shares SET lastConnectedAt = :at WHERE id = :id")
    fun setLastConnectedAt(id: String, at: Long)

    @Query("UPDATE shares SET terminalState = :state WHERE id = :id")
    fun setTerminalState(id: String, state: String)

    @Query("DELETE FROM shares WHERE id = :id")
    fun delete(id: String)

    @Query("SELECT rootKey FROM shares WHERE id = :id")
    fun getRootKey(id: String): ByteArray?

    @Query("UPDATE shares SET rootKey = :key WHERE id = :id")
    fun setRootKey(id: String, key: ByteArray)

    @Query("UPDATE shares SET rootKey = NULL WHERE id = :id")
    fun clearRootKey(id: String)

    @Query("SELECT backend FROM shares WHERE id = :id")
    fun getBackend(id: String): String?

    @Query("SELECT registration FROM shares WHERE id = :id")
    fun getRegistration(id: String): ByteArray?

    @Query("UPDATE shares SET registration = :data WHERE id = :id")
    fun setRegistration(id: String, data: ByteArray)

    @Query("UPDATE shares SET registration = NULL WHERE id = :id")
    fun clearRegistration(id: String)
}
