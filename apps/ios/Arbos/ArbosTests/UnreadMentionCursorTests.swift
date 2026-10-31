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

    /// The relay's `people::at_token` rule: a handle is not continued by a word character,
    /// so `@jo` is not `@jo-b`.
    func testHandleIsAWholeToken() {
        XCTAssertFalse(AppModel.textMentions(handle: "jo", in: "ping @jo-b about it"))
        XCTAssertTrue(AppModel.textMentions(handle: "jo-b", in: "ping @jo-b about it"))
        XCTAssertFalse(AppModel.textMentions(handle: "jacob", in: "mail jacob@example.com"))
        XCTAssertTrue(AppModel.textMentions(handle: "jacob", in: "(@jacob) take a look"))
        XCTAssertTrue(AppModel.textMentions(handle: "Jacob", in: "@jacob"))
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
