import Observation

/// The roster's navigation state — the shares pushed onto the stack. Held in the
/// environment so programmatic opens (the add-flow's "Open share", later app
/// quick actions) can push the browser, not only the roster's own value-based
/// `NavigationLink`s. The roster is home *and* switcher, so there's one path.
@Observable
@MainActor
final class BrowseRouter {
    var path: [ShareRoute] = []

    /// A share to open once a transient sheet (the add flow) has dismissed.
    /// Consumed in the sheet's `onDismiss`, so we never mutate the nav stack while
    /// the sheet is still on screen (which SwiftUI handles poorly).
    var openAfterDismiss: ShareID?

    func open(_ id: ShareID) {
        path.append(.browse(id))
    }
}
