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

/// Scout (Arbonites, 2026-10-30): a picture or video whose upload fails disappears, but the
/// caption typed with it is sent on its own — the recipient sees a bare caption and the
/// sender is never told ([feedback caption-sent-without-media]).
///
/// Words and media are one message. A send that carries files is committed to the outbox
/// before its first upload byte leaves and written into the chat document only once every
/// attachment has a relay `ref`, so nothing is posted until all of it can be.
@MainActor
final class OutboxSendTests: XCTestCase {
    private let chatId = "fx-group-launch"

    private func fixtureModel() -> AppModel {
        let model = AppModel()
        model.loadFixture()
        model.fixtureAttachDoc(chatId: chatId)
        return model
    }

    private func clip() -> AppModel.PendingAttachment {
        AppModel.PendingAttachment(name: "clip.mp4", mime: "video/mp4", data: Data(repeating: 7, count: 2_048))
    }

    /// The fixture relay is `http://fixture.invalid`, so every upload fails — which is the
    /// simulated bad connection, with no live relay touched.
    private func settle(until done: () -> Bool) {
        let deadline = Date().addingTimeInterval(20)
        while Date() < deadline, !done() {
            RunLoop.main.run(until: Date().addingTimeInterval(0.02))
        }
    }

    func testAFailedUploadPostsNothingAndKeepsTheWholeMessage() throws {
        let model = fixtureModel()
        let posted = model.messages[chatId]?.count ?? 0

        model.sendPeople(chatId: chatId, text: "look at this", replyTo: nil, attachments: [clip()])

        // The caption did not go out on its own: the chat document is untouched.
        XCTAssertEqual(model.messages[chatId]?.count ?? 0, posted,
                       "nothing is written until the video is on the relay")

        // The sender still has the whole message.
        let own = try XCTUnwrap(model.messages(of: chatId).last)
        XCTAssertTrue(model.isUnsent(own.id), "it is in the outbox, not in the document")
        XCTAssertEqual(own.plainText, "look at this")
        XCTAssertEqual(own.role, .user)
        let named: [String] = own.parts.compactMap {
            if case .attachment(_, let a) = $0 { return a.name } else { return nil }
        }
        XCTAssertEqual(named, ["clip.mp4"], "the video is on the row, with its caption")
        XCTAssertTrue(own.pending, "still uploading")

        // The upload fails against `fixture.invalid`: the row stays and gains its Retry mark.
        settle { model.messages(of: chatId).last?.status == .error }
        let failed = try XCTUnwrap(model.messages(of: chatId).last)
        XCTAssertEqual(failed.id, own.id, "same message, not a replacement")
        XCTAssertEqual(failed.status, .error, "Retry is on it")
        XCTAssertFalse(failed.pending)
        XCTAssertEqual(model.messages[chatId]?.count ?? 0, posted,
                       "and still nothing was posted for anyone else")
    }

    func testACaptionLessFailedUploadIsStillARowTheSenderCanSee() throws {
        let model = fixtureModel()
        model.sendPeople(chatId: chatId, text: "", replyTo: nil, attachments: [clip()])
        settle { model.messages(of: chatId).last?.status == .error }

        let own = try XCTUnwrap(model.messages(of: chatId).last)
        XCTAssertTrue(model.isUnsent(own.id))
        XCTAssertEqual(own.parts.count, 1, "no empty text part: \(own.parts)")
        guard case .attachment = own.parts[0] else {
            return XCTFail("expected the video, got \(own.parts[0])")
        }
    }

    func testDeleteOnAnUnsentMessageTakesItAndItsBytesAndNothingElse() throws {
        let model = fixtureModel()
        let posted = model.messages[chatId]?.count ?? 0
        model.sendPeople(chatId: chatId, text: "look at this", replyTo: nil, attachments: [clip()])
        let own = try XCTUnwrap(model.messages(of: chatId).last)
        settle { model.messages(of: chatId).last?.status == .error }

        model.deleteMessage(chatId: chatId, messageId: own.id)

        XCTAssertFalse(model.isUnsent(own.id))
        XCTAssertNil(model.messages(of: chatId).first { $0.id == own.id }, "gone from the chat")
        XCTAssertEqual(model.messages[chatId]?.count ?? 0, posted, "the document never had it")
    }

