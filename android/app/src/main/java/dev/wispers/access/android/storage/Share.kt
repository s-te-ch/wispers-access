package dev.wispers.access.android.storage

import java.time.Instant

@JvmInline
value class ShareId(val value: String)

/**
 * A share that can never come back: the hub rejected this node's credentials
 * (share deleted server-side) or reported it revoked from the roster. Persisted
 * so the state survives restarts and renders while offline.
 */
enum class ShareTerminalState {
    /** The share was removed on the server side (group deleted at the hub). */
    REMOVED,

    /** This device's access was revoked from the share's roster. */
    REVOKED,
}

data class Share(
    val id: ShareId,
    val nickname: String,
    val createdAt: Instant,
    val lastConnectedAt: Instant?,
    val iconPng: ByteArray?,
    val terminalState: ShareTerminalState?,
)
