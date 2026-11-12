import XCTest
import ArbosFFI
@testable import Arbos

/// Affinity (Arbonites, 2026-10-28 16:52 UTC, confirmed by Scout): "bug where we are referred to
/// as our handles instead of our usernames in server" — a channel bubble read "A" where a group
/// bubble reads "Scout". The homeserver names a person by handle; the display name comes from
/// the server's roster ([feedback server-member-names-2026-10-28]).
@MainActor
final class ServerChannelSenderTests: XCTestCase {
    private let channel = "01CHANNELGENERAL0000000000"
    private let server = "01SERVERTEST0000000000000"

    private func scout(name: String? = "Scout") -> ServerMember {
        ServerMember(id: "test_01SCOUT", handle: "A", name: name, avatarUrl: "https://relay.test/a/scout.png",
                     role: "member", joinedAt: 0)
    }

    private func scratchServer(members: [ServerMember]) -> Server {
        let raw: [String: Any] = ["id": server, "name": "test server", "members": [] as [Any]]
        return Server.parse(raw)!.with(members: members)
    }

    /// A channel row as `decodeMatrixRow` yields it: `senderHandle` set, no `senderName`.
    private func row(_ id: String, sender: String, handle: String, _ text: String) -> Message {
        Message(id: id, chatId: channel, role: .user, sender: sender, status: .done, turn: nil, decision: nil,
                subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil,
                createdAt: 1_700_000_000_000, completedAt: nil, pending: false,
                parts: [.text(id: "\(id)-t", text: text)], hiddenCount: 0, senderHandle: handle, senderName: nil)
    }

    private func model(members: [ServerMember]) -> AppModel {
        let m = AppModel()
        m.isFixture = true
        m.servers = [scratchServer(members: members)]
        return m
    }

    func testAMemberRowRendersTheDisplayNameNotTheHandle() {
        let m = model(members: [scout()])
        // The `.other` bubble labels its sender through `personName(handle:)` — the server's
        // roster answers even when the person shares no group with me.
        XCTAssertEqual(m.personName(handle: "A"), "Scout")
        XCTAssertEqual(m.personId(handle: "A"), "test_01SCOUT")
        XCTAssertEqual(m.personName(handle: "a"), "Scout", "GitHub logins are case-insensitive")
        guard case .user(let target) = m.personTarget(handle: "A", avatarUrl: nil, chatId: channel) else {
            return XCTFail("a person target")
        }
        XCTAssertEqual(target.displayName, "Scout", "the pane the bubble opens is Scout's, not a bare @A")
        // A roster without a name: the bubble falls back to the handle, as before.
        XCTAssertNil(model(members: [scout(name: nil)]).personName(handle: "A"))
        XCTAssertNil(model(members: []).personName(handle: "A"))
    }

    func testAChannelRowTakesNameAndPictureFromTheRoster() {
        var r = row("m1", sender: "user:test_01scout", handle: "A", "yo")
        XCTAssertTrue(AppModel.fillChannelSender(&r, from: scout()))
        XCTAssertEqual(r.senderName, "Scout")
        XCTAssertEqual(r.senderAvatar, "https://relay.test/a/scout.png")
        XCTAssertEqual(r.senderHandle, "A", "the handle stays the handle")

        var bare = row("m2", sender: "user:test_01scout", handle: "A", "yo")
        XCTAssertFalse(AppModel.fillChannelSender(&bare, from: nil), "no roster: nothing invented")
        XCTAssertNil(bare.senderName, "the handle is never written as the name")

        var nameless = row("m3", sender: "user:test_01scout", handle: "A", "yo")
        XCTAssertTrue(AppModel.fillChannelSender(&nameless, from: scout(name: nil)))
        XCTAssertNil(nameless.senderName)
        XCTAssertEqual(nameless.senderAvatar, "https://relay.test/a/scout.png")
    }

    func testRosterLookupGoesByUserIdThenHandle() {
        let m = model(members: [scout()])
        XCTAssertEqual(m.channelSender(sender: "user:test_01scout", handle: nil)?.name, "Scout",
                       "the Matrix localpart is lower-case; relay ids keep their case")
        XCTAssertEqual(m.channelSender(sender: "user:gh_other", handle: "A")?.name, "Scout", "by handle when the id is unknown")
        XCTAssertNil(m.channelSender(sender: "agent:01COMMS", handle: nil), "agents are not people")
        XCTAssertNil(m.channelSender(sender: "user:gh_other", handle: "nobody"))
    }

    func testThePreviewLabelsPeopleLikeAGroupDoes() {
        let me = "user:test_01ME"
        XCTAssertEqual(AppModel.previewWho(sender: "user:test_01scout", senderName: "A", me: me), "@A")
        XCTAssertEqual(AppModel.previewWho(sender: "user:test_01me", senderName: "affinity", me: me), "You")
        XCTAssertEqual(AppModel.previewWho(sender: "agent:01COMMS", senderName: "Comms", me: me), "Comms")
        XCTAssertNil(AppModel.previewWho(sender: "system", senderName: "", me: me))
        XCTAssertNil(AppModel.previewWho(sender: "system", senderName: "launchcrew", me: me))
        XCTAssertNil(AppModel.previewWho(sender: "", senderName: "", me: me), "no row yet")
    }

