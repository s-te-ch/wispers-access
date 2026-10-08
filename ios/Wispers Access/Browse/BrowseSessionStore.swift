import Foundation
import Observation
import WispersAccessSdk

/// App-level set of open browsing sessions — the apps currently "warm". Each
/// session keeps its `WKWebView` alive so re-opening is instant and page state
/// persists.
///
/// iPhone has no per-document task switcher, so the roster doubles as the
/// switcher: opening an app pushes its browser; backing out to the roster (which
/// marks what's live) is how you switch. On iPad each app has a window of its
/// own instead, several of which can be on screen at once. A session whose
/// browser left the screen is torn down after a warm-TTL to free resources.
@MainActor
@Observable
final class BrowseSessionStore {
    private(set) var sessions: [BrowseSession] = []
    /// The apps whose browser is currently on screen: at most one on iPhone,
    /// one per app window on iPad.
    private(set) var onScreen: Set<SharedAppId> = []

    /// How long a backgrounded session stays warm before it's torn down.
    private let warmTTL: Duration = .seconds(300)
    @ObservationIgnored private var evictionTasks: [SharedAppId: Task<Void, Never>] = [:]

    /// Reports a site icon harvested by a session's web view (app, bytes, rank).
    @ObservationIgnored private let onIcon: (SharedAppId, Data, Int) -> Void

    init(onIcon: @escaping (SharedAppId, Data, Int) -> Void = { _, _, _ in }) {
        self.onIcon = onIcon
    }

    func session(for key: SharedAppId) -> BrowseSession? {
        sessions.first { $0.key == key }
    }

    /// Whether an app has a live (warm) session — drives the roster's live marker.
    func isWarm(_ key: SharedAppId) -> Bool {
        sessions.contains { $0.key == key }
    }

    /// Ensures a warm session exists for the app and marks it the on-screen one.
    /// Called by the browser as it appears, so navigating to an app is all it
    /// takes to start or resume it.
    @discardableResult
    func open(_ share: Share, _ app: SharedApp, proxy: PerAppProxy, auth: ProxyAuth) -> BrowseSession {
        let key = SharedAppId(shareID: share.id, appID: app.id)
        let session: BrowseSession
        if let existing = self.session(for: key) {
            session = existing
        } else {
            session = BrowseSession(share: share, app: app, proxy: proxy, auth: auth, onIcon: onIcon)
            sessions.append(session)
            session.start()
        }
        markActive(key)
        return session
    }

    /// Marks an app's browser as on screen: cancels any pending eviction.
    func markActive(_ key: SharedAppId) {
        cancelEviction(key)
        onScreen.insert(key)
    }

    /// The browser for this app left the screen: start its warm-TTL countdown.
    /// Re-opening within the TTL cancels it and reuses the warm web view.
    func resignActive(_ key: SharedAppId) {
        onScreen.remove(key)
        scheduleEviction(key)
    }

    /// Closes every session of a share now: web views released, evictions
    /// cancelled. For a share being removed.
    func close(_ shareID: ShareId) {
        for session in sessions where session.key.shareID == shareID {
            close(session.key)
        }
    }

    func close(_ key: SharedAppId) {
        cancelEviction(key)
        if let index = sessions.firstIndex(where: { $0.key == key }) {
            sessions[index].stop()
            sessions.remove(at: index)
        }
        onScreen.remove(key)
    }

    private func scheduleEviction(_ key: SharedAppId) {
        cancelEviction(key)
        let ttl = warmTTL
        evictionTasks[key] = Task { [weak self] in
            try? await Task.sleep(for: ttl)
            guard let self, !Task.isCancelled else { return }
            // Skip if it was re-opened while the timer ran.
            guard !self.onScreen.contains(key) else { return }
            self.close(key)
        }
    }

    private func cancelEviction(_ key: SharedAppId) {
        evictionTasks[key]?.cancel()
        evictionTasks[key] = nil
    }
}
