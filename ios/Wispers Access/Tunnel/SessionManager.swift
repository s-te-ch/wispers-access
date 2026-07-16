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

    /// Tears down a share's cached node + connection (e.g. when it's removed).
    func discard(_ shareID: ShareID) async {
        if let conn = connections.removeValue(forKey: shareID) {
            try? await conn.closeGracefully()
        }
        nodes.removeValue(forKey: shareID)
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
