import XCTest
@testable import Arbos

/// The unread-mention `@`: which rows count, which ones are unread, and the order they may
/// be handed to the relay in.
///
/// The ordering matters more than it looks. `PUT /chats/{id}/mentions/read` takes only the
/// chat's **oldest** unread mention, because its cursor is a watermark and honouring a newer
/// id would swallow the older ones under it. Anything else is a no-op that still answers 204,
/// so a client that marks the wrong row is told it worked and watches the count come back on
/// the next `GET /me/unread` — a stuck badge with a success behind it. These cover the two
/// places that could get it wrong: naming the unread rows, and ordering the run.
final class UnreadMentionCursorTests: XCTestCase {
    private let me = "user:gh_me"

    private func msg(_ id: String, _ sender: String, _ text: String, role: Role = .user) -> Message {
        Message(id: id, chatId: "c1", role: role, sender: sender, status: .done, turn: nil, decision: nil,
                subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: 0,
                completedAt: nil, pending: false, parts: [.text(id: id + "-t", text: text)], hiddenCount: 0)
    }

    // MARK: Which rows are mentions

    func testDirectMentionIsCounted() {
        let rows = [msg("m1", "user:gh_mira", "morning"), msg("m2", "user:gh_mira", "@jacob can you look")]
        XCTAssertEqual(AppModel.directMentions(in: rows, handle: "jacob", me: me), ["m2"])
    }

    /// D5, Jacob 2026-10-31: `@here` is not a mention. The row's own `mentions` field cannot
    /// tell them apart — the apps expand `@here` into member ids before the row is written —
    /// so the text token is what separates them.
    func testHereIsNotAMention() {
        let rows = [msg("m1", "user:gh_mira", "@here standup in five")]
        XCTAssertEqual(AppModel.directMentions(in: rows, handle: "jacob", me: me), [])
    }

    func testEveryoneIsNotAMention() {
        let rows = [msg("m1", "user:gh_mira", "@everyone ship it")]
        XCTAssertEqual(AppModel.directMentions(in: rows, handle: "jacob", me: me), [])
    }

    func testMyOwnLineNeverMentionsMe() {
        let rows = [msg("m1", me, "@jacob note to self")]
        XCTAssertEqual(AppModel.directMentions(in: rows, handle: "jacob", me: me), [])
    }

    func testSystemRowNeverMentionsMe() {
        let rows = [msg("m1", "", "@jacob joined", role: .system)]
        XCTAssertEqual(AppModel.directMentions(in: rows, handle: "jacob", me: me), [])
    }

    /// A port of the relay's `eligibility::word_at` — the scan the `@` mark is counted with.
    /// A handle is continued only by an alphanumeric, `_` or `-`.
    func testHandleRunsToAHandleBoundary() {
        XCTAssertFalse(AppModel.textMentions(handle: "jo", in: "ping @jo-b about it"))
        XCTAssertTrue(AppModel.textMentions(handle: "jo-b", in: "ping @jo-b about it"))
        XCTAssertFalse(AppModel.textMentions(handle: "jo", in: "@jo_b shipped it"))
        XCTAssertFalse(AppModel.textMentions(handle: "jacob", in: "@jacobs-ladder"))
        XCTAssertTrue(AppModel.textMentions(handle: "jacob", in: "(@jacob) take a look"))
        XCTAssertTrue(AppModel.textMentions(handle: "Jacob", in: "@jacob"))
        XCTAssertFalse(AppModel.textMentions(handle: "jacob", in: "mail jacob@example.com"))
    }

    /// **The cases where the relay's two tokenisers disagree, pinned so nobody unifies them.**
    ///
    /// The mark is counted with `word_at`, which never looks at the character before the `@`
    /// and lets `.` end a handle. `people::at_token` — the rule for collectives and for the
    /// member ids a send resolves to — is stricter on both counts, and every case here is one
    /// it would answer `false` to. `at_token` implies `word_at`, so a client that ported the
    /// strict one counts fewer rows than the relay, cannot see a mention the relay counted,
    /// and names the wrong message as oldest; the route answers 204 to anything but the
    /// oldest, so that is a stuck `@` hidden behind a success. This client got that wrong
    /// once already.
    func testScanMatchesWordAtWhereTheTwoRelayRulesDisagree() {
        XCTAssertTrue(AppModel.textMentions(handle: "jacob", in: "thanks @jacob."),
                      "`.` ends a handle in word_at, so a handle at the end of a sentence counts")
        XCTAssertTrue(AppModel.textMentions(handle: "jacob", in: "see @jacob.smith"))
        XCTAssertTrue(AppModel.textMentions(handle: "jacob", in: "@@jacob"),
                      "word_at does not examine the character before the `@`")
        XCTAssertTrue(AppModel.textMentions(handle: "jacob", in: "mail me@jacob.com"))
        XCTAssertTrue(AppModel.textMentions(handle: "jacob", in: "cc_@jacob please"))
    }

