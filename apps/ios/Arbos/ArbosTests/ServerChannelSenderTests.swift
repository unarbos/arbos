import XCTest
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
}

private extension Server {
    func with(members: [ServerMember]) -> Server {
        var s = self
        s.members = members
        return s
    }
}
