// Device token storage (crdt-sync §10.1): the device token lives in the iOS
// Keychain, keyed by relay URL. The relay URL itself is a plain default.

import Foundation
import Security

struct DeviceCredentials: Codable, Equatable {
    var relayUrl: String
    var deviceId: String
    var deviceToken: String
}

enum Keychain {
    private static let service = "sh.arbos.ios.relay"

    static func load(relayUrl: String) -> DeviceCredentials? {
        loadData(account: relayUrl).flatMap { try? JSONDecoder().decode(DeviceCredentials.self, from: $0) }
    }

    static func store(_ creds: DeviceCredentials) throws {
        try storeData(try JSONEncoder().encode(creds), account: creds.relayUrl)
    }

    static func delete(relayUrl: String) {
        deleteData(account: relayUrl)
    }

    static func loadData(account: String) -> Data? {
        var query = baseQuery(account: account)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        guard status == errSecSuccess, let data = item as? Data else { return nil }
        return data
    }

    static func storeData(_ data: Data, account: String) throws {
        let query = baseQuery(account: account)
        // `ThisDeviceOnly`: the relay binds these tokens to this device, so they are no use
        // anywhere else — and without it the item travels in an encrypted iTunes/Finder
        // backup and comes back live on whatever device that backup is restored to.
        // `AfterFirstUnlock` stays, so a wake behind the lock screen can still read it.
        let attributes: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        let status = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
        if status == errSecItemNotFound {
            var add = query
            for (k, v) in attributes { add[k] = v }
            let addStatus = SecItemAdd(add as CFDictionary, nil)
            guard addStatus == errSecSuccess else { throw KeychainError.status(addStatus) }
        } else if status != errSecSuccess {
            throw KeychainError.status(status)
        }
    }

    static func deleteData(account: String) {
        SecItemDelete(baseQuery(account: account) as CFDictionary)
    }

    private static func baseQuery(account: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }

    enum KeychainError: Error, LocalizedError {
        case status(OSStatus)

        var errorDescription: String? {
            switch self {
            case .status(let s):
                return "Keychain error \(s)"
            }
        }
    }
}
