package dev.wispers.access.android.storage

import androidx.room.Dao
import androidx.room.Entity
import androidx.room.Insert
import androidx.room.OnConflictStrategy
import androidx.room.PrimaryKey
import androidx.room.Query
import kotlinx.coroutines.flow.Flow

/** When a share was last reached from this device, for the roster's "LAST 5M AGO". */
@Entity(tableName = "share_activity")
internal class ShareActivityEntity(
    @PrimaryKey val share: String,
    val lastConnectedAt: Long,
)

/**
 * A site icon harvested while browsing one app, plus the rank-ladder rung it
 * came from (manifest-maskable 4 > manifest 3 > apple-touch-icon 2 > favicon
 * 1), so a better icon replaces a worse one but never the reverse.
 */
@Entity(tableName = "app_icons", primaryKeys = ["share", "app"])
internal class AppIconEntity(
    val share: String,
    val app: String,
    val png: ByteArray,
    val rank: Int,
)

@Dao
internal interface ExtrasDao {
    @Query("SELECT * FROM share_activity")
    fun observeActivity(): Flow<List<ShareActivityEntity>>

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    fun putActivity(entity: ShareActivityEntity)

    @Query("DELETE FROM share_activity WHERE share = :share")
    fun deleteActivity(share: String)

    @Query("SELECT * FROM app_icons")
    fun observeIcons(): Flow<List<AppIconEntity>>


    @Query("SELECT * FROM app_icons WHERE share = :share AND app = :app")
    fun icon(share: String, app: String): AppIconEntity?

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    fun putIcon(entity: AppIconEntity)

    @Query("DELETE FROM app_icons WHERE share = :share")
    fun deleteIcons(share: String)
}
