import Foundation
import Observation
import WebKit
import WispersAccessSdk

/// One app of one share, as the browse sessions tell them apart.
struct BrowseKey: Hashable, Sendable {
    let shareID: ShareId
    let appID: String
}

/// One live browsing session: an app's URL on the SDK's proxy and a retained
/// `WKWebView`, kept alive so the page persists while the user is on another
/// share or back on the list. The iOS answer to Android's per-share task —
/// concurrency lives in-app rather than in the OS switcher.
@MainActor
@Observable
final class BrowseSession: Identifiable {
    let key: BrowseKey
    let name: String
    nonisolated var id: BrowseKey { key }

    private(set) var url: URL?
    var isLoading = true
    private(set) var startupError: String?

    @ObservationIgnored let webView: WKWebView
    @ObservationIgnored private let proxy: PerAppProxy
    @ObservationIgnored private let auth: ProxyAuth
    @ObservationIgnored private let loadObserver = WebViewLoadObserver()
    @ObservationIgnored private let harvester: IconHarvester

    init(
        share: Share,
        app: SharedApp,
        proxy: PerAppProxy,
        auth: ProxyAuth,
        onIcon: @escaping (BrowseKey, Data, Int) -> Void = { _, _, _ in }
    ) {
        self.key = BrowseKey(shareID: share.id, appID: app.id)
        self.name = app.name.isEmpty ? share.name : app.name
        self.proxy = proxy
        self.auth = auth
        self.harvester = IconHarvester(key: key, onIcon: onIcon)

        let configuration = WKWebViewConfiguration()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = true
        configuration.userContentController.add(harvester, name: IconHarvester.messageName)
        self.webView = WKWebView(frame: .zero, configuration: configuration)
        self.webView.allowsBackForwardNavigationGestures = true
        self.webView.navigationDelegate = loadObserver
        loadObserver.onLoading = { [weak self] loading in self?.isLoading = loading }
        // Re-harvest on each page-finish: the best icon can appear late (manifest
        // fetch) or change as the user navigates within the site.
        loadObserver.onFinished = { [weak self] in
            guard let self else { return }
            harvester.harvest(in: webView)
        }
    }

    /// Asks the proxy for the app's URL, which binds its port on first use,
    /// and points the web view at it.
    func start() {
        startupError = nil
        isLoading = true
        Task { [weak self] in
            guard let self else { return }
            do {
                // Install the proxy-auth cookie before the first load; without
                // it the proxy 403s the web view like any other local process.
                await webView.configuration.websiteDataStore.httpCookieStore
                    .setCookie(auth.cookie())
                let base = try await proxy.baseUrl(share: key.shareID, appId: key.appID)
                let url = URL(string: base + "/")
                self.url = url
                if let url { webView.load(URLRequest(url: url)) }
            } catch {
                startupError = error.localizedDescription
            }
        }
    }

    func reload() { webView.reload() }

    /// Tears the session down; the web view is released with it. The app's
    /// port stays bound in the proxy, as ports are per app, not per session.
    func stop() {
        // Break the userContentController → handler retention explicitly; the web
        // view is about to be released, but this keeps teardown tidy.
        webView.configuration.userContentController.removeScriptMessageHandler(forName: IconHarvester.messageName)
    }
}

/// Reports `WKWebView` load state back to its `BrowseSession`.
final class WebViewLoadObserver: NSObject, WKNavigationDelegate {
    var onLoading: (Bool) -> Void = { _ in }
    var onFinished: () -> Void = {}

    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
        onLoading(true)
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        onLoading(false)
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        onLoading(false)
        onFinished()
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        onLoading(false)
    }
}
