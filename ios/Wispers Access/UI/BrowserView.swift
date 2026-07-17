import SwiftUI
import WebKit

/// The browser for one open share, pushed onto the roster's navigation stack. It
/// shows the share's retained `WKWebView` (kept alive by `BrowseSessionStore`, so
/// page state survives switching). Backing out returns to the roster — which is
/// how you switch shares — while the session stays warm for a while.
struct BrowserView: View {
    @Environment(ShareManager.self) private var manager
    @Environment(ShareStore.self) private var store
    let shareID: ShareID

    var body: some View {
        ZStack {
            Color(.systemBackground).ignoresSafeArea()

            if let session = manager.browser.session(for: shareID) {
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
                Button { manager.browser.session(for: shareID)?.reload() } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .foregroundStyle(AccessColor.primaryDark)
                .disabled(manager.browser.session(for: shareID) == nil)
            }
        }
        .onAppear {
            // Ensure the (warm) session exists and mark it on-screen.
            if let share = store.metadata(for: shareID) {
                manager.browser.open(share, using: manager.sessions)
            }
        }
        .onDisappear {
            manager.browser.resignActive(shareID)
        }
    }

    private var title: String {
        guard let nickname = store.metadata(for: shareID)?.nickname, !nickname.isEmpty else {
            return "Share"
        }
        return nickname
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
