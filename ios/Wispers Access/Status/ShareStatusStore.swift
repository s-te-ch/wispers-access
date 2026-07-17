import Foundation
import Observation

/// Whether a share's serving node is reachable, for the status dot.
enum Availability {
    case checking  // not yet known, or hub unreachable
    case online
    case offline
}

/// Per-share serving-node availability, refreshed while a screen is visible.
/// The iOS analog of Android's `ShareStatusTracker` — a single source the list
/// and detail screens both read so they can't disagree.
@Observable
@MainActor
final class ShareStatusStore {
    /// Absent key = not checked yet; `nil` value = checked but unknown (hub
    /// unreachable); `true`/`false` = online/offline.
    private(set) var statuses: [ShareID: Bool?] = [:]

    func availability(for id: ShareID) -> Availability {
        switch statuses[id] {
        case .some(.some(true)): return .online
        case .some(.some(false)): return .offline
        default: return .checking
        }
    }

    /// Refreshes every id concurrently via `groupInfo()` (node 1's `isOnline`).
    func refresh(_ ids: [ShareID], using sessions: SessionManager) async {
        await withTaskGroup(of: (ShareID, Bool?).self) { group in
            for id in ids {
                group.addTask { (id, await sessions.isServingNodeOnline(id)) }
            }
            for await (id, online) in group {
                statuses[id] = online
            }
        }
    }
}
