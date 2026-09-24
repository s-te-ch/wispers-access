import Observation
import WispersAccessSdk

/// A destination reachable from the roster.
enum ShareRoute: Hashable {
    case browse(BrowseKey)
    case detail(ShareId)
}

/// The roster's navigation state — the routes pushed onto the stack. Held in the
/// environment so programmatic opens (the add-flow's "Open", app quick actions)
/// can push a screen, not only the roster's own value-based `NavigationLink`s.
/// The roster is home *and* switcher, so there's one path.
@Observable
@MainActor
final class BrowseRouter {
    var path: [ShareRoute] = []

    /// A share to open once a transient sheet (the add flow) has dismissed.
    /// Consumed in the sheet's `onDismiss`, so we never mutate the nav stack while
    /// the sheet is still on screen (which SwiftUI handles poorly).
    var openAfterDismiss: ShareId?

    /// Opens a share the quickest way there is: its only app, or its detail
    /// screen when there are several to choose from, or none yet.
    func open(_ share: Share) {
        if share.state == .live, share.apps.count == 1 {
            path.append(.browse(BrowseKey(shareID: share.id, appID: share.apps[0].id)))
        } else {
            path.append(.detail(share.id))
        }
    }

    func open(_ key: BrowseKey) {
        path.append(.browse(key))
    }
}
