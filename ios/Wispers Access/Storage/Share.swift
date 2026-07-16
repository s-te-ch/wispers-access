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
}