    /// `word_at` resumes past the whole needle after a non-boundary, so a continued handle
    /// earlier in the line does not hide a real mention later in it.
    func testAContinuedHandleDoesNotHideALaterMention() {
        XCTAssertTrue(AppModel.textMentions(handle: "jacob", in: "@jacobson and @jacob"))
        XCTAssertFalse(AppModel.textMentions(handle: "jacob", in: "@jacobson and @jacobs"))
    }

    /// The relay trims the handle and drops leading `@`s before scanning; a handle that is
    /// nothing but those matches nothing rather than matching every `@`.
    func testEmptyHandleMatchesNothing() {
        XCTAssertFalse(AppModel.textMentions(handle: "", in: "@jacob here"))
        XCTAssertFalse(AppModel.textMentions(handle: "@", in: "@jacob here"))
        XCTAssertTrue(AppModel.textMentions(handle: " @jacob ", in: "ping @jacob"))
    }

    func testMentionsAreOldestFirst() {
        let rows = [msg("m1", "user:gh_mira", "@jacob one"), msg("m2", "user:gh_sam", "unrelated"),
                    msg("m3", "user:gh_mira", "@jacob two")]
        XCTAssertEqual(AppModel.directMentions(in: rows, handle: "jacob", me: me), ["m1", "m3"])
    }

    // MARK: Which of them are unread

    /// The relay sends a count, not a list. The cursor only ever moves forward and only ever
    /// from the oldest, so everything under it is read and the unread ones are the tail.
    func testUnreadMentionsAreTheTail() {
        XCTAssertEqual(AppModel.unreadMentionIds(all: ["a", "b", "c", "d"], count: 2), ["c", "d"])
        XCTAssertEqual(AppModel.unreadMentionIds(all: ["a", "b", "c"], count: 0), [])
        XCTAssertEqual(AppModel.unreadMentionIds(all: ["a", "b"], count: 5), ["a", "b"])
    }

    // MARK: The order they may be sent in

    func testRunStartsAtTheOldestUnread() {
        let run = AppModel.seenMentionRun(unread: ["a", "b", "c"], seen: ["a", "b", "c"])
        XCTAssertEqual(run, ["a", "b", "c"])
        XCTAssertEqual(run.first, "a", "the first mark must name the oldest unread mention")
    }

    /// The case that would present as a stuck badge: the reader jumps to the newest of three
    /// and only that one is seen. Naming it would be a no-op the relay still answers 204 to,
    /// so the run is empty and nothing is sent until the gap above closes.
    func testSeeingTheNewestAloneSendsNothing() {
        XCTAssertEqual(AppModel.seenMentionRun(unread: ["a", "b", "c"], seen: ["c"]), [])
    }

    func testRunStopsAtTheFirstUnseen() {
        XCTAssertEqual(AppModel.seenMentionRun(unread: ["a", "b", "c"], seen: ["a", "c"]), ["a"])
    }

    func testRunGrowsWhenTheGapCloses() {
        XCTAssertEqual(AppModel.seenMentionRun(unread: ["a", "b", "c"], seen: ["a", "c"]), ["a"])
        XCTAssertEqual(AppModel.seenMentionRun(unread: ["a", "b", "c"], seen: ["a", "b", "c"]), ["a", "b", "c"])
    }

    func testNothingSeenSendsNothing() {
        XCTAssertEqual(AppModel.seenMentionRun(unread: ["a", "b"], seen: []), [])
    }

    func testNoUnreadMentionsSendsNothing() {
        XCTAssertEqual(AppModel.seenMentionRun(unread: [], seen: ["a", "b"]), [])
    }

    /// Each mark clears exactly one, so the next walk starts one further along: what the
    /// serial walk does between calls, and the reason it cannot batch.
    func testTheRunAdvancesOneMentionPerMark() {
        let seen: Set<String> = ["a", "b", "c"]
        XCTAssertEqual(AppModel.seenMentionRun(unread: ["a", "b", "c"], seen: seen).first, "a")
        XCTAssertEqual(AppModel.seenMentionRun(unread: ["b", "c"], seen: seen).first, "b")
        XCTAssertEqual(AppModel.seenMentionRun(unread: ["c"], seen: seen).first, "c")
        XCTAssertNil(AppModel.seenMentionRun(unread: [], seen: seen).first)
    }
}
