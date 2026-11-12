import XCTest
@testable import Arbos

/// A people row names the sender in its preview only where the row's own header does
/// not ([feedback doubled-sender-prefix-2026-10-30], reported again by Scout
/// 2026-11-05): the shot read "Affinity @Z" in a 1:1 row's header and "@Z: hahahaha"
/// in its preview line, the same person twice. A group row is headed by the group and
/// carries no handle, so it still names whoever wrote.
final class HomePreviewWhoTests: XCTestCase {
    func testAOneToOneRowDoesNotRepeatThePersonItsHeaderNames() {
        XCTAssertEqual(HomeView.previewWho(group: false, mine: false, sender: "user:gh_461", handle: "Z"), "",
                       "the 1:1 row header is already \"Affinity @Z\"")
        XCTAssertEqual(HomeView.previewWho(group: true, mine: false, sender: "user:gh_461", handle: "Z"), "@Z",
                       "a group row is headed by the group, so it names the sender")
    }

    func testMyOwnLineStaysMineAndAnAgentKeepsItsName() {
        XCTAssertEqual(HomeView.previewWho(group: false, mine: true, sender: "user:gh_me", handle: "jacob"), "You")
        XCTAssertEqual(HomeView.previewWho(group: true, mine: true, sender: "user:gh_me", handle: "jacob"), "You")
        XCTAssertEqual(HomeView.previewWho(group: false, mine: false, sender: "agent:01COMMS", handle: "comms"), "@comms",
                       "an agent in a 1:1 chat is not the person the header names")
        XCTAssertEqual(HomeView.previewWho(group: false, mine: false, sender: "system", handle: nil), "",
                       "no handle, no label")
    }

    /// A picture with no words is the chat's newest message, and its row says so with the
    /// word its notification has; seen on a native chat, where the row kept "You: Hi Alice
    /// got it" after a picture came in. A message with words previews as it did.
    @MainActor func testAFileWithNoWordsIsTheRowsPreview() {
        func message(_ id: String, sender: String, _ parts: [Part]) -> Message {
            Message(id: id, chatId: "dm1", role: .user, sender: sender, status: .done, turn: nil, decision: nil,
                    subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil,
                    createdAt: 1_700_000_000_000, completedAt: nil, pending: false,
                    parts: parts, hiddenCount: 0, senderHandle: "alice", senderName: nil)
        }
        let dm = PeopleChat(id: "dm1", kind: "dm", createdBy: "gh_me", createdAt: 0, title: "", pending: false,
                            members: [], headSeq: 0, readSeq: 0, unread: false, lockState: "people_only",
                            lastActivityAt: 0, myRole: "owner", muted: false, silenced: false)
        var group = dm
        group.kind = "group"
        let model = AppModel()
        model.isFixture = true
        let words = message("m1", sender: "user:gh_alice", [.text(id: "m1-t", text: "Hi Alice got it")])
        let picture = message("m2", sender: "user:gh_alice", [.attachment(id: "m2-a", attachment: Attachment(
            name: "cat.png", mime: "image/png", blobRef: "blob-1", kind: "image"))])
        // A native chat's rows and a Loro room's messages are read by the one builder.
        model.channelRows["dm1"] = [words, picture]
        XCTAssertEqual(HomeView.peoplePreview(dm, in: model), "Attachment")
        XCTAssertEqual(HomeView.peoplePreview(group, in: model), "@alice: Attachment")
        model.channelRows["dm1"] = [picture, words]
        XCTAssertEqual(HomeView.peoplePreview(dm, in: model), "Hi Alice got it")
        model.channelRows["dm1"] = []
        XCTAssertEqual(HomeView.peoplePreview(dm, in: model), "Say hi")
    }

    /// The rollback (`matrix_people_native` off): the chats this device read from the
    /// homeserver are dialled as Loro rooms as soon as the relay says so. They used to wait
    /// for the launch loop's next turn, 15 s of spinner on the chat that was opened.
    func testAChatHandedBackIsDialledWithoutWaitingForTheLoop() throws {
        let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Arbos/Store/ServerStore.swift")
        guard let store = try? String(contentsOf: url, encoding: .utf8) else {
            throw XCTSkip("sources not beside the test bundle (CI without the tree)")
        }
        let handedBack = try XCTUnwrap(store.range(of: "if !peopleNative, !nativeRooms.isEmpty {"))
        let next = store[handedBack.upperBound...].prefix(120)
        XCTAssertTrue(next.contains("applyNativeRooms([:])") && next.contains("await refreshPeople()"), String(next))
    }
}
