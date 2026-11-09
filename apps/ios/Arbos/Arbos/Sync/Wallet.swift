// The wallet every user has (crates/arbos-wallet): one 12-word phrase, kept in the iCloud
// Keychain, is their TAO account and the key to their encrypted chat history. It is made on
// the account's first encrypting device and shown once; every later device of theirs gets it
// from iCloud Keychain (or the person types it) and opens the history with it. The relay never
// sees it.

import Foundation
import Security
import ArbosFFI

/// The one wallet step that can be on screen.
enum WalletPrompt: Identifiable, Equatable {
    /// A phrase was just made: show it once.
    case backup(String)
    /// The account's history is locked on this device: ask for the phrase.
    case restore

    var id: String {
        switch self {
        case .backup: return "backup"
        case .restore: return "restore"
        }
    }
}

/// Keychain items for the wallet. The phrase syncs through iCloud Keychain; the Matrix store
/// key never leaves this device.
enum WalletVault {
    private static let service = "sh.arbos.ios.wallet"

    private static func query(user: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: "phrase:\(user)",
            kSecAttrSynchronizable as String: true,
        ]
    }

    static func phrase(user: String) -> String? {
        var q = query(user: user)
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var item: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &item) == errSecSuccess, let data = item as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    /// Add only: an existing phrase is never overwritten (it may be holding money).
    static func save(_ phrase: String, user: String) throws {
        var add = query(user: user)
        add[kSecValueData as String] = Data(phrase.utf8)
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        let status = SecItemAdd(add as CFDictionary, nil)
        if status == errSecDuplicateItem, Self.phrase(user: user) == phrase { return }
        guard status == errSecSuccess else { throw Keychain.KeychainError.status(status) }
    }

    /// The random key that encrypts this device's Matrix store, made once.
    static func storeKey(device: String) -> String? {
        let account = "matrix-store:\(device)"
        if let data = Keychain.loadData(account: account) { return String(data: data, encoding: .utf8) }
        var bytes = [UInt8](repeating: 0, count: 32)
        guard SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess else { return nil }
        let key = Data(bytes).base64EncodedString()
        guard (try? Keychain.storeData(Data(key.utf8), account: account)) != nil else { return nil }
        return key
    }

    /// This device's Matrix store directory (Application Support, excluded from backups: the
    /// keys in it are only good on this device).
    static func storeDir(device: String) -> String {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        var dir = base.appendingPathComponent("matrix", isDirectory: true).appendingPathComponent(device, isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? dir.setResourceValues(values)
        return dir.path
    }
}

extension RelayAPI {
    /// `PUT /me/wallet`: people can pay this account by handle.
    func linkWallet(address: String, proof: String) async throws {
        _ = try await request("PUT", "/me/wallet", body: ["address": address, "proof": proof])
    }

    func unlinkWallet() async throws {
        _ = try await request("DELETE", "/me/wallet")
    }

    /// Someone's linked address, or nil when they have not turned payments on.
    func wallet(of userId: String) async throws -> String? {
        do {
            return (try await json("GET", "/users/\(userId)/wallet") as? [String: Any])?["address"] as? String
        } catch let e as RelayError where e.status == 404 {
            return nil
        }
    }
}

extension AppModel {
    var walletPhrase: String? { user.flatMap { WalletVault.phrase(user: $0.id) } }

    var walletAddressValue: String? { walletPhrase.flatMap { try? walletAddress(phrase: $0) } }

    /// Right after the Matrix client starts. The phrase on this device opens (or, on the
    /// account's first device, sets up) the key backup. With no phrase: the first device makes
    /// one and shows it once; a later device asks for it — it never makes a second one.
    func ensureKeys(_ client: ArbosFFI.MatrixClient) async {
        guard let me = user else { return }
        let backup = await client.backup()
        if let phrase = WalletVault.phrase(user: me.id) {
            do {
                try await client.recover(passphrase: walletRecoveryPassphrase(phrase: phrase))
            } catch {
                walletPrompt = .restore
            }
        } else if backup == "off" {
            let phrase = walletNewPhrase()
            guard (try? WalletVault.save(phrase, user: me.id)) != nil else { return }
            try? await client.recover(passphrase: walletRecoveryPassphrase(phrase: phrase))
            walletPrompt = .backup(phrase)
        } else if backup == "locked" {
            walletPrompt = .restore
        }
    }

