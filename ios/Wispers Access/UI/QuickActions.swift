import UIKit
import Observation

/// Home-screen quick actions: long-press the app icon to jump straight into a
/// recent share. The dynamic item list is rebuilt from the roster; the selected
/// one is captured by the UIKit lifecycle and routed once SwiftUI is ready.
enum QuickAction {
    static let openShareType = "dev.wispers.access.openShare"
    private static let shareIDKey = "shareID"

    /// The target share of an "open share" shortcut, if the item is one of ours.
    static func shareID(from item: UIApplicationShortcutItem) -> ShareID? {
        guard item.type == openShareType,
            let raw = item.userInfo?[shareIDKey] as? String
        else { return nil }
        return ShareID(raw)
    }

    /// The dynamic shortcut list from the roster: most-recently-connected first,
    /// capped to the four the launcher shows.
    static func shortcutItems(for shares: [ShareMetadata]) -> [UIApplicationShortcutItem] {
        shares
            .sorted { ($0.lastConnectedAt ?? $0.createdAt) > ($1.lastConnectedAt ?? $1.createdAt) }
            .prefix(4)
            .map { share in
                UIApplicationShortcutItem(
                    type: openShareType,
                    localizedTitle: share.nickname.isEmpty ? "Untitled share" : share.nickname,
                    localizedSubtitle: nil,
                    icon: UIApplicationShortcutIcon(systemImageName: "globe"),
                    userInfo: [shareIDKey: share.id.value as NSString]
                )
            }
    }
}

/// A one-slot handoff for a quick action captured by the UIKit app lifecycle
/// (cold-launch scene connection options, or a warm `performActionFor`) before
/// the SwiftUI scene is ready to route it. `RootView` drains it into the router.
@MainActor
@Observable
final class QuickActionInbox {
    static let shared = QuickActionInbox()
    var pendingShareID: ShareID?

    nonisolated init() {}

    @discardableResult
    func handle(_ item: UIApplicationShortcutItem) -> Bool {
        guard let id = QuickAction.shareID(from: item) else { return false }
        pendingShareID = id
        return true
    }
}

/// Bridges the UIKit app lifecycle to SwiftUI for the parts SwiftUI doesn't
/// surface — here, home-screen quick actions. Installed via
/// `@UIApplicationDelegateAdaptor`; it only reads the launch/warm shortcut and
/// otherwise returns SwiftUI's default scene configuration untouched.
final class AppDelegate: NSObject, UIApplicationDelegate {
    func application(
        _ application: UIApplication,
        configurationForConnecting connectingSceneSession: UISceneSession,
        options: UIScene.ConnectionOptions
    ) -> UISceneConfiguration {
        // Cold launch from a quick action: the shortcut rides in on the scene's
        // connection options. Capture it here, and route a custom scene delegate
        // so warm launches (which SwiftUI's own scene delegate otherwise swallows)
        // reach us too.
        if let shortcut = options.shortcutItem {
            QuickActionInbox.shared.handle(shortcut)
        }
        let config = UISceneConfiguration(name: nil, sessionRole: connectingSceneSession.role)
        config.delegateClass = SceneDelegate.self
        return config
    }
}

/// A scene delegate whose ONLY job is to catch warm-launch quick actions — the
/// app-delegate `performActionFor` never fires under SwiftUI. It deliberately does
/// not implement `scene(_:willConnectTo:)`, so SwiftUI still owns window setup.
final class SceneDelegate: NSObject, UIWindowSceneDelegate {
    func windowScene(
        _ windowScene: UIWindowScene,
        performActionFor shortcutItem: UIApplicationShortcutItem,
        completionHandler: @escaping (Bool) -> Void
    ) {
        completionHandler(QuickActionInbox.shared.handle(shortcutItem))
    }
}
