import Foundation
import WispersConnect

/// A QUIC stream plus the connection it came from, so a mid-request failure can
/// evict exactly that connection.
struct StreamLease: Sendable {
    let stream: QuicStream
    let connection: QuicConnection
}

/// Per-share `Node` + `QuicConnection` cache — the iOS port of the Android
/// `SessionManager`. Connections are established lazily and reused across
/// requests; a stream open gets one retry so a silently-dead connection is
/// dropped and re-established. Callers MUST report mid-request failures via
/// `invalidate` (opening a stream on a dead connection can still succeed as
/// local bookkeeping, so the corpse otherwise stays cached and every later
/// request fails the same way).
actor SessionManager {
    private let storageProvider: @Sendable (ShareID) async throws -> NodeStorage
    private let onConnected: @Sendable (ShareID) async -> Void

    private var nodes: [ShareID: Node] = [:]
    private var connections: [ShareID: QuicConnection] = [:]

    init(
        storageProvider: @escaping @Sendable (ShareID) async throws -> NodeStorage,
        onConnected: @escaping @Sendable (ShareID) async -> Void = { _ in }
    ) {
        self.storageProvider = storageProvider
        self.onConnected = onConnected
    }

    /// Opens a fresh stream to the share's serving node, reusing the cached
    /// connection. One retry: if the first attempt fails, the connection is
    /// evicted and a second attempt reconnects.
    func openStream(_ shareID: ShareID) async throws -> StreamLease {
        do {
            return try await withDeadline(seconds: Self.openTimeout) { try await self.tryOpen(shareID) }
        } catch {
            try Task.checkCancellation()
            return try await withDeadline(seconds: Self.openTimeout) { try await self.tryOpen(shareID) }
        }
    }

    /// Hub-reported availability of the share's serving node (node 1), or nil
    /// when the hub can't be reached (typically this device is offline).
    func isServingNodeOnline(_ shareID: ShareID) async -> Bool? {
        do {
            let node = try await resolveNode(shareID)
            let info = try await node.groupInfo()
            return info.nodes.first { $0.nodeNumber == Self.servingNodeNumber }?.isOnline
        } catch {
            return nil
        }
    }

    /// Evicts and closes `lease`'s connection (unless already replaced).
    func invalidate(_ shareID: ShareID, _ lease: StreamLease) async {
        await evict(shareID, lease.connection)
    }

    /// Drops and closes every cached connection (e.g. on a network change),
    /// returning the affected shares.
    @discardableResult
    func evictAll() async -> [ShareID] {
        let stale = connections
        connections.removeAll()
        for conn in stale.values { try? await conn.closeGracefully() }
        return Array(stale.keys)
    }

    /// Logs the share's node out of the hub — deregistering this device from the
    /// roster — then drops its cached node + connection. Best-effort: an
    /// unreachable hub doesn't block teardown (the roster entry lingers until
    /// pruned). Restores a node from storage if one isn't cached, since `logout`
    /// needs a live handle.
    func logoutAndDiscard(_ shareID: ShareID) async {
        if let conn = connections.removeValue(forKey: shareID) {
            try? await conn.closeGracefully()
        }
        var node = nodes.removeValue(forKey: shareID)
        if node == nil,
            let storage = try? await storageProvider(shareID),
            let restored = try? await storage.restoreOrInit() {
            node = restored.0
        }
        if let node {
            try? await node.logout()
        }
    }

    private func tryOpen(_ shareID: ShareID) async throws -> StreamLease {
        let node = try await resolveNode(shareID)
        let conn = try await resolveConnection(shareID, node: node)
        do {
            return StreamLease(stream: try await conn.openStream(), connection: conn)
        } catch {
            await evict(shareID, conn)
            throw error
        }
    }

    private func resolveNode(_ shareID: ShareID) async throws -> Node {
        if let node = nodes[shareID] { return node }
        // storageProvider applies `overrideHubAddr` before returning, so the
        // node restores against the share's own (possibly self-hosted) hub.
        let storage = try await storageProvider(shareID)
        let (node, _) = try await storage.restoreOrInit()
        nodes[shareID] = node
        return node
    }

    private func resolveConnection(_ shareID: ShareID, node: Node) async throws -> QuicConnection {
        if let conn = connections[shareID] { return conn }
        let conn = try await node.connectQuic(peerNodeNumber: Self.servingNodeNumber)
        connections[shareID] = conn
        await onConnected(shareID)
        return conn
    }

    private func evict(_ shareID: ShareID, _ conn: QuicConnection) async {
        if connections[shareID] === conn { connections.removeValue(forKey: shareID) }
        try? await conn.closeGracefully()
    }

    private static let servingNodeNumber: Int32 = 1
    private static let openTimeout: Double = 10
}