    // MARK: Native people chats (phase 3) — the Mac's `native_people_tests`, case for case.

    private func listed(_ room: String, _ chat: String) -> ArbosFFI.MatrixPeopleRow {
        ArbosFFI.MatrixPeopleRow(roomId: room, chatId: chat, kind: "dm", encrypted: true, unread: 0, mentions: 0,
                                 lastText: "", lastSender: "", lastSenderName: "", lastTs: 0)
    }

    func testANativeChatNeedsTheRelayAndTheHomeserverToNameTheSameRoom() {
        func room(_ rows: [(String, String)], _ chat: String, relay: [String: String] = ["C1": "!r1:hs"]) -> String? {
            AppModel.confirmedRoom(relay: relay, homeserver: rows.map { listed($0.0, $0.1) }, chatId: chat)
        }
        XCTAssertEqual(room([("!r1:hs", "C1")], "C1"), "!r1:hs")
        XCTAssertNil(room([], "C1"), "not listed by the homeserver yet")
        XCTAssertNil(room([("!evil:hs", "C1")], "C1"), "another room says it is this chat; the relay's is not listed")
        XCTAssertEqual(room([("!evil:hs", "C1"), ("!r1:hs", "C1")], "C1"), "!r1:hs", "the impostor beside the real room changes nothing")
        XCTAssertNil(room([("!r1:hs", "C2")], "C1"), "the relay's room says it is a different chat")
        XCTAssertNil(room([("!r2:hs", "C2")], "C2"), "a chat the relay does not name is not native")
        XCTAssertNil(room([("!r1:hs", "C1")], "C1", relay: [:]))
    }

