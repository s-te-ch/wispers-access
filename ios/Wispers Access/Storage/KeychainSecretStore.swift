import Foundation
import Security
import WispersAccessSdk

/// The SDK's secret store on the Keychain: one generic-password item per
/// share and key, the account namespaced by the share id so every joined
/// share keeps its own key material under one service.
///
/// The SDK calls these from its own threads, so the type is `nonisolated`
/// and holds only immutable state.
nonisolated final class KeychainSecretStore: SecretStore, @unchecked Sendable {
    private let service = "dev.wispers.access.ios"

    func load(share: ShareId, key: String) throws -> Data? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account(share, key),
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        switch status {
        case errSecSuccess: return result as? Data
        case errSecItemNotFound: return nil
        default: throw SecretStoreError.Failed(message: Self.describe(status))
        }
    }

    func save(share: ShareId, key: String, value: Data) throws {
        // Upsert: try to update an existing item, fall back to adding one.
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account(share, key),
        ]
        let attrs: [String: Any] = [
            kSecValueData as String: value,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlock,
        ]
        let status = SecItemUpdate(query as CFDictionary, attrs as CFDictionary)
        if status == errSecItemNotFound {
            var addQuery = query
            addQuery.merge(attrs) { _, new in new }
            let addStatus = SecItemAdd(addQuery as CFDictionary, nil)
            guard addStatus == errSecSuccess else {
                throw SecretStoreError.Failed(message: Self.describe(addStatus))
            }
        } else if status != errSecSuccess {
            throw SecretStoreError.Failed(message: Self.describe(status))
        }
    }

    func delete(share: ShareId, key: String) throws {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account(share, key),
        ]
        let status = SecItemDelete(query as CFDictionary)
        if status != errSecSuccess && status != errSecItemNotFound {
            throw SecretStoreError.Failed(message: Self.describe(status))
        }
    }

    private func account(_ share: ShareId, _ key: String) -> String { "\(share)/\(key)" }

    private static func describe(_ status: OSStatus) -> String {
        let message = SecCopyErrorMessageString(status, nil) as String? ?? "Keychain error"
        return "\(message) (\(status))"
    }
}
