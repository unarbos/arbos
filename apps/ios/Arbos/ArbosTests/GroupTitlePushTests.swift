import XCTest
@testable import Arbos

/// Notification shape per chat kind (2026-10-27): a DM is titled with the sender; a group or
/// shared project chat is titled with the chat and names the sender in the body, so the
/// banner says which group a message came from. The relay's APNs payload and the app's own
/// `/me/pushes` banner both go through `MessagePush`.
final class GroupTitlePushTests: XCTestCase {
    func testDmIsTitledWithTheSender() throws {
        let info = MessagePush.userInfo(chatId: "c1", projectId: "people", title: "@alice", body: "hello",
                                        senderKind: "user", senderId: "u1", senderName: "Alice", senderHandle: "alice",
                                        avatar: nil, avatarColor: nil, avatarUrl: nil, chatKind: "dm", text: "hello")
        let push = try XCTUnwrap(MessagePush(userInfo: info))
        XCTAssertEqual(push.title, "@alice")
        XCTAssertEqual(push.body, "hello")
        XCTAssertEqual(push.chatKind, "dm")
        XCTAssertNil(push.groupName)
        XCTAssertEqual(push.text, "hello")
    }

    func testGroupIsTitledWithTheGroupAndCarriesTheSenderInTheBody() throws {
        let info = MessagePush.userInfo(chatId: "c2", projectId: "people", title: "Arbonites", body: "Doug: Five.",
                                        senderKind: "agent", senderId: "p9", senderName: "Doug", senderHandle: "",
                                        avatar: "palm", avatarColor: "amber", avatarUrl: nil,
                                        chatTitle: "Arbonites", chatKind: "group", text: "Five.",
                                        chatImageUrl: "https://relay.example/blob/people:c2/pic.png?token=x")
        let push = try XCTUnwrap(MessagePush(userInfo: info))
        XCTAssertEqual(push.title, "Arbonites")
        XCTAssertEqual(push.body, "Doug: Five.")
        XCTAssertEqual(push.chatKind, "group")
        XCTAssertEqual(push.groupName, "Arbonites")
        XCTAssertEqual(push.text, "Five.")
        XCTAssertEqual(push.groupImageUrl, "https://relay.example/blob/people:c2/pic.png?token=x")
        XCTAssertEqual(push.senderKind, "agent")
    }

    /// The relay's APNs payload shape (`aps.alert` + flat keys) parses to the same thing.
    func testRelayPayloadWithoutChatKindDefaultsFromTheTitle() throws {
        let remote: [AnyHashable: Any] = [
            "aps": ["alert": ["title": "Research", "body": "@sam: ping", "summary-arg": "@sam"], "category": "ARBOS_MESSAGE", "thread-id": "c3"] as [String: Any],
            "chat_id": "c3", "project_id": "p1", "chat_title": "Research", "text": "ping",
            "sender": ["kind": "user", "id": "u2", "name": "Sam", "handle": "sam"],
        ]
        let push = try XCTUnwrap(MessagePush(userInfo: remote))
        XCTAssertEqual(push.chatKind, "group", "an older relay without chat_kind: a titled chat is a group")
        XCTAssertEqual(push.groupName, "Research")
        XCTAssertEqual(push.text, "ping")
        XCTAssertNil(push.groupImageUrl, "the extension never gets a tokenised URL")
    }

    /// Phase 5: an alert for an end-to-end encrypted message says only that there is one and
    /// carries the event; once the extension has read it, the words stand where that was.
    func testAnEncryptedAlertCarriesItsEventAndShowsTheWordsOnceRead() throws {
        func remote(_ matrix: [String: Any], title: String, body: String, group: String?) -> [AnyHashable: Any] {
            var info: [AnyHashable: Any] = [
                "aps": ["alert": ["title": title, "body": body], "category": "ARBOS_MESSAGE"] as [String: Any],
                "chat_id": "c4", "project_id": "people", "text": "New message", "matrix": matrix,
                "sender": ["kind": "user", "id": "u2", "name": "Sam", "handle": "sam"],
            ]
            if let group { info["chat_title"] = group }
            return info
        }
        let event: [String: Any] = ["type": "m.room.encrypted", "event_id": "$e", "content": ["ciphertext": "AwgA"]]
        var group = try XCTUnwrap(MessagePush(userInfo: remote(["room_id": "!r:hs", "event": event],
                                                               title: "Research", body: "@sam: New message", group: "Research")))
        let sealed = try XCTUnwrap(group.sealed)
        XCTAssertEqual(sealed.roomId, "!r:hs")
        XCTAssertEqual(try JSONSerialization.jsonObject(with: Data(sealed.event.utf8)) as? NSDictionary, event as NSDictionary)
        group.show("see you at noon")
        XCTAssertEqual(group.body, "@sam: see you at noon")
        XCTAssertEqual(group.text, "see you at noon")
        var dm = try XCTUnwrap(MessagePush(userInfo: remote(["room_id": "!r:hs", "event": event],
                                                            title: "@sam", body: "New message", group: nil)))
        dm.show("see you at noon")
        XCTAssertEqual(dm.body, "see you at noon")
        // Not a room, or no event: an alert like any other.
        for matrix in [["room_id": "r", "event": event], ["room_id": "!r:hs"], [:]] as [[String: Any]] {
            XCTAssertNil(try XCTUnwrap(MessagePush(userInfo: remote(matrix, title: "@sam", body: "New message", group: nil))).sealed)
        }
    }

    /// What the extension reads the message with is kept where both it and the app can reach
    /// it (their App Group), and is gone once signed out.
    func testThePreviewVaultIsSharedThroughTheAppGroupAndForgets() throws {
        XCTAssertNotNil(PreviewVault.storeRoot, "the test host has the App Group")
        let kept = PreviewVault.load()
        defer { PreviewVault.save(kept) }
        let device = PreviewVault.Device(userId: "@gh_7:arbos.test", deviceId: "D1", storeKey: "k")
        PreviewVault.save(device)
        XCTAssertEqual(PreviewVault.load(), device)
        PreviewVault.save(nil)
        XCTAssertNil(PreviewVault.load())
    }

    /// The Matrix store an earlier build kept in the app's own container is moved, whole, to
    /// where the extension can read it, once; it is the device's only copy of its keys.
    func testAnEarlierStoreIsMovedToTheAppGroupWhole() throws {
        let fm = FileManager.default
        let device = "TESTDEVICE-\(UUID().uuidString)"
        let old = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("matrix").appendingPathComponent(device)
        let shared = try XCTUnwrap(PreviewVault.storeRoot).appendingPathComponent(device)
        defer { try? fm.removeItem(at: old); try? fm.removeItem(at: shared) }
        try fm.createDirectory(at: old, withIntermediateDirectories: true)
        try Data("keys".utf8).write(to: old.appendingPathComponent("matrix-sdk-crypto.sqlite3"))
        XCTAssertEqual(WalletVault.storeDir(device: device), shared.path)
        XCTAssertEqual(try Data(contentsOf: shared.appendingPathComponent("matrix-sdk-crypto.sqlite3")), Data("keys".utf8))
        XCTAssertFalse(fm.fileExists(atPath: old.path))
        // Again, and for a device with no earlier store: the same place.
        XCTAssertEqual(WalletVault.storeDir(device: device), shared.path)
        XCTAssertEqual(try fm.contentsOfDirectory(atPath: shared.path), ["matrix-sdk-crypto.sqlite3"])
    }
}
