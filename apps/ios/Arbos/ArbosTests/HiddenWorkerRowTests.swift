import XCTest
@testable import Arbos

/// Grok parity trait 15 (feedback `hide-worker-rows-and-tz-2026-10-24`): agentd marks the spawn row of
/// a background subagent the user never named with `worker: true`. The transcript draws nothing for
/// it — Grok shows no "N messages with …" line for its own workers — while an agent-to-agent send
/// (no flag) keeps its "Messaged ◉ X" line, and the worker row's `activity` still reaches the
/// working line.
final class HiddenWorkerRowTests: XCTestCase {
    private let chat = "fx-comms-main"
    private let t0: Int64 = 1_700_000_000_000

    private func spawn(_ id: Id, name: String, worker: Bool, at: Int64) -> Message {
        Message(id: id, chatId: chat, role: .assistant, sender: "agent", status: .done, turn: .spawn, decision: "new",
                subagentId: "sub-\(id)", subagentName: name, handoffMessages: 2, worker: worker, inReplyTo: nil, usage: [:],
                model: nil, createdAt: at, completedAt: at + 5_000, pending: false,
                parts: [.status(id: "\(id)-s", code: .handoff, message: "Handed to subagent \(name)")], hiddenCount: 2,
                activity: "Searching for surf schools Santa Teresa")
    }

    private func systemLines(_ rows: [Row]) -> [String] {
        rows.compactMap { row in
            // The first rendered message always carries a date separator ("Nov 14, 2023 at 10:13 PM"),
            // also a `.system` row; the assertion is about hand-off lines, not dates.
            if row.id.hasSuffix("#date") { return nil }
            if case .system(_, let text, _) = row.kind { return text }
            return nil
        }
    }

    func testAWorkerSpawnRowDrawsNothingButATeammateSendKeepsItsLine() {
        let messages = [
            spawn("w1", name: "santa-teresa-surf-schools", worker: true, at: t0),
            spawn("t1", name: "Comms", worker: false, at: t0 + 1_000),
        ]
        let rows = Transcript.rows(messages, projectId: "fx-comms")
        XCTAssertFalse(rows.contains { $0.messageId == "w1" }, "no row at all for the worker: \(rows.map(\.id))")
        XCTAssertEqual(systemLines(rows), [Transcript.handoffText([("Comms", 2)])])
    }

    func testWorkerFlagParsesAndDefaultsOff() {
        let flagged: [String: Any] = ["id": "w1", "role": "assistant", "sender": "agent", "status": "done", "turn": "spawn",
                                      "subagentName": "x", "worker": true, "createdAt": t0, "parts": []]
        let plain: [String: Any] = ["id": "t1", "role": "assistant", "sender": "agent", "status": "done", "turn": "spawn",
                                    "subagentName": "y", "createdAt": t0, "parts": []]
        XCTAssertTrue(DocJSON.message(flagged, chatId: chat)?.worker ?? false)
        XCTAssertFalse(DocJSON.message(plain, chatId: chat)?.worker ?? true)
    }

    /// Arbonites, 2026-10-28 (01M627S7…, 01M627VG…): a box wrote a hand-off row and a job row
    /// into a group with a bare `sender: "agent"` and no words. A people chat draws neither — no
    /// "Messaged ◉ Bob" from no one, no empty bubble; the same row with the agent's sender, or in
    /// a project chat, keeps its hand-off line.
    func testASenderlessBlankRowDrawsNothingInAPeopleChat() {
        func row(_ id: String, sender: String, role: String = "assistant", at: Int64, extra: [String: Any] = [:]) -> Message {
            var json: [String: Any] = ["id": id, "role": role, "sender": sender, "status": "done", "createdAt": at,
                                       "parts": [["id": "\(id)-t", "kind": "text", "text": ""]]]
            json.merge(extra) { _, new in new }
            return DocJSON.message(json, chatId: chat)!
        }
        let handoff: [String: Any] = ["turn": ["kind": "spawn"], "subagentName": "Bob", "handoffMessages": 1]
        var morning = row("m1", sender: "user:gh_bob", role: "user", at: t0)
        morning.parts = [.text(id: "m1-t", text: "morning")]
        let messages = [
            morning,
            row("h1", sender: "agent", at: t0 + 1_000, extra: handoff),
            row("u1", sender: "", role: "user", at: t0 + 1_500),
        ]
        XCTAssertTrue(messages[1].isSenderlessBlank)
        XCTAssertTrue(messages[2].isSenderlessBlank)
        XCTAssertFalse(messages[0].isSenderlessBlank)
        let group = Transcript.rows(messages, projectId: "people", me: "user:gh_me", people: true, multiParty: true)
        XCTAssertFalse(group.contains { $0.messageId == "h1" || $0.messageId == "u1" }, "\(group.map(\.id))")
        XCTAssertTrue(group.contains { $0.messageId == "m1" })

        let named = row("h2", sender: "agent:p-bubble", at: t0, extra: handoff)
        XCTAssertFalse(named.isSenderlessBlank)
        let attributed = Transcript.rows([named], projectId: "people", me: "user:gh_me", people: true, multiParty: true)
        XCTAssertEqual(systemLines(attributed), [Transcript.handoffText([("Bob", 1)])])
        let project = Transcript.rows([messages[1]], projectId: "fx-comms")
        XCTAssertEqual(systemLines(project), [Transcript.handoffText([("Bob", 1)])])
    }
}
