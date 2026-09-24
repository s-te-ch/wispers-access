import Foundation
import WispersAccessSdk

/// Static roster for App Store screenshots: replaces the SDK's store and the
/// live host polling with fixed shares and statuses, so captures are
/// deterministic and need no backend. Debug-only — the launch argument is
/// compiled out of release builds.
///
/// Activate with the `--demo` launch argument (an Xcode scheme, or
/// `xcrun simctl launch booted <bundle-id> --demo`).
@MainActor
enum DemoMode {
    static let active: Bool = {
        #if DEBUG
            return ProcessInfo.processInfo.arguments.contains("--demo")
        #else
            return false
        #endif
    }()

    /// What the demo manager throws for anything that needs a host.
    struct NotAvailable: LocalizedError {
        var errorDescription: String? { "Not available in demo mode." }
    }

    /// A ShareManager without an SDK client: the fixed roster, and icons and
    /// activity in a throwaway temp directory — the real files are never
    /// touched.
    static func makeManager() -> ShareManager {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("demo-roster")
        // A previous run's seed would resurface with stale timestamps.
        try? FileManager.default.removeItem(at: dir)
        let icons = ShareIconStore(fileURL: dir.appendingPathComponent("share-icons.json"))
        let activity = ShareActivityStore(fileURL: dir.appendingPathComponent("share-activity.json"))
        let now = Date()
        var shares: [Share] = []
        for entry in roster {
            shares.append(
                Share(
                    id: entry.slug,
                    name: entry.nickname,
                    label: entry.slug,
                    transport: .iroh,
                    apps: [SharedApp(id: "app", name: entry.nickname, kind: .web)],
                    state: .live,
                    joinedAt: now.addingTimeInterval(-entry.joined)
                )
            )
            activity.markConnected(entry.slug, at: now.addingTimeInterval(-entry.lastConnected))
            if let url = Bundle.main.url(forResource: entry.slug, withExtension: "png"),
                let png = try? Data(contentsOf: url)
            {
                icons.update(png, rank: 1, for: BrowseKey(shareID: entry.slug, appID: "app"))
            }
        }
        return ShareManager(client: nil, icons: icons, activity: activity, shares: shares)
    }

    /// A screen to open on launch (`--demo-detail <slug>`, e.g. `grafana`), so
    /// capture scripts can shoot the detail screen without synthesizing taps.
    static var initialRoute: ShareRoute? {
        guard active else { return nil }
        let args = ProcessInfo.processInfo.arguments
        guard let flag = args.firstIndex(of: "--demo-detail"), flag + 1 < args.count else {
            return nil
        }
        return .detail(args[flag + 1])
    }

    /// Whether to open the add-app sheet on launch (`--demo-add`), pre-filled
    /// with `sampleInvite`, so capture scripts can shoot the code-entry screen.
    static var presentAddSheet: Bool {
        active && ProcessInfo.processInfo.arguments.contains("--demo-add")
    }

    /// Shaped like a real `waserver invite` code for an iroh share (endpoint
    /// id, secret) but pure fiction — it parses, so the screenshot shows the
    /// transport note, and joins nothing.
    static let sampleInvite =
        "wax1_iroh_30b1c2fed7e381856aad2334030bd0cf4c316ad4ee2fb08a4eebe661718c5977_d4ab8faf8841f2790da9bc82ee370ec7"

    /// Fixed per-share availability, replacing the host poll.
    static var statuses: [ShareId: Availability] {
        Dictionary(uniqueKeysWithValues: roster.map { ($0.slug, $0.availability) })
    }

    // Known self-hosted tools anchor the "that's my stack" reaction; one bespoke
    // entry shows shares aren't limited to famous products. Names are nominative
    // word-mark use; the icons are our own brand-colored glyphs, not the logos.
    private static let roster = [
        Entry(
            nickname: "Stats (Grafana)",
            slug: "grafana",
            availability: .online,
            lastConnected: 2 * 60,
            joined: 24 * day
        ),
        Entry(
            nickname: "ERP (Odoo)",
            slug: "odoo",
            availability: .online,
            lastConnected: 60 * 60,
            joined: 18 * day
        ),
        Entry(
            nickname: "Files (Nextcloud)",
            slug: "nextcloud",
            availability: .online,
            lastConnected: 30 * 60,
            joined: 11 * day
        ),
        Entry(
            nickname: "Duty Roster",
            slug: "duty-roster",
            availability: .offline,
            lastConnected: 1 * day,
            joined: 5 * day
        ),
    ]

    private static let day: TimeInterval = 24 * 60 * 60

    private struct Entry {
        let nickname: String
        let slug: String
        let availability: Availability
        let lastConnected: TimeInterval
        let joined: TimeInterval
    }
}
