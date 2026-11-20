import XCTest
@testable import Arbos

/// Where the recovery phrase is kept (`WalletVault`): on this device only, or in the iCloud
/// Keychain, and the move between them. An account never ends up with two phrases or none.
final class WalletVaultTests: XCTestCase {
    private let user = "vault-test-\(UUID().uuidString)"
    private let phrase = "legal winner thank year wave sausage worth useful legal winner thank yellow"

    /// The keychain as `WalletVault` uses it: one item per account and per synced-or-not, and
    /// a query for either (`kSecAttrSynchronizableAny`) matching both.
    private var items: [String: Data] = [:]
    private var failAdd = false

    private func keys(_ q: [String: Any]) -> [String] {
        let account = q[kSecAttrAccount as String] as! String
        let synced = q[kSecAttrSynchronizable as String] as? Bool
        return [true, false].filter { synced == nil || synced == $0 }.map { "\(account)|\($0)" }
    }

    override func setUp() {
        WalletVault.add = { [self] q in
            if failAdd { return errSecNotAvailable }
            let key = keys(q)[0]
            guard items[key] == nil else { return errSecDuplicateItem }
            items[key] = q[kSecValueData as String] as? Data
            return errSecSuccess
        }
        WalletVault.copy = { [self] q in keys(q).compactMap { items[$0] }.first }
        WalletVault.delete = { [self] q in
            let found = keys(q).filter { items[$0] != nil }
            found.forEach { items[$0] = nil }
            return found.isEmpty ? errSecItemNotFound : errSecSuccess
        }
    }

    func testAPhraseStaysOnThisDeviceUntilMovedAndComesBackWhole() throws {
        try WalletVault.save(phrase, user: user)
        XCTAssertEqual(WalletVault.phrase(user: user), phrase)
        XCTAssertFalse(WalletVault.isSynced(user: user), "nothing goes to iCloud unasked")

        try WalletVault.move(user: user, synced: true)
        XCTAssertTrue(WalletVault.isSynced(user: user))
        XCTAssertEqual(WalletVault.phrase(user: user), phrase)
        // Moved, not copied: taking it out again leaves nothing behind in iCloud.
        try WalletVault.move(user: user, synced: false)
        XCTAssertFalse(WalletVault.isSynced(user: user))
        XCTAssertEqual(WalletVault.phrase(user: user), phrase)
        // Already where it was asked to be.
        try WalletVault.move(user: user, synced: false)
        XCTAssertEqual(WalletVault.phrase(user: user), phrase)
    }

    /// An account from before the choice existed: its phrase is in iCloud Keychain.
    func testAPhraseAlreadyInICloudIsFoundAndCanBeTakenOut() throws {
        try WalletVault.save(phrase, user: user, synced: true)
        XCTAssertTrue(WalletVault.isSynced(user: user))
        XCTAssertEqual(WalletVault.phrase(user: user), phrase)
        try WalletVault.move(user: user, synced: false)
        XCTAssertFalse(WalletVault.isSynced(user: user))
        XCTAssertEqual(WalletVault.phrase(user: user), phrase)
    }

    func testASecondPhraseIsRefusedWhereverTheFirstIs() throws {
        let other = "letter advice cage absurd amount doctor acoustic avoid letter advice cage above"
        for synced in [false, true] {
            try WalletVault.save(phrase, user: user, synced: synced)
            XCTAssertThrowsError(try WalletVault.save(other, user: user, synced: !synced))
            XCTAssertThrowsError(try WalletVault.save(other, user: user, synced: synced))
            XCTAssertEqual(WalletVault.phrase(user: user), phrase)
            // The same phrase again is not an error.
            XCTAssertNoThrow(try WalletVault.save(phrase, user: user, synced: synced))
            WalletVault.discard(unshown: other, user: user)
            XCTAssertEqual(WalletVault.phrase(user: user), phrase, "only the exact phrase is discarded")
            WalletVault.discard(unshown: phrase, user: user)
            XCTAssertNil(WalletVault.phrase(user: user))
        }
    }

    func testAMoveThatFailsLeavesThePhraseWhereItWas() throws {
        XCTAssertThrowsError(try WalletVault.move(user: user, synced: true), "no phrase to move")
        try WalletVault.save(phrase, user: user)
        failAdd = true
        XCTAssertThrowsError(try WalletVault.move(user: user, synced: true))
        XCTAssertFalse(WalletVault.isSynced(user: user))
        XCTAssertEqual(WalletVault.phrase(user: user), phrase)
    }
}
