package dev.wispers.access.android.storage

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
)

internal class ShareInfoRow(
    val id: String,
    val nickname: String,
    val createdAt: Long,
    val lastConnectedAt: Long?,
)
