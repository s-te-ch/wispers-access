import SwiftUI
import UIKit

/// The app's root: a navigation stack rooted at the share roster. The roster is
/// both home and switcher — tapping a share pushes its app's browser (or its
/// detail screen when there is more than one app), the ⓘ pushes the detail
/// screen. Backing out to the roster is how you switch.
struct RootView: View {
    @Environment(BrowseRouter.self) private var router
    @Environment(ShareManager.self) private var manager
    @Environment(QuickActionInbox.self) private var quickActions
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        @Bindable var router = router
        NavigationStack(path: $router.path) {
            ShareListScreen()
                .navigationDestination(for: ShareRoute.self) { route in
                    switch route {
                    case .browse(let id, let appID): BrowserView(shareID: id, appID: appID)
                    case .detail(let id): ShareDetailScreen(shareID: id)
                    }
                }
        }
        // Route a quick action: cold-launch (already set before we appear) and
        // warm (set while running) both land here.
        .onAppear(perform: routeQuickAction)
        .onChange(of: quickActions.pendingShareID) { routeQuickAction() }
        // Keep the app-icon shortcuts current — Apple's cue to refresh them.
        .onChange(of: scenePhase) { _, phase in
            if phase == .background {
                UIApplication.shared.shortcutItems = QuickAction.shortcutItems(
                    for: manager.shares, activity: manager.activity)
            }
        }
    }

    /// Opens a pending quick-action share once, if it still exists.
    private func routeQuickAction() {
        guard let id = quickActions.pendingShareID else { return }
        quickActions.pendingShareID = nil
        guard let share = manager.share(id) else { return }
        router.open(share)
    }
}
