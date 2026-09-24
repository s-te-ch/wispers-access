import Foundation
import Testing
@testable import WispersAccessSdk

/// The framework links and the bindings reach the Rust side: a client in a
/// scratch directory starts offline, answers, and rejects a bad invite with
/// the SDK's own error.
@Test func aClientStartsOfflineThroughTheBindings() throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("wispers-access-sdk-\(UUID().uuidString)")
    defer { try? FileManager.default.removeItem(at: dir) }

    let client = try Client(config: ClientConfig(dataDir: dir.path, secrets: nil, observer: nil))
    #expect(try client.shares().isEmpty)
    #expect(try client.share(key: "nope") == nil)
    #expect(throws: SdkError.self) { try validateInvite(inviteCode: "nope") }
}

/// A foreign secret store and observer are accepted where the SDK asks for
/// them, and the client still starts.
@Test func foreignCallbacksPlugIn() throws {
    final class MemorySecrets: SecretStore, @unchecked Sendable {
        var items: [String: Data] = [:]
        func load(share: ShareId, key: String) throws -> Data? { items["\(share)/\(key)"] }
        func save(share: ShareId, key: String, value: Data) throws { items["\(share)/\(key)"] = value }
        func delete(share: ShareId, key: String) throws { items["\(share)/\(key)"] = nil }
    }
    final class Changes: Observer, @unchecked Sendable {
        var seen: [Share] = []
        func onShareChanged(share: Share) { seen.append(share) }
    }
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("wispers-access-sdk-\(UUID().uuidString)")
    defer { try? FileManager.default.removeItem(at: dir) }

    let client = try Client(config: ClientConfig(
        dataDir: dir.path, secrets: MemorySecrets(), observer: Changes()))
    #expect(try client.shares().isEmpty)
}
