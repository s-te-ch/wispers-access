import Observation
import UIKit
import WispersAccessSdk

/// A destination reachable from the roster.
enum ShareRoute: Hashable {
    case browse(BrowseKey)
    case detail(ShareId)
}

/// A main window's navigation state: on iPhone the routes pushed onto the
/// roster's stack, on iPad the share selected in the sidebar. Held in the
/// environment so programmatic opens (the add-flow's "Open", app quick actions)
/// can navigate, not only the roster's own links.
@Observable
@MainActor
final class BrowseRouter {
    /// Whether apps open in windows of their own, which the OS switches between
    /// like any other app's (iPad). Otherwise there is just the one window, and
    /// apps are pushed onto the roster's stack (iPhone).
    static let opensWindows = UIApplication.shared.supportsMultipleScenes

    var path: [ShareRoute] = []

    /// The share whose detail the sidebar shows (iPad).
    var selection: ShareId?

    /// A share to open once a transient sheet (the add flow) has dismissed.
    /// Consumed in the sheet's `onDismiss`, so we never mutate the nav stack while
    /// the sheet is still on screen (which SwiftUI handles poorly).
    var openAfterDismiss: ShareId?

    /// Starts out showing a share's detail, if given one: selected on iPad,
    /// pushed on iPhone.
    init(showing detail: ShareId? = nil) {
        guard let detail else { return }
        if Self.opensWindows {
            selection = detail
        } else {
            path = [.detail(detail)]
        }
    }

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
