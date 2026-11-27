import XCTest
import ArbosFFI
@testable import Arbos

/// Swift `toggleReaction` must match core `set_reaction` for flat keys.
/// Skin-tone variants collapse to the base emoji (core `normalize_reaction_emoji`).
/// Legacy nested emoji buckets are covered by crates/arbos-ffi
/// `toggle_reaction_flat_twice_and_legacy_once_clear`.
final class ReactionToggleTests: XCTestCase {
    func testUnReactClearsFlatKey() throws {
        let doc = CoreChatDoc(key: "react-flat")
        try doc.insertUserMessage(id: "m1", text: "hi", deviceId: "dev", author: .init(sender: "user:ken"))
        // Capsule / keyboard may send a toned glyph; the chip key is the base.
        try doc.toggleReaction(messageId: "m1", emoji: "👍🏽", sender: "user:ken")
        doc.commit()
        XCTAssertEqual(doc.readAll().first?.reactions.first?.emoji, "👍")

        try doc.toggleReaction(messageId: "m1", emoji: "👍🏿", sender: "user:ken")
        doc.commit()
        XCTAssertTrue(doc.readAll().first?.reactions.isEmpty == true)
    }

    func testToggleOnThenOffMatchesCoreFlatShape() throws {
        let doc = CoreChatDoc(key: "react-roundtrip")
        try doc.insertUserMessage(id: "m2", text: "hi", deviceId: "dev", author: .init(sender: "user:ken"))
        try doc.toggleReaction(messageId: "m2", emoji: "❤️", sender: "user:ken")
        doc.commit()
        XCTAssertEqual(doc.readAll().first?.reactions.map(\.emoji), ["❤️"])
        try doc.toggleReaction(messageId: "m2", emoji: "❤️", sender: "user:ken")
        doc.commit()
        XCTAssertTrue(doc.readAll().first?.reactions.isEmpty == true)
    }
}

/// Polls (`docs/polls.md`) through the same chat doc: what the iPhone writes is what the relay
/// checks and the Mac reads. One vote per person, the last one wins, and the transcript draws
/// the card in place of the plain-text twin an older build shows.
final class PollTests: XCTestCase {
    private func doc(_ key: String) throws -> CoreChatDoc {
        let doc = CoreChatDoc(key: key)
        try doc.insertPollMessage(id: "p1", question: " Where? ", options: ["Lisbon", "", "Berlin"], multiple: false,
                                  deviceId: "dev", author: .init(sender: "user:ana", name: "Ana"))
        doc.commit()
        return doc
    }

    private func poll(_ doc: CoreChatDoc) -> Poll? {
        doc.readAll().first?.parts.lazy.compactMap(\.pollValue).first
    }

    func testAPollIsReadAsItsPartAndItsPlainText() throws {
        let doc = try doc("poll-read")
        let message = try XCTUnwrap(doc.readAll().first)
        XCTAssertEqual(message.plainText, "Poll: Where?\n1. Lisbon\n2. Berlin")
        let poll = try XCTUnwrap(poll(doc))
        XCTAssertEqual(poll.question, "Where?")
        XCTAssertEqual(poll.options.map(\.label), ["Lisbon", "Berlin"])
        XCTAssertFalse(poll.multiple)
        XCTAssertEqual(poll.voters, 0)
    }

