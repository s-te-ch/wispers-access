import Foundation
import Observation

/// Per-share site icons harvested while browsing (web-app-manifest icon /
/// apple-touch-icon / favicon), kept as image bytes plus the rank-ladder rung
/// that produced them — so a better icon replaces a worse one but never the
/// reverse. Non-secret and purely cosmetic, so it's its own JSON file rather than
/// bloating the share metadata or the Keychain. Mirrors Android's
/// `iconPng`/`iconRank` columns.
@Observable
@MainActor
final class ShareIconStore {
    private(set) var records: [String: IconRecord] = [:]

    private let fileURL: URL

    init(fileURL: URL? = nil) {
        self.fileURL = fileURL ?? Self.defaultFileURL()
        load()
    }

    /// The cached icon bytes for a share, if any — feeds `ShareAvatar`.
    func iconData(for id: ShareID) -> Data? { records[id.value]?.png }

    /// The rank of the cached icon (0 if none) — the bar a new one must beat.
    func rank(for id: ShareID) -> Int { records[id.value]?.rank ?? 0 }

    /// Stores a harvested icon, but only if it out-ranks what's cached (a higher
    /// rung of the ladder). The caller has already validated the bytes decode.
    func update(_ png: Data, rank: Int, for id: ShareID) {
        guard rank > self.rank(for: id) else { return }
        records[id.value] = IconRecord(rank: rank, png: png)
        persist()
    }

    func remove(_ id: ShareID) {
        guard records.removeValue(forKey: id.value) != nil else { return }
        persist()
    }

    // MARK: - Private

    private func load() {
        guard let data = try? Data(contentsOf: fileURL) else { return }
        records = (try? JSONDecoder().decode([String: IconRecord].self, from: data)) ?? [:]
    }

    private func persist() {
        do {
            let data = try JSONEncoder().encode(records)
            try FileManager.default.createDirectory(
                at: fileURL.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            try data.write(to: fileURL, options: .atomic)
        } catch {
            // Icons are a cosmetic cache; a failed write just means re-harvesting
            // on the next visit, so don't take the app down.
            assertionFailure("Failed to persist share icons: \(error)")
        }
    }

    private static func defaultFileURL() -> URL {
        let base = FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        return base.appendingPathComponent("share-icons.json")
    }
}

/// A cached site icon: the image bytes and the rank-ladder rung that produced it
/// (manifest-maskable 4 > manifest 3 > apple-touch-icon 2 > favicon 1).
struct IconRecord: Codable {
    var rank: Int
    var png: Data
}
