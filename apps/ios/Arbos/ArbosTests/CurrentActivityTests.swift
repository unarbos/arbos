import XCTest
@testable import Arbos

/// Working-line detail after a new ask ([feedback APfcjvZgPRJE2pyaBuczRSI] / #2974):
/// a main turn left `streaming` under a later user line is dead and must not keep its
/// step on the awaiting-reply line ("Finding sock stores…" under "ask it to run LS").
/// A live job row still may (§2.3).
@MainActor
final class CurrentActivityTests: XCTestCase {
    private let chat = "c-activity"

    /// Inside the 15-minute freshness window `currentActivity` / `isStreaming` share.
    private var now: Int64 { nowMs() }

    private func row(id: String, role: Role, status: MessageStatus, at: Int64,
                     activity: String? = nil, job: JobRef? = nil,
                     parts: [Part] = []) -> Message {
        var m = Message(id: id, chatId: chat, role: role, sender: role == .user ? "user:me" : "agent",
                        status: status, turn: role == .assistant ? .reply : nil, decision: nil,
                        subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil,
                        createdAt: at, completedAt: status == .streaming ? nil : at + 1_000,
                        pending: false, parts: parts, hiddenCount: 0)
        m.activity = activity
        m.job = job
        return m
    }

    func testStaleMainTurnStepDoesNotSurviveANewAsk() {
        let model = AppModel()
        let t0 = now - 60_000
        model.messages[chat] = [
            row(id: "a1", role: .assistant, status: .streaming, at: t0,
                activity: "Finding sock stores in LA"),
            row(id: "u1", role: .user, status: .done, at: t0 + 30_000,
                parts: [.text(id: "u1-t", text: "Hey Bob, can you message one of my other agents and ask it to run LS?")]),
        ]
        XCTAssertNil(model.currentActivity(chat),
                     "a dead main turn under a new ask must not keep its step")
    }

    func testLiveMainTurnStillShowsItsStep() {
        let model = AppModel()
        let t0 = now - 5_000
        model.messages[chat] = [
            row(id: "u1", role: .user, status: .done, at: t0,
                parts: [.text(id: "u1-t", text: "find socks")]),
            row(id: "a1", role: .assistant, status: .streaming, at: t0 + 1_000,
                activity: "Finding sock stores in LA"),
        ]
        XCTAssertEqual(model.currentActivity(chat), "Finding sock stores in LA")
    }

    func testLiveJobStepSurvivesANewAsk() {
        let model = AppModel()
        let t0 = now - 60_000
        let job = JobRef(["id": "j1", "title": "Sock hunt"])!
        model.messages[chat] = [
            row(id: "a1", role: .assistant, status: .streaming, at: t0,
                activity: "Finding sock stores in LA", job: job),
            row(id: "u1", role: .user, status: .done, at: t0 + 30_000,
                parts: [.text(id: "u1-t", text: "ask it to run LS")]),
        ]
        XCTAssertEqual(model.currentActivity(chat), "Finding sock stores in LA",
                       "a background job stays live after a new user line (duplex §2.3)")
    }

    func testShortMainTurnFallsThroughToALiveJob() {
        let model = AppModel()
        let t0 = now - 5_000
        let job = JobRef(["id": "j1", "title": "Reads"])!
        model.messages[chat] = [
            row(id: "job", role: .assistant, status: .streaming, at: t0,
                activity: "Reading delsoulsurf.com", job: job),
            row(id: "main", role: .assistant, status: .streaming, at: t0 + 1_000),
        ]
        XCTAssertEqual(model.currentActivity(chat), "Reading delsoulsurf.com",
                       "a short main turn with no activity yet still yields the job (duplex §2.4)")
    }
}