    func testAVoteIsOnePerPersonAndTheLastOneWins() throws {
        let doc = try doc("poll-vote")
        try doc.setPollVote(messageId: "p1", sender: "user:ana", selections: ["1"])
        try doc.setPollVote(messageId: "p1", sender: "user:ben", selections: ["2"])
        try doc.setPollVote(messageId: "p1", sender: "user:ana", selections: ["2"])
        doc.commit()
        var poll = try XCTUnwrap(poll(doc))
        XCTAssertEqual(poll.votes, ["2": ["user:ana", "user:ben"]])
        XCTAssertEqual(poll.voters, 2)
        XCTAssertEqual(poll.picked(by: "user:ana"), ["2"])
        XCTAssertEqual(poll.foot(by: "Ana"), "Ana's poll · 2 voted · pick one")

        // What a tap sends: a swap, or a retraction when it is the option already held.
        XCTAssertEqual(poll.picking("1", by: "user:ana"), ["1"])
        XCTAssertEqual(poll.picking("2", by: "user:ana"), [])
        try doc.setPollVote(messageId: "p1", sender: "user:ana", selections: [])
        doc.commit()
        poll = try XCTUnwrap(self.poll(doc))
        XCTAssertEqual(poll.votes, ["2": ["user:ben"]])

        // The core refuses what the poll does not allow; nothing is written.
        XCTAssertThrowsError(try doc.setPollVote(messageId: "p1", sender: "user:ana", selections: ["1", "2"]))
        XCTAssertThrowsError(try doc.setPollVote(messageId: "p1", sender: "user:ana", selections: ["9"]))
    }

    func testAMultipleAnswerPollTogglesEachOption() {
        let poll = Poll(question: "Q", options: [ChoiceOption(id: "1", label: "A"), ChoiceOption(id: "2", label: "B")],
                        max: 2, votes: ["1": ["user:ana"]])
        XCTAssertEqual(poll.picking("2", by: "user:ana"), ["1", "2"])
        XCTAssertEqual(poll.picking("1", by: "user:ana"), [])
        XCTAssertEqual(poll.foot(by: nil), "Your poll · 1 voted · pick any")
    }

    func testThePollSheetsBoundsAreTheCoresBounds() {
        XCTAssertNotNil(pollText(question: "Q", options: ["A", "B"], multiple: false))
        XCTAssertNil(pollText(question: " ", options: ["A", "B"], multiple: false))
        XCTAssertNil(pollText(question: "Q", options: ["A", " "], multiple: false))
        XCTAssertNil(pollText(question: "Q", options: Array(repeating: "A", count: SendPollSheet.optionsMax + 1), multiple: true))
        XCTAssertNotNil(pollText(question: "Q", options: Array(repeating: "A", count: SendPollSheet.optionsMax), multiple: true))
    }

    func testThePollRowIsTheCardAndNotATextBubble() throws {
        let doc = try doc("poll-row")
        let rows = Transcript.rows(doc.readAll(), projectId: "people:c1", me: "user:ben", people: true, multiParty: true)
        let kinds = rows.map(\.kind)
        XCTAssertTrue(kinds.contains { if case .poll(let poll, let by, _, _) = $0 { return poll.question == "Where?" && by == "Ana" }; return false })
        XCTAssertFalse(kinds.contains { if case .other = $0 { return true }; if case .member = $0 { return true }; return false },
                       "the plain-text twin draws no bubble")
        // Ana's poll starts after her avatar; read as Ana it is hers, on her side.
        let lane = Theme.memberLane(composerInset: 0)
        let theirs = try XCTUnwrap(kinds.first { if case .poll = $0 { return true }; return false })
        XCTAssertEqual(RowView.reactionLeading(for: theirs, composerInset: 0), lane)
        let own = Transcript.rows(doc.readAll(), projectId: "people:c1", me: "user:ana", people: true, multiParty: true)
        let mine = try XCTUnwrap(own.first { if case .poll(_, nil, _, _) = $0.kind { return true }; return false })
        XCTAssertEqual(mine.role, .user)
        XCTAssertEqual(RowView.reactionLeading(for: mine.kind, composerInset: 0), 0)
    }
}

/// A people chat on the homeserver shows my reaction with the tap, not with the homeserver's
/// event (tester report, 2026-11-13, "a little lag when selecting an emoji reaction"; taking one off
/// lagged the same way).
final class NativeReactionTests: XCTestCase {
    private let me = "user:gh_me"

    private func row(_ id: String, reactions: [MessageReaction] = []) -> Message {
        var m = Message(id: id, chatId: "dm1", role: .user, sender: "user:gh_alice", status: .done, turn: nil, decision: nil,
                        subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil,
                        createdAt: 1_700_000_000_000, completedAt: nil, pending: false,
                        parts: [.text(id: "\(id)-t", text: "hi")], hiddenCount: 0)
        m.reactions = reactions
        return m
    }

