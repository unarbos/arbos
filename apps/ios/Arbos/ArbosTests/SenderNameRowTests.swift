import XCTest
@testable import Arbos

/// Jacob, 2026-10-23 (feedback `sender-name-in-bubble-2026-10-23`): the sender's name sits
/// inside the bubble in group chats and server channels — on the first bubble of a sender
/// run, people and agents alike (grouped messages, Jacob 2026-10-28, supersedes #1447's
/// name on every agent reply; `GroupedMessagesTests`) — and a 1:1 chat (me ↔ person,
/// me ↔ agent) labels nobody. Own messages never carry a name. No bubble row carries a time (Jacob, 2026-10-24, feedback
/// `timestamps-hover-only-2026-10-24`): `.other` / `.member` have no time slot; the transcript's
/// drag-to-reveal is the only way to see one.
final class SenderNameRowTests: XCTestCase {
    private let chat = "fx-group-launch"
    private let t0: Int64 = 1_700_000_000_000

    private func person(_ i: Int, sender: String, handle: String, _ text: String) -> Message {
        Message(id: "m\(i)", chatId: chat, role: .user, sender: sender, status: .done, turn: nil, decision: nil, subagentId: nil,
                subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: t0 + Int64(i) * 1_000, completedAt: nil,
                pending: false, parts: [.text(id: "m\(i)-t", text: text)], hiddenCount: 0, senderHandle: handle, senderName: handle)
    }

    private func agent(_ i: Int, _ text: String) -> Message {
        Message(id: "m\(i)", chatId: chat, role: .assistant, sender: "agent:fx-comms", status: .done, turn: .reply, decision: "reply",
                subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: t0 + Int64(i) * 1_000,
                completedAt: nil, pending: false, parts: [.text(id: "m\(i)-t", text: text)], hiddenCount: 0,
                senderHandle: "comms", senderName: "Comms")
    }

    /// `(text or "agent", named)` for every bubble from someone else, plus `("mine", false)`.
    private func names(_ rows: [Row]) -> [(String, Bool)] {
        rows.compactMap { row in
            switch row.kind {
            case .other(let text, _, _, let named, _, _): return (text, named)
            case .member(_, _, _, _, _, let named, _, _): return ("agent", named)
            case .user: return ("mine", false)
            default: return nil
            }
        }
    }

    private var messages: [Message] {
        [
            person(0, sender: "user:fx-u3", handle: "mira-k", "hi all"),
            person(1, sender: "user:fx-u3", handle: "mira-k", "second"),
            person(2, sender: "user:fx-me", handle: "jacob", "mine"),
            agent(3, "on it"),
            agent(4, "done"),
        ]
    }

    func testGroupAndChannelBubblesNameTheirSenderButNotMine() {
        let rows = Transcript.rows(messages, projectId: "people:\(chat)", me: "user:fx-me", people: true, multiParty: true)
        let got = names(rows)
        XCTAssertEqual(got.map(\.0), ["hi all", "second", "mine", "agent", "agent"])
        // The first bubble of a run — people and agents alike (these two agent replies
        // quote nothing and sit 1 s apart, so they are one run); mine: never.
        XCTAssertEqual(got.map(\.1), [true, false, false, true, false], "named on the first bubble of a run, never on mine")
    }

    func testDMBubblesCarryNoName() {
        let rows = Transcript.rows(messages, projectId: "people:\(chat)", me: "user:fx-me", people: true, multiParty: false)
        let got = names(rows)
        XCTAssertEqual(got.map(\.0), ["hi all", "second", "mine", "agent", "agent"])
        XCTAssertTrue(got.allSatisfy { !$0.1 }, "a 1:1 chat labels nobody")
    }

    func testAgentChatBubblesCarryNoName() {
        // Me ↔ agent (no people chat): the agent speaks through `.assistant`, never `.member`.
        let rows = Transcript.rows([person(0, sender: "user", handle: "jacob", "hey"), agent(1, "hello")],
                                   projectId: "fx-comms", me: nil, people: false, multiParty: false)
        XCTAssertTrue(rows.contains { if case .assistant = $0.kind { return true } else { return false } })
        XCTAssertFalse(rows.contains { if case .member = $0.kind { return true } else { return false } })
    }

    func testNamedBubbleStacksNameQuoteText() {
        // The order inside the bubble is name → quote → text (a layout fact the view
        // builds from these two row facts).
        var reply = person(1, sender: "user:fx-u4", handle: "sam", "answer")
        reply.replyTo = "m0"
        let rows = Transcript.rows([person(0, sender: "user:fx-u3", handle: "mira-k", "question"), reply],
                                   projectId: "people:\(chat)", me: "user:fx-me", people: true, multiParty: true)
        guard case .other(let text, _, _, let named, let quote, _)? = rows.last?.kind else {
            return XCTFail("the reply is someone else's bubble: \(String(describing: rows.last?.kind))")
        }
        XCTAssertTrue(named)
        XCTAssertNotNil(quote)
        XCTAssertEqual(text, "answer")
    }
}
