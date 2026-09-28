package dev.wispers.access.android.storage

import androidx.room.Dao
import androidx.room.Entity
import androidx.room.Insert
import androidx.room.OnConflictStrategy
import androidx.room.Query

/** One secret of one share, under the key the SDK's transport chose. */
@Entity(tableName = "secrets", primaryKeys = ["share", "key"])
internal class SecretEntity(
    val share: String,
    val key: String,
    val value: ByteArray,
)

@Dao
internal interface SecretDao {
    @Query("SELECT value FROM secrets WHERE share = :share AND `key` = :key")
    fun get(share: String, key: String): ByteArray?

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    fun put(entity: SecretEntity)

    @Query("DELETE FROM secrets WHERE share = :share AND `key` = :key")
    fun delete(share: String, key: String)
}
