import Foundation
import Observation
import WispersAccessSdk

/// Availability of a share for the status dot and labels. `.checking`, `.online`,
/// `.offline` and `.unknown` are transient observations; `.removed` / `.revoked`
/// are terminal — the host node has definitively turned this device away.
enum Availability {
    case checking  // not yet checked
    case online
    case offline
    case unknown  // check failed for a reason other than the host node saying no
    case removed
    case revoked
}

extension ShareState {
    /// The SDK's terminal states as availabilities; nil while live.
    var availability: Availability? {
        switch self {
        case .live: nil
        case .removed: .removed
        case .revoked: .revoked
        }
    }
}

/// Per-share availability, refreshed while a screen is visible: a single
/// source the list and detail screens both read so they can't disagree. A
/// check is the SDK's `refresh`, which reaches the host node over the share's
/// transport: an answer means online, a refusal offline.
@Observable
@MainActor
final class ShareStatusStore {
    /// Absent key = not checked yet.
    private(set) var statuses: [ShareId: Availability] = [:]

    func availability(for id: ShareId) -> Availability {
        statuses[id] ?? .checking
    }

    /// Refreshes every live share concurrently, each under its own deadline
    /// so one unreachable host node can't wedge the others. Terminal shares
    /// are skipped: the SDK's store already knows.
    func refresh(_ shares: [Share], using client: Client?, activity: ShareActivityStore) async {
        // Demo roster: fixed statuses, no host to poll.
        guard let client else {
            statuses = DemoMode.statuses
            return
        }
        let live = shares.filter { $0.state == .live }
        await withTaskGroup(of: (ShareId, Availability).self) { group in
            for share in live {
                group.addTask {
                    let availability = await Self.check(share.id, using: client)
                    return (share.id, availability)
                }
            }
            for await (id, availability) in group {
                statuses[id] = availability
                if availability == .online { activity.markConnected(id) }
            }
        }
    }

    private static func check(_ id: ShareId, using client: Client) async -> Availability {
        do {
            let changed = try await withDeadline(seconds: checkTimeout) {
                try await client.refresh(share: id)
            }
            return changed?.state.availability ?? .online
        } catch let error as SdkError {
            switch error {
            case .HostNode: return .offline
            default: return .unknown
            }
        } catch {
            return .unknown
        }
    }

    /// Generous per-share deadline: a reachable host node answers in well
    /// under a second; only a blackholing connect runs into this.
    private static let checkTimeout: Double = 10
}