    func testRetrySendsTheSameMessageAgainRatherThanAnEmptyOne() throws {
        let model = fixtureModel()
        model.sendPeople(chatId: chatId, text: "look at this", replyTo: nil, attachments: [clip()])
        let own = try XCTUnwrap(model.messages(of: chatId).last)
        settle { model.messages(of: chatId).last?.status == .error }

        model.resendFailed(chatId: chatId, messageId: own.id)

        // Retry runs the upload again under the same id: nothing is removed from the relay
        // first, because nothing was ever posted, and the words are still with the video.
        let trying = try XCTUnwrap(model.messages(of: chatId).last)
        XCTAssertEqual(trying.id, own.id)
        XCTAssertTrue(trying.pending, "it reads as sending again")
        XCTAssertEqual(trying.plainText, "look at this")
        XCTAssertTrue(model.isUnsent(own.id), "and it is still the sender's alone")

        // `fixture.invalid` refuses this attempt too, so it comes to rest where it was.
        settle { model.messages(of: chatId).last?.status == .error }
        XCTAssertEqual(model.messages(of: chatId).last?.id, own.id)
        XCTAssertEqual(model.messages(of: chatId).filter { $0.id == own.id }.count, 1,
                       "one row, not a second copy")
    }

    func testTheOutboxSurvivesTheAppGoingAway() throws {
        let relay = "http://fixture.invalid"
        let account = "outbox-test-\(UUID().uuidString)"
        defer { OutboxStore.wipe(relayUrl: relay, account: account) }

        let store = OutboxStore(relayUrl: relay, account: account)
        let staged = try XCTUnwrap(store.stage(clip(), of: "m1", at: 0, kind: "video"))
        store.save([OutboxMessage(id: "m1", chatId: chatId, kind: .people, text: "look at this",
                                  attachments: [staged], createdAt: 1_700_000_000_000)])

        // A second launch reads it back — bytes and all, so Retry has something to upload.
        let reopened = OutboxStore(relayUrl: relay, account: account).load()
        XCTAssertEqual(reopened.count, 1, "the message is still here after the app went away")
        XCTAssertEqual(reopened[0].text, "look at this")
        XCTAssertEqual(reopened[0].attachments.map(\.name), ["clip.mp4"])
        XCTAssertEqual(reopened[0].attachments[0].bytes, 2_048)
        XCTAssertEqual(OutboxStore(relayUrl: relay, account: account)
            .bytes("m1", reopened[0].attachments[0])?.count, 2_048)
    }

    func testAnEntryWhoseBytesAreGoneIsNotOfferedForRetry() throws {
        let relay = "http://fixture.invalid"
        let account = "outbox-test-\(UUID().uuidString)"
        defer { OutboxStore.wipe(relayUrl: relay, account: account) }

        let store = OutboxStore(relayUrl: relay, account: account)
        let staged = try XCTUnwrap(store.stage(clip(), of: "m1", at: 0, kind: "video"))
        store.save([OutboxMessage(id: "m1", chatId: chatId, kind: .people, text: "look at this",
                                  attachments: [staged], createdAt: 1)])
        store.discard("m1")

        XCTAssertTrue(OutboxStore(relayUrl: relay, account: account).load().isEmpty,
                      "a row that cannot be retried is not an outbox row")
    }

    func testAStagedAttachmentDrawsTheSameCardItWillOnceItLands() {
        let a = OutboxAttachment(name: "clip.mp4", mime: "video/mp4", kind: "video", bytes: 2_048, file: "0")
        let drawn = Attachment(outbox: a)
        XCTAssertEqual(drawn.name, "clip.mp4")
        XCTAssertEqual(drawn.mime, "video/mp4")
        XCTAssertEqual(drawn.kind, "video")
        XCTAssertEqual(drawn.bytes, 2_048)
        XCTAssertNil(drawn.blobRef, "it is not on the relay yet")
    }
}
