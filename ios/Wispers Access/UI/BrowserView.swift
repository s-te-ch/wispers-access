import SwiftUI
import WebKit
import WispersAccessSdk

/// The browser for one app of one share, pushed onto the roster's navigation
/// stack. It shows the retained `WKWebView` (kept alive by `BrowseSessionStore`,
/// so page state survives switching). Backing out returns to the roster — which
/// is how you switch — while the session stays warm for a while.
struct BrowserView: View {
    @Environment(ShareManager.self) private var manager
    let key: SharedAppId

    var body: some View {
        ZStack {
            Color(.systemBackground).ignoresSafeArea()

            if let reason = unavailable {
                // A quick action or a restored window can lead to an app this
                // device can't open.
                ContentUnavailableView(
                    "Can't open this app", systemImage: "exclamationmark.triangle",
                    description: Text(reason))
            } else if let session = manager.browser.session(for: key) {
                SessionWebView(session: session)
                    .ignoresSafeArea(.container, edges: .bottom)
                switch session.startupError {
                case .portInUse(let message):
                    PortInUseView(message: message) { session.start() }
                case .other(let message):
                    ProxyErrorView(message: message) { session.start() }
                case nil where session.isLoading:
                    ConnectingOverlay()
                case nil:
                    EmptyView()
                }
            } else {
                ConnectingOverlay()
            }
        }
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button { manager.browser.session(for: key)?.reload() } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .foregroundStyle(AccessColor.primaryDark)
                .disabled(manager.browser.session(for: key) == nil)
            }
        }
        .onAppear {
            // Ensure the (warm) session exists and mark it on-screen.
            if let share = manager.share(key.shareID),
                let app = share.apps.first(where: { $0.id == key.appID }),
                whyUnavailable(app) == nil,
                let proxy = manager.proxy
            {
                manager.browser.open(
                    share, app, proxy: proxy, auth: manager.proxyAuth,
                    forwardProxy: manager.forwardProxy)
            }
        }
        .onDisappear {
            manager.browser.resignActive(key)
        }
    }

    /// Why this device can't open the app, if it can't.
    private var unavailable: String? {
        manager.share(key.shareID)?.apps.first { $0.id == key.appID }.flatMap(whyUnavailable)
    }

    private var title: String {
        guard let share = manager.share(key.shareID) else { return "App" }
        let app = share.apps.first { $0.id == key.appID }
        let name = app?.name ?? key.appID
        return name.isEmpty ? share.name : name
    }
}

/// Displays a session's retained `WKWebView` (never recreated, so page state
/// survives switching away and back).
private struct SessionWebView: UIViewRepresentable {
    let session: BrowseSession

    func makeUIView(context: Context) -> WKWebView { session.webView }
    func updateUIView(_ uiView: WKWebView, context: Context) {}
}

private struct ConnectingOverlay: View {
    var body: some View {
        ZStack {
            Color(.systemBackground)
            VStack(spacing: 16) {
                ProgressView()
                Text("Connecting…").foregroundStyle(.secondary)
            }
        }
        .ignoresSafeArea()
    }
}

/// A forward-proxy app whose share's loopback ports can't all be bound, such
/// as when another share's app holds one of them.
private struct PortInUseView: View {
    let message: String
    let onRetry: () -> Void

    var body: some View {
        ContentUnavailableView {
            Label("Port in use", systemImage: "exclamationmark.triangle")
        } description: {
            Text("\(message) Close what's using it, then try again.")
        } actions: {
            Button("Retry", action: onRetry)
                .buttonStyle(.borderedProminent)
        }
    }
}

private struct ProxyErrorView: View {
    let message: String
    let onRetry: () -> Void

    var body: some View {
        ContentUnavailableView {
            Label("Connection problem", systemImage: "wifi.exclamationmark")
        } description: {
            Text(message)
        } actions: {
            Button("Retry", action: onRetry)
                .buttonStyle(.borderedProminent)
        }
    }
}
