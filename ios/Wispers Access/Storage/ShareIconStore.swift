import Foundation
import Observation
import WispersAccessSdk

/// Per-app site icons harvested while browsing (web-app-manifest icon /
/// apple-touch-icon / favicon), kept as image bytes plus the rank-ladder rung
/// that produced them — so a better icon replaces a worse one but never the
/// reverse. Keyed by share and app. Non-secret and purely cosmetic, so it's
/// its own JSON file rather than bloating the SDK's store or the Keychain.
@Observable
@MainActor
final class ShareIconStore {
    private(set) var records: [String: IconRecord] = [:]

    private let fileURL: URL

    init(fileURL: URL? = nil) {
        self.fileURL = fileURL ?? Self.defaultFileURL()
        load()
    }

    /// The cached icon bytes for an app, if any — feeds `ShareAvatar`.
    func iconData(for key: SharedAppId) -> Data? { records[Self.record(key)]?.png }

    /// An icon to stand for the share as a whole: the first of its apps that
    /// has one.
    func iconData(forAnyAppOf share: Share) -> Data? {
        share.apps.lazy
            .compactMap { self.iconData(for: SharedAppId(shareID: share.id, appID: $0.id)) }
            .first
    }

    /// The rank of the cached icon (0 if none) — the bar a new one must beat.
    func rank(for key: SharedAppId) -> Int { records[Self.record(key)]?.rank ?? 0 }

    /// Stores a harvested icon unless the cached one out-ranks it (a higher
    /// rung of the ladder), or is the same rank and the same bytes. Same rank
    /// with different bytes replaces, so a site that changes its icon is
    /// picked up. The caller has already validated the bytes decode.
    func update(_ png: Data, rank: Int, for key: SharedAppId) {
        if let cached = records[Self.record(key)] {
            guard rank > cached.rank || (rank == cached.rank && cached.png != png) else { return }
        }
        records[Self.record(key)] = IconRecord(rank: rank, png: png)
        persist()
    }

    /// Drops every app icon of a share.
    func remove(_ shareID: ShareId) {
        let prefix = "\(shareID)/"
        let before = records.count
        records = records.filter { !$0.key.hasPrefix(prefix) }
        if records.count != before { persist() }
    }

    private static func record(_ key: SharedAppId) -> String { "\(key.shareID)/\(key.appID)" }

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
