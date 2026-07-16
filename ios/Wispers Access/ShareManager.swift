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

    init(store: ShareStore? = nil) {
        // Constructed here rather than in a default argument: `ShareStore.init`
        // is main-actor-isolated, and default arguments evaluate nonisolated.
        self.store = store ?? ShareStore()
    }

    /// Joins a share from a `wax_` invite code: parse → persist metadata (incl.
    /// any self-hosted backend) → restore/init the node → register → activate.
    /// On any failure the half-created share is rolled back so a retry is clean.
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
            return id
        } catch {
            wipe(id)
            throw error
        }
    }

    /// Tears a share down locally: wipes its Keychain secrets and metadata. (Hub
    /// logout — `node.logout()` — is deferred to the teardown UI in a later phase.)
    func delete(_ id: ShareID) {
        wipe(id)
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
