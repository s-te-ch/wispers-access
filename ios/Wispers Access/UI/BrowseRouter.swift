import Observation
import WispersAccessSdk

/// A destination reachable from the roster.
enum ShareRoute: Hashable {
    case browse(ShareId, appID: String)
    case detail(ShareId)

    /// Where a share's card leads: straight into its only app, or to the
    /// detail screen, which lists several apps, none yet, or the reason a
    /// terminal share can't be opened.
    static func forCard(_ share: Share) -> ShareRoute {
        if share.state == .live, share.apps.count == 1 {
            return .browse(share.id, appID: share.apps[0].id)
        }
        return .detail(share.id)
    }
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

    func open(_ share: Share) {
        path.append(.forCard(share))
    }
}
