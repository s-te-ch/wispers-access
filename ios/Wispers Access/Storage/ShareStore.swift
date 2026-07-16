import Foundation
import Observation

/// Persists the non-secret metadata for every joined share as JSON in the app's
/// Application Support directory, and publishes it for the UI. Secrets live in
/// the Keychain (`KeychainShareStore`); this is the roster of shares plus their
/// display fields and self-hosted backend.
@Observable
@MainActor
final class ShareStore {
    private(set) var shares: [ShareMetadata] = []

    private let fileURL: URL

    init(fileURL: URL? = nil) {
        self.fileURL = fileURL ?? Self.defaultFileURL()
        load()
    }

    func metadata(for id: ShareID) -> ShareMetadata? {
        shares.first { $0.id == id }
    }

    /// The self-hosted backend for a share, or nil for the managed backend.
    func backend(for id: ShareID) -> String? {
        metadata(for: id)?.backend
    }

    func add(_ share: ShareMetadata) {
        shares.removeAll { $0.id == share.id }
        shares.append(share)
        persist()
    }

    func setNickname(_ nickname: String, for id: ShareID) {
        mutate(id) { $0.nickname = nickname }
    }

    func markConnected(_ id: ShareID, at date: Date = Date()) {
        mutate(id) { $0.lastConnectedAt = date }
    }

    func remove(_ id: ShareID) {
        shares.removeAll { $0.id == id }
        persist()
    }

    // MARK: - Private

    private func mutate(_ id: ShareID, _ transform: (inout ShareMetadata) -> Void) {
        guard let index = shares.firstIndex(where: { $0.id == id }) else { return }
        transform(&shares[index])
        persist()
    }

    private func load() {
        guard let data = try? Data(contentsOf: fileURL) else { return }
        shares = (try? Self.decoder.decode([ShareMetadata].self, from: data)) ?? []
    }

    private func persist() {
        do {
            let data = try Self.encoder.encode(shares)
            try FileManager.default.createDirectory(
                at: fileURL.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            try data.write(to: fileURL, options: .atomic)
        } catch {
            // Metadata is a cache of what the Keychain already anchors; a failed
            // write shouldn't take the app down, only lose a nickname on restart.
            assertionFailure("Failed to persist share metadata: \(error)")
        }
    }

    private static func defaultFileURL() -> URL {
        let base = FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        return base.appendingPathComponent("shares.json")
    }

    private static let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }()

    private static let encoder: JSONEncoder = {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        return encoder
    }()
}
