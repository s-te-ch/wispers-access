package dev.wispers.access.android.proxy

import dev.wispers.access.sdk.ShareState

/**
 * Availability of a share for the status dot and labels.
 * ONLINE/OFFLINE/UNKNOWN/OUTDATED are transient observations.
 * REMOVED/REVOKED are terminal.
 */
enum class ShareAvailability {
    /** The host node answered. */
    ONLINE,

    /** The host node could not be reached, or refused. */
    OFFLINE,

    /** The check failed for another reason. */
    UNKNOWN,

    /** The host node wants a newer app. */
    OUTDATED,

    /** The share was removed by its host, or this device forgotten. */
    REMOVED,

    /** This device's access was revoked. */
    REVOKED,
}

/** The SDK's non-live states as availabilities; null while live. */
fun ShareState.toAvailability(): ShareAvailability? = when (this) {
    ShareState.LIVE -> null
    ShareState.OUTDATED -> ShareAvailability.OUTDATED
    ShareState.REMOVED -> ShareAvailability.REMOVED
    ShareState.REVOKED -> ShareAvailability.REVOKED
}

/** Whether the host node has turned this device away for good. */
val ShareState.isTerminal: Boolean
    get() = this == ShareState.REMOVED || this == ShareState.REVOKED
