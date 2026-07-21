package dev.wispers.access.android.demo

import android.app.Activity
import android.content.pm.ApplicationInfo
import dev.wispers.access.android.proxy.ShareAvailability
import dev.wispers.access.android.storage.Share
import dev.wispers.access.android.storage.ShareId
import java.time.Duration
import java.time.Instant

/**
 * Static roster for store screenshots: replaces the Room-backed roster and the
 * live hub polling with fixed shares and statuses, so captures are
 * deterministic and need no backend. Debug-only — the launch extra is ignored
 * in release builds, and the icon assets ship only in the debug source set.
 *
 * Activate with:
 *
 *     adb shell am start -n dev.wispers.access.android/.MainActivity --ez demo true
 */
object DemoMode {
    /** The fixed roster, or null when demo mode is off (the normal case). */
    var shares: List<Share>? = null
        private set

    /** Fixed per-share availability, replacing the hub poll. */
    var statuses: Map<ShareId, ShareAvailability> = emptyMap()
        private set

    val active: Boolean get() = shares != null

    fun maybeActivate(activity: Activity) {
        if (!activity.intent.getBooleanExtra(EXTRA_DEMO, false)) return
        if (activity.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE == 0) return
        val now = Instant.now()
        shares = ROSTER.map { it.toShare(activity, now) }
        statuses = ROSTER.associate { ShareId(it.slug) to it.availability }
    }

    private const val EXTRA_DEMO = "demo"

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
        fun toShare(activity: Activity, now: Instant) = Share(
            id = ShareId(slug),
            nickname = nickname,
            createdAt = now - joined,
            lastConnectedAt = now - lastConnected,
            iconPng = activity.assets.open("demo/$slug.png").readBytes(),
            terminalState = null,
        )
    }
}
