import XCTest
@testable import Arbos

/// Grok parity, hide the plumbing (Jacob 2026-11-06): a routine's prompt reaches the chat as a
/// **user-role** message whose sender is `routine:<id>`, so it inherited every user-bubble rule
/// and a long `Tell the user (reply_to 01M…)` drew as a slab. It collapses to one quiet line
/// naming the routine, its own words are not drawn, and a reply that quotes it names the routine
/// instead of repeating the prompt.
final class RoutinePromptRowTests: XCTestCase {
    private let chat = "fx-comms-main"
    private let t0: Int64 = 1_700_000_000_000

    private let prompt = "Tell the user (reply_to 01M6N21WFJQ92GJX6QF1ZQ8K3W) what changed in the "
        + "repository since yesterday. Do not greet them."

    /// Built through `DocJSON.message` rather than the memberwise initialiser, so the decode of
    /// `routineName` — which this change added and the client had never read — is under test too.
    private func routinePrompt(_ id: Id, name: String?, at: Int64) -> Message {
        var raw: [String: Any] = [
            "id": id, "role": "user", "sender": "routine:rt_7KQ2", "status": "done",
            "createdAt": at,
            "parts": [["id": "\(id)-t", "kind": "text", "text": prompt]],
        ]
        if let name { raw["routineName"] = name }
        return DocJSON.message(raw, chatId: chat)!
    }

    private func reply(_ id: Id, to: Id, at: Int64) -> Message {
        DocJSON.message([
            "id": id, "role": "assistant", "sender": "agent:doug", "status": "done",
            "inReplyTo": to, "createdAt": at,
            "parts": [["id": "\(id)-t", "kind": "text", "text": "Three merges landed."]],
        ], chatId: chat)!
    }

    func testARoutinePromptIsOneQuietLineAndNeverABubble() {
        let rows = Transcript.rows(
            [routinePrompt("r1", name: "Morning stand-up", at: t0), reply("a1", to: "r1", at: t0 + 1_000)],
            projectId: "fx-comms")

        let labels: [String] = rows.compactMap {
            if case .routinePrompt(let label) = $0.kind { return label }
            return nil
        }
        XCTAssertEqual(labels, ["Routine · Morning stand-up"])

        XCTAssertFalse(rows.contains { row in
            guard row.messageId == "r1" else { return false }
            if case .user = row.kind { return true }
            return false
        }, "the prompt must not keep a bubble")

        XCTAssertFalse(rows.contains { row in
            if case .user(let text, _, _, _) = row.kind { return text.contains("reply_to") }
            return false
        }, "the prompt's own words are not drawn anywhere")
    }

    func testAReplyQuotesTheRoutineByNameNotThePrompt() {
        let messages = [routinePrompt("r1", name: "Morning stand-up", at: t0)]
        let quote = Quote.forReply("r1", in: messages)
        XCTAssertEqual(quote.text, "Routine · Morning stand-up")
        XCTAssertFalse(quote.text.contains("reply_to"), "the header must not repeat the plumbing")
    }

    /// `routineName` is written by the relay beside the sender and is absent on rows older than
    /// the field. The line still has to say something true.
    func testANamelessRoutineFallsBackToItsId() {
        XCTAssertEqual(routinePrompt("r1", name: nil, at: t0).routinePromptLabel, "Routine · rt_7KQ2")
        XCTAssertEqual(routinePrompt("r2", name: "   ", at: t0).routinePromptLabel, "Routine · rt_7KQ2",
                       "a whitespace name is no name")
        XCTAssertEqual(routinePrompt("r3", name: "Morning stand-up", at: t0).routineName, "Morning stand-up",
                       "the field decodes at all")
    }

    func testAnOrdinaryUserMessageIsUntouched() {
        let mine = DocJSON.message([
            "id": "u1", "role": "user", "sender": "user", "status": "done", "createdAt": t0,
            "parts": [["id": "u1-t", "kind": "text", "text": "what changed today?"]],
        ], chatId: chat)!
        XCTAssertNil(mine.routinePromptLabel)

        let rows = Transcript.rows([mine], projectId: "fx-comms")
        XCTAssertTrue(rows.contains { if case .user = $0.kind { return true } else { return false } },
                      "a person's message keeps its bubble")
        XCTAssertFalse(rows.contains { if case .routinePrompt = $0.kind { return true } else { return false } })
    }
}
