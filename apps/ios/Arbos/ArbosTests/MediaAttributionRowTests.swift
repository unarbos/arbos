import XCTest
@testable import Arbos

/// Jacob, 2026-10-23 (feedback `media-sender-attribution-2026-10-23`): a picture or file
/// row carries the same sender decoration as a text row — the avatar column with the
/// sender's avatar on the message's last media row, the name above the first row in a
/// group / channel, nothing in a 1:1 chat — and consecutive media from one sender group
/// like text.
final class MediaAttributionRowTests: XCTestCase {
    private let chat = "fx-group-launch"
    private let t0: Int64 = 1_700_000_000_000

    private func photos(_ i: Int, sender: String, handle: String, count: Int, caption: String = "", role: Role = .user) -> Message {
        var parts: [Part] = (0..<count).map {
            .attachment(id: "m\(i)-a\($0)", attachment: Attachment(name: "scout-\($0).jpg", mime: "image/jpeg", blobRef: "b\(i)-\($0)", kind: "image"))
        }
        if !caption.isEmpty { parts.append(.text(id: "m\(i)-t", text: caption)) }
        return Message(id: "m\(i)", chatId: chat, role: role, sender: sender, status: .done, turn: role == .user ? nil : .reply,
                       decision: nil, subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil,
                       createdAt: t0 + Int64(i) * 1_000, completedAt: nil, pending: false, parts: parts, hiddenCount: 0,
                       senderHandle: handle, senderName: "Scout")
    }

    /// `(named, avatar)` per media row, in order.
    private func media(_ rows: [Row]) -> [(MediaChrome.Sender, Bool, Bool)] {
        rows.compactMap { row in
            switch row.kind {
            case .image(_, let c), .file(_, _, let c): return (c.sender, c.named, c.avatar)
            default: return nil
            }
        }
    }

    func testAPersonsPhotoInAGroupIsNamedWithTheirAvatar() {
        let rows = Transcript.rows([photos(0, sender: "user:fx-u9", handle: "scout", count: 1)],
                                   projectId: "people:\(chat)", me: "user:fx-me", people: true, multiParty: true)
        let m = media(rows)
        XCTAssertEqual(m.count, 1)
        XCTAssertEqual(m[0].0, .person(handle: "scout", avatar: nil))
        XCTAssertTrue(m[0].1, "the name above the picture — the group names its senders")
        XCTAssertTrue(m[0].2, "the only row shows the avatar, like a text bubble")
    }

    func testAnAgentsPhotoInAGroupIsNamedFromItsLane() {
        let rows = Transcript.rows([photos(0, sender: "agent:fx-comms", handle: "comms", count: 1, role: .assistant)],
                                   projectId: "people:\(chat)", me: "user:fx-me", people: true, multiParty: true)
        let m = media(rows)
        XCTAssertEqual(m.count, 1)
        XCTAssertEqual(m[0].0, .agent(handle: "comms", avatar: nil, agentId: "fx-comms"))
        XCTAssertTrue(m[0].1 && m[0].2)
    }

    func testAPhotoInAOneToOneChatKeepsTheAvatarButNoName() {
        // DM with a person: avatar column, no name label.
        let dm = Transcript.rows([photos(0, sender: "user:fx-u9", handle: "scout", count: 1)],
                                 projectId: "people:fx-dm", me: "user:fx-me", people: true, multiParty: false)
        XCTAssertEqual(media(dm).map { ($0.1, $0.2) }.first.map { [$0.0, $0.1] }, [false, true])
        // Me ↔ agent: plain, like the agent's bubble.
        let agent = Transcript.rows([photos(0, sender: "agent", handle: "home", count: 1, role: .assistant)],
                                    projectId: "fx-home", me: nil, people: false, multiParty: false)
        XCTAssertEqual(media(agent).first?.0, .plain)
        XCTAssertEqual(media(agent).first?.1, false)
    }

    func testConsecutivePhotosFromOneSenderGroupLikeText() {
        let rows = Transcript.rows([
            photos(0, sender: "user:fx-u9", handle: "scout", count: 2),
            photos(1, sender: "user:fx-u9", handle: "scout", count: 0, caption: "and a caption"),
        ], projectId: "people:\(chat)", me: "user:fx-me", people: true, multiParty: true)
        // Name once (above the first picture); the avatar on each message's last row.
        XCTAssertEqual(media(rows).map { [$0.1, $0.2] }, [[true, false], [false, true]])
        let captionNamed: [Bool] = rows.compactMap { if case .other(_, _, _, let named, _, _) = $0.kind { return named }; return nil }
        XCTAssertEqual(captionNamed, [false], "same sender run: the caption is not named again")
    }

    func testAPhotoWithACaptionPutsTheNameOnThePhotoAndTheAvatarOnTheText() {
        let rows = Transcript.rows([photos(0, sender: "user:fx-u9", handle: "scout", count: 1, caption: "look")],
                                   projectId: "people:\(chat)", me: "user:fx-me", people: true, multiParty: true)
        XCTAssertEqual(media(rows).map { [$0.1, $0.2] }, [[true, false]], "name on the picture; the caption's bubble draws the avatar")
        let captionNamed: [Bool] = rows.compactMap { if case .other(_, _, _, let named, _, _) = $0.kind { return named }; return nil }
        XCTAssertEqual(captionNamed, [false], "the name is not repeated in the caption")
    }

    func testMyPhotoHasNoChrome() {
        let rows = Transcript.rows([photos(0, sender: "user:fx-me", handle: "jacob", count: 1)],
                                   projectId: "people:\(chat)", me: "user:fx-me", people: true, multiParty: true)
        XCTAssertEqual(media(rows).first?.0, .mine)
        XCTAssertEqual(media(rows).first?.1, false)
    }
}
