import Foundation

/// Stable identifier for a joined share. Minted fresh per join; it keys both the
/// Keychain node-state items and the metadata record.
nonisolated struct ShareID: Hashable, Codable, Sendable {
    let value: String

    init(_ value: String) { self.value = value }

    static func random() -> ShareID { ShareID(UUID().uuidString) }

    // Encode as a bare string so the on-disk JSON reads `"id": "…"`.
    init(from decoder: Decoder) throws {
        value = try decoder.singleValueContainer().decode(String.self)
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(value)
    }
}

/// A share that can never come back: the hub rejected this node's credentials
/// (share deleted server-side) or reported it revoked from the roster.
/// Persisted so the state survives restarts and renders while offline.
nonisolated enum TerminalShareState: String, Codable, Sendable {
    /// The share was removed on the server side (group deleted at the hub).
    case removed
    /// This device's access was revoked from the share's roster.
    case revoked
}

/// Non-secret, user-visible metadata for a joined share. The node's secrets
/// (root key + registration blob) live in the Keychain via `KeychainShareStore`;
/// this is everything else, persisted as plain JSON by `ShareStore`.
nonisolated struct ShareMetadata: Codable, Identifiable, Equatable, Sendable {
    let id: ShareID
    var nickname: String
    /// Self-hosted Wispers Connect backend URL for this share, or nil for the
    /// managed backend. Applied via `overrideHubAddr` before the node restores.
    var backend: String?
    let createdAt: Date
    var lastConnectedAt: Date?
    /// Set once the hub definitively rejects this node; nil while live.
    var terminalState: TerminalShareState?
}
