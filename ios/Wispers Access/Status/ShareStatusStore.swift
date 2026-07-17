import Foundation
import Observation

/// Availability of a share for the status dot and labels. `.checking`, `.online`,
/// `.offline` and `.unknown` are transient observations; `.removed` / `.revoked`
/// are terminal — the hub has definitively rejected this device.
enum Availability {
    case checking  // not yet checked
    case online
    case offline
    case unknown  // check failed — typically the hub is unreachable
    case removed
    case revoked
}

extension TerminalShareState {
    /// The persisted terminal state as an availability, for uniform rendering.
    var availability: Availability {
        switch self {
        case .removed: return .removed
        case .revoked: return .revoked
        }
    }
}

/// Per-share availability, refreshed while a screen is visible. The iOS analog
/// of Android's `ShareStatusTracker` — a single source the list and detail
/// screens both read so they can't disagree.
@Observable
@MainActor
final class ShareStatusStore {
    /// Absent key = not checked yet.
    private(set) var statuses: [ShareID: Availability] = [:]

    func availability(for id: ShareID) -> Availability {
        statuses[id] ?? .checking
    }

    /// Refreshes every id concurrently, each under its own deadline so one
    /// blackholing hub (an unreachable self-hosted backend hangs the connect
    /// for minutes) can't wedge the others. A terminal result is persisted to
    /// `store`, and terminal shares are skipped — that state is forever.
    func refresh(_ ids: [ShareID], using sessions: SessionManager, store: ShareStore) async {
        let live = ids.filter { store.metadata(for: $0)?.terminalState == nil }
        await withTaskGroup(of: (ShareID, Availability).self) { group in
            for id in live {
                group.addTask {
                    let availability =
                        (try? await withDeadline(seconds: Self.checkTimeout) {
                            await sessions.checkAvailability(id)
                        }) ?? .unknown
                    return (id, availability)
                }
            }
            for await (id, availability) in group {
                statuses[id] = availability
                switch availability {
                case .removed: store.markTerminal(id, .removed)
                case .revoked: store.markTerminal(id, .revoked)
                default: break
                }
            }
        }
    }

    /// Generous per-share deadline: a healthy hub answers in well under a
    /// second; only a blackholing connect runs into this.
    private static let checkTimeout: Double = 10
}