    func testTheRelaysMappingIsTakenOnlyAsWritten() {
        let people: Set<String> = ["C1", "C2", "C3"]
        func parsed(_ json: String) -> [String: String]? {
            AppModel.parsePeopleRooms(try! JSONSerialization.jsonObject(with: Data(json.utf8), options: .fragmentsAllowed), people: people)
        }
        XCTAssertEqual(parsed(#"{"rooms":{}}"#), [:])
        XCTAssertEqual(parsed(#"{"rooms":{"C1":"!r1:hs"}}"#), ["C1": "!r1:hs"])
        // Dropped: a chat this phone does not list, something that is not a room id, a room
        // given to two chats, and a room id longer than one can be.
        XCTAssertEqual(parsed(#"{"rooms":{"C9":"!r9:hs","C1":"r1","C2":"!same:hs","C3":"!same:hs"}}"#), [:])
        XCTAssertEqual(parsed(#"{"rooms":{"C1":"!\#(String(repeating: "r", count: 255))"}}"#), [:])
        // Not the answer at all: what was known is kept (nil).
        for bad in [#"{}"#, #"{"rooms":[]}"#, #"{"rooms":{"C1":7}}"#, #"{"rooms":{"C1":null}}"#,
                    #"{"rooms":{},"more":true}"#, #""servers_off""#, #"null"#] {
            XCTAssertNil(parsed(bad), bad)
        }
    }

    func testOnlyTheRelaysTrueTurnsPeopleNativeOn() {
        func parsed(_ flag: String) -> Bool? {
            let json = #"{"homeserver":"https://hs","user_id":"@gh_7:hs","access_token":"t","device_id":"D"\#(flag)}"#
            return MatrixCredentials.parse(try! JSONSerialization.jsonObject(with: Data(json.utf8)))?.peopleNative
        }
        XCTAssertEqual(parsed(#","people_native":true"#), true)
        for off in ["", #","people_native":false"#, #","people_native":1"#, #","people_native":"true""#, #","people_native":null"#] {
            XCTAssertEqual(parsed(off), false, off)
        }
    }

    func testTheHistoryKeyIsTheRelaysStringOrNone() {
        func parsed(_ key: String) -> String? {
            let json = #"{"homeserver":"https://hs","user_id":"@gh_7:hs","access_token":"t","device_id":"D"\#(key)}"#
            return MatrixCredentials.parse(try! JSONSerialization.jsonObject(with: Data(json.utf8)))?.historyKey
        }
        XCTAssertEqual(parsed(#","history_key":"6kpsY+KcUgq+9VB7Ey7F+ZVHdq6+vnuSQh7qaRRG0iw""#), "6kpsY+KcUgq+9VB7Ey7F+ZVHdq6+vnuSQh7qaRRG0iw")
        for none in ["", #","history_key":null"#, #","history_key":7"#] {
            XCTAssertNil(parsed(none), none)
        }
    }

    /// A row as `arbos_matrix::rows::message_row` writes it.
    private func native(_ id: String, mxid: String, text: String, locked: Bool = false, pending: Bool = false,
                        extra: String = "") -> ArbosFFI.MatrixMessageRow {
        let sender = "user:" + mxid.dropFirst().split(separator: ":")[0]
        let json = #"{"id":"\#(id)","eventId":"$\#(id)","role":"user","sender":"\#(sender)","senderHandle":"h","status":"done","#
            + #""createdAt":1700,"completedAt":1700,"parts":[{"id":"$\#(id)#text","kind":"text","text":"\#(text)"}]\#(extra)}"#
        return ArbosFFI.MatrixMessageRow(eventId: pending ? "~\(id)" : "$\(id)", id: id, sender: sender, senderName: "h", text: text,
                                         createdAt: 1700, locked: locked, pending: pending, json: json)
    }

    func testANativeRowIsTheChatsMessageFromTheRelaysSpellingOfItsSender() {
        let locked = "🔒 Waiting for this message's key"
        let rows = [
            native("01THEIRS", mxid: "@test_01abc:arbos.life", text: "lunch?"),
            native("01LOCKED", mxid: "@test_01abc:arbos.life", text: locked, locked: true),
            native("01MINE", mxid: "@gh_7:arbos.life", text: "on my way", pending: true,
                   extra: #","reactions":{"👍":{"user:test_01abc":true}},"replyTo":"01THEIRS""#),
        ]
        let members = ["user:gh_7", "user:test_01ABC"]
        let messages = AppModel.nativeMessages(rows, chatId: "C1", members: members)
        XCTAssertEqual(messages.map(\.id), ["01THEIRS", "01LOCKED", "01MINE"])
        XCTAssertTrue(messages.allSatisfy { $0.chatId == "C1" && $0.role == .user })
        let (theirs, undecrypted, mine) = (messages[0], messages[1], messages[2])
        XCTAssertEqual(theirs.sender, "user:test_01ABC", "the relay's spelling, so the roster and \"mine\" match")
        XCTAssertFalse(theirs.pending)
        // What this phone cannot decrypt is shown as that, never as the envelope.
        XCTAssertEqual(undecrypted.plainText, locked)
        // My own send, before the homeserver has it.
        XCTAssertEqual(mine.sender, "user:gh_7")
        XCTAssertTrue(mine.pending)
        XCTAssertEqual(mine.replyTo, "01THEIRS")
        XCTAssertEqual(mine.reactions, [MessageReaction(emoji: "👍", senders: ["user:test_01ABC"])])
        // A sender nobody in the chat's roster matches keeps the homeserver's spelling.
        XCTAssertEqual(AppModel.nativeMessages([rows[0]], chatId: "C1", members: ["user:gh_7"])[0].sender, "user:test_01abc")
        // Who sent a row is the account the homeserver names, whatever the row's JSON says.
        var forged = rows[0]
        forged.json = forged.json.replacingOccurrences(of: "user:test_01abc", with: "user:gh_7")
        XCTAssertEqual(AppModel.nativeMessages([forged], chatId: "C1", members: members)[0].sender, "user:test_01ABC")
    }

    /// A file kept on the homeserver is loaded through the Matrix client, wherever an
    /// attachment is loaded by URL; a relay blob's key is not such a URL.
    func testAFileOnTheHomeserverIsLoadedThroughTheMatrixClient() async throws {
        XCTAssertNil(MatrixMediaProtocol.url(slot: "people:C1", key: "att-1-plan.png"))
        XCTAssertNil(MatrixMediaProtocol.url(slot: "01PROJECT", key: "mx:$e/0"), "only a people chat has such files")
        let model = AppModel()
        model.isFixture = true
        let url = try XCTUnwrap(model.blobURL(projectId: "people:C1", key: "mx:$abc_-/0"))
        XCTAssertEqual(url.scheme, MatrixMediaProtocol.scheme)
        MatrixMediaProtocol.serve { chat, reference in
            guard chat == "C1", reference == "$abc_-/0" else { throw URLError(.fileDoesNotExist) }
            return ("image/png", Data([1, 2, 3]))
        }
        let fetched = await MediaFetch.data(url, headers: model.authHeaders)
        XCTAssertEqual(fetched?.0, Data([1, 2, 3]))
        XCTAssertEqual(fetched?.1, "image/png")
        let other = try XCTUnwrap(model.blobURL(projectId: "people:C2", key: "mx:$abc_-/0"))
        let missing = await MediaFetch.data(other, headers: [:])
        XCTAssertNil(missing)
    }

    /// A file on the relay goes into a native chat's message as the part the chat doc writes.
    func testAnAttachmentPartIsTheChatDocsShape() throws {
        let json = try AppModel.attachmentPart(Attachment(name: "plan.png", mime: "image/png", blobRef: "att-1-plan.png", kind: "image"))
        let part = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any])
        XCTAssertEqual(part["kind"] as? String, "attachment")
        XCTAssertEqual(part["attachment"] as? [String: String],
                       ["name": "plan.png", "mime": "image/png", "ref": "att-1-plan.png", "kind": "image"])
    }
}

private extension Server {
    func with(members: [ServerMember]) -> Server {
        var s = self
        s.members = members
        return s
    }
}
