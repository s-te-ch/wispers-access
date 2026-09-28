import Foundation
import Network
import os

/// Tells the SDK to drop its cached connections when the device's network
/// path changes: Wi-Fi to cellular, one Wi-Fi to another, a VPN coming up.
///
/// This is what Safari does on handover: the OS signal beats waiting for
/// blackholed connections to hit their request timeouts, so the first request
/// after the switch reconnects immediately instead of stalling. While the app
/// is in the foreground the SDK also redials the connections in use, hiding
/// the setup cost from the user's next tap; in the background it only drops,
/// to avoid redialling on every handover while the phone roams unused. That
/// split lives in `ShareManager.networkChanged`, which this calls.
@MainActor
final class NetworkMonitor {
    private let monitor = NWPathMonitor()
    private var lastPath: NWPath?

    /// Starts watching. `onChange` runs on the main actor for every change
    /// to a usable path; the first path reported is the baseline, not a change.
    func start(onChange: @escaping @MainActor @Sendable () -> Void) {
        monitor.pathUpdateHandler = { path in
            Task { @MainActor in self.pathUpdated(path, onChange: onChange) }
        }
        monitor.start(queue: DispatchQueue(label: "dev.wispers.access.network-monitor"))
    }

    private func pathUpdated(_ path: NWPath, onChange: () -> Void) {
        let previous = lastPath
        lastPath = path
        guard let previous else {
            Logger.app.info("Network path: \(Self.describe(path), privacy: .public)")
            return
        }
        guard path.status == .satisfied, previous != path else { return }
        Logger.app.info(
            "Network path changed (\(Self.describe(previous), privacy: .public) -> \(Self.describe(path), privacy: .public)), dropping cached connections"
        )
        onChange()
    }

    /// The path's interfaces by name, best first, for the log line.
    private static func describe(_ path: NWPath) -> String {
        let names = path.availableInterfaces.map(\.name).joined(separator: ",")
        return names.isEmpty ? "none" : names
    }
}
