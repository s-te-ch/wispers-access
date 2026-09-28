package dev.wispers.access.android.demo

import android.app.Activity
import android.content.pm.ApplicationInfo
import dev.wispers.access.android.BrowseKey
import dev.wispers.access.android.proxy.ShareAvailability
import dev.wispers.access.sdk.AppKind
import dev.wispers.access.sdk.Share
import dev.wispers.access.sdk.ShareId
import dev.wispers.access.sdk.ShareState
import dev.wispers.access.sdk.SharedApp
import dev.wispers.access.sdk.Transport
import java.time.Duration
import java.time.Instant

/**
 * Static roster for store screenshots: replaces the SDK's store and the live
 * host polling with fixed shares and statuses, so captures are deterministic
 * and need no backend. Debug-only — the launch extra is ignored in release
 * builds, and the icon assets ship only in the debug source set.
 *
 * Activate with:
 *
 *     adb shell am start -n dev.wispers.access.android/.MainActivity --ez demo true
 */
object DemoMode {
    /** The fixed roster, or null when demo mode is off (the normal case). */
    var shares: List<Share>? = null
        private set

    /** Fixed per-share availability, replacing the host poll. */
    var statuses: Map<ShareId, ShareAvailability> = emptyMap()
        private set

    /** The bundled icons, per app. */
    var icons: Map<BrowseKey, ByteArray> = emptyMap()
        private set

    /** When each demo share was last reached. */
    var lastConnected: Map<ShareId, Instant> = emptyMap()
        private set

    val active: Boolean get() = shares != null

    fun maybeActivate(activity: Activity) {
        if (!activity.intent.getBooleanExtra(EXTRA_DEMO, false)) return
        if (activity.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE == 0) return
        val now = Instant.now()
        shares = ROSTER.map { it.toShare(now) }
        statuses = ROSTER.associate { it.slug to it.availability }
        lastConnected = ROSTER.associate { it.slug to now - it.lastConnected }
        icons = ROSTER.associate { entry ->
            BrowseKey(entry.slug, APP_ID) to activity.assets.open("demo/${entry.slug}.png").readBytes()
        }
    }

    private const val EXTRA_DEMO = "demo"
    private const val APP_ID = "app"

    // Known self-hosted tools anchor the "that's my stack" reaction; one bespoke
    // entry shows shares aren't limited to famous products. Names are nominative
    // word-mark use; the icons are our own brand-colored glyphs, not the logos.
    private val ROSTER = listOf(
        DemoShare(
            nickname = "Stats (Grafana)",
            slug = "grafana",
            availability = ShareAvailability.ONLINE,
            lastConnected = Duration.ofMinutes(2),
            joined = Duration.ofDays(24),
        ),
        DemoShare(
            nickname = "ERP (Odoo)",
            slug = "odoo",
            availability = ShareAvailability.ONLINE,
            lastConnected = Duration.ofHours(1),
            joined = Duration.ofDays(18),
        ),
        DemoShare(
            nickname = "Files (Nextcloud)",
            slug = "nextcloud",
            availability = ShareAvailability.ONLINE,
            lastConnected = Duration.ofMinutes(30),
            joined = Duration.ofDays(11),
        ),
        DemoShare(
            nickname = "Duty Roster",
            slug = "duty-roster",
            availability = ShareAvailability.OFFLINE,
            lastConnected = Duration.ofDays(1),
            joined = Duration.ofDays(5),
        ),
    )

    private data class DemoShare(
        val nickname: String,
        val slug: String,
        val availability: ShareAvailability,
        val lastConnected: Duration,
        val joined: Duration,
    ) {
        fun toShare(now: Instant) = Share(
            id = slug,
            name = nickname,
            label = slug,
            transport = Transport.IROH,
            apps = listOf(SharedApp(id = APP_ID, name = nickname, kind = AppKind.WEB)),
            state = ShareState.LIVE,
            joinedAt = now - joined,
        )
    }
}
