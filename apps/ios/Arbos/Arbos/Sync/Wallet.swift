// The wallet every user has (crates/arbos-wallet): one 12-word phrase is their TAO account and
// the key to their encrypted chat history. It is made on the account's first encrypting device
// and shown once, where the person chooses whether it also goes into their iCloud Keychain;
// every later device of theirs gets it from there, or the person types it, and opens the
// history with it. The relay never sees it.

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

enum Wallet {
    /// TAO cheques are off: nothing in the app signs or cashes one. What kept them off is fixed
    /// in code — the payee address is shown, pinned per person and signed exactly as shown;
    /// cheques expire (`arbos_wallet::chain::CHEQUE_PERIOD`, ~7 h) and their signature is
    /// checked when read; numbers are kept per address and only used by a sent cheque — but
    /// none of it has run on a phone. Turn this on only after an end-to-end test on a device
    /// against a local subtensor: send, cash, cash twice, let one expire, change the payee's
    /// linked address, kill the app mid-cash and reopen it.
    static let chequesEnabled = false

    /// A cheque this device signed and sent: it holds its number until the chain has moved
    /// past it or it has expired.
    struct Issued: Codable, Equatable {
        let nonce: UInt64
        /// Unix seconds, estimated from block time.
        let expiresAt: UInt64
    }

    /// Expiry is an estimate (blocks can run late): a number is only reused this long after.
    static let expiryMargin: UInt64 = 3600

    /// The number for the next cheque: the lowest one from the chain's next nonce (`base`,
    /// as of the last snapshot) that no cheque still cashable holds. The lowest, not one past
    /// the highest, because the chain cashes an account's cheques in order: an expired cheque
    /// leaves a gap that would block every later one until something fills it.
    ///
    /// Limitation: this is per device. Two devices with the same phrase, both offline, can
    /// hand out the same number; whichever cheque is cashed second then bounces as already
    /// used. That is safe (nothing is paid twice) but one payee has to ask again.
    static func nextNonce(base: UInt64, issued: [Issued], now: UInt64) -> UInt64 {
        let held = Set(issued.filter { $0.nonce >= base && $0.expiresAt + expiryMargin > now }.map(\.nonce))
        var nonce = base
        while held.contains(nonce) { nonce += 1 }
        return nonce
    }
}

/// Who a cheque in a chat would pay: the address this device first saw for them.
struct ChequePayee: Equatable {
    let userId: String
    let name: String
    let address: String
    /// The relay now gives this different address for them. Nothing is signed to it until the
    /// person confirms the change.
    var changedTo: String?
}

/// What became of a cheque handed to the chain.
enum ChequeCashing: Equatable {
    case paid
    /// Submitted, not finalized yet: not a bounce.
    case pending
    case refused(String)
}

/// Keychain items for the wallet. The phrase is kept on this device only, or in the iCloud
/// Keychain when the person chose that; the Matrix store key never leaves this device.
enum WalletVault {
    private static let service = "sh.arbos.ios.wallet"

    /// The keychain calls the phrase goes through. Replaced in the unit tests, whose unsigned
    /// host has no keychain (every call there is errSecMissingEntitlement).
    static var add: ([String: Any]) -> OSStatus = { SecItemAdd($0 as CFDictionary, nil) }
    static var delete: ([String: Any]) -> OSStatus = { SecItemDelete($0 as CFDictionary) }
    static var copy: ([String: Any]) -> Data? = { query in
        var item: CFTypeRef?
        return SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess ? item as? Data : nil
    }

