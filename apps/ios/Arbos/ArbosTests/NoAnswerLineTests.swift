import XCTest
@testable import Arbos

/// Jacob, "run bubble sort" (Mac, 2026-10-25 18:08 UTC, image 0.1.132): the answer went out as a
/// `send_message` bubble; the fresh result row that bubble opened ended wordless and Done, and the
/// transcript drew "The agent didn't answer. Ask again." under the answer. The stock line is for a
/// turn that said nothing: a wordless finished row whose turn (same `inReplyTo`) has another visible
/// assistant row draws nothing; a turn with no words at all still gets the line.
final class NoAnswerLineTests: XCTestCase {
    private let chat = "fx-main"
    private let t0: Int64 = 1_700_000_000_000

    private func reply(_ id: Id, ask: String, text: String, at: Int64) -> Message {
        let parts: [Part] = text.isEmpty
            ? [.text(id: "\(id)-t", text: ""), .status(id: "\(id)-s", code: .working, message: "Working…")]
            : [.text(id: "\(id)-t", text: text)]
        return Message(id: id, chatId: chat, role: .assistant, sender: "agent", status: .done, turn: nil, decision: "reply",
                       subagentId: nil, subagentName: nil, inReplyTo: ask, usage: [:], model: nil,
                       createdAt: at, completedAt: at + 2_000, pending: false, parts: parts, hiddenCount: 0)
    }

    private func stockLines(_ rows: [Row]) -> [String] {
        rows.compactMap { row in
            if case .assistant(let blocks, _, _, _) = row.kind {
                for block in blocks {
                    if case .paragraph(let text) = block, text.hasSuffix("didn't answer. Ask again.") { return text }
                }
            }
            return nil
        }
    }

    func testNoStockLineWhenTheTurnAnsweredInAnotherRow() {
        let messages = [
            reply("r1", ask: "u1", text: "Sorted: 1, 2, 3, 5, 8.", at: t0),
            reply("r2", ask: "u1", text: "", at: t0 + 1_000),
        ]
        let rows = Transcript.rows(messages, projectId: "fx")
        XCTAssertTrue(stockLines(rows).isEmpty, "no 'didn't answer' under an answer: \(rows.map(\.id))")
        XCTAssertFalse(rows.contains { $0.messageId == "r2" }, "the wordless trailing row draws nothing")
    }

    func testATurnThatSaidNothingKeepsTheStockLine() {
        let rows = Transcript.rows([reply("r3", ask: "u2", text: "", at: t0)], projectId: "fx")
        XCTAssertEqual(stockLines(rows).count, 1, "\(rows.map(\.id))")
    }
}
