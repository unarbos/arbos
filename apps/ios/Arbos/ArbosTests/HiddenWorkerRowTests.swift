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
}
