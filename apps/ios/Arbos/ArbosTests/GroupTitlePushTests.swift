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
}
