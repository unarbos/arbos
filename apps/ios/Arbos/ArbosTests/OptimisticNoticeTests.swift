import XCTest
@testable import Arbos

/// "<me> added <agent>" is in the chat the moment Add is tapped; the relay's own row replaces
/// the local copy when it lands (agent-added-notice-2026-10-26, Jacob's "the system notice
/// arrives very late"). Mirrors crates/arbos-mac `model::optimistic_notice_tests`.
final class OptimisticNoticeTests: XCTestCase {
    private func relayRow(_ id: String, _ text: String, at: Int64) -> Message {
        Message(
            id: id, chatId: "c", role: .system, sender: "system",
            status: .done, turn: nil, decision: nil, subagentId: nil, subagentName: nil,
            inReplyTo: nil, usage: [:], model: nil,
            createdAt: at, completedAt: at, pending: false,
            parts: [.text(id: "\(id)-t", text: text)], hiddenCount: 0
        )
    }

    private func userRow(_ id: String, at: Int64) -> Message {
        Message(
            id: id, chatId: "c", role: .user, sender: "user:u_me",
            status: .done, turn: nil, decision: nil, subagentId: nil, subagentName: nil,
            inReplyTo: nil, usage: [:], model: nil,
            createdAt: at, completedAt: at, pending: false,
            parts: [.text(id: "\(id)-t", text: "hi")], hiddenCount: 0
        )
    }

    func testTheLocalLineReadsLikeTheRelays() {
        XCTAssertEqual(OptimisticNotice.addedLine(who: "Jacob", names: ["Affine"]), "Jacob added Affine")
        XCTAssertEqual(OptimisticNotice.addedLine(who: "Jacob", names: ["Home", "hjkb", "Doug"]), "Jacob added Home, hjkb and Doug")
        XCTAssertNil(OptimisticNotice.addedLine(who: "Jacob", names: []))
        let n = OptimisticNotice(text: "Jacob added Affine", now: 1_000)
        let m = n.message(chatId: "c")
        XCTAssertEqual(m.role, .system)
        XCTAssertEqual(m.plainText, "Jacob added Affine")
        XCTAssertTrue(m.id.hasPrefix(OptimisticNotice.idPrefix))
    }

    func testALocalNoticeSurvivesSnapshotsUntilTheRelayRowConfirmsIt() {
        let now: Int64 = 100_000
        var notices = [OptimisticNotice(text: "Jacob added Affine", now: now)]
        // Snapshot 1: something unrelated arrived; the local line stays, in time order.
        var snap = [userRow("m1", at: now - 5_000), userRow("m2", at: now + 300)]
        XCTAssertEqual(OptimisticNotice.reconcile(&snap, notices: &notices, chatId: "c", now: now + 400), 0)
        XCTAssertEqual(snap.map(\.plainText), ["hi", "Jacob added Affine", "hi"])
        XCTAssertEqual(snap[1].chatId, "c")
        XCTAssertEqual(notices.count, 1)
        // Snapshot 2: the relay's row (its clock a little behind ours) — the local copy goes,
        // the relay's stays, nothing doubles up.
        snap = [userRow("m1", at: now - 5_000), userRow("m2", at: now + 300),
                relayRow("r1", "Jacob added Affine", at: now - 2_000)]
        XCTAssertEqual(OptimisticNotice.reconcile(&snap, notices: &notices, chatId: "c", now: now + 900), 1)
        XCTAssertTrue(notices.isEmpty)
        XCTAssertEqual(snap.filter { $0.plainText == "Jacob added Affine" }.count, 1)
        XCTAssertFalse(snap.contains { $0.id.hasPrefix(OptimisticNotice.idPrefix) })
    }

    func testOtherNoticesAndOldRowsDoNotConfirmAndAStaleNoticeExpires() {
        let now: Int64 = 100_000
        var notices = [OptimisticNotice(text: "Jacob added Affine", now: now)]
        var snap = [relayRow("old", "Jacob added Affine", at: now - 60_000),
                    relayRow("other", "Jacob removed Affine", at: now + 10)]
        XCTAssertEqual(OptimisticNotice.reconcile(&snap, notices: &notices, chatId: "c", now: now + 100), 0)
        XCTAssertEqual(notices.count, 1)
        XCTAssertEqual(snap.count, 3)
        // Past the TTL with no confirmation: the local line is withdrawn.
        snap = [userRow("m1", at: now - 5_000)]
        OptimisticNotice.reconcile(&snap, notices: &notices, chatId: "c", now: now + OptimisticNotice.ttlMs)
        XCTAssertTrue(notices.isEmpty)
        XCTAssertEqual(snap.count, 1)
    }

    /// The relay's row confirms the local one by facts (actor + subjects by id), whatever the
    /// words — a stale local name no longer leaves two lines (#2004).
    func testFactsConfirmTheLocalNoticeWhateverTheWords() {
        let now: Int64 = 1_000
        let facts = MembershipNotice(kind: "added", actor: "gh_1", subjects: [.init(kind: "agent", id: "p1")])
        let local = OptimisticNotice(text: "Const added Aureum", now: now, notice: facts)
        var relay = relayRow("r1", "Jacob added Aureum", at: now + 50)
        relay.notice = facts
        XCTAssertTrue(local.isConfirmed(by: relay))
        relay.notice = MembershipNotice(kind: "added", actor: "gh_2", subjects: [.init(kind: "agent", id: "p1")])
        XCTAssertFalse(local.isConfirmed(by: relay))
    }

    /// A membership line is worded from the live roster: a rename reaches old lines.
    func testNoticeTextFollowsTheLiveNames() {
        let n = MembershipNotice(kind: "added", actor: "gh_1", subjects: [.init(kind: "user", id: "gh_2"), .init(kind: "agent", id: "p1")])
        let names = ["user:gh_1": "Jacob", "user:gh_2": "Doug", "agent:p1": "Aureum"]
        XCTAssertEqual(n.text { kind, id in names["\(kind):\(id)"] }, "Jacob added Doug and Aureum")
        XCTAssertNil(n.text { kind, id in kind == "agent" ? nil : names["\(kind):\(id)"] })
        XCTAssertEqual(MembershipNotice(kind: "left", actor: "gh_1", subjects: []).text { _, id in names["user:\(id)"] }, "Jacob left")
    }

    @MainActor
    func testAddMembersPaintsTheNoticeBeforeTheRelayAnswers() {
        let model = AppModel()
        model.loadFixture()
        let chatId = "fx-group-launch"
        let before = model.messages(of: chatId).count
        model.showMembershipNotice(chatId: chatId, names: ["Lium"])
        let after = model.messages(of: chatId)
        XCTAssertEqual(after.count, before + 1)
        XCTAssertEqual(after.last?.role, .system)
        XCTAssertTrue(after.last?.plainText.hasSuffix(" added Lium") == true, "\(after.last?.plainText ?? "")")
        XCTAssertEqual(model.optimisticNotices[chatId]?.count, 1)
    }
}
