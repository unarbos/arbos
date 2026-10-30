import XCTest
@testable import Arbos

/// Jacob, 2026-10-27 16:05 UTC (Arbonites, Doug agent on agentd 0.1.150): the SN120 chart PNG was
/// delivered onto the job's hidden worker row (`job` + `hidden`) and the app drew nothing — "the
/// chart is in the card above" over a different message. A hidden/job row still draws its
/// attachment cards — under the agent's avatar and name like its bubbles
/// ([feedback forward-attachments-ios-2026-10-27]) — while everything else on it (status, text,
/// the "no answer" line) stays hidden.
final class HiddenJobRowAttachmentTests: XCTestCase {
    private let chat = "01M5MHDEZFB5SS6VRK9T12NVAC"
    private let t0: Int64 = 1_700_000_000_000

    private var ask: [String: Any] {
        ["id": "u1", "role": "user", "sender": "user:gh_jacob", "senderHandle": "jacob", "status": "done",
         "createdAt": t0, "parts": [["id": "t", "kind": "text", "text": "@doug-agent can you plot the subnet 120 price"]]]
    }

    /// Doug's ack names the agent (`agent:<id>`, handle, name) as every people-chat reply does.
    private var ack: [String: Any] {
        ["id": "a1", "role": "assistant", "sender": "agent:01DOUG", "senderHandle": "doug-agent", "senderName": "Doug agent",
         "status": "done", "createdAt": t0 + 500, "inReplyTo": "u1",
         "parts": [["id": "t", "kind": "text", "text": "Plotting the SN120 price history now."]]]
    }

    /// The 0.1.150 worker row: a bare `agent`, no handle, hidden, the job's row.
    private func jobRow(withAttachment: Bool) -> [String: Any] {
        var parts: [[String: Any]] = [
            ["id": "s", "kind": "status", "code": "working", "message": "Plotting SN120 price"],
            ["id": "t", "kind": "text", "text": "Done."],
        ]
        if withAttachment {
            parts.append(["id": "att1", "kind": "attachment",
                          "attachment": ["name": "sn120_price.png", "mime": "image/png", "ref": "7ba1fc21",
                                         "bytes": 20_260, "kind": "image", "title": "SN120 price"]])
        }
        return ["id": "j1", "role": "assistant", "sender": "agent", "status": "done",
                "createdAt": t0 + 1_000, "inReplyTo": "u1", "hidden": true,
                "job": ["id": "01M5Z1QCTG61DZ98MRAWH95FBF", "title": "Running the program", "kind": "code"],
                "parts": parts]
    }

    private func messages(withAttachment: Bool) throws -> [Message] {
        try [ask, ack, jobRow(withAttachment: withAttachment)].map { try XCTUnwrap(DocJSON.message($0, chatId: chat)) }
    }

    func testAHiddenJobRowStillDrawsItsAttachmentCardUnderTheAgent() throws {
        let ms = try messages(withAttachment: true)
        XCTAssertTrue(ms[2].hidden)
        XCTAssertNotNil(ms[2].job)
        let rows = Transcript.rows(ms, projectId: "doug", people: true, multiParty: true)
        let mine = rows.filter { $0.messageId == "j1" }
        XCTAssertEqual(mine.map(\.id), ["j1#att1"], "exactly the card, nothing else: \(rows.map(\.id))")
        // Labelled pattern: a new associated value on `.image` then fails here by name,
        // not as "missing pattern #n".
        guard case .image(item: let item, fromUser: let fromUser, caption: _, lane: let lane, source: _,
                          forwardedFrom: _, sender: let sender, named: let named, quote: _) = mine[0].kind else {
            return XCTFail("an image card: \(mine[0].kind)")
        }
        XCTAssertEqual(item.name, "sn120_price.png")
        XCTAssertEqual(item.slot, "doug")
        XCTAssertFalse(fromUser)
        XCTAssertTrue(lane, "agent media in a people chat sits on the avatar lane")
        let doug: MediaSender? = MediaSender.agent(id: "01DOUG", handle: "doug-agent", name: "Doug agent", avatar: nil)
        XCTAssertEqual(sender, doug, "attribution borrowed from Doug's ack answering the same ask")
        XCTAssertTrue(named, "Doug's name above the card in a group")
    }

    func testAHiddenJobRowWithoutAFileStillDrawsNothing() throws {
        let ms = try messages(withAttachment: false)
        let rows = Transcript.rows(ms, projectId: "doug", people: true, multiParty: true)
        XCTAssertFalse(rows.contains { $0.messageId == "j1" }, "wordless by design: \(rows.map(\.id))")
    }

    func testHiddenRowAgentFallsBackToTheRowItself() throws {
        let ms = try messages(withAttachment: true)
        XCTAssertEqual(Transcript.hiddenRowAgent(ms[2], in: ms).id, "a1")
        XCTAssertEqual(Transcript.hiddenRowAgent(ms[2], in: [ms[2]]).id, "j1", "no sibling: the row itself, no lane")
        XCTAssertEqual(Transcript.hiddenRowAgent(ms[1], in: ms).id, "a1", "a named row keeps its own attribution")
    }
}
