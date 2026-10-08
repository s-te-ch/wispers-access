import SwiftUI
import WispersAccessSdk

/// A main window's root. On iPhone, a navigation stack rooted at the share
/// roster; on iPad, the desktop app's sidebar layout, with each app opening in
/// a window of its own (see `BrowseRouter.opensWindows`).
struct RootView: View {
    @Environment(ShareManager.self) private var manager
    @Environment(QuickActionInbox.self) private var quickActions
    @Environment(\.openWindow) private var openWindow
    @State private var router = BrowseRouter(showing: DemoMode.initialDetail)

    var body: some View {
        Group {
            if BrowseRouter.opensWindows {
                ShareSplitView()
            } else {
                ShareStack()
            }
        }
        .environment(router)
        // Route a quick action: cold-launch (already set before we appear) and
        // warm (set while running) both land here.
        .onAppear(perform: routeQuickAction)
        .onChange(of: quickActions.pending) { routeQuickAction() }
    }

    /// Opens a pending quick-action app once, if it still exists.
    private func routeQuickAction() {
        guard let key = quickActions.pending else { return }
        quickActions.pending = nil
        guard let share = manager.share(key.shareID), share.state == .live,
            share.apps.contains(where: { $0.id == key.appID })
        else { return }
        if BrowseRouter.opensWindows {
            openWindow(value: key)
        } else {
            router.open(key)
        }
    }
}

/// The iPhone layout: the roster is both home and switcher — tapping a share's
/// app pushes its browser, its header pushes the detail screen. Backing out to
/// the roster is how you switch.
private struct ShareStack: View {
    @Environment(BrowseRouter.self) private var router

    var body: some View {
        @Bindable var router = router
        NavigationStack(path: $router.path) {
            ShareListScreen()
                .navigationDestination(for: ShareRoute.self) { route in
                    switch route {
                    case .browse(let key): BrowserView(key: key)
                    case .detail(let id): ShareDetailScreen(shareID: id)
                    }
                }
        }
    }
}
