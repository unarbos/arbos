import XCTest
@testable import Arbos

/// A long group chat must decode in linear time. Before 2026-10-23 `readAll`
/// crossed the FFI once per row and each crossing projected the whole doc, so a
/// 2 000-row transcript took ~12 s per snapshot in release and the iPhone lagged
/// on every backfilled row (long-chat-perf).
final class LongTranscriptPerfTests: XCTestCase {
    private func longTranscript(rows: Int) throws -> CoreChatDoc {
        let doc = CoreChatDoc(key: "perf-long")
        for i in 0..<rows {
            try doc.insertUserMessage(
                id: "m\(i)", text: "row \(i): a line of chat with a few words in it", deviceId: "dev",
                author: .init(sender: i % 3 == 0 ? "user:me" : "user:other", handle: i % 3 == 0 ? "me" : "other"),
                replyTo: i % 10 == 0 && i > 0 ? "m\(i - 1)" : nil
            )
        }
        doc.commit()
        return doc
    }

    func testReadAllOfTwoThousandRowsIsFast() throws {
        let rows = 2_000
        let doc = try longTranscript(rows: rows)
        XCTAssertEqual(doc.count, rows)

        let started = Date()
        let all = doc.readAll()
        let elapsed = Date().timeIntervalSince(started)

        XCTAssertEqual(all.count, rows)
        XCTAssertEqual(all.first?.id, "m0")
        XCTAssertEqual(all.last?.id, "m\(rows - 1)")
        XCTAssertEqual(all[10].replyTo, "m9")
        // Generous for a Debug simulator build; the quadratic path was tens of seconds.
        XCTAssertLessThan(elapsed, 2.0, "readAll of \(rows) rows took \(elapsed) s")
    }

    /// The row builder runs on every transcript change (each streaming delta included):
    /// 5 000 mixed messages — people, markdown answers, relay lines, replies, media —
    /// must build in well under a second, cold and warm. Replies used to scan the whole
    /// list each (`Quote.forReply`), and every answer was re-parsed on every rebuild.
    func testRowsForFiveThousandMessagesBuildFast() {
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        let messages = AppModel.longGroupTranscript(rows: 5_000, before: now)
        XCTAssertEqual(messages.count, 5_000)

        let cold = Date()
        let rows = Transcript.rows(messages, projectId: "people:fx-group-launch", me: "user:fx-me", people: true)
        let coldS = Date().timeIntervalSince(cold)

        let warm = Date()
        let again = Transcript.rows(messages, projectId: "people:fx-group-launch", me: "user:fx-me", people: true)
        let warmS = Date().timeIntervalSince(warm)

        XCTAssertEqual(rows, again)
        XCTAssertGreaterThan(rows.count, 5_000, "replies, date lines and media add rows")
        XCTAssertTrue(rows.contains { if case .user(_, _, let q, _) = $0.kind { return q != nil } else { return false } }, "my replies carry a quote")
        XCTAssertTrue(rows.contains { if case .member(let blocks, _, _, _, _, _, _, _) = $0.kind { return blocks.count > 1 } else { return false } }, "markdown answers split into blocks")
        XCTAssertLessThan(coldS, 3.0, "cold rows() over 5 000 messages took \(coldS) s")
        XCTAssertLessThan(warmS, 1.0, "warm rows() over 5 000 messages took \(warmS) s")
    }

    func testSingleRowReadMatchesReadAll() throws {
        let rows = 300
        let doc = try longTranscript(rows: rows)
        let all = doc.readAll()
        for ix in [0, 1, rows / 2, rows - 1] {
            XCTAssertEqual(doc.readMessage(ix), all[ix])
        }
        XCTAssertNil(doc.readMessage(rows))
    }
}
