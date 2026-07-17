import Foundation
import Observation
import WispersConnect

/// App coordinator for joined shares: bridges the UI to the wispers-connect node
/// lifecycle and the two stores (Keychain secrets + JSON metadata). Phase 1 owns
/// the join flow and the roster; serving/tunnelling arrives in a later phase.
/// Injected via `@Environment`; its observable state is `store`.
@Observable
@MainActor
final class ShareManager {
    let store: ShareStore

    /// Per-share serving-node availability, polled while a screen is visible.
    let status = ShareStatusStore()

    /// The shares currently open for browsing, switchable in-app.
    let browser = BrowseSessionStore()

    /// App-wide cache of live nodes + QUIC connections for browsing. Lazy so its
    /// closures can capture `self` weakly; `@ObservationIgnored` since it isn't
    /// view-observable state.
    @ObservationIgnored private(set) lazy var sessions = SessionManager(
        storageProvider: { [weak self] id in
            guard let self else { throw CancellationError() }
            return try await self.storageFor(id)
        },
        onConnected: { [weak self] id in await self?.store.markConnected(id) }
    )

    init(store: ShareStore? = nil) {
        // Constructed here rather than in a default argument: `ShareStore.init`
        // is main-actor-isolated, and default arguments evaluate nonisolated.
        self.store = store ?? ShareStore()
    }

    /// Joins a share from a `wax_` invite code: parse → persist metadata (incl.
    /// any self-hosted backend) → restore/init the node → register → activate →
    /// adopt the connectivity group's name as the label. On any failure the
    /// half-created share is rolled back so a retry is clean.
    @discardableResult
    func join(inviteCode: String) async throws -> ShareID {
        let invite = try InviteCode.parse(inviteCode)
        let id = ShareID.random()
        store.add(
            ShareMetadata(
                id: id,
                nickname: "",
                backend: invite.backend,
                createdAt: Date(),
                lastConnectedAt: nil
            )
        )
        do {
            let storage = try storageFor(id)
            let (node, _) = try await storage.restoreOrInit()
            try await node.register(token: invite.registrationToken)
            try await node.activate(activationCode: invite.activationCode)
            store.markConnected(id)
            // Adopt the connectivity group's display name as the label — best
            // effort: the share is already joined and usable, so a failed
            // groupInfo() (or a blank name) must not fail the join.
            if let info = try? await node.groupInfo(), let name = info.name, !name.isEmpty {
                store.setNickname(name, for: id)
            }
            return id
        } catch {
            wipe(id)
            throw error
        }
    }

    /// Removes a share: drops it from the roster immediately (so the UI updates
    /// at once), then best-effort logs the node out of the hub — deregistering
    /// this device — and wipes its Keychain secrets. Local removal is instant;
    /// hub deregistration tolerates being offline.
    func delete(_ id: ShareID) {
        store.remove(id)
        Task {
            await sessions.logoutAndDiscard(id)
            try? KeychainShareStore(shareID: id.value).deleteAll()
        }
    }

    /// A `NodeStorage` for the share, already pointed at the share's self-hosted
    /// hub if it has one. `overrideHubAddr` is applied *before* the caller runs
    /// `restoreOrInit()`: restoring a registered node contacts the hub
    /// immediately, so the override has to be in place first.
    func storageFor(_ id: ShareID) throws -> NodeStorage {
        let storage = NodeStorage.withCallbacks(KeychainShareStore(shareID: id.value))
        if let backend = store.backend(for: id) {
            try storage.overrideHubAddr(backend)
        }
        return storage
    }

    private func wipe(_ id: ShareID) {
        try? KeychainShareStore(shareID: id.value).deleteAll()
        store.remove(id)
    }
}
