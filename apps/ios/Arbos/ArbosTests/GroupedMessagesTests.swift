import SwiftUI
import XCTest
@testable import Arbos

/// Grouped messages ([feedback grouped-messages-ios-2026-10-28], Jacob 2026-10-28, from the
/// Arbonites' ask): consecutive messages from one sender within `Transcript.runWindowMs`
/// form a run — the name and the avatar once, on the first message; the later ones under
/// its text with the avatar column empty and `runGap` between them. The run breaks on a
/// sender change, on the window passing, at a system line or a date line,
/// and at a reply quote. People and agents group alike, my own messages too.
final class GroupedMessagesTests: XCTestCase {
    private let chat = "fx-group-launch"
    private let t0: Int64 = 1_700_000_000_000

    private func person(_ id: String, sender: String, handle: String, _ text: String, at: Int64,
                        replyTo: String? = nil) -> Message {
        var m = Message(id: id, chatId: chat, role: .user, sender: sender, status: .done, turn: nil, decision: nil,
                        subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: at,
                        completedAt: nil, pending: false, parts: [.text(id: "\(id)-t", text: text)], hiddenCount: 0,
                        senderHandle: handle, senderName: handle)
        m.replyTo = replyTo
        return m
    }

    private func agent(_ id: String, _ text: String, at: Int64, inReplyTo: String? = nil) -> Message {
        Message(id: id, chatId: chat, role: .assistant, sender: "agent:fx-comms", status: .done, turn: .reply,
                decision: "reply", subagentId: nil, subagentName: nil, inReplyTo: inReplyTo, usage: [:], model: nil,
                createdAt: at, completedAt: nil, pending: false, parts: [.text(id: "\(id)-t", text: text)],
                hiddenCount: 0, senderHandle: "comms", senderName: "Comms")
    }

    private func system(_ id: String, _ text: String, at: Int64) -> Message {
        Message(id: id, chatId: chat, role: .system, sender: "system", status: .done, turn: nil, decision: nil,
                subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: at,
                completedAt: nil, pending: false, parts: [.text(id: "\(id)-t", text: text)], hiddenCount: 0)
    }

    private func rows(_ messages: [Message], multiParty: Bool = true) -> [Row] {
        Transcript.rows(messages, projectId: "people:\(chat)", me: "user:fx-me", people: true, multiParty: multiParty)
    }

    /// `(text or "agent" / "mine", named, avatar, gapBefore)` for every bubble row.
    private func shape(_ rows: [Row]) -> [(String, Bool, Bool, CGFloat)] {
        rows.compactMap { row in
            switch row.kind {
            case .other(let text, _, _, let named, _, _): return (text, named, row.showsAvatar, row.gapBefore)
            case .member(_, _, _, _, _, let named, _, _): return ("agent", named, row.showsAvatar, row.gapBefore)
            case .user: return ("mine", false, row.showsAvatar, row.gapBefore)
            default: return nil
            }
        }
    }

    func testTheWindowIsTwoMinutesAndTheGapsAreTighter() {
        XCTAssertEqual(Transcript.runWindowMs, 120_000, "a couple of minutes = 2 min")
        XCTAssertEqual(Transcript.runGap, 2, "Jacob's 2026-11-01 reference measures 4–5 px at 2.03 px/pt; was 4")
        XCTAssertEqual(Transcript.turnGap, 13, "the same reference measures 24–28 px between runs; was 16")
        XCTAssertEqual(Transcript.systemGap, 20)
        XCTAssertLessThan(Transcript.runGap, Transcript.turnGap)
    }

