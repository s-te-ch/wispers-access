package dev.wispers.access.android.proxy

import dev.wispers.access.android.storage.ShareTerminalState

/**
 * Result of a hub availability check for one share. ONLINE/OFFLINE/UNKNOWN are
 * transient observations; REMOVED/REVOKED are terminal — the hub has
 * definitively rejected this device, and the share can never come back.
 */
enum class ShareAvailability {
    /** The serving node holds a live hub connection. */
    ONLINE,

    /** Hub reachable, serving node not connected. */
    OFFLINE,

    /** The check failed transiently — typically the hub is unreachable. */
    UNKNOWN,

    /** The share was removed on the server side (credentials rejected). */
    REMOVED,

    /** This device's access was revoked from the share's roster. */
    REVOKED,
}

/** The persisted terminal state as an availability, for uniform rendering. */
fun ShareTerminalState.toAvailability(): ShareAvailability = when (this) {
    ShareTerminalState.REMOVED -> ShareAvailability.REMOVED
    ShareTerminalState.REVOKED -> ShareAvailability.REVOKED
}
