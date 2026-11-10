import XCTest
@testable import Arbos

/// [feedback AG5fMKSEGcm19R9A3FU1oKw]: a send that never opens an assistant row used to go
/// blank after the short awaiting bridge. The working cheat must hold through a cold wake,
/// and when it expires the chat must still show the stock "didn't answer" line.
@MainActor
final class MissedReplyTests: XCTestCase {
    private let chatId = "fx-silent-main"
    private let projectId = "fx-silent"

    private func user(_ id: Id, at: Int64, text: String = "Find me the best sushi in la") -> Message {
        Message(id: id, chatId: chatId, role: .user, sender: "user", status: .done, turn: nil, decision: nil,
                subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil,
                createdAt: at, completedAt: at, pending: false,
                parts: [.text(id: "\(id)-t", text: text)], hiddenCount: 0)
    }

    private func streamingAssistant(_ id: Id, at: Int64) -> Message {
        Message(id: id, chatId: chatId, role: .assistant, sender: "agent", status: .streaming, turn: .reply,
                decision: "reply", subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil,
                createdAt: at, completedAt: nil, pending: false,
                parts: [.status(id: "\(id)-s", code: .working, message: "Working…")], hiddenCount: 0)
    }

    private func modelEndingWithUser(at: Int64) -> AppModel {
        let model = AppModel()
        model.projects = [
            Project(id: projectId, name: "Bob", icon: "B", color: "#1084FE", state: .awake,
                    stateSince: at, stateDetail: nil, model: "", effort: .low,
                    spendTodayUsd: 0, spendCapUsd: 0, avatar: nil, avatarColor: nil),
        ]
        model.chats = [
            projectId: [
                Chat(id: chatId, projectId: projectId, title: "Bob", isPinned: false,
                     unread: false, lastMessageAt: at),
            ],
        ]
        model.machine = Machine(state: .awake, since: at, spec: "", reported: true, rawState: .awake)
        model.messages[chatId] = [user("u1", at: at)]
        return model
    }

    func testAwaitingBridgeHoldsForAMinuteThenMissedReplyTakesOver() {
        let fresh = nowMs() - 5_000
        let model = modelEndingWithUser(at: fresh)
        XCTAssertEqual(AppModel.awaitingReplyMs, 60_000)
        XCTAssertTrue(model.isAwaitingReply(chatId), "a fresh ask shows the working cheat")
        XCTAssertFalse(model.missedReply(chatId), "not missed while still awaiting")

        let stale = nowMs() - AppModel.awaitingReplyMs - 1_000
        model.messages[chatId] = [user("u1", at: stale)]
        XCTAssertFalse(model.isAwaitingReply(chatId), "the bridge expired with no assistant row")
        XCTAssertTrue(model.missedReply(chatId), "ChatView must draw the didn't-answer line")
    }

    func testAWakingMachineKeepsTheWorkingCheatPastTheBridge() {
        let stale = nowMs() - AppModel.awaitingReplyMs - 5_000
        let model = modelEndingWithUser(at: stale)
        model.machine = Machine(state: .waking, since: stale, spec: "", reported: true, rawState: .waking)
        XCTAssertTrue(model.machineWaiting(for: chatId))
        XCTAssertTrue(model.isAwaitingReply(chatId), "cold wake must not expire into a blank chat")
        XCTAssertFalse(model.missedReply(chatId))
    }

    func testAWakingProjectKeepsTheWorkingCheatPastTheBridge() {
        let stale = nowMs() - AppModel.awaitingReplyMs - 5_000
        let model = modelEndingWithUser(at: stale)
        model.projects[0].state = .waking
        XCTAssertTrue(model.machineWaiting(for: chatId))
        XCTAssertTrue(model.isAwaitingReply(chatId))
        XCTAssertFalse(model.missedReply(chatId))
    }

    func testAStreamingAssistantEndsAwaitingAndMissed() {
        let at = nowMs() - 2_000
        let model = modelEndingWithUser(at: at)
        model.messages[chatId] = [user("u1", at: at), streamingAssistant("a1", at: at + 500)]
        XCTAssertTrue(model.isStreaming(chatId))
        XCTAssertFalse(model.isAwaitingReply(chatId))
        XCTAssertFalse(model.missedReply(chatId))
    }

    func testPeopleChatsNeverShowMissedReply() {
        let stale = nowMs() - AppModel.awaitingReplyMs - 5_000
        let model = AppModel()
        model.loadFixture()
        let dm = "fx-dm-unconst"
        model.messages[dm] = [
            Message(id: "dm-u1", chatId: dm, role: .user, sender: "user:fx-me", status: .done, turn: nil,
                    decision: nil, subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil,
                    createdAt: stale, completedAt: stale, pending: false,
                    parts: [.text(id: "dm-u1-t", text: "on my way")], hiddenCount: 0),
        ]
        XCTAssertNotNil(model.peopleChat(dm))
        XCTAssertFalse(model.isAwaitingReply(dm))
        XCTAssertFalse(model.missedReply(dm), "a human DM has no agent to miss")
    }
}
