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
                                  showVoters: true, deviceId: "dev", author: .init(sender: "user:ana", name: "Ana"))
        doc.commit()
        return doc
    }

    private func poll(_ doc: CoreChatDoc, as viewer: String? = nil) -> Poll? {
        doc.readAll(as: viewer).first?.parts.lazy.compactMap(\.pollValue).first
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
        XCTAssertEqual(poll.foot(by: "Ana"), "Ana's poll · 2 voted · pick one · names shown")

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
                        max: 2, votes: ["1": ["user:ana"]], counts: ["1": 1], voters: 1)
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

    /// The new choices ride on the part: who voted is shown only where the author said so, a
    /// close by hand or by time closes the card, and only a poll posted with them can be closed.
    func testVisibilityAndClosingAreReadFromThePart() throws {
        let doc = CoreChatDoc(key: "poll-options")
        try doc.insertPollMessage(id: "p1", question: "Where?", options: ["Lisbon", "Berlin"], multiple: false,
                                  showVoters: true, closesInMs: 3_600_000, deviceId: "dev",
                                  author: .init(sender: "user:ana", name: "Ana"))
        try doc.setPollVote(messageId: "p1", sender: "user:ben", selections: ["1"])
        doc.commit()
        var poll = try XCTUnwrap(poll(doc))
        let now = nowMs()
        XCTAssertEqual(poll.showVoters, true)
        XCTAssertTrue(poll.closable)
        let closesAt = try XCTUnwrap(poll.deadline)
        XCTAssertTrue((now + 3_590_000...now + 3_610_000).contains(closesAt))
        XCTAssertFalse(poll.closed(now: now))
        XCTAssertTrue(poll.closed(now: closesAt), "its own time closes it")
        XCTAssertEqual(poll.foot(by: "Ana", now: closesAt - 2 * 3_600_000 - 1),
                       "Ana's poll · 1 voted · pick one · names shown · closes in 2 h")

        // Only its author closes it; then the core takes no vote, change or retraction.
        XCTAssertThrowsError(try doc.closePoll(messageId: "p1", sender: "user:ben"))
        usleep(3_000) // a vote in the very millisecond of the close is not before it
        try doc.closePoll(messageId: "p1", sender: "user:ana")
        doc.commit()
        poll = try XCTUnwrap(self.poll(doc))
        XCTAssertTrue(poll.closed(now: nowMs()), "closed by hand, an hour before its time")
        XCTAssertLessThan(try XCTUnwrap(poll.deadline), closesAt)
        XCTAssertEqual(poll.foot(by: nil), "Your poll · 1 voted · closed · names shown")
        XCTAssertThrowsError(try doc.setPollVote(messageId: "p1", sender: "user:cat", selections: ["2"]))
        XCTAssertThrowsError(try doc.setPollVote(messageId: "p1", sender: "user:ben", selections: []))
        XCTAssertEqual(try XCTUnwrap(self.poll(doc)).votes, ["1": ["user:ben"]])

        // A part from before the choices existed: counts only, never closed, not closable.
        let old = Poll(question: "Q", options: [ChoiceOption(id: "1", label: "A"), ChoiceOption(id: "2", label: "B")],
                       max: 1, votes: [:])
        XCTAssertNil(old.showVoters)
        XCTAssertFalse(old.closable)
        XCTAssertFalse(old.closed(now: .max))
    }

    /// A poll that hides its voters reaches the phone with counts and my own vote: nobody
    /// else's id is in the part the card is drawn from, so no screen can name them.
    func testAPollThatHidesItsVotersGivesThePhoneNoOtherNames() throws {
        let doc = CoreChatDoc(key: "poll-hidden")
        try doc.insertPollMessage(id: "p1", question: "Where?", options: ["Lisbon", "Berlin"], multiple: false,
                                  showVoters: false, deviceId: "dev", author: .init(sender: "user:zoe", name: "Zoe"))
        try doc.setPollVote(messageId: "p1", sender: "user:ana", selections: ["2"])
        try doc.setPollVote(messageId: "p1", sender: "user:me", selections: ["2"])
        doc.commit()
        let mine = try XCTUnwrap(poll(doc, as: "user:me"))
        XCTAssertEqual(mine.counts, ["2": 2])
        XCTAssertEqual(mine.voters, 2)
        XCTAssertEqual(mine.votes, ["2": ["user:me"]], "my own vote, so the card can mark it")
        XCTAssertEqual(mine.picked(by: "user:me"), ["2"])
        XCTAssertEqual(mine.foot(by: "Zoe"), "Zoe's poll · 2 voted · pick one")

        let nobody = try XCTUnwrap(poll(doc))
        XCTAssertEqual(nobody.votes, [:], "no names at all without a reader")
        XCTAssertEqual(nobody.counts, ["2": 2])
        // The row itself, as it crosses the FFI: Ana's stored vote fields are not on it.
        let row = try XCTUnwrap(doc.rawRow(0, as: "user:me"))
        XCTAssertFalse(row.contains("user:ana"), row)
        XCTAssertTrue(row.contains("vote|user:me"), "my own fields stay: \(row)")
    }

    /// A reply quotes a poll by its question: the strip above the composer and the header in
    /// the reply's bubble both read "Poll: <question>", not the list of options.
    func testAReplyQuotesAPollByItsQuestion() throws {
        let doc = try doc("poll-quote")
        let message = try XCTUnwrap(doc.readAll().first)
        XCTAssertEqual(Quote.line(of: message, maxChars: 160), "Poll: Where?")
        XCTAssertEqual(Quote.forReply("p1", in: [message]).text, "Poll: Where?")
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
