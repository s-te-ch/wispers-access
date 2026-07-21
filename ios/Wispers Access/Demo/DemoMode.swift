import Foundation

/// Static roster for App Store screenshots: replaces the persisted roster and
/// the live hub polling with fixed shares and statuses, so captures are
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

    /// A ShareManager whose stores live in a throwaway temp directory seeded
    /// with the fixed roster — the real shares.json / share-icons.json are
    /// never touched.
    static func makeManager() -> ShareManager {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("demo-roster")
        // A previous run's seed would resurface with stale timestamps.
        try? FileManager.default.removeItem(at: dir)
        let store = ShareStore(fileURL: dir.appendingPathComponent("shares.json"))
        let icons = ShareIconStore(fileURL: dir.appendingPathComponent("share-icons.json"))
        let now = Date()
        for entry in roster {
            store.add(
                ShareMetadata(
                    id: entry.id,
                    nickname: entry.nickname,
                    backend: nil,
                    createdAt: now.addingTimeInterval(-entry.joined),
                    lastConnectedAt: now.addingTimeInterval(-entry.lastConnected)
                )
            )
            if let url = Bundle.main.url(forResource: entry.slug, withExtension: "png"),
                let png = try? Data(contentsOf: url)
            {
                icons.update(png, rank: 1, for: entry.id)
            }
        }
        return ShareManager(store: store, icons: icons)
    }

    /// A screen to open on launch (`--demo-detail <slug>`, e.g. `grafana`), so
    /// capture scripts can shoot the detail screen without synthesizing taps.
    static var initialRoute: ShareRoute? {
        guard active else { return nil }
        let args = ProcessInfo.processInfo.arguments
        guard let flag = args.firstIndex(of: "--demo-detail"), flag + 1 < args.count else {
            return nil
        }
        return .detail(ShareID(args[flag + 1]))
    }

    /// Fixed per-share availability, replacing the hub poll.
    static var statuses: [ShareID: Availability] {
        Dictionary(uniqueKeysWithValues: roster.map { ($0.id, $0.availability) })
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

        var id: ShareID { ShareID(slug) }
    }
}
