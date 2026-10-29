import XCTest
@testable import Arbos

/// [feedback arbonites-blank-chat-ios-2026-10-28]: Jacob's iPhone showed the Arbonites group
/// (3 020 messages) as a header and a composer, no rows and no spinner, while the relay had the
/// device caught up. Every path that can publish an empty transcript over a doc that has rows
/// now reads row by row, keeps the rows it can, and shows the spinner until rows are on screen.
final class BlankTranscriptGuardTests: XCTestCase {
    private let chat = "01M5MHDEZFB5SS6VRK9T12NVAC"

    private func row(_ id: String, _ text: String, sender: String = "user:gh_1") -> Message? {
        DocJSON.message(["id": id, "role": "user", "sender": sender, "status": "done", "createdAt": 1_793_230_798_136,
                         "parts": [["id": "\(id)-p", "kind": "text", "text": text]]], chatId: chat)
    }

    /// The FFI answers `"[]"` when it cannot serialise the list; a cut or garbled payload does
    /// not parse. Either way the rows that do read come back, and only the bad one is left out.
    func testUnreadableListFallsBackToRowsAndSkipsOnlyTheBadRow() {
        let rows: [Int: Message?] = [0: row("a", "Time to move full time to Arbos"), 1: nil, 2: row("c", "AR???")]
        for raw in ["[]", "[{\"id\":\"a\",", "not json"] {
            let out = CoreChatDoc.decodeRows(raw, count: 3, chatId: chat) { rows[$0] ?? nil }
            XCTAssertEqual(out.map(\.id), ["a", "c"], "payload \(raw)")
        }
    }

    /// Row shapes from the Arbonites doc: a blank sender-`agent` row (#2247), a row with no id,
    /// and a row that is not an object. The good rows around them still read.
    func testOffendingRowShapesDoNotDropTheirNeighbours() {
        let raw = """
        [{"id":"01M627S7RQ1G77SNHT71QZ9VAS","role":"assistant","sender":"agent","status":"done","parts":[]},
         {"role":"user","sender":"user:gh_1","parts":[{"kind":"text","text":"no id"}]},
         "glued",
         {"id":"01M62E4Z9NFB0G8J0QYJDN720C","role":"user","sender":"user:gh_295543220","status":"done",
          "replyTo":"01M629TTT1PRZKNTDN5FAKAZJT","parts":[{"id":"p","kind":"text","text":"AR???"}]}]
        """
        let out = CoreChatDoc.decodeRows(raw, count: 4, chatId: chat) { _ in nil }
        XCTAssertEqual(out.map(\.id), ["01M627S7RQ1G77SNHT71QZ9VAS", "01M62E4Z9NFB0G8J0QYJDN720C"])
    }

    func testRealDocReadsTheSameRowByRowAsInOneCall() throws {
        let doc = CoreChatDoc(key: chat)
        for i in 0..<5 {
            try doc.insertUserMessage(id: "m\(i)", text: "row \(i)", deviceId: "ios:test",
                                      author: .init(sender: "user:gh_\(i % 2)", handle: "h\(i % 2)"),
                                      replyTo: i == 4 ? "m1" : nil)
        }
        doc.commit()
        let whole = doc.readAll().map(\.id)
        let fallback = CoreChatDoc.decodeRows("[]", count: doc.count, chatId: chat, row: doc.readMessage).map(\.id)
        XCTAssertEqual(whole, ["m0", "m1", "m2", "m3", "m4"])
        XCTAssertEqual(fallback, whole)
    }

    func testAnEmptyReadNeverReplacesADocWithRows() {
        XCTAssertFalse(AppModel.shouldPublishSnapshot(rows: 0, docCount: 3_020))
        XCTAssertTrue(AppModel.shouldPublishSnapshot(rows: 0, docCount: 0))
        XCTAssertTrue(AppModel.shouldPublishSnapshot(rows: 3_019, docCount: 3_020))
    }

    /// A saved copy that reads back blank is not resumed at its cursor (6120 = Arbonites head):
    /// the relay would send nothing and the chat would stay blank.
    func testABlankStoredCopyIsRefetched() {
        XCTAssertTrue(AppModel.storedCopyIsBlank(rows: 0, docCount: 3_020, cursor: 6_120))
        XCTAssertTrue(AppModel.storedCopyIsBlank(rows: 0, docCount: 0, cursor: 6_120))
        XCTAssertFalse(AppModel.storedCopyIsBlank(rows: 3_020, docCount: 3_020, cursor: 6_120))
        XCTAssertFalse(AppModel.storedCopyIsBlank(rows: 0, docCount: 0, cursor: 0))
    }

    /// The state Jacob saw: a people group with an empty published transcript over a doc that
    /// has rows. It is not "known empty" (no blank screen), it counts as loading (spinner), and a
    /// re-read paints the rows.
    @MainActor
    func testABlankPeopleChatShowsTheSpinnerAndRereads() {
        let model = AppModel()
        model.loadFixture()
        let chatId = "fx-group-launch"
        model.fixtureAttachDoc(chatId: chatId)
        let shown = model.messages(of: chatId).count
        XCTAssertGreaterThan(shown, 0)
        XCTAssertFalse(model.transcriptPending(chatId))

        model.messages[chatId] = []
        XCTAssertTrue(model.transcriptPending(chatId))
        XCTAssertFalse(model.hasKnownEmptyTranscript(chatId))

        model.repaintIfBlank(chatId)
        XCTAssertEqual(model.messages(of: chatId).count, shown)
        XCTAssertFalse(model.transcriptPending(chatId))
    }

    /// An optimistic new chat (no doc rows) keeps its empty hint, not a spinner.
    @MainActor
    func testANewEmptyChatIsStillKnownEmpty() {
        let model = AppModel()
        model.messages["new-chat"] = []
        XCTAssertTrue(model.hasKnownEmptyTranscript("new-chat"))
        XCTAssertFalse(model.transcriptPending("new-chat"))
    }
}