    func testContinuesRunRule() {
        let w = Transcript.runWindowMs
        XCTAssertTrue(Transcript.continuesRun(prevSender: "user:a", prevAt: t0, sender: "user:a", at: t0 + 1_000, quoted: false))
        XCTAssertTrue(Transcript.continuesRun(prevSender: "user:a", prevAt: t0, sender: "user:a", at: t0 + w, quoted: false),
                      "exactly the window still groups")
        XCTAssertFalse(Transcript.continuesRun(prevSender: "user:a", prevAt: t0, sender: "user:a", at: t0 + w + 1, quoted: false),
                       "one ms past the window breaks")
        XCTAssertFalse(Transcript.continuesRun(prevSender: "user:a", prevAt: t0, sender: "user:b", at: t0 + 1_000, quoted: false),
                       "another sender breaks")
        XCTAssertFalse(Transcript.continuesRun(prevSender: "user:a", prevAt: t0, sender: "user:a", at: t0 + 1_000, quoted: true),
                       "a reply quote breaks")
        XCTAssertFalse(Transcript.continuesRun(prevSender: nil, prevAt: 0, sender: "user:a", at: t0, quoted: false), "no run yet")
        XCTAssertFalse(Transcript.continuesRun(prevSender: "user:a", prevAt: t0, sender: "user:a", at: 0, quoted: false),
                       "a row without a clock never groups")
        XCTAssertTrue(Transcript.continuesRun(prevSender: "user:a", prevAt: t0, sender: "user:a", at: t0 - 30_000, quoted: false),
                      "a clock a little behind the run's (another device) still groups")
    }

    func testSameSenderWithinTheWindowContinuesTheRun() {
        let got = shape(rows([
            person("m1", sender: "user:fx-u3", handle: "mira-k", "hi all", at: t0),
            person("m2", sender: "user:fx-u3", handle: "mira-k", "two", at: t0 + 30_000),
            person("m3", sender: "user:fx-u3", handle: "mira-k", "three", at: t0 + 90_000),
        ]))
        XCTAssertEqual(got.map(\.0), ["hi all", "two", "three"])
        XCTAssertEqual(got.map(\.1), [true, false, false], "the name once, on the run's first message")
        XCTAssertEqual(got.map(\.2), [true, false, false], "the avatar once, on the run's first message")
        // The first row follows the date line (systemGap); the rest sit in the in-run gap.
        XCTAssertEqual(got.map(\.3), [Transcript.systemGap, Transcript.runGap, Transcript.runGap])
    }

    func testASenderChangeOrTheWindowPassingStartsANewRun() {
        let w = Transcript.runWindowMs
        let got = shape(rows([
            person("m1", sender: "user:fx-u3", handle: "mira-k", "a", at: t0),
            person("m2", sender: "user:fx-u4", handle: "sam", "b", at: t0 + 10_000),
            person("m3", sender: "user:fx-u4", handle: "sam", "c", at: t0 + 10_000 + w + 1),
            person("m4", sender: "user:fx-u3", handle: "mira-k", "d", at: t0 + 10_000 + w + 2_000),
        ]))
        XCTAssertEqual(got.map(\.1), [true, true, true, true], "every message starts a run")
        XCTAssertEqual(got.map(\.2), [true, true, true, true])
        XCTAssertEqual(got.map(\.3), [Transcript.systemGap, Transcript.turnGap, Transcript.turnGap, Transcript.turnGap])
    }

    func testASystemLineOrAQuoteBreaksTheRun() {
        let got = shape(rows([
            person("m1", sender: "user:fx-u3", handle: "mira-k", "a", at: t0),
            system("s1", "Jacob added Doug", at: t0 + 1_000),
            person("m2", sender: "user:fx-u3", handle: "mira-k", "b", at: t0 + 2_000),
            person("m5", sender: "user:fx-u3", handle: "mira-k", "re: that", at: t0 + 3_000, replyTo: "m1"),
            person("m6", sender: "user:fx-u3", handle: "mira-k", "after the quote", at: t0 + 4_000),
        ]))
        XCTAssertEqual(got.map(\.0), ["a", "b", "re: that", "after the quote"])
        XCTAssertEqual(got.map(\.1), [true, true, true, false], "the quoted bubble starts a run of its own; the next line joins it")
        XCTAssertEqual(got.map(\.2), [true, true, true, false])
        XCTAssertEqual(got.map(\.3), [Transcript.systemGap, Transcript.systemGap, Transcript.turnGap, Transcript.runGap])
    }

    func testADateLineBreaksTheRun() {
        let got = shape(rows([
            person("m1", sender: "user:fx-u3", handle: "mira-k", "a", at: t0),
            person("m2", sender: "user:fx-u3", handle: "mira-k", "next hour", at: t0 + 2 * 3_600_000),
        ]))
        XCTAssertEqual(got.map(\.1), [true, true])
        XCTAssertEqual(got.map(\.2), [true, true])
        XCTAssertEqual(got.map(\.3), [Transcript.systemGap, Transcript.systemGap], "a date line sits between them")
    }

