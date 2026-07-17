import SwiftUI
import UIKit

/// The app's root: a navigation stack rooted at the share roster. The roster is
/// both home and switcher — tapping a share pushes its browser, the ⓘ pushes the
/// detail screen. There's no modal browser and no separate open-shares list;
/// backing out to the roster is how you switch between shares.
struct RootView: View {
    @Environment(BrowseRouter.self) private var router
    @Environment(ShareStore.self) private var store
    @Environment(QuickActionInbox.self) private var quickActions
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        @Bindable var router = router
        NavigationStack(path: $router.path) {
            ShareListScreen()
                .navigationDestination(for: ShareRoute.self) { route in
                    switch route {
                    case .browse(let id): BrowserView(shareID: id)
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
                UIApplication.shared.shortcutItems = QuickAction.shortcutItems(for: store.shares)
            }
        }
    }

    /// Opens a pending quick-action share once, if it still exists.
    private func routeQuickAction() {
        guard let id = quickActions.pendingShareID else { return }
        quickActions.pendingShareID = nil
        guard store.metadata(for: id) != nil else { return }
        router.open(id)
    }
}

/// A destination reachable from the roster.
enum ShareRoute: Hashable {
    case browse(ShareID)
    case detail(ShareID)
}
