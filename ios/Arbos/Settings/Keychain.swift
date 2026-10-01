import Foundation
import Security

/// Minimal generic-password store. Secrets never touch UserDefaults.
enum Keychain {
    private static let service = "com.unarbos.arbos.ios"

    static func read(_ account: String) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
            // Must match the write, or a value saved without iCloud sync is
            // invisible to a query that would accept a synced one.
            kSecAttrSynchronizable as String: false,
        ]
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        guard status == errSecSuccess, let data = item as? Data else {
            if status != errSecItemNotFound {
                NSLog("Keychain read failed for %@: %d", account, Int(status))
            }
            return nil
        }
        return String(data: data, encoding: .utf8)
    }

    /// Stores `value`, or clears the account when it is blank. Returns whether
    /// the value can be read back afterwards: the caller shows the difference,
    /// because a key that silently failed to save looks exactly like one that
    /// was never typed.
    @discardableResult
    static func write(_ value: String, account: String) -> Bool {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            delete(account)
            return true
        }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrSynchronizable as String: false,
        ]
        // Replace, not update: an update with a changed accessibility class
        // fails, and a fresh add is the same cost.
        SecItemDelete(query as CFDictionary)
        var insert = query
        insert[kSecValueData as String] = Data(trimmed.utf8)
        // The call can be answered with the phone locked, and a background
        // reconnect has to be able to read the key. `WhenUnlocked` would make
        // that fail for reasons nobody could diagnose from the call screen.
        insert[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        let status = SecItemAdd(insert as CFDictionary, nil)
        if status != errSecSuccess {
            NSLog("Keychain add failed for %@: %d", account, Int(status))
            return false
        }
        return read(account) == trimmed
    }

    static func delete(_ account: String) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrSynchronizable as String: false,
        ]
        SecItemDelete(query as CFDictionary)
    }
}
