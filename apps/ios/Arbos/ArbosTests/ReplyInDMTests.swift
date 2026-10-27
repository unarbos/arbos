import XCTest
@testable import Arbos

/// Reply in DM (#1836, Jacob 2026-10-25: "the DM reply shows no context"): a DM row that
/// answers a message in another chat carries `replyRef` — its address plus a snapshot — and
/// the transcript draws the same quote header as an in-chat reply plus "Replying to <who> in
/// <chat>". The header links to the original only when the viewer is a member of that chat.
@MainActor
final class ReplyInDMTests: XCTestCase {
    private let dm = "fx-dm-unconst"
    private let group = "fx-group-launch"

    private func fixtureModel() -> AppModel {
        let model = AppModel()
        model.loadFixture()
        model.fixtureAttachDoc(chatId: dm)
        return model
    }

    private func groupRow(_ model: AppModel, id: String) -> Message {
        model.messages(of: group).first { $0.id == id }!
    }

    private func dmRows(_ model: AppModel) -> [Row] {
        Transcript.rows(model.messages(of: dm), projectId: "\(peopleSlot):\(dm)", me: model.myUserId,
                        people: true, multiParty: false)
    }

    func testReplyRefCarriesAddressAndSnapshot() {
        let model = fixtureModel()
        let original = groupRow(model, id: "g1")
        let ref = model.replyRef(for: original, in: group, projectId: nil)
        XCTAssertEqual(ref.chatId, group)
        XCTAssertEqual(ref.projectId, "people")
        XCTAssertEqual(ref.messageId, "g1")
        XCTAssertEqual(ref.chatTitle, "Launch week")
        XCTAssertEqual(ref.sender, "user:fx-u3")
        XCTAssertEqual(ref.senderName, "Mira Kaplan")
        XCTAssertEqual(ref.senderHandle, "mira-k")
        XCTAssertEqual(ref.text, "Launch checklist is here: https://notion.so/arbos/launch-week")
        XCTAssertEqual(ref.label, "Replying to Mira Kaplan in Launch week")
        // An attachment-only original names its file; a long one is cut on a word.
        let file = model.replyRef(for: groupRow(model, id: "g2"), in: group, projectId: nil)
        XCTAssertEqual(file.text, "press-kit-v3.pdf")
        let long = ReplyRef.snapshot(String(repeating: "word ", count: 100))
        XCTAssertTrue(long.hasSuffix("…") && long.count <= ReplyRef.snapshotChars + 1)
        // Parsing a row: well-formed → ref; no message id → nil.
        XCTAssertNotNil(ReplyRef(row: ["chatId": "c", "messageId": "m"] as [String: Any]))
        XCTAssertNil(ReplyRef(row: ["chatId": "c"] as [String: Any]))
    }

    func testDMSendWritesReplyRefAndTheBubbleQuotesWithChatName() {
        let model = fixtureModel()
        let ref = model.replyRef(for: groupRow(model, id: "g1"), in: group, projectId: nil)

        model.sendPeople(chatId: dm, text: "Works", replyTo: "d1", replyRef: ref)

        let sent = model.messages(of: dm).last!
        XCTAssertEqual(sent.plainText, "Works")
        XCTAssertEqual(sent.replyRef, ref, "the row carries the cross-chat reference")
        XCTAssertNil(sent.replyTo, "a cross-chat reply never carries an in-chat replyTo as well")

        guard case .user(_, _, let quote?)? = dmRows(model).last?.kind else {
            return XCTFail("the sent bubble carries a quote: \(String(describing: dmRows(model).last?.kind))")
        }
        XCTAssertEqual(quote.messageId, "g1")
        XCTAssertEqual(quote.text, "Launch checklist is here: https://notion.so/arbos/launch-week")
        XCTAssertEqual(quote.source?.label, "Replying to Mira Kaplan in Launch week")
        XCTAssertNil(quote.forwardedFrom)
    }

    func testQuoteLinksOnlyWhenTheViewerIsInTheSourceChat() {
        let model = fixtureModel()
        XCTAssertTrue(model.isMember(ofChat: group), "I am in Launch week")
        XCTAssertFalse(model.isMember(ofChat: "fx-not-my-chat"))

        let mine = model.replyRef(for: groupRow(model, id: "g1"), in: group, projectId: nil)
        XCTAssertTrue(model.reveal(mine), "a member's tap opens the source chat on the original")
        XCTAssertEqual(model.pendingReveal, MessageReveal(chatId: group, messageId: "g1"))
        if case .chat(let chatId, let projectId)? = model.path.last {
            XCTAssertEqual(chatId, group)
            XCTAssertEqual(projectId, peopleSlot)
        } else {
            XCTFail("navigation went to the group chat: \(model.path)")
        }

        model.pendingReveal = nil
        model.path = [.chat(chatId: dm, projectId: peopleSlot)]
        var outside = mine
        outside.chatId = "fx-not-my-chat"
        outside.chatTitle = "Design crit"
        XCTAssertFalse(model.reveal(outside), "a non-member sees the quote without the link")
        XCTAssertNil(model.pendingReveal)
        if case .chat(let chatId, _)? = model.path.last { XCTAssertEqual(chatId, dm) } else { XCTFail("stayed in the DM") }
    }

    func testReplyInDMArmsTheDMComposer() async {
        let model = fixtureModel()
        let original = groupRow(model, id: "g1")
        model.replyInDM(to: original, in: group, projectId: nil)
        // openDM is async even in the fixture; give the task a turn.
        for _ in 0..<20 where model.pendingDMReply == nil {
            try? await Task.sleep(for: .milliseconds(20))
        }
        let armed = model.pendingDMReply
        XCTAssertNotNil(armed, "the DM is opened with the cross-chat reply pending")
        XCTAssertEqual(armed?.ref.messageId, "g1")
        XCTAssertEqual(armed?.ref.chatTitle, "Launch week")
        // My own row and an agent's have no peer to DM.
        model.pendingDMReply = nil
        model.replyInDM(to: groupRow(model, id: "g2").with(sender: "user:fx-me"), in: group, projectId: nil)
        try? await Task.sleep(for: .milliseconds(50))
        XCTAssertNil(model.pendingDMReply)
    }
}

private extension Message {
    func with(sender: String) -> Message {
        var m = self
        m.sender = sender
        return m
    }
}
