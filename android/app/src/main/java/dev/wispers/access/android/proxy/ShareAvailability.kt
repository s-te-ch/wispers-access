package dev.wispers.access.android.proxy

import dev.wispers.access.sdk.ShareState

/**
 * Availability of a share for the status dot and labels. ONLINE/OFFLINE/UNKNOWN
 * are transient observations; REMOVED/REVOKED are terminal — the host node has
 * definitively turned this device away.
 */
enum class ShareAvailability {
    /** The host node answered. */
    ONLINE,

    /** The host node could not be reached, or refused. */
    OFFLINE,

    /** The check failed for another reason. */
    UNKNOWN,

    /** The share was removed by its host, or this device forgotten. */
    REMOVED,

    /** This device's access was revoked. */
    REVOKED,
}

/** The SDK's terminal states as availabilities; null while live. */
fun ShareState.toAvailability(): ShareAvailability? = when (this) {
    ShareState.LIVE -> null
    ShareState.REMOVED -> ShareAvailability.REMOVED
    ShareState.REVOKED -> ShareAvailability.REVOKED
}
