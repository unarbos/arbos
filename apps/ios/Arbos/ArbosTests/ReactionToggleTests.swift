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
        XCTAssertTrue(kinds.contains { if case .poll(let poll, let by) = $0 { return poll.question == "Where?" && by == "Ana" }; return false })
        XCTAssertFalse(kinds.contains { if case .other = $0 { return true }; if case .member = $0 { return true }; return false },
                       "the plain-text twin draws no bubble")
    }
}

/// A people chat on the homeserver shows my reaction with the tap, not with the homeserver's
/// event (feedback 2026-11-13, "a little lag when selecting an emoji reaction"; taking one off
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

    @MainActor func testAReactionShowsWithTheTapAndLeavesWhenTheSendFails() {
        let model = AppModel()
        model.isFixture = true
        model.channelRows["dm1"] = [row("m1")]
        func mine() -> Bool { model.messages(of: "dm1")[0].reactions.contains { $0.emoji == "👍" && $0.senders.contains(me) } }

        // No await in between: this is what the view reads in the frame of the tap.
        XCTAssertEqual(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "👍", me: me), true)
        XCTAssertTrue(mine(), "the row carries my reaction before the homeserver answers")

        model.dropReaction(chatId: "dm1", messageId: "m1", emoji: "👍", me: me)
        XCTAssertFalse(mine(), "a refused reaction does not stay on the row")
        XCTAssertNil(model.heldReactions["dm1"]?.first)

        // Taking one off is the same in reverse.
        model.channelRows["dm1"] = [row("m1", reactions: [MessageReaction(emoji: "👍", senders: [me, "user:gh_alice"])])]
        XCTAssertEqual(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "👍", me: me), false)
        XCTAssertFalse(mine())
        XCTAssertEqual(model.messages(of: "dm1")[0].reactions.first?.senders, ["user:gh_alice"], "only mine left")
        model.dropReaction(chatId: "dm1", messageId: "m1", emoji: "👍", me: me)
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
        XCTAssertEqual(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "👍", me: me), true)
        model.applyMatrixTimeline(roomId: "!r", rows: timeline("{}"))
        XCTAssertTrue(mine(), "rows without my reaction yet do not take it off")
        XCTAssertEqual(model.heldReactions["dm1"]?.count, 1, "still waiting for the homeserver")

        // The echo: the rows agree, nothing is laid over them any more.
        model.applyMatrixTimeline(roomId: "!r", rows: timeline(#"{"👍":{"\#(me)":true}}"#))
        XCTAssertTrue(mine())
        XCTAssertNil(model.heldReactions["dm1"])
        model.applyMatrixTimeline(roomId: "!r", rows: timeline("{}"))
        XCTAssertFalse(mine(), "once settled, the homeserver's rows are the truth again")

        // An event that never comes back does not pin the row for good.
        var held = [AppModel.HeldReaction(messageId: "m1", emoji: "👍", on: true, sent: true, at: 1_000)]
        var rows = [row("m1")]
        AppModel.holdReactions(&rows, held: &held, me: me, now: 61_001)
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
        XCTAssertEqual(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "❤️", me: me), true)
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
        XCTAssertEqual(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "❤️", me: me), true)
        XCTAssertNil(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "❤️", me: me))
        model.dropReaction(chatId: "dm1", messageId: "m1", emoji: "❤️", me: me)
        XCTAssertFalse(mine())
        // Tapped off and on again over one the homeserver has, refused: it is still there.
        model.applyMatrixTimeline(roomId: "!r", rows: timeline(echo))
        XCTAssertEqual(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "❤️", me: me), false)
        XCTAssertNil(model.holdReaction(chatId: "dm1", messageId: "m1", emoji: "❤️", me: me))
        model.dropReaction(chatId: "dm1", messageId: "m1", emoji: "❤️", me: me)
        XCTAssertTrue(mine())
    }
}

/// A reaction restamps the rows of its one message instead of rebuilding the transcript, and
/// the result is what the rebuild gives (tester report, 2026-11-16: the double-tap heart "is
/// just not smooth").
final class ReactionRowTests: XCTestCase {
    @MainActor func testAReactionRestampsItsMessageAsAFullRebuildWould() throws {
        let model = AppModel()
        model.loadFixture()
        let chatId = "fx-group-launch"
        let me = try XCTUnwrap(model.myUserId)
        func build(_ list: [Message]) -> [Row] {
            Transcript.rows(list, projectId: "\(peopleSlot):\(chatId)", me: me, people: true, multiParty: true)
        }
        let before = model.messages(of: chatId)
        XCTAssertGreaterThan(before.count, 1)
        for i in before.indices {
            // On, a second emoji, then both off again: each step against the rebuild.
            var list = before
            var rows = build(list)
            for emoji in ["❤️", "🔥", "❤️", "🔥"] {
                let was = Transcript.index(list)
                list[i].reactions = MessageReaction.toggling(list[i].reactions, emoji: emoji, sender: me)
                let edited = try XCTUnwrap(Transcript.reactionOnlyEdit(list, was: was), "message \(i) \(emoji)")
                XCTAssertEqual(edited.id, list[i].id)
                Transcript.restampReactions(&rows, for: edited)
                XCTAssertEqual(rows, build(list), "message \(i) after \(emoji)")
            }
        }

        // Anything but a reaction on one message is not this path.
        let was = Transcript.index(before)
        XCTAssertNil(Transcript.reactionOnlyEdit(before, was: was), "nothing changed")
        var two = before
        two[0].reactions = [MessageReaction(emoji: "🔥", senders: [me])]
        two[1].reactions = [MessageReaction(emoji: "🔥", senders: [me])]
        XCTAssertNil(Transcript.reactionOnlyEdit(two, was: was), "two messages changed")
        var other = before
        other[0].reactions = [MessageReaction(emoji: "🔥", senders: [me])]
        other[0].createdAt += 1
        XCTAssertNil(Transcript.reactionOnlyEdit(other, was: was), "more than its reactions changed")
    }

    /// Two quick taps on a chip are the chip's (on, then off again), not a heart for the
    /// message (tester report, 2026-11-16, "double reaction").
    func testADoubleTapWithATapOnAChipInItIsNotAHeart() {
        ChatGestures.chipTapAt = 100
        XCTAssertTrue(ChatGestures.doubleTapOnChip(now: 100.3))
        XCTAssertFalse(ChatGestures.doubleTapOnChip(now: 101), "a later double-tap on the bubble is")
        ChatGestures.chipTapAt = 0
    }
}
