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

    /// Site icons harvested while browsing, for the roster/detail avatars.
    let icons = ShareIconStore()

    /// The shares currently open for browsing, switchable in-app. Lazy so its
    /// icon callback can capture `self` (to feed `icons`).
    @ObservationIgnored private(set) lazy var browser = BrowseSessionStore(
        onIcon: { [weak self] id, png, rank in
            self?.icons.update(png, rank: rank, for: id)
        }
    )

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
    /// half-created share is rolled back so a retry is clean — including
    /// deregistering from the hub if registration got far enough to leave a node
    /// there (otherwise a failed activation would strand a `.registered` node
    /// server-side, and repeated retries would pile them up).
    @discardableResult
    func join(inviteCode: String, onStep: (JoinStep) -> Void = { _ in }) async throws -> ShareID {
        onStep(.validating)
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
            onStep(.initializing)
            let storage = try storageFor(id)
            let (node, _) = try await storage.restoreOrInit()
            onStep(.registering)
            try await node.register(token: invite.registrationToken)
            onStep(.activating)
            try await node.activate(activationCode: invite.activationCode)
            store.markConnected(id)
            // Adopt the connectivity group's display name as the label — best
            // effort: the share is already joined and usable, so a failed
            // groupInfo() (or a blank name) must not fail the join.
            if let info = try? await node.groupInfo(), let name = info.name, !name.isEmpty {
                store.setNickname(name, for: id)
            }
            // `storage` owns the callbacks holder and frees it on deinit, but the
            // node calls back into it during register/activate (saveRootKey,
            // saveRegistration). Keep it alive until they've all returned so
            // release-build ARC can't drop it after `restoreOrInit`.
            withExtendedLifetime(storage) {}
            // Activation has now persisted, so prime the share's status from a
            // fresh (activated) node — otherwise the roster keeps the "CHECKING"
            // that a status poll fired mid-join (before activation landed) left.
            await status.refresh([id], using: sessions)
            return id
        } catch {
            // Roll back the partial join. `logoutAndDiscard` deregisters from the
            // hub iff a registration actually persisted (so it undoes a completed
            // `register` even when `activate` then failed) and no-ops for failures
            // that never reached the hub; `wipe` then clears local secrets + the
            // roster entry.
            await sessions.logoutAndDiscard(id)
            wipe(id)
            throw error
        }
    }

    /// Removes a share: tears down any open browse session and drops it from the
    /// roster immediately (so the UI updates at once), then best-effort logs the
    /// node out of the hub — deregistering this device — and wipes its Keychain
    /// secrets. Local removal is instant; hub deregistration tolerates being
    /// offline.
    func delete(_ id: ShareID) {
        // Stop the warm browse session first: its loopback proxy holds the node
        // we're about to log out, and leaving it up would leak the proxy (and show
        // a broken page if its browser is on screen).
        browser.close(id)
        icons.remove(id)
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

/// The steps a join walks through, in order, for the add-share progress UI.
/// Each `join` reports the step it's *entering*; steps before it are done.
enum JoinStep: Int, CaseIterable {
    case validating
    case initializing
    case registering
    case activating

    var label: String {
        switch self {
        case .validating: "Validating invitation code…"
        case .initializing: "Generating node identity…"
        case .registering: "Registering…"
        case .activating: "Activating…"
        }
    }
}
