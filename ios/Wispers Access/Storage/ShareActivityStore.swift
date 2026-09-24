import Foundation
import Observation
import WispersAccessSdk

/// When each share was last reached from this device, for the roster's
/// "LAST 5M AGO". UI-only state the SDK does not keep, persisted as JSON.
@Observable
@MainActor
final class ShareActivityStore {
    private(set) var lastConnected: [ShareId: Date] = [:]

    private let fileURL: URL

    init(fileURL: URL? = nil) {
        self.fileURL = fileURL ?? Self.defaultFileURL()
        load()
    }

    func lastConnected(_ id: ShareId) -> Date? { lastConnected[id] }

    func markConnected(_ id: ShareId, at date: Date = Date()) {
        lastConnected[id] = date
        persist()
    }

    func remove(_ id: ShareId) {
        guard lastConnected.removeValue(forKey: id) != nil else { return }
        persist()
    }

    private func load() {
        guard let data = try? Data(contentsOf: fileURL) else { return }
        lastConnected = (try? Self.decoder.decode([ShareId: Date].self, from: data)) ?? [:]
    }

    private func persist() {
        do {
            let data = try Self.encoder.encode(lastConnected)
            try FileManager.default.createDirectory(
                at: fileURL.deletingLastPathComponent(), withIntermediateDirectories: true)
            try data.write(to: fileURL, options: .atomic)
        } catch {
            // Cosmetic state; losing it costs a label on restart, not the app.
            assertionFailure("Failed to persist share activity: \(error)")
        }
    }

    private static func defaultFileURL() -> URL {
        FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
            .appendingPathComponent("share-activity.json")
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
