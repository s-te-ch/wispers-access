import Foundation
import Observation
import WispersAccessSdk
import os

/// App coordinator for joined shares: the SDK client, the shares as its store
/// has them, and the app-side state around them (availability, icons, last
/// use, open browse sessions). Injected via `@Environment`. In demo mode there
/// is no client and the roster is fixed.
@Observable
@MainActor
final class ShareManager {
    /// The shares as the SDK's store has them, newest join last. Reloaded
    /// whenever the SDK reports a change.
    private(set) var shares: [Share] = []

    /// Per-share availability, polled while a screen is visible.
    let status = ShareStatusStore()

    /// Site icons harvested while browsing, for the roster/detail avatars.
    let icons: ShareIconStore

    /// When each share was last reached, for the roster's "LAST 5M AGO".
    let activity: ShareActivityStore

    /// The SDK client, or nil in demo mode.
    @ObservationIgnored let client: Client?

    /// One loopback proxy for the whole app, a port per app it serves.
    @ObservationIgnored let proxy: PerAppProxy?

    /// The secret every proxied request must carry; installed into each web
    /// view before its first load.
    @ObservationIgnored let proxyAuth = ProxyAuth()

    /// The shares currently open for browsing, switchable in-app. Lazy so its
    /// icon callback can capture `self` (to feed `icons`).
    @ObservationIgnored private(set) lazy var browser = BrowseSessionStore(
        onIcon: { [weak self] id, png, rank in
            self?.icons.update(png, rank: rank, for: id)
        }
    )

    init(
        client: Client?,
        icons: ShareIconStore? = nil,
        activity: ShareActivityStore? = nil,
        shares: [Share] = []
    ) {
        self.client = client
        self.proxy = client?.startPerAppProxy(requiredCookie: proxyAuth.requiredCookie)
        self.icons = icons ?? ShareIconStore()
        self.activity = activity ?? ShareActivityStore()
        self.shares = shares
    }

    /// The real thing: a client on the app's data directory, secrets in the
    /// Keychain, and this manager told about every change. The observer is
    /// created first, since the client wants it at construction, and wired to
    /// the manager once there is one.
    static func live() -> ShareManager {
        // The SDK's lines into the unified log, once per process.
        try? installLogSink(sink: OSLogSink(), level: .info)
        let relay = ShareChangeRelay()
        let client: Client
        do {
            client = try Client(config: ClientConfig(
                dataDir: Self.dataDirectory().path,
                secrets: KeychainSecretStore(),
                observer: relay
            ))
        } catch {
            // Without a client there is nothing this app can do; surface it.
            fatalError("could not start the Wispers Access SDK: \(error)")
        }
        let manager = ShareManager(client: client)
        relay.onChange = { [weak manager] in manager?.reload() }
        manager.reload()
        return manager
    }

    func share(_ id: ShareId) -> Share? {
        shares.first { $0.id == id }
    }

    /// Joins a share from an invite code. The SDK does the work and rolls a
    /// failed join back; this reports the two steps the UI shows.
    @discardableResult
    func join(inviteCode: String, onStep: (JoinStep) -> Void = { _ in }) async throws -> Share {
        guard let client else { throw DemoMode.NotAvailable() }
        onStep(.validating)
        _ = try validateInvite(inviteCode: inviteCode.trimmingCharacters(in: .whitespacesAndNewlines))
        onStep(.joining)
        let share = try await client.join(inviteCode: inviteCode.trimmingCharacters(in: .whitespacesAndNewlines))
        reload()
        activity.markConnected(share.id)
        await status.refresh([share], using: client, activity: activity)
        return share
    }

    /// Removes a share: tears down its open browse sessions and drops it from
    /// the roster at once, then lets the SDK leave it, which tells the host
    /// node where it can and forgets the share and its secrets either way.
    func delete(_ id: ShareId) {
        browser.close(id)
        icons.remove(id)
        activity.remove(id)
        shares.removeAll { $0.id == id }
        guard let client else { return }
        Task {
            do {
                try await client.leave(share: id)
            } catch {
                Logger.app.error("leaving share \(id) failed: \(error)")
            }
            reload()
        }
    }

    /// Re-reads the roster from the SDK's store.
    func reload() {
        guard let client else { return }
        do {
            shares = try client.shares()
        } catch {
            Logger.app.error("reading shares failed: \(error)")
        }
    }
}

/// The steps a join walks through, for the add-share progress UI. Each `join`
/// reports the step it's *entering*; steps before it are done.
enum JoinStep: Int, CaseIterable {
    case validating
    case joining

    var label: String {
        switch self {
        case .validating: "Validating invitation code…"
        case .joining: "Joining through the host…"
        }
    }
}

/// The SDK's observer, called from its threads: hops to the main actor and
/// tells the manager to reload.
nonisolated final class ShareChangeRelay: Observer, @unchecked Sendable {
    var onChange: @MainActor () -> Void = {}

    func onShareChanged(share: Share) {
        Task { @MainActor in self.onChange() }
    }
}

extension Share: @retroactive Identifiable {}

extension ShareManager {
    /// The SDK's state lives under Application Support, apart from the files
    /// the pre-SDK app wrote there.
    static func dataDirectory() -> URL {
        FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
            .appendingPathComponent("sdk")
    }
}
