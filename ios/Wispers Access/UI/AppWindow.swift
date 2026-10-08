import SwiftUI
import WispersAccessSdk

/// An iPad window of its own for one app, opened from the share detail, so the
/// OS switches between the apps you have open as between any others — the
/// iPad's answer to Android's per-share tasks. Opening an app that already has
/// a window brings that one forward. The window closes itself once its app is
/// gone: the share removed from this device, or the app no longer shared.
struct AppWindow: View {
    /// Nil only if the system restored a window whose value it couldn't read.
    let key: BrowseKey?

    @Environment(ShareManager.self) private var manager
    @Environment(\.dismissWindow) private var dismissWindow

    var body: some View {
        NavigationStack {
            if let key {
                BrowserView(key: key)
            }
        }
        .onChange(of: isGone, initial: true) { _, gone in
            if gone { dismissWindow() }
        }
    }

    private var isGone: Bool {
        guard let key, let share = manager.share(key.shareID) else { return true }
        return !share.apps.contains { $0.id == key.appID }
    }
}
