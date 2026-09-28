package dev.wispers.access.android

import dev.wispers.access.sdk.ShareId

/** One app of one share, as the app tells them apart: what the user opens. */
data class BrowseKey(val shareId: ShareId, val appId: String) {
    /** A stable string for intents and shortcut ids. */
    val token: String get() = "$shareId/$appId"

    companion object {
        fun parse(token: String): BrowseKey? {
            val slash = token.indexOf('/')
            if (slash <= 0 || slash == token.lastIndex) return null
            return BrowseKey(token.substring(0, slash), token.substring(slash + 1))
        }
    }
}
