import Foundation
import Observation

/// App-level set of open browsing sessions — the shares currently "open",
/// switchable in-app. Sessions stay alive across navigation and while the
/// browser is dismissed, so switching back is instant and page state persists.
@MainActor
@Observable
final class BrowseSessionStore {
    private(set) var sessions: [BrowseSession] = []
    var activeShareID: ShareID?
    /// Whether the full-screen browser is on screen.
    var isPresented = false

    var activeSession: BrowseSession? {
        sessions.first { $0.shareID == activeShareID }
    }

    /// Opens (or re-focuses) a share and presents the browser.
    func open(_ share: ShareMetadata, using sessionManager: SessionManager) {
        if !sessions.contains(where: { $0.shareID == share.id }) {
            let session = BrowseSession(share: share, sessionManager: sessionManager)
            sessions.append(session)
            session.start()
        }
        activeShareID = share.id
        isPresented = true
    }

    func switchTo(_ shareID: ShareID) {
        activeShareID = shareID
    }

    /// Closes one open share. Dismisses the browser when the last one closes.
    func close(_ shareID: ShareID) {
        if let index = sessions.firstIndex(where: { $0.shareID == shareID }) {
            sessions[index].stop()
            sessions.remove(at: index)
        }
        if activeShareID == shareID {
            activeShareID = sessions.last?.shareID
        }
        if sessions.isEmpty {
            isPresented = false
        }
    }
}
