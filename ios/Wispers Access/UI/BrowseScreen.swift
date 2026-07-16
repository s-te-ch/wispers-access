import Observation
import SwiftUI
import WebKit

/// Owns one share's browsing session: starts its loopback proxy, exposes the
/// URL to point the WebView at, and tracks load/startup state. The QUIC
/// connection behind the proxy is cached app-wide by `SessionManager`, so
/// leaving and reopening a share reuses it.
@Observable
@MainActor
final class BrowseModel {
    private(set) var url: URL?
    private(set) var isLoading = true
    private(set) var startupError: String?

    @ObservationIgnored private var proxy: LoopbackProxy?
    @ObservationIgnored private weak var webView: WKWebView?

    func start(share: ShareMetadata, sessions: SessionManager) async {
        guard proxy == nil else { return }
        startupError = nil
        let proxy = LoopbackProxy(shareID: share.id, sessions: sessions)
        self.proxy = proxy
        do {
            let port = try await proxy.start()
            url = URL(string: "http://127.0.0.1:\(port)/")
        } catch {
            startupError = error.localizedDescription
        }
    }

    func stop() {
        proxy?.stop()
        proxy = nil
        url = nil
        isLoading = true
    }

    func setLoading(_ loading: Bool) { isLoading = loading }
    func attach(_ webView: WKWebView) { self.webView = webView }
    func reload() { webView?.reload() }
}

/// Opens a share: a WKWebView pointed at the share's loopback proxy, with a
/// connecting overlay and a startup-error retry.
struct BrowseScreen: View {
    let share: ShareMetadata
    @Environment(ShareManager.self) private var manager
    @State private var model = BrowseModel()

    var body: some View {
        ZStack {
            if let error = model.startupError {
                ProxyErrorView(message: error) {
                    Task { await restart() }
                }
            } else if let url = model.url {
                WebView(
                    url: url,
                    onLoadingChange: { model.setLoading($0) },
                    onWebView: { model.attach($0) }
                )
                .ignoresSafeArea(.container, edges: .bottom)
            }
            if model.isLoading && model.startupError == nil {
                ConnectingOverlay()
            }
        }
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    model.reload()
                } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .disabled(model.url == nil)
            }
        }
        .task { await model.start(share: share, sessions: manager.sessions) }
        .onDisappear { model.stop() }
    }

    private var title: String {
        share.nickname.isEmpty ? "Share" : share.nickname
    }

    private func restart() async {
        model.stop()
        await model.start(share: share, sessions: manager.sessions)
    }
}

/// Bridges a `WKWebView` into SwiftUI: JavaScript on, native back/forward swipe,
/// and load state reported through the navigation delegate.
private struct WebView: UIViewRepresentable {
    let url: URL
    let onLoadingChange: (Bool) -> Void
    let onWebView: (WKWebView) -> Void

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = true
        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.allowsBackForwardNavigationGestures = true
        webView.navigationDelegate = context.coordinator
        // Defer the reference hand-off and initial load out of the view-update
        // pass to avoid mutating state mid-render.
        let onWebView = onWebView
        DispatchQueue.main.async {
            onWebView(webView)
            webView.load(URLRequest(url: url))
        }
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {}

    func makeCoordinator() -> Coordinator {
        Coordinator(onLoadingChange: onLoadingChange)
    }

    final class Coordinator: NSObject, WKNavigationDelegate {
        private let onLoadingChange: (Bool) -> Void

        init(onLoadingChange: @escaping (Bool) -> Void) {
            self.onLoadingChange = onLoadingChange
        }

        func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
            onLoadingChange(false)
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            onLoadingChange(false)
        }

        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
            onLoadingChange(false)
        }
    }
}

private struct ConnectingOverlay: View {
    var body: some View {
        ZStack {
            Color(.systemBackground).ignoresSafeArea()
            VStack(spacing: 16) {
                ProgressView()
                Text("Connecting…").foregroundStyle(.secondary)
            }
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