    /// The phrase the person typed: it must open this account's backup before it is kept.
    /// Returns what went wrong, in words.
    func restoreWallet(_ typed: String) async -> String? {
        guard let me = user else { return "Sign in first." }
        guard let phrase = try? walletCheckPhrase(phrase: typed) else {
            return "That isn't a recovery phrase. Check each word."
        }
        if let client = matrixClient {
            do {
                try await client.recover(passphrase: walletRecoveryPassphrase(phrase: phrase))
            } catch {
                return "That phrase doesn't belong to this account."
            }
        }
        do { try WalletVault.save(phrase, user: me.id) } catch { return error.localizedDescription }
        walletPrompt = nil
        return nil
    }

    // MARK: Cheques

    private static let snapshotFile = FileManager.default
        .urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("tao-snapshot.json")

    /// The saved chain snapshot (signs and reads cheques offline), if any.
    var chequeSnapshot: String? {
        (try? Data(contentsOf: Self.snapshotFile)).flatMap { String(data: $0, encoding: .utf8) }
    }

    /// Take a fresh snapshot while online; keeps the next free nonce.
    @discardableResult
    func refreshChequeSnapshot() async -> Bool {
        guard let address = walletAddressValue,
              let snap = try? await walletSnapshot(address: address) else { return false }
        try? Data(snap.json.utf8).write(to: Self.snapshotFile, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
        let key = "walletNextNonce"
        UserDefaults.standard.set(max(UInt64(UserDefaults.standard.integer(forKey: key)), snap.nonce), forKey: key)
        return true
    }

    /// Sign a cheque to the other person in a DM and send it there. Works offline once a
    /// snapshot exists; the address comes from the relay when online. Returns an error to show.
    func sendCheque(chatId: Id, amount: String) async -> String? {
        guard let phrase = walletPhrase else { return "Set up your wallet first." }
        guard let chat = peopleChat(chatId), let me = user,
              let peer = chat.humans.first(where: { $0.id != me.id }) else {
            return "Cheques go to one person, in a direct chat."
        }
        let to: String
        do {
            guard let address = try await relay.wallet(of: peer.id) else {
                return "\(peer.name ?? "@\(peer.handle)") hasn't turned on payments yet."
            }
            to = address
        } catch {
            return "Couldn't look up their wallet. Try again when you're online."
        }
        if chequeSnapshot == nil { await refreshChequeSnapshot() }
        guard let snapshot = chequeSnapshot else { return "Go online once so this phone can sign cheques." }
        let key = "walletNextNonce"
        let nonce = UInt64(UserDefaults.standard.integer(forKey: key))
        let cheque: WalletCheque
        do {
            cheque = try walletSignCheque(phrase: phrase, snapshotJson: snapshot, to: to, amount: amount, nonce: nonce)
        } catch {
            return error.localizedDescription
        }
        UserDefaults.standard.set(nonce + 1, forKey: key)
        let id = newId()
        sendPeople(chatId: chatId, text: "Sent you a \(walletFormatTao(rao: cheque.rao)) cheque", replyTo: nil,
                   extraParts: [["id": "\(id)-cheque", "kind": "cheque", "cheque": cheque.bytes]])
        return nil
    }

    /// Cash a cheque; nil on success, else why it bounced.
    func cashCheque(_ bytes: String) async -> String? {
        do {
            _ = try await walletCashCheque(bytes: bytes)
            var cashed = Set(UserDefaults.standard.stringArray(forKey: "cashedCheques") ?? [])
            cashed.insert(bytes)
            UserDefaults.standard.set(Array(cashed), forKey: "cashedCheques")
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    func isCashed(_ bytes: String) -> Bool {
        (UserDefaults.standard.stringArray(forKey: "cashedCheques") ?? []).contains(bytes)
    }
}