    @MainActor func testAReactionShowsWithTheTapAndLeavesWhenTheSendFails() throws {
        let model = AppModel()
        model.isFixture = true
        model.channelRows["dm1"] = [row("m1")]
        func mine() -> Bool { model.messages(of: "dm1")[0].reactions.contains { $0.emoji == "👍" && $0.senders.contains(me) } }

        // No await in between: this is what the view reads in the frame of the tap.
        XCTAssertEqual(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "👍", me: me)?.sent, true)
        XCTAssertTrue(mine(), "the row carries my reaction before the homeserver answers")

        model.dropReaction(chatId: "dm1", send: try XCTUnwrap(model.heldReactions["dm1"]?.first?.send), me: me)
        XCTAssertFalse(mine(), "a refused reaction does not stay on the row")
        XCTAssertNil(model.heldReactions["dm1"])

        // Taking one off is the same in reverse.
        model.channelRows["dm1"] = [row("m1", reactions: [MessageReaction(emoji: "👍", senders: [me, "user:gh_alice"])])]
        XCTAssertEqual(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "👍", me: me)?.sent, false)
        XCTAssertFalse(mine())
        XCTAssertEqual(model.messages(of: "dm1")[0].reactions.first?.senders, ["user:gh_alice"], "only mine left")
        model.dropReaction(chatId: "dm1", send: try XCTUnwrap(model.heldReactions["dm1"]?.first?.send), me: me)
        XCTAssertTrue(mine(), "a refused removal puts it back")
    }

    /// The homeserver's rows replace the chat's rows whole. One that arrives between the tap and
    /// the reaction's own event (someone else's message) used to take the chip off again.
    @MainActor func testATimelineThatArrivesFirstDoesNotUndoItAndTheEchoSettlesIt() throws {
        let model = AppModel()
        model.loadFixture()
        let me = try XCTUnwrap(model.myUserId)
        model.applyNativeRooms(["dm1": "!r"])
        model.matrixSnapshot = MatrixSnapshot(spaces: [], channels: [], people: [MatrixPeopleRow(
            roomId: "!r", chatId: "dm1", kind: "dm", encrypted: true, unread: 0, mentions: 0,
            lastText: "", lastSender: "", lastSenderName: "", lastTs: 0)])
        func timeline(_ reactions: String) -> [MatrixMessageRow] {
            [MatrixMessageRow(eventId: "$e1", id: "m1", sender: "user:gh_alice", senderName: "alice", text: "hi",
                              createdAt: 1, locked: false, pending: false,
                              json: #"{"id":"m1","role":"user","sender":"user:gh_alice","reactions":\#(reactions)}"#)]
        }
        func mine() -> Bool { model.messages(of: "dm1").first?.reactions.contains { $0.emoji == "👍" && $0.senders.contains(me) } == true }

        model.applyMatrixTimeline(roomId: "!r", rows: timeline("{}"))
        XCTAssertEqual(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "👍", me: me)?.sent, true)
        model.applyMatrixTimeline(roomId: "!r", rows: timeline("{}"))
        XCTAssertTrue(mine(), "rows without my reaction yet do not take it off")
        XCTAssertEqual(model.heldReactions["dm1"]?.count, 1, "still waiting for the homeserver")

        // The echo: the rows agree, nothing is laid over them any more.
        model.applyMatrixTimeline(roomId: "!r", rows: timeline(#"{"👍":{"\#(me)":true}}"#))
        XCTAssertTrue(mine())
        XCTAssertNil(model.heldReactions["dm1"])
        model.applyMatrixTimeline(roomId: "!r", rows: timeline("{}"))
        XCTAssertFalse(mine(), "once settled, the homeserver's rows are the truth again")

        // A tap that has waited behind a send for ten minutes is given up; the rows win.
        var held = [AppModel.HeldReaction(messageId: "m1", emoji: "👍", on: true, sent: false, at: 1_000)]
        var rows = [row("m1")]
        AppModel.holdReactions(&rows, held: &held, me: me, now: 1_001 + AppModel.waitingTapMs)
        XCTAssertTrue(rows[0].reactions.isEmpty)
        XCTAssertTrue(held.isEmpty)
    }

    /// Taking a reaction off is as quick as putting it on, also while the first is still on
    /// its way (tester reports, 2026-11-13 "and with removing it too", 2026-11-16 double-tap):
    /// the row flips with each tap, and the homeserver is asked for the second once it has
    /// confirmed the first.
    @MainActor func testASecondTapFlipsTheRowAtOnceAndGoesOutWhenTheFirstIsConfirmed() throws {
        let model = AppModel()
        model.loadFixture()
        let me = try XCTUnwrap(model.myUserId)
        model.applyNativeRooms(["dm1": "!r"])
        model.matrixSnapshot = MatrixSnapshot(spaces: [], channels: [], people: [MatrixPeopleRow(
            roomId: "!r", chatId: "dm1", kind: "dm", encrypted: true, unread: 0, mentions: 0,
            lastText: "", lastSender: "", lastSenderName: "", lastTs: 0)])
        func timeline(_ reactions: String) -> [MatrixMessageRow] {
            [MatrixMessageRow(eventId: "$e1", id: "m1", sender: "user:gh_alice", senderName: "alice", text: "hi",
                              createdAt: 1, locked: false, pending: false,
                              json: #"{"id":"m1","role":"user","sender":"user:gh_alice","reactions":\#(reactions)}"#)]
        }
        func mine() -> Bool { model.messages(of: "dm1").first?.reactions.contains { $0.emoji == "❤️" && $0.senders.contains(me) } == true }
        let echo = #"{"❤️":{"\#(me)":true}}"#

        model.applyMatrixTimeline(roomId: "!r", rows: timeline("{}"))
        XCTAssertEqual(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "❤️", me: me)?.sent, true)
        XCTAssertTrue(mine())
        XCTAssertNil(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "❤️", me: me), "nothing to send yet: the first is on its way")
        XCTAssertFalse(mine(), "the row lost the heart with the second tap, no await in between")

        model.applyMatrixTimeline(roomId: "!r", rows: timeline("{}"))
        XCTAssertFalse(mine())
        XCTAssertEqual(model.heldReactions["dm1"]?.map(\.sent), [true], "still waiting for the first")

        // The first is confirmed: the row keeps showing what was tapped last, and the removal goes out.
        model.applyMatrixTimeline(roomId: "!r", rows: timeline(echo))
        XCTAssertFalse(mine(), "the confirmed heart does not come back onto the row")
        XCTAssertEqual(model.heldReactions["dm1"]?.map(\.sent), [false], "the removal was sent")

        model.applyMatrixTimeline(roomId: "!r", rows: timeline("{}"))
        XCTAssertFalse(mine())
        XCTAssertNil(model.heldReactions["dm1"], "settled")

        // Tapped on and off, and the first send is refused: the homeserver never had it.
        XCTAssertEqual(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "❤️", me: me)?.sent, true)
        XCTAssertNil(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "❤️", me: me))
        model.dropReaction(chatId: "dm1", send: try XCTUnwrap(model.heldReactions["dm1"]?.first?.send), me: me)
        XCTAssertFalse(mine())
        // Tapped off and on again over one the homeserver has, refused: it is still there.
        model.applyMatrixTimeline(roomId: "!r", rows: timeline(echo))
        XCTAssertEqual(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "❤️", me: me)?.sent, false)
        XCTAssertNil(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "❤️", me: me))
        model.dropReaction(chatId: "dm1", send: try XCTUnwrap(model.heldReactions["dm1"]?.first?.send), me: me)
        XCTAssertTrue(mine())
    }

    // MARK: Through the send path, with a stand-in for the Matrix client's reaction call

    private final class Homeserver: @unchecked Sendable {
        var calls: [Bool] = []
        var keys: [String] = []
        var answer: Result<Bool, Error> = .success(true)
        var hangs = false
    }

    /// A model with one native chat holding one message, and the stand-in wired in.
    @MainActor private func nativeModel(_ homeserver: Homeserver) throws -> (model: AppModel, me: String) {
        let model = AppModel()
        model.loadFixture()
        model.applyNativeRooms(["dm1": "!r"])
        model.matrixSnapshot = MatrixSnapshot(spaces: [], channels: [], people: [MatrixPeopleRow(
            roomId: "!r", chatId: "dm1", kind: "dm", encrypted: true, unread: 0, mentions: 0,
            lastText: "", lastSender: "", lastSenderName: "", lastTs: 0)])
        model.nativeReact = { _, _, key, on in
            homeserver.calls.append(on)
            homeserver.keys.append(key)
            if homeserver.hangs { try await Task.sleep(for: .seconds(30)) }
            return try homeserver.answer.get()
        }
        timeline(model, "{}")
        return (model, try XCTUnwrap(model.myUserId))
    }

    @MainActor private func timeline(_ model: AppModel, _ reactions: String) {
        model.applyMatrixTimeline(roomId: "!r", rows: [MatrixMessageRow(
            eventId: "$e1", id: "m1", sender: "user:gh_alice", senderName: "alice", text: "hi",
            createdAt: 1, locked: false, pending: false,
            json: #"{"id":"m1","role":"user","sender":"user:gh_alice","reactions":\#(reactions)}"#)])
    }

    @MainActor private func mine(_ model: AppModel, _ me: String, _ emoji: String = "❤️") -> Bool {
        model.messages(of: "dm1").first?.reactions.contains { $0.emoji == emoji && $0.senders.contains(me) } == true
    }

    @MainActor private func until(_ what: String, _ condition: () -> Bool, file: StaticString = #filePath, line: UInt = #line) async {
        for _ in 0..<500 where !condition() { try? await Task.sleep(for: .milliseconds(10)) }
        XCTAssertTrue(condition(), what, file: file, line: line)
    }

    @MainActor func testATapIsSentAndTakingItOffIsSent() async throws {
        let homeserver = Homeserver()
        let (model, me) = try nativeModel(homeserver)

        model.toggleReaction(chatId: "dm1", messageId: "m1", emoji: "❤️")
        XCTAssertTrue(mine(model, me), "on the row with the tap")
        await until("the homeserver was asked to put it on") { homeserver.calls == [true] }
        timeline(model, #"{"❤️":{"\#(me)":true}}"#)
        XCTAssertNil(model.heldReactions["dm1"], "the echo settled it")

        model.toggleReaction(chatId: "dm1", messageId: "m1", emoji: "❤️")
        XCTAssertFalse(mine(model, me), "off the row with the tap")
        await until("the homeserver was asked to take it off") { homeserver.calls == [true, false] }
        timeline(model, "{}")
        XCTAssertFalse(mine(model, me))
        XCTAssertNil(model.heldReactions["dm1"])
    }

    @MainActor func testARefusedSendAndARowWithNoEventYetPutTheRowBack() async throws {
        struct Refused: Error {}
        let homeserver = Homeserver()
        let (model, me) = try nativeModel(homeserver)

        homeserver.answer = .failure(Refused())
        model.toggleReaction(chatId: "dm1", messageId: "m1", emoji: "❤️")
        XCTAssertTrue(mine(model, me))
        await until("a refused reaction leaves the row") { !mine(model, me) }
        XCTAssertNil(model.heldReactions["dm1"])

        homeserver.answer = .success(false)
        model.toggleReaction(chatId: "dm1", messageId: "m1", emoji: "❤️")
        XCTAssertTrue(mine(model, me))
        await until("a row with no event yet does not keep it") { !mine(model, me) }
        XCTAssertNil(model.heldReactions["dm1"])
    }

    @MainActor func testATapWaitingBehindAnotherGoesOutOnTheEcho() async throws {
        let homeserver = Homeserver()
        let (model, me) = try nativeModel(homeserver)

        model.toggleReaction(chatId: "dm1", messageId: "m1", emoji: "❤️")
        model.toggleReaction(chatId: "dm1", messageId: "m1", emoji: "❤️")
        XCTAssertFalse(mine(model, me), "the row shows the last tap")
        await until("the first was sent") { homeserver.calls == [true] }

        timeline(model, #"{"❤️":{"\#(me)":true}}"#)
        XCTAssertFalse(mine(model, me), "the confirmed heart does not come back onto the row")
        await until("the removal went out on the echo") { homeserver.calls == [true, false] }
    }

    /// The Mac put 👍🏽 on for this account; the phone shows it in the 👍 chip. Taking it off
    /// here finds nothing under `👍` in the client, which answers as if it had sent: no echo
    /// will come, and nothing else may arrive in a quiet chat. The chip comes back by itself.
    @MainActor func testAnAcceptedSendWithNoEchoGoesBackByItself() async throws {
        let homeserver = Homeserver()
        let (model, me) = try nativeModel(homeserver)
        model.reactionEchoGrace = .milliseconds(20)
        timeline(model, #"{"👍🏽":{"\#(me)":true}}"#)
        XCTAssertTrue(mine(model, me, "👍"))

        model.toggleReaction(chatId: "dm1", messageId: "m1", emoji: "👍")
        XCTAssertFalse(mine(model, me, "👍"))
        await until("with no echo and no timeline the row goes back to the homeserver's") { mine(model, me, "👍") }
        XCTAssertNil(model.heldReactions["dm1"])
        XCTAssertEqual(homeserver.calls, [false])

        // A send that never returns does not pin the row either.
        homeserver.hangs = true
        model.reactionSendLimit = .milliseconds(20)
        model.toggleReaction(chatId: "dm1", messageId: "m1", emoji: "❤️")
        XCTAssertTrue(mine(model, me))
        await until("a hung send lets go of the row") { !mine(model, me) }
        XCTAssertNil(model.heldReactions["dm1"])
    }

    /// The Mac's 👍🏽 again: tap the chip off (the client answers Ok and sends nothing), tap it
    /// on again inside the grace, and a third time later. The entry has no timer left by
    /// then; the third tap is sent as a send of its own, and with no echo for that one
    /// either the chip comes back. It used to stay off with nothing sent, for good.
    @MainActor func testATapOnAnEntryWhoseEchoNeverCameIsSentAnew() async throws {
        let homeserver = Homeserver()
        let (model, me) = try nativeModel(homeserver)
        model.reactionEchoGrace = .milliseconds(30)
        timeline(model, #"{"👍🏽":{"\#(me)":true}}"#)

        model.toggleReaction(chatId: "dm1", messageId: "m1", emoji: "👍")
        model.toggleReaction(chatId: "dm1", messageId: "m1", emoji: "👍")
        XCTAssertTrue(mine(model, me, "👍"), "the second tap shows it on again")
        await until("the grace ran out with the second tap waiting") { model.heldReactions["dm1"]?.first?.overdue == true }
        XCTAssertEqual(homeserver.calls, [false])

        model.toggleReaction(chatId: "dm1", messageId: "m1", emoji: "👍")
        XCTAssertFalse(mine(model, me, "👍"), "off with the third tap")
        await until("the third tap was sent") { homeserver.calls == [false, false] }
        await until("and with no echo the row goes back to the homeserver's") { mine(model, me, "👍") }
        XCTAssertNil(model.heldReactions["dm1"])
    }

    /// `toggleReaction` is where the written key is made: a toned pick reaches the row and
    /// the homeserver as the untoned pick does.
    @MainActor func testATonedPickReachesTheRowAndTheHomeserverUnderTheUntonedKey() async throws {
        let homeserver = Homeserver()
        let (model, me) = try nativeModel(homeserver)
        model.toggleReaction(chatId: "dm1", messageId: "m1", emoji: "✌🏽")
        XCTAssertTrue(mine(model, me, "✌️"))
        await until("sent") { homeserver.keys == ["✌️"] }
    }

    /// What comes back late for one send does not act on a later tap's entry, and an expiry
    /// does not lose a tap that is waiting behind the send.
    @MainActor func testALateAnswerLeavesANewerEntryAloneAndAWaitingTapIsKept() throws {
        let model = AppModel()
        model.isFixture = true
        model.channelRows["dm1"] = [row("m1")]
        func mine() -> Bool { model.messages(of: "dm1")[0].reactions.contains { $0.emoji == "❤️" && $0.senders.contains(me) } }

        let first = try XCTUnwrap(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "❤️", me: me))
        model.expireReaction(chatId: "dm1", send: first.send, me: me)
        XCTAssertFalse(mine(), "the first timed out")
        let second = try XCTUnwrap(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "❤️", me: me))
        model.dropReaction(chatId: "dm1", send: first.send, me: me)
        model.expireReaction(chatId: "dm1", send: first.send, me: me)
        XCTAssertTrue(mine(), "the first send's late failure does not take the second tap's heart off")
        XCTAssertEqual(model.heldReactions["dm1"]?.map(\.send), [second.send])

        // Tapped off again behind it: the expiry keeps the entry, and an echo that comes
        // after more than a minute still sends the removal.
        XCTAssertNil(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "❤️", me: me))
        model.expireReaction(chatId: "dm1", send: second.send, me: me)
        XCTAssertEqual(model.heldReactions["dm1"]?.count, 1)
        var held = try XCTUnwrap(model.heldReactions["dm1"])
        var rows = [row("m1", reactions: [MessageReaction(emoji: "❤️", senders: [me])])]
        let next = AppModel.holdReactions(&rows, held: &held, me: me, now: held[0].at + 120_000)
        XCTAssertEqual(next.map(\.sent), [false], "the waiting tap goes out")
        XCTAssertNotEqual(next.first?.send, second.send, "as a send of its own")
        XCTAssertTrue(rows[0].reactions.isEmpty, "and the row keeps showing it")
    }

    /// The phone writes a toned pick under the key the untoned pick has, so the two are one
    /// chip (tester report, 2026-11-16, "double reaction").
    func testATonedPickIsWrittenUnderTheUntonedKey() throws {
        XCTAssertEqual(MessageReaction.writeKey("✌🏽"), "✌️")
        XCTAssertEqual(MessageReaction.writeKey("☝🏽"), "☝️")
        XCTAssertEqual(MessageReaction.writeKey("✍🏽"), "✍️")
        XCTAssertEqual(MessageReaction.writeKey("🕵🏽‍♂️"), "🕵️‍♂️")
        XCTAssertEqual(MessageReaction.writeKey("👍🏽"), "👍", "an emoji-default base takes no U+FE0F")
        XCTAssertEqual(MessageReaction.writeKey("❤️"), "❤️")
        XCTAssertEqual(Array(MessageReaction.writeKey("✌").unicodeScalars).count, 1, "a bare key is toggled as it is stored")
        XCTAssertEqual(Array(MessageReaction.writeKey("\u{270C}\u{FE0F}\u{1F3FD}").unicodeScalars).map(\.value), [0x270C, 0xFE0F], "no second U+FE0F")

        let doc = CoreChatDoc(key: "react-tone")
        try doc.insertUserMessage(id: "m1", text: "hi", deviceId: "dev", author: .init(sender: "user:ken"))
        try doc.toggleReaction(messageId: "m1", emoji: MessageReaction.writeKey("✌🏽"), sender: "user:ken")
        try doc.toggleReaction(messageId: "m1", emoji: MessageReaction.writeKey("✌️"), sender: "user:ana")
        doc.commit()
        XCTAssertEqual(doc.readAll().first?.reactions.map(\.senders.count), [2], "one chip, two people")
    }
}
