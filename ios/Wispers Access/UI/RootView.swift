import SwiftUI

/// The app's root: a navigation stack rooted at the share roster. The roster is
/// both home and switcher — tapping a share pushes its browser, the ⓘ pushes the
/// detail screen. There's no modal browser and no separate open-shares list;
/// backing out to the roster is how you switch between shares.
struct RootView: View {
    @Environment(BrowseRouter.self) private var router

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
    }
}

/// A destination reachable from the roster.
enum ShareRoute: Hashable {
    case browse(ShareID)
    case detail(ShareID)
}
