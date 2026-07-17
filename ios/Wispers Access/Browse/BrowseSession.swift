import Foundation
import Observation
import WebKit

/// One live share-browsing session: its loopback proxy and a retained
/// `WKWebView`, kept alive so the page persists while the user is on another
/// share or back on the list. The iOS answer to Android's per-share task —
/// concurrency lives in-app rather than in the OS switcher.
@MainActor
@Observable
final class BrowseSession: Identifiable {
    let shareID: ShareID
    let name: String
    nonisolated var id: ShareID { shareID }

    private(set) var url: URL?
    var isLoading = true
    private(set) var startupError: String?

    @ObservationIgnored let webView: WKWebView
    @ObservationIgnored private let proxy: LoopbackProxy
    @ObservationIgnored private let loadObserver = WebViewLoadObserver()

    init(share: ShareMetadata, sessionManager: SessionManager) {
        self.shareID = share.id
        self.name = share.nickname.isEmpty ? "Untitled share" : share.nickname
        self.proxy = LoopbackProxy(shareID: share.id, sessions: sessionManager)

        let configuration = WKWebViewConfiguration()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = true
        self.webView = WKWebView(frame: .zero, configuration: configuration)
        self.webView.allowsBackForwardNavigationGestures = true
        self.webView.navigationDelegate = loadObserver
        loadObserver.onLoading = { [weak self] loading in self?.isLoading = loading }
    }

    /// Starts the proxy and points the web view at it.
    func start() {
        startupError = nil
        isLoading = true
        Task { [weak self] in
            guard let self else { return }
            do {
                let port = try await proxy.start()
                let url = URL(string: "http://127.0.0.1:\(port)/")
                self.url = url
                if let url { webView.load(URLRequest(url: url)) }
            } catch {
                startupError = error.localizedDescription
            }
        }
    }

    func reload() { webView.reload() }

    /// Tears the session down (proxy stops; the web view is released with it).
    func stop() { proxy.stop() }
}

/// Reports `WKWebView` load state back to its `BrowseSession`.
final class WebViewLoadObserver: NSObject, WKNavigationDelegate {
    var onLoading: (Bool) -> Void = { _ in }

    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
        onLoading(true)
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        onLoading(false)
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        onLoading(false)
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        onLoading(false)
    }
}
