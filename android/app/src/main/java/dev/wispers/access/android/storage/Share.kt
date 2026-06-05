package dev.wispers.access.android.storage

import java.time.Instant

@JvmInline
value class ShareId(val value: String)

data class Share(
    val id: ShareId,
    val nickname: String,
    val createdAt: Instant,
    val lastConnectedAt: Instant?,
    val iconPng: ByteArray?,
)