    /// `synced`: the iCloud Keychain item, the device-only one, or (nil) whichever is there.
    private static func query(user: String, synced: Bool?) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: "phrase:\(user)",
            kSecAttrSynchronizable as String: synced.map { $0 as Any } ?? kSecAttrSynchronizableAny,
        ]
    }

    private static func phrase(user: String, synced: Bool?) -> String? {
        var q = query(user: user, synced: synced)
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        return copy(q).flatMap { String(data: $0, encoding: .utf8) }
    }

    static func phrase(user: String) -> String? { phrase(user: user, synced: nil) }

    /// Whether the phrase is in the iCloud Keychain (an account from before the choice
    /// existed has it there).
    static func isSynced(user: String) -> Bool { phrase(user: user, synced: true) != nil }

    /// Add only: an existing phrase, synced or not, is never overwritten (it may be holding
    /// money). Device-only unless the person chose iCloud Keychain.
    static func save(_ phrase: String, user: String, synced: Bool = false) throws {
        if let kept = Self.phrase(user: user), kept != phrase { throw Keychain.KeychainError.status(errSecDuplicateItem) }
        var add = query(user: user, synced: synced)
        add[kSecValueData as String] = Data(phrase.utf8)
        // A device-only phrase does not ride an encrypted backup onto another phone either.
        add[kSecAttrAccessible as String] = synced ? kSecAttrAccessibleAfterFirstUnlock : kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let status = Self.add(add)
        if status == errSecDuplicateItem, Self.phrase(user: user, synced: synced) == phrase { return }
        guard status == errSecSuccess else { throw Keychain.KeychainError.status(status) }
    }

    /// Move the phrase into the iCloud Keychain, or out of it onto this device only. The copy
    /// is written and read back before the other is deleted: a failure leaves the phrase
    /// where it was. Taking it out of iCloud takes it off the person's other devices too.
    static func move(user: String, synced: Bool) throws {
        guard let phrase = Self.phrase(user: user) else { throw Keychain.KeychainError.status(errSecItemNotFound) }
        try save(phrase, user: user, synced: synced)
        guard Self.phrase(user: user, synced: synced) == phrase else { throw Keychain.KeychainError.status(errSecItemNotFound) }
        let status = delete(query(user: user, synced: !synced))
        guard status == errSecSuccess || status == errSecItemNotFound else { throw Keychain.KeychainError.status(status) }
    }

    /// Forget a phrase this device made a moment ago and never showed, when another device's
    /// backup turned out to be there first. Only that exact phrase: anything else stored here
    /// may be holding money and is left alone.
    static func discard(unshown phrase: String, user: String) {
        guard Self.phrase(user: user) == phrase else { return }
        _ = delete(query(user: user, synced: nil))
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

    /// This device's Matrix store directory (excluded from backups: the keys in it are only
    /// good on this device). In the App Group's container, where the notification extension
    /// can read a message's key (`PreviewVault`); a store an earlier build kept in Application
    /// Support is moved there once, whole. Without the group, or if the move fails, the store
    /// stays where it was.
    static func storeDir(device: String) -> String {
        let fm = FileManager.default
        let old = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("matrix", isDirectory: true).appendingPathComponent(device, isDirectory: true)
        var dir = old
        if let root = PreviewVault.storeRoot {
            let shared = root.appendingPathComponent(device, isDirectory: true)
            try? fm.createDirectory(at: root, withIntermediateDirectories: true)
            let here = fm.fileExists(atPath: shared.path)
            if here || !fm.fileExists(atPath: old.path) || (try? fm.moveItem(at: old, to: shared)) != nil { dir = shared }
        }
        try? fm.createDirectory(at: dir, withIntermediateDirectories: true)
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? dir.setResourceValues(values)
        return dir.path
    }
}

extension WalletError {
    /// The words for the person. UniFFI's own description is a debug dump
    /// (`ArbosFFI.WalletError.Invalid("…")`).
    var words: String {
        switch self {
        case .Invalid(let words), .Chain(let words): return words
        }
    }
}

