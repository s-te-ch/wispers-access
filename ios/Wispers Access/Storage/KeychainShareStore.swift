import Foundation
import Security
import WispersConnect

/// Per-share Keychain storage for a wispers-connect node's secrets. Adapts the
/// reference app's single-identity `KeychainStorage` to Access's multi-share
/// world by namespacing each item's account with the share id, so every joined
/// share gets an isolated root key and registration blob under one service.
///
/// The wispers-connect FFI invokes these callbacks synchronously from its own
/// threads, so the type is `nonisolated` and holds only immutable state.
nonisolated final class KeychainShareStore: NodeStorageCallbacksProtocol {
    private let service = "dev.wispers.access.ios"
    private let shareID: String

    init(shareID: String) { self.shareID = shareID }

    // MARK: - NodeStorageCallbacksProtocol

    func loadRootKey() throws -> Data? { try read(item: "root-key") }
    func saveRootKey(_ key: Data) throws { try write(item: "root-key", data: key) }
    func deleteRootKey() throws { try delete(item: "root-key") }

    func loadRegistration() throws -> Data? { try read(item: "registration") }
    func saveRegistration(_ data: Data) throws { try write(item: "registration", data: data) }
    func deleteRegistration() throws { try delete(item: "registration") }

    /// Removes every Keychain item this share owns — used when a join fails
    /// partway or a share is torn down.
    func deleteAll() throws {
        try delete(item: "root-key")
        try delete(item: "registration")
    }

    // MARK: - Keychain helpers

    private func account(_ item: String) -> String { "\(shareID)/\(item)" }

    private func read(item: String) throws -> Data? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account(item),
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        switch status {
        case errSecSuccess:
            return result as? Data
        case errSecItemNotFound:
            return nil
        default:
            throw KeychainError.unhandled(status)
        }
    }

    private func write(item: String, data: Data) throws {
        // Upsert: try to update an existing item, fall back to adding one.
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account(item),
        ]
        let attrs: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlock,
        ]
        let status = SecItemUpdate(query as CFDictionary, attrs as CFDictionary)
        if status == errSecItemNotFound {
            var addQuery = query
            addQuery.merge(attrs) { _, new in new }
            let addStatus = SecItemAdd(addQuery as CFDictionary, nil)
            guard addStatus == errSecSuccess else { throw KeychainError.unhandled(addStatus) }
        } else if status != errSecSuccess {
            throw KeychainError.unhandled(status)
        }
    }

    private func delete(item: String) throws {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account(item),
        ]
        let status = SecItemDelete(query as CFDictionary)
        if status != errSecSuccess && status != errSecItemNotFound {
            throw KeychainError.unhandled(status)
        }
    }
}

enum KeychainError: LocalizedError {
    case unhandled(OSStatus)

    var errorDescription: String? {
        switch self {
        case .unhandled(let status):
            let message = SecCopyErrorMessageString(status, nil) as String? ?? "Keychain error"
            return "\(message) (\(status))"
        }
    }
}
