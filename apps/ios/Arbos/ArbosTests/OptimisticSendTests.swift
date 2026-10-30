import XCTest
@testable import Arbos

/// The own bubble is in the chat the moment Send returns — before the doc's change
/// event, the coalesced re-read or the relay ACK (iphone-group-send-instant-2026-10-25,
/// Jacob's "my message takes 5+ seconds to appear in group chats").
@MainActor
final class OptimisticSendTests: XCTestCase {
    private func fixtureModel(chatId: String) -> AppModel {
        let model = AppModel()
        model.loadFixture()
        model.fixtureAttachDoc(chatId: chatId)
        return model
    }

    private func rows(_ model: AppModel, _ chatId: String, multiParty: Bool) -> [Row] {
        Transcript.rows(model.messages(of: chatId), projectId: "\(peopleSlot):\(chatId)", me: model.myUserId,
                        people: true, multiParty: multiParty)
    }

    func testGroupSendPaintsOwnRowBeforeReturning() throws {
        let chatId = "fx-group-launch"
        let model = fixtureModel(chatId: chatId)
        let before = model.messages(of: chatId)
        let version = model.messagesVersion[chatId] ?? 0
        XCTAssertGreaterThan(before.count, 0, "the fixture group has history")

        model.sendPeople(chatId: chatId, text: "launch call moved to ten", replyTo: nil)

        // No run-loop turn in between: this is what the view sees in the same frame as the tap.
        let after = model.messages(of: chatId)
        XCTAssertEqual(after.count, before.count + 1, "exactly one new row, at once")
        let own = try XCTUnwrap(after.last)
        XCTAssertEqual(own.role, .user)
        XCTAssertEqual(own.sender, "user:fx-me", "my own line carries my sender")
        XCTAssertEqual(own.plainText, "launch call moved to ten")
        XCTAssertTrue(own.pending, "the relay has not ACKed: the bubble is in the pending (dim) state")
        XCTAssertGreaterThan(model.messagesVersion[chatId] ?? 0, version, "the view's change key moved with the paint")

        // The transcript builder turns it into my bubble, pending, at the tail.
        let built = rows(model, chatId, multiParty: true)
        let last = try XCTUnwrap(built.last)
        XCTAssertEqual(last.messageId, own.id)
        guard case .user(let text, let delivery, _, _) = last.kind else {
            return XCTFail("tail row is \(last.kind), expected my bubble")
        }
        XCTAssertEqual(text, "launch call moved to ten")
        XCTAssertEqual(delivery, .pending)
    }

    func testOneToOneSendPaintsOwnRowBeforeReturning() throws {
        let chatId = "fx-dm-unconst"
        let model = fixtureModel(chatId: chatId)
        let before = model.messages(of: chatId).count

        model.sendPeople(chatId: chatId, text: "on my way", replyTo: nil)

        let after = model.messages(of: chatId)
        XCTAssertEqual(after.count, before + 1)
        XCTAssertEqual(after.last?.plainText, "on my way")
        XCTAssertEqual(after.last?.pending, true)
    }

    func testPaintedRowSurvivesTheDocReadWithoutADuplicate() throws {
        let chatId = "fx-group-launch"
        let model = fixtureModel(chatId: chatId)
        let before = model.messages(of: chatId).count

        model.sendPeople(chatId: chatId, text: "and the deck is in the drive", replyTo: nil)
        let painted = try XCTUnwrap(model.messages(of: chatId).last)

        // Let the doc's change event, the coalesce window and the re-read run.
        let deadline = Date().addingTimeInterval(1.0)
        while Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.02)) }

        let after = model.messages(of: chatId)
        XCTAssertEqual(after.count, before + 1, "the re-read replaces the painted row, it does not add a second one")
        XCTAssertEqual(after.filter { $0.id == painted.id }.count, 1)
        let own = try XCTUnwrap(after.last)
        XCTAssertEqual(own.id, painted.id, "same id before and after the re-read, so the bubble keeps its identity")
        XCTAssertEqual(own.plainText, painted.plainText)
        XCTAssertEqual(own.sender, painted.sender)
        XCTAssertEqual(own.createdAt, painted.createdAt, "the painted row is the doc's row, not a guess")
        XCTAssertEqual(own.parts, painted.parts, "part ids match so attachment rows keep their identity")
        XCTAssertTrue(own.pending, "still pending: no ACK arrived")
    }
}
