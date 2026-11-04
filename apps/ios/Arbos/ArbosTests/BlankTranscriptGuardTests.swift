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

    /// A row nested deeper than JSONSerialization's limit (~512; 400 parses, 600 does not) makes
    /// the whole list fail to parse though the core serialises it fine. On the TF 2048 build a
    /// copy of the real Arbonites doc with one 700-deep row appended drew a blank chat with no
    /// spinner. Now the rows around it read and only that row is left out.
    func testAOverlyDeepRowIsSkippedNotTheWholeTranscript() {
        var deep = "1"
        for _ in 0..<700 { deep = "{\"k\":" + deep + "}" }
        let good = { (id: String, text: String) in
            "{\"id\":\"\(id)\",\"role\":\"user\",\"sender\":\"user:gh_1\",\"status\":\"done\",\"parts\":[{\"id\":\"\(id)-p\",\"kind\":\"text\",\"text\":\"\(text)\"}]}"
        }
        let deepRow = "{\"id\":\"01M62ZZCORRUPTROWDEEP000000\",\"role\":\"user\",\"sender\":\"user:gh_1\",\"meta\":\(deep)}"
        let raw = "[" + [good("a", "Can’t wait!"), deepRow, good("c", "AR???")].joined(separator: ",") + "]"
        XCTAssertNil(try? JSONSerialization.jsonObject(with: Data(raw.utf8)), "premise: the whole list must not parse")

        let perRow = [good("a", "Can’t wait!"), deepRow, good("c", "AR???")]
        let out = CoreChatDoc.decodeRows(raw, count: perRow.count, chatId: chat) { ix in
            (try? JSONSerialization.jsonObject(with: Data(perRow[ix].utf8))).flatMap { DocJSON.message($0, chatId: self.chat) }
        }
        XCTAssertEqual(out.map(\.id), ["a", "c"])
        XCTAssertEqual(out.last?.plainText, "AR???")
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

    // MARK: The lazy stack's identity ([feedback iphone-blank-chat-on-open-2026-11-03])

    private func lead(pending: Bool = false, caughtUp: Bool = true, fixture: Bool = false,
                      messagesEmpty: Bool = false, knownEmpty: Bool = false, rowsEmpty: Bool = false,
                      agentCopy: Bool = true, searchHits: Bool = false,
                      visibleRowsEmpty: Bool = false) -> ChatSession.TranscriptLead {
        ChatSession.transcriptLead(pending: pending, caughtUp: caughtUp, fixture: fixture,
                                   messagesEmpty: messagesEmpty, knownEmpty: knownEmpty,
                                   rowsEmpty: rowsEmpty, agentCopy: agentCopy,
                                   searchHits: searchHits, visibleRowsEmpty: visibleRowsEmpty)
    }

    func testTheLeadChildIsTheOneTheTranscriptDraws() {
        XCTAssertEqual(lead(pending: true), .pending)
        XCTAssertEqual(lead(caughtUp: false, messagesEmpty: true), .catchUp)
        XCTAssertEqual(lead(caughtUp: false, messagesEmpty: true, knownEmpty: true), .empty)
        // A people chat or channel with nothing in it stays blank; it has no lead child at all.
        XCTAssertEqual(lead(caughtUp: false, messagesEmpty: true, knownEmpty: true, agentCopy: false), .rowsOnly)
        XCTAssertEqual(lead(rowsEmpty: true), .empty)
        XCTAssertEqual(lead(rowsEmpty: true, agentCopy: false), .rowsOnly)
        XCTAssertEqual(lead(searchHits: true, visibleRowsEmpty: true), .noMatches)
        XCTAssertEqual(lead(), .rowsOnly)
        // The fixture is always caught up, which is why no rig open ever showed the empty hint.
        XCTAssertEqual(lead(caughtUp: false, fixture: true, messagesEmpty: true), .rowsOnly)
    }

    /// The reported open: an agent chat whose rows land while its empty hint is on screen. The
    /// hint leaves and the window's rows arrive in one step, so the stack must be a new one.
    func testRowsArrivingUnderTheEmptyHintRekeysTheStack() {
        let before = ChatSession.transcriptStackIdentity(lead: .empty, banner: false, query: nil)
        let after = ChatSession.transcriptStackIdentity(lead: .rowsOnly, banner: false, query: nil)
        XCTAssertNotEqual(before, after)
    }

    /// Every other transition into and out of a lead child, and the waking banner.
    func testEveryLeadChildComingOrGoingRekeysTheStack() {
        let leads: [ChatSession.TranscriptLead] = [.rowsOnly, .pending, .catchUp, .empty, .noMatches]
        var seen: Set<String> = []
        for l in leads {
            for banner in [false, true] {
                let id = ChatSession.transcriptStackIdentity(lead: l, banner: banner, query: nil)
                XCTAssertTrue(seen.insert(id).inserted, "\(l) banner=\(banner) shares a key with another state")
            }
        }
    }

    /// The hot path: rows up, no keyword, no banner. Nothing a streaming token or a window grow
    /// step changes reaches the key, so the transcript never rebuilds while it is being read.
    func testTheKeyIsStillConstantWithTheRowsUp() {
        let id = ChatSession.transcriptStackIdentity(lead: .rowsOnly, banner: false, query: nil)
        XCTAssertEqual(id, ChatSession.transcriptStackIdentity(lead: .rowsOnly, banner: false, query: nil))
        XCTAssertEqual(id, "all:rowsOnly")
    }

    /// The shape — what the open-path re-settle watches — must not move with the keyword. It
    /// does not: leaving a keyword is `closeSearch`'s business, and a result tap closes search
    /// in order to land on its own message, which the tail is the last place for.
    func testTheShapeIsBlindToTheKeyword() {
        XCTAssertEqual(ChatSession.transcriptStackShape(lead: .rowsOnly, banner: false), "rowsOnly")
        XCTAssertNotEqual(ChatSession.transcriptStackShape(lead: .rowsOnly, banner: false),
                          ChatSession.transcriptStackShape(lead: .noMatches, banner: false))
        XCTAssertNotEqual(ChatSession.transcriptStackShape(lead: .rowsOnly, banner: false),
                          ChatSession.transcriptStackShape(lead: .rowsOnly, banner: true))
    }

    /// A keyword still gets its own stack per keyword (#2450), lead child and all.
    func testAKeywordStillKeysTheStack() {
        let all = ChatSession.transcriptStackIdentity(lead: .rowsOnly, banner: false, query: nil)
        let hit = ChatSession.transcriptStackIdentity(lead: .rowsOnly, banner: false, query: "deck")
        let other = ChatSession.transcriptStackIdentity(lead: .rowsOnly, banner: false, query: "launch")
        let none = ChatSession.transcriptStackIdentity(lead: .noMatches, banner: false, query: "zzz")
        XCTAssertEqual(Set([all, hit, other, none]).count, 4)
    }
}
