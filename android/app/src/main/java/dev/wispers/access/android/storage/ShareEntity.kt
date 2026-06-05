package dev.wispers.access.android.storage

import androidx.room.ColumnInfo
import androidx.room.Entity
import androidx.room.PrimaryKey

@Entity(tableName = "shares")
internal class ShareEntity(
    @PrimaryKey val id: String,
    val nickname: String,
    val createdAt: Long,
    val lastConnectedAt: Long?,
    val rootKey: ByteArray?,
    val registration: ByteArray?,
    val iconPng: ByteArray? = null,
    // Ladder rung the cached icon came from (0 = none → generated tile). The
    // defaultValue must mirror the migration's `DEFAULT 0`, or Room's schema
    // validation rejects the migration.
    @ColumnInfo(defaultValue = "0") val iconRank: Int = 0,
)

internal class ShareInfoRow(
    val id: String,
    val nickname: String,
    val createdAt: Long,
    val lastConnectedAt: Long?,
    val iconPng: ByteArray?,
)
