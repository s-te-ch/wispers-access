import SwiftUI
import UIKit
import WispersAccessSdk

/// An iPad window of its own for one app, opened from the share detail, so the
/// OS switches between the apps you have open as between any others — the
/// iPad's answer to Android's per-share tasks. Opening an app that already has
/// a window brings that one forward. The window closes itself once its app is
/// gone: the share removed from this device, or the app no longer shared.
struct AppWindow: View {
    /// Nil only if the system restored a window whose value it couldn't read.
    let key: SharedAppId?

    @Environment(ShareManager.self) private var manager
    @Environment(\.dismissWindow) private var dismissWindow
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        NavigationStack {
            if let key {
                BrowserView(key: key)
                    .toolbar {
                        ToolbarItem(placement: .topBarLeading) {
                            sharesButton
                        }
                    }
            }
        }
        .onChange(of: isGone, initial: true) { _, gone in
            if gone { dismissWindow() }
        }
    }

    /// The way back to the share list, which an app window otherwise lacks:
    /// the window stays open behind it, like an app you switched away from.
    private var sharesButton: some View {
        Button {
            RosterWindow.show(openWindow: openWindow)
        } label: {
            // Spelled out: toolbars show a Label's icon alone, and a bare
            // chevron above a web page reads as the site's own back.
            HStack(spacing: 4) {
                Image(systemName: "chevron.backward")
                Text("Shares")
            }
            .padding(.horizontal, 6)
        }
        .foregroundStyle(AccessColor.primaryDark)
        .keyboardShortcut("0")
    }

    private var isGone: Bool {
        guard let key, let share = manager.share(key.shareID) else { return true }
        return !share.apps.contains { $0.id == key.appID }
    }
}

/// The main window with the share list, as the app windows find their way back
/// to it. SwiftUI can open a new main window but not bring an existing one
/// forward, so each main window registers its scene session as it comes to the
/// front, and the last one registered is the one shown.
enum RosterWindow {
    /// The main window group's id, for opening a new one.
    static let id = "roster"

    private static var session: UISceneSession?

    /// Brings the last main window forward, or opens a new one when there is
    /// none left (closed in the app switcher, say).
    static func show(openWindow: OpenWindowAction) {
        if let session, UIApplication.shared.openSessions.contains(session) {
            UIApplication.shared.activateSceneSession(for: UISceneSessionActivationRequest(session: session))
        } else {
            openWindow(id: id)
        }
    }

    /// Records a main window's scene session; see `Registrar`.
    static func register(_ session: UISceneSession) {
        self.session = session
    }

    /// Placed in a main window's background: registers the window's scene
    /// session once the view is in a window, and again whenever it's asked to
    /// (the window becoming active), so the front-most main window wins.
    struct Registrar: UIViewRepresentable {
        var isActive: Bool

        func makeUIView(context: Context) -> SessionView { SessionView() }

        func updateUIView(_ view: SessionView, context: Context) {
            if isActive { view.register() }
        }

        final class SessionView: UIView {
            override func didMoveToWindow() {
                super.didMoveToWindow()
                register()
            }

            func register() {
                if let session = window?.windowScene?.session {
                    RosterWindow.register(session)
                }
            }
        }
    }
}