    func testAgentsGroupLikePeopleUnlessTheyQuote() {
        let got = shape(rows([
            person("m1", sender: "user:fx-me", handle: "jacob", "ask", at: t0),
            agent("a1", "first part", at: t0 + 10_000),
            agent("a2", "second part", at: t0 + 20_000),
            agent("a3", "answering the ask", at: t0 + 40_000, inReplyTo: "m1"),
            agent("a4", "and a note", at: t0 + 50_000),
        ]))
        XCTAssertEqual(got.map(\.0), ["mine", "agent", "agent", "agent", "agent"])
        XCTAssertEqual(got.map(\.1), [false, true, false, true, false],
                       "an unquoted follow-up continues the agent's run; a quoted reply starts one")
        XCTAssertEqual(got.map(\.2), [false, true, false, true, false])
        XCTAssertEqual(got.map(\.3), [Transcript.systemGap, Transcript.turnGap, Transcript.runGap, Transcript.turnGap, Transcript.runGap])
    }

    func testMyOwnMessagesGroupTooAndADMGroupsItsAvatars() {
        // A DM names nobody (`multiParty: false`) but still shows the avatar once per run.
        let got = shape(rows([
            person("m1", sender: "user:fx-me", handle: "jacob", "one", at: t0),
            person("m2", sender: "user:fx-me", handle: "jacob", "two", at: t0 + 5_000),
            person("m3", sender: "user:fx-u3", handle: "mira-k", "hey", at: t0 + 10_000),
            person("m4", sender: "user:fx-u3", handle: "mira-k", "you there?", at: t0 + 20_000),
        ], multiParty: false))
        XCTAssertEqual(got.map(\.0), ["mine", "mine", "hey", "you there?"])
        XCTAssertEqual(got.map(\.1), [false, false, false, false], "a DM labels nobody")
        XCTAssertEqual(got.map(\.2), [false, false, true, false], "Mira's avatar once")
        XCTAssertEqual(got.map(\.3), [Transcript.systemGap, Transcript.runGap, Transcript.turnGap, Transcript.runGap],
                       "my second line sits in the in-run gap; Mira's run opens the turn gap once")
    }

    func testTheAvatarSitsOnTheLastAvatarRowOfTheRunsFirstMessage() {
        // A picture with a note: two rows of one message — the card, then the bubble. The
        // avatar goes on the last of them (the Mac's rule: the disc at the message's bottom);
        // the card keeps the lane. The next message of the run shows none.
        var withPicture = person("m1", sender: "user:fx-u3", handle: "mira-k", "see the plan", at: t0)
        let a = Attachment(name: "plan.pdf", mime: "application/pdf", blobRef: "blob-1", kind: "file", bytes: 1_024)
        withPicture.parts = [.attachment(id: "m1-a", attachment: a), .text(id: "m1-t", text: "see the plan")]
        let got = rows([withPicture, person("m2", sender: "user:fx-u3", handle: "mira-k", "ok?", at: t0 + 1_000)])
        let avatars = got.compactMap { row -> (String, Bool)? in
            switch row.kind {
            case .file: return ("file", row.showsAvatar)
            case .other(let text, _, _, _, _, _): return (text, row.showsAvatar)
            default: return nil
            }
        }
        XCTAssertEqual(avatars.map(\.0), ["file", "see the plan", "ok?"])
        XCTAssertEqual(avatars.map(\.1), [false, true, false])
    }

    /// The row's layout facts the view reads: an avatar-less continuation still keeps the
    /// lane, so `RowView` draws an empty slot of the avatar's width in its place.
    func testTheAvatarSlotIsTheAvatarsWidth() {
        XCTAssertEqual(Theme.memberLane(composerInset: Theme.composerInset),
                       Theme.avatarColumnInset + Theme.memberAvatar + Theme.memberAvatarGap,
                       "the lane is inset + avatar + gap; the empty slot is the avatar's width at the same inset")
    }
}
