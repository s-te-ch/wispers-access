import Foundation
import Observation

/// App-level set of open browsing sessions — the shares currently "warm". Each
/// session keeps its loopback proxy + `WKWebView` alive so re-opening a share is
/// instant and page state persists.
///
/// iPhone has no per-document task switcher, so the roster doubles as the
/// switcher: opening a share pushes its browser; backing out to the roster (which
/// marks what's live) is how you switch. A backgrounded session is torn down
/// after a warm-TTL to free resources, so "several shares open at once" holds
/// without leaking proxies forever.
@MainActor
@Observable
final class BrowseSessionStore {
    private(set) var sessions: [BrowseSession] = []
    /// The share whose browser is currently on screen, if any.
    private(set) var activeShareID: ShareID?

    /// How long a backgrounded session stays warm before it's torn down.
    private let warmTTL: Duration = .seconds(300)
    @ObservationIgnored private var evictionTasks: [ShareID: Task<Void, Never>] = [:]

    func session(for shareID: ShareID) -> BrowseSession? {
        sessions.first { $0.shareID == shareID }
    }

    /// Whether a share has a live (warm) session — drives the roster's live marker.
    func isWarm(_ shareID: ShareID) -> Bool {
        sessions.contains { $0.shareID == shareID }
    }

    /// Ensures a warm session exists for the share and marks it the on-screen one.
    /// Called by the browser as it appears, so navigating to a share (a row tap,
    /// or Open from detail) is all it takes to start or resume it.
    @discardableResult
    func open(_ share: ShareMetadata, using sessionManager: SessionManager) -> BrowseSession {
        let session: BrowseSession
        if let existing = self.session(for: share.id) {
            session = existing
        } else {
            session = BrowseSession(share: share, sessionManager: sessionManager)
            sessions.append(session)
            session.start()
        }
        markActive(share.id)
        return session
    }

    /// Marks a share's browser as on screen: cancels any pending eviction.
    func markActive(_ shareID: ShareID) {
        cancelEviction(shareID)
        activeShareID = shareID
    }

    /// The browser for this share left the screen: start its warm-TTL countdown.
    /// Re-opening within the TTL cancels it and reuses the warm web view.
    func resignActive(_ shareID: ShareID) {
        if activeShareID == shareID { activeShareID = nil }
        scheduleEviction(shareID)
    }

    /// Closes a share now: proxy stops, web view is released, eviction cancelled.
    func close(_ shareID: ShareID) {
        cancelEviction(shareID)
        if let index = sessions.firstIndex(where: { $0.shareID == shareID }) {
            sessions[index].stop()
            sessions.remove(at: index)
        }
        if activeShareID == shareID { activeShareID = nil }
    }

    private func scheduleEviction(_ shareID: ShareID) {
        cancelEviction(shareID)
        let ttl = warmTTL
        evictionTasks[shareID] = Task { [weak self] in
            try? await Task.sleep(for: ttl)
            guard let self, !Task.isCancelled else { return }
            // Skip if it was re-opened while the timer ran.
            guard self.activeShareID != shareID else { return }
            self.close(shareID)
        }
    }

    private func cancelEviction(_ shareID: ShareID) {
        evictionTasks[shareID]?.cancel()
        evictionTasks[shareID] = nil
    }
}
