import SwiftUI
import WebKit
import WispersAccessSdk

/// The browser for one app of one share, pushed onto the roster's navigation
/// stack. It shows the retained `WKWebView` (kept alive by `BrowseSessionStore`,
/// so page state survives switching). Backing out returns to the roster — which
/// is how you switch — while the session stays warm for a while.
struct BrowserView: View {
    @Environment(ShareManager.self) private var manager
    let key: BrowseKey

    var body: some View {
        ZStack {
            Color(.systemBackground).ignoresSafeArea()

            if let session = manager.browser.session(for: key) {
                SessionWebView(session: session)
                    .ignoresSafeArea(.container, edges: .bottom)
                if let error = session.startupError {
                    ProxyErrorView(message: error) { session.start() }
                } else if session.isLoading {
                    ConnectingOverlay()
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
                let proxy = manager.proxy
            {
                manager.browser.open(share, app, proxy: proxy, auth: manager.proxyAuth)
            }
        }
        .onDisappear {
            manager.browser.resignActive(key)
        }
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
