import Foundation
import Observation
import WebKit
import WispersAccessSdk

/// Identifies one shared app across this app. An app's id is unique only
/// within its share, so it takes both. Browse sessions, cached icons, routes,
/// quick actions and iPad windows all refer to an app by it; Codable for the
/// windows, which the system restores with their value.
nonisolated struct SharedAppId: Hashable, Codable, Sendable {
    let shareID: ShareId
    let appID: String
}

/// One live browsing session: an app's URL and a retained `WKWebView`, kept
/// alive so the page persists while the user is on another share or back on
/// the list. The iOS answer to Android's per-share task — concurrency lives
/// in-app rather than in the OS switcher.
///
/// A reverse-proxy app is opened at its port on the SDK's per-app proxy. A
/// forward-proxy app is opened at its start URL, in its share's data store
/// (see `ShareDataStore`), with the share's loopback origins forwarded by the
/// SDK for as long as the session lives.
@MainActor
@Observable
final class BrowseSession: Identifiable {
    let key: SharedAppId
    let name: String
    nonisolated var id: SharedAppId { key }

    private(set) var url: URL?
    var isLoading = true
    private(set) var startupError: StartupError?

    @ObservationIgnored let webView: WKWebView
    @ObservationIgnored private let share: Share
    @ObservationIgnored private let app: SharedApp
    @ObservationIgnored private let proxy: PerAppProxy
    @ObservationIgnored private let auth: ProxyAuth
    @ObservationIgnored private let forwardProxy: Task<ForwardProxy, Error>?
    /// Holds the share's loopback ports while a forward-proxy app is open.
    @ObservationIgnored private var loopbackForwarder: LoopbackForwarder?
    @ObservationIgnored private let loadObserver = WebViewLoadObserver()
    @ObservationIgnored private let harvester: IconHarvester

    init(
        share: Share,
        app: SharedApp,
        proxy: PerAppProxy,
        auth: ProxyAuth,
        forwardProxy: Task<ForwardProxy, Error>?,
        onIcon: @escaping (SharedAppId, Data, Int) -> Void = { _, _, _ in }
    ) {
        self.key = SharedAppId(shareID: share.id, appID: app.id)
        self.name = app.name.isEmpty ? share.name : app.name
        self.share = share
        self.app = app
        self.proxy = proxy
        self.auth = auth
        self.forwardProxy = forwardProxy
        self.harvester = IconHarvester(key: key, onIcon: onIcon)

        let configuration = WKWebViewConfiguration()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = true
        configuration.userContentController.add(harvester, name: IconHarvester.messageName)
        if app.mode == .forwardProxy {
            configuration.websiteDataStore = ShareDataStore.store(for: share.id)
        }
        self.webView = WKWebView(frame: .zero, configuration: configuration)
        self.webView.allowsBackForwardNavigationGestures = true
        self.webView.navigationDelegate = loadObserver
        loadObserver.onLoading = { [weak self] loading in self?.isLoading = loading }
        // A page that never arrived (unreachable, refused by policy) would
        // leave the web view blank; say why instead.
        loadObserver.onFailed = { [weak self] error in self?.startupError = .other(error.localizedDescription) }
        // Re-harvest on each page-finish: the best icon can appear late (manifest
        // fetch) or change as the user navigates within the site.
        loadObserver.onFinished = { [weak self] in
            guard let self else { return }
            harvester.harvest(in: webView)
        }
    }

    /// Loads the app. A reverse-proxy app first needs its port on the per-app
    /// proxy; a forward-proxy app needs its share's data store routed through
    /// the forward proxy and the share's loopback origins forwarded. A failure
    /// shows on the session's error screen.
    func start() {
        startupError = nil
        isLoading = true
        Task { [weak self] in
            guard let self else { return }
            do {
                let url = try await app.mode == .forwardProxy
                    ? prepareForwardProxy() : prepareReverseProxy()
                self.url = url
                webView.load(URLRequest(url: url))
            } catch {
                startupError = StartupError(error)
            }
        }
    }

    /// Asks the per-app proxy for the app's URL, which binds its port on
    /// first use.
    private func prepareReverseProxy() async throws -> URL {
        // Install the proxy-auth cookie before the first load; without it the
        // proxy 403s the web view like any other local process.
        await webView.configuration.websiteDataStore.httpCookieStore.setCookie(auth.cookie())
        let base = try await proxy.baseUrl(share: key.shareID, appId: key.appID)
        guard let url = URL(string: base + "/") else { throw BrowseError.badURL(base) }
        return url
    }

    /// Routes the share's data store through the forward proxy, and has the
    /// SDK forward the share's loopback origins, with the cookie that lets
    /// this store's requests through, set before the first load.
    private func prepareForwardProxy() async throws -> URL {
        guard let forwardProxy, let startURL = app.startUrl else { throw BrowseError.noForwardProxy }
        guard let url = URL(string: startURL) else { throw BrowseError.badURL(startURL) }
        let proxy = try await forwardProxy.value
        let store = webView.configuration.websiteDataStore
        try ShareDataStore.route(store, for: share, through: proxy)
        let forwarder = try await proxy.forwardLoopbackOrigins(share: share.id)
        loopbackForwarder = forwarder
        if !forwarder.ports().isEmpty {
            // Cookies aren't scoped by port, so one per loopback name covers
            // every forwarded port.
            for domain in ["localhost", "127.0.0.1"] {
                await store.httpCookieStore.setCookie(Self.cookie(forwarder.cookie(), for: domain))
            }
        }
        return url
    }

    /// The forwarding cookie for one loopback name, `HttpOnly` so the app's
    /// pages can't read or delete it.
    private static func cookie(_ cookie: RequiredCookie, for domain: String) -> HTTPCookie {
        HTTPCookie(properties: [
            .domain: domain,
            .path: "/",
            .name: cookie.name,
            .value: cookie.value,
            HTTPCookiePropertyKey("HttpOnly"): "TRUE",
        ])!
    }


    func reload() { webView.reload() }

    /// Tears the session down; the web view is released with it. The app's
    /// port stays bound in the proxy, as ports are per app, not per session.
    func stop() {
        // Break the userContentController → handler retention explicitly; the web
        // view is about to be released, but this keeps teardown tidy.
        webView.configuration.userContentController.removeScriptMessageHandler(forName: IconHarvester.messageName)
        // The share's loopback ports are released once its last session lets go.
        loopbackForwarder = nil
    }
}

/// Why a session couldn't start, for its error screen.
enum StartupError: Equatable {
    /// A port the share's loopback origins need is taken. The SDK's message
    /// says by whom.
    case portInUse(String)
    case other(String)

    init(_ error: Error) {
        if case let SdkError.PortInUse(message) = error {
            self = .portInUse(message)
        } else {
            self = .other(error.localizedDescription)
        }
    }
}

/// Why a session couldn't be set up, past what the SDK reports.
enum BrowseError: LocalizedError {
    case badURL(String)
    case noForwardProxy

    var errorDescription: String? {
        switch self {
        case .badURL(let url): "The app's address is not a URL: \(url)"
        case .noForwardProxy: "This app needs the forward proxy, which isn't running."
        }
    }
}

/// Reports `WKWebView` load state back to its `BrowseSession`.
final class WebViewLoadObserver: NSObject, WKNavigationDelegate {
    var onLoading: (Bool) -> Void = { _ in }
    var onFinished: () -> Void = {}
    var onFailed: (Error) -> Void = { _ in }

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
        // A navigation the page cancelled itself, or replaced, isn't a failure.
        if (error as NSError).code != NSURLErrorCancelled { onFailed(error) }
    }
}