/// What to show for a wallet failure.
func walletWords(_ error: Error) -> String {
    (error as? WalletError)?.words ?? error.localizedDescription
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
    /// one, keeps it on this device and shows it once, where the person may put it in iCloud
    /// Keychain; a later device asks for it — it never makes a second one.
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
            do {
                try await client.recover(passphrase: walletRecoveryPassphrase(phrase: phrase))
                walletPrompt = .backup(phrase)
            } catch {
                // Another device set the backup up first: its phrase is the account's, not
                // this one. Showing this one would have the person write down the wrong words.
                WalletVault.discard(unshown: phrase, user: me.id)
                walletPrompt = .restore
            }
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
        // The phrase must open this account's backup before it is kept. A locked backup is
        // opened with it; one already open on this device is checked without changing anything
        // (`recover` would accept any phrase there). With no backup, or a state not yet known,
        // nothing can vouch for the phrase and it is not kept.
        guard let client = matrixClient else {
            return "This device can't check that phrase right now. Try again once your chats have loaded."
        }
        do {
            let passphrase = try walletRecoveryPassphrase(phrase: phrase)
            switch await client.backup() {
            case "locked": try await client.recover(passphrase: passphrase)
            case "on": try await client.checkPassphrase(passphrase: passphrase)
            default:
                return "This device can't check that phrase right now. Try again once your chats have loaded."
            }
        } catch {
            return "That phrase doesn't belong to this account."
        }
        do { try WalletVault.save(phrase, user: me.id) } catch { return "Couldn't save your recovery phrase. Something went wrong." }
        walletPrompt = nil
        // The keys are here now: read the people chats whose rows were locked again.
        for chat in nativeRooms.keys { openChannel(chat) }
        return nil
    }

    /// Put the phrase in the iCloud Keychain, or take it out and keep it on this device only.
    /// Returns what went wrong, in words.
    func setWalletSynced(_ synced: Bool) -> String? {
        guard let me = user, (try? WalletVault.move(user: me.id, synced: synced)) != nil else {
            return synced ? "Couldn't save your recovery phrase to iCloud Keychain. It is still on this iPhone."
                          : "Couldn't take your recovery phrase out of iCloud Keychain. It is still there."
        }
        return nil
    }

    // MARK: Cheques

    // "-2": the first snapshot format had no anchor block; a file in it is never read.
    private static let snapshotFile = FileManager.default
        .urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("tao-snapshot-2.json")

    /// The saved chain snapshot (signs and reads cheques offline), if any.
    var chequeSnapshot: String? {
        (try? Data(contentsOf: Self.snapshotFile)).flatMap { String(data: $0, encoding: .utf8) }
    }

    private func issuedCheques(_ address: String) -> [Wallet.Issued] {
        UserDefaults.standard.data(forKey: "walletIssued:\(address)")
            .flatMap { try? JSONDecoder().decode([Wallet.Issued].self, from: $0) } ?? []
    }

    private func setIssuedCheques(_ issued: [Wallet.Issued], _ address: String) {
        UserDefaults.standard.set(try? JSONEncoder().encode(issued), forKey: "walletIssued:\(address)")
    }

    /// Take a fresh snapshot while online: a new block to anchor cheques to, and the chain's
    /// next nonce for this address (which also retires every sent cheque below it).
    @discardableResult
    func refreshChequeSnapshot() async -> Bool {
        guard let address = walletAddressValue,
              let snap = try? await walletSnapshot(address: address) else { return false }
        // Application Support does not exist on a fresh install until something makes it.
        do {
            try FileManager.default.createDirectory(at: Self.snapshotFile.deletingLastPathComponent(),
                                                    withIntermediateDirectories: true)
            try Data(snap.json.utf8).write(to: Self.snapshotFile,
                                           options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
        } catch {
            return false
        }
        UserDefaults.standard.set(String(snap.nonce), forKey: "walletChainNonce:\(address)")
        setIssuedCheques(issuedCheques(address).filter { $0.nonce >= snap.nonce }, address)
        return true
    }

    private func payeeKey(_ me: String, _ peer: String) -> String { "walletPayee:\(me):\(peer)" }

    /// Who a cheque in this DM would pay. The relay only says where their address is; this
    /// device remembers the first one it saw for each person, and a different one later is
    /// reported (`changedTo`), never silently used. Offline, the remembered address still works.
    func chequePayee(chatId: Id) async throws -> ChequePayee {
        guard draftDM?.id != chatId, let chat = peopleChat(chatId), let me = user,
              let peer = chat.humans.first(where: { $0.id != me.id }) else {
            throw WalletError.Invalid("Cheques go to one person, in a direct chat you've already started.")
        }
        let name = peer.name ?? "@\(peer.handle)"
        let key = payeeKey(me.id, peer.id)
        let pinned = UserDefaults.standard.string(forKey: key)
        let found: String?
        do {
            found = try await relay.wallet(of: peer.id)
        } catch {
            guard let pinned else {
                throw WalletError.Invalid("Couldn't look up their wallet. Try again when you're online.")
            }
            return ChequePayee(userId: peer.id, name: name, address: pinned)
        }
        guard let found, let address = try? walletCheckAddress(address: found) else {
            throw WalletError.Invalid("\(name) hasn't turned on payments yet.")
        }
        guard let pinned else {
            UserDefaults.standard.set(address, forKey: key)
            return ChequePayee(userId: peer.id, name: name, address: address)
        }
        return ChequePayee(userId: peer.id, name: name, address: pinned, changedTo: address == pinned ? nil : address)
    }

    /// The person confirmed that `payee` moved to the new address: remember that one instead.
    func acceptChangedPayee(_ payee: ChequePayee) -> ChequePayee {
        guard let me = user, let address = payee.changedTo else { return payee }
        UserDefaults.standard.set(address, forKey: payeeKey(me.id, payee.userId))
        return ChequePayee(userId: payee.userId, name: payee.name, address: address)
    }

    /// Sign a cheque to exactly `to` — the address the sheet showed for the other person in
    /// this DM — and send it there. Works offline for a few hours after a snapshot. Returns an
    /// error to show.
    func sendCheque(chatId: Id, amount: String, to: String) async -> String? {
        guard Wallet.chequesEnabled else { return "Cheques aren't available yet." }
        guard let phrase = walletPhrase, let address = walletAddressValue else { return "Set up your wallet first." }
        guard draftDM?.id != chatId, let chat = peopleChat(chatId), let me = user,
              let peer = chat.humans.first(where: { $0.id != me.id }) else {
            return "Cheques go to one person, in a direct chat."
        }
        guard UserDefaults.standard.string(forKey: payeeKey(me.id, peer.id)) == to else {
            return "Their address changed. Check it before you sign."
        }
        // A newer snapshot when online; the saved one otherwise (signing refuses a stale one).
        await refreshChequeSnapshot()
        guard let snapshot = chequeSnapshot,
              let base = UserDefaults.standard.string(forKey: "walletChainNonce:\(address)").flatMap({ UInt64($0) }) else {
            return "Go online once so this phone can sign cheques."
        }
        let issued = issuedCheques(address)
        let nonce = Wallet.nextNonce(base: base, issued: issued, now: UInt64(Date().timeIntervalSince1970))
        let cheque: WalletCheque
        do {
            cheque = try walletSignCheque(phrase: phrase, snapshotJson: snapshot, to: to, amount: amount, nonce: nonce)
        } catch {
            return walletWords(error)
        }
        guard cheque.to == to, cheque.from == address else { return "That cheque didn't come out right. Nothing was sent." }
        let id = newId()
        sendPeople(chatId: chatId, text: "Sent you a \(walletFormatTao(rao: cheque.rao)) cheque", replyTo: nil,
                   extraParts: [["id": "\(id)-cheque", "kind": "cheque", "cheque": cheque.bytes]])
        // The number is taken only now, with the cheque in the chat: a signature or a send that
        // failed above used nothing up.
        setIssuedCheques(issued + [Wallet.Issued(nonce: cheque.nonce, expiresAt: cheque.expiresAt)], address)
        return nil
    }

    /// Cash a cheque, or look again at one that was left pending.
    func cashCheque(_ bytes: String) async -> ChequeCashing {
        guard Wallet.chequesEnabled else { return .refused("Cheques aren't available yet.") }
        let defaults = UserDefaults.standard
        var pending = defaults.dictionary(forKey: "pendingCheques") as? [String: String] ?? [:]
        defer { defaults.set(pending, forKey: "pendingCheques") }
        do {
            switch try await walletCashCheque(bytes: bytes, since: pending[bytes].flatMap({ UInt64($0) })) {
            case .paid:
                pending[bytes] = nil
                defaults.set((defaults.stringArray(forKey: "cashedCheques") ?? []) + [bytes], forKey: "cashedCheques")
                return .paid
            case .pending(let since):
                pending[bytes] = String(since)
                return .pending
            }
        } catch let error as WalletError {
            // The network failing says nothing about the cheque: a pending one stays pending.
            if case .Chain = error { return pending[bytes] == nil ? .refused(error.words) : .pending }
            pending[bytes] = nil
            return .refused(error.words)
        } catch {
            return .refused(error.localizedDescription)
        }
    }

    /// The payer's account has moved past this cheque's number (as of the last snapshot): it
    /// was cashed, or that number went to something else. Only known for this device's address.
    func chequeNumberSpent(_ cheque: WalletCheque) -> Bool {
        UserDefaults.standard.string(forKey: "walletChainNonce:\(cheque.from)").flatMap({ UInt64($0) }).map { $0 > cheque.nonce } ?? false
    }

    func isCashed(_ bytes: String) -> Bool {
        (UserDefaults.standard.stringArray(forKey: "cashedCheques") ?? []).contains(bytes)
    }

    /// Submitted earlier and not yet seen finalized.
    func isChequePending(_ bytes: String) -> Bool {
        (UserDefaults.standard.dictionary(forKey: "pendingCheques") as? [String: String])?[bytes] != nil
    }
}
