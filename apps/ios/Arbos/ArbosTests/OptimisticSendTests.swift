import SwiftUI
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

/// The reaction chip is on the bubble the moment `toggleReaction` returns — before the
/// doc's change event, the coalesced re-read or the relay ACK
/// ([feedback iphone-reaction-instant-2026-11-13]; TestFlight
/// app-01M7B901FGH3KDZPX8T7DYXPPZ / #3048). Mirrors `OptimisticSendTests` for reactions.
@MainActor
final class OptimisticReactionTests: XCTestCase {
    private func fixtureModel(chatId: String) -> AppModel {
        let model = AppModel()
        model.loadFixture()
        model.fixtureAttachDoc(chatId: chatId)
        return model
    }

    private func rows(_ model: AppModel, _ chatId: String) -> [Row] {
        Transcript.rows(model.messages(of: chatId), projectId: "\(peopleSlot):\(chatId)", me: model.myUserId,
                        people: true, multiParty: true)
    }

    private func reactionOn(_ model: AppModel, chatId: String, messageId: Id, emoji: String) -> MessageReaction? {
        model.messages(of: chatId).first { $0.id == messageId }?
            .reactions.first { MessageReaction.sameBase($0.emoji, emoji) }
    }

    func testAddPaintsTheChipBeforeReturning() throws {
        let chatId = "fx-group-launch"
        let model = fixtureModel(chatId: chatId)
        let target = try XCTUnwrap(model.messages(of: chatId).first { $0.role == .user })
        let me = try XCTUnwrap(model.myUserId)
        let version = model.messagesVersion[chatId] ?? 0
        XCTAssertNil(reactionOn(model, chatId: chatId, messageId: target.id, emoji: "🔥"),
                     "start with no fire on this row")

        model.toggleReaction(chatId: chatId, messageId: target.id, emoji: "🔥")

        // No run-loop turn in between: this is what the view sees in the same frame as the tap.
        let chip = try XCTUnwrap(reactionOn(model, chatId: chatId, messageId: target.id, emoji: "🔥"))
        XCTAssertEqual(chip.emoji, "🔥")
        XCTAssertEqual(chip.senders, [me])
        XCTAssertGreaterThan(model.messagesVersion[chatId] ?? 0, version, "the view's change key moved with the paint")

        let built = rows(model, chatId)
        let row = try XCTUnwrap(built.first { $0.messageId == target.id && !$0.reactions.isEmpty })
        XCTAssertEqual(row.reactions.map(\.emoji), ["🔥"])
        XCTAssertEqual(row.reactions.first?.senders, [me])
    }

    func testRemovePaintsTheChipAwayBeforeReturning() throws {
        let chatId = "fx-group-launch"
        let model = fixtureModel(chatId: chatId)
        let target = try XCTUnwrap(model.messages(of: chatId).first { $0.role == .user })
        let me = try XCTUnwrap(model.myUserId)

        model.toggleReaction(chatId: chatId, messageId: target.id, emoji: "❤️")
        XCTAssertEqual(reactionOn(model, chatId: chatId, messageId: target.id, emoji: "❤️")?.senders, [me])

        model.toggleReaction(chatId: chatId, messageId: target.id, emoji: "❤️")

        XCTAssertNil(reactionOn(model, chatId: chatId, messageId: target.id, emoji: "❤️"),
                     "the chip is gone in the same turn as the second tap")
        let built = rows(model, chatId)
        let row = built.first { $0.messageId == target.id }
        XCTAssertEqual(row?.reactions.contains { MessageReaction.sameBase($0.emoji, "❤️") } ?? false, false)
    }

    func testPaintedReactionSurvivesTheDocRead() throws {
        let chatId = "fx-group-launch"
        let model = fixtureModel(chatId: chatId)
        let target = try XCTUnwrap(model.messages(of: chatId).first { $0.role == .user })
        let me = try XCTUnwrap(model.myUserId)

        model.toggleReaction(chatId: chatId, messageId: target.id, emoji: "💯")
        XCTAssertEqual(reactionOn(model, chatId: chatId, messageId: target.id, emoji: "💯")?.senders, [me])

        // Let the doc's change event, the coalesce window and the re-read run.
        let deadline = Date().addingTimeInterval(1.0)
        while Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.02)) }

        let chip = try XCTUnwrap(reactionOn(model, chatId: chatId, messageId: target.id, emoji: "💯"))
        XCTAssertEqual(chip.senders, [me], "the re-read keeps my reaction; it does not drop the paint")
    }
}

/// A reaction restamps the rows of its one message instead of rebuilding the transcript, and
/// the result is what the rebuild gives (tester report, 2026-11-16: the double-tap heart "is
/// just not smooth").
final class ReactionRowTests: XCTestCase {
    @MainActor func testAReactionRestampsItsMessageAsAFullRebuildWould() throws {
        let model = AppModel()
        model.loadFixture()
        let me = try XCTUnwrap(model.myUserId)
        let group = model.messages(of: "fx-group-launch")
        try restampMatchesRebuild(group, me: me) {
            Transcript.rows($0, projectId: "\(peopleSlot):fx-group-launch", me: me, people: true, multiParty: true)
        }

        // The same group with a member blocked: their messages fold into one row.
        let blocked = try XCTUnwrap(group.first { $0.sender != me && $0.role == .user }?.sender)
        let folded = Transcript.rows(group, projectId: "\(peopleSlot):fx-group-launch", me: me, people: true, multiParty: true, hiddenSenders: [blocked])
        XCTAssertTrue(folded.contains { if case .hidden = $0.kind { return true }; return false }, "the fixture folds a row")
        try restampMatchesRebuild(group, me: me) {
            Transcript.rows($0, projectId: "\(peopleSlot):fx-group-launch", me: me, people: true, multiParty: true, hiddenSenders: [blocked])
        }

        // An agent chat, with a reply made of tool calls around its prose and a subagent's reply.
        var agent = model.messages(of: "fx-comms-main")
        var tools = try XCTUnwrap(agent.last { $0.role == .assistant })
        tools.id = "tools"
        tools.parts = [.tool(id: "t1", call: ToolCall(id: "c1", name: "shell"), isError: false), .text(id: "t2", text: "Done."),
                       .tool(id: "t3", call: ToolCall(id: "c2", name: "read"), isError: true)]
        var sub = tools
        sub.id = "sub"
        sub.subagentId = "sub-1"
        sub.subagentName = "Scout"
        agent += [tools, sub]
        try restampMatchesRebuild(agent, me: me) { Transcript.rows($0, projectId: "fx-comms", me: me) }
    }

    private func restampMatchesRebuild(_ before: [Message], me: String, build: ([Message]) -> [Row],
                                       file: StaticString = #filePath, line: UInt = #line) throws {
        XCTAssertGreaterThan(before.count, 1, file: file, line: line)
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
                XCTAssertEqual(rows, build(list), "message \(i) after \(emoji)", file: file, line: line)
            }
        }
    }

    @MainActor func testOnlyAReactionOnOneMessageIsTakenForOne() throws {
        let model = AppModel()
        model.loadFixture()
        let me = try XCTUnwrap(model.myUserId)
        let before = model.messages(of: "fx-group-launch")

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

    /// In the open chat a reaction takes the restamp branch of `refreshRows`, on the newest
    /// message (where the tail rebuild used to take it) and on an earlier one (the full pass).
    @MainActor func testAReactionInTheOpenChatTakesTheReactionPath() async throws {
        let model = AppModel()
        model.loadFixture()
        let chatId = "fx-comms-main"
        let scene = try XCTUnwrap(UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.first)
        let window = UIWindow(windowScene: scene)
        window.frame = CGRect(x: 0, y: 0, width: 390, height: 844)
        window.rootViewController = UIHostingController(rootView: ChatView(chatId: chatId, projectId: "fx-comms").environmentObject(model))
        window.makeKeyAndVisible()
        defer { window.isHidden = true }
        func path(after change: () -> Void) async -> PerfSignpost.RowsPath? {
            try? await Task.sleep(for: .milliseconds(300))
            PerfSignpost.lastRowsPath = nil
            change()
            for _ in 0..<300 where PerfSignpost.lastRowsPath == nil { try? await Task.sleep(for: .milliseconds(10)) }
            return PerfSignpost.lastRowsPath
        }
        let first = await path {}
        _ = first
        let list = model.messages(of: chatId)
        let newest = try XCTUnwrap(list.last).id
        let earlier = try XCTUnwrap(list.first).id
        XCTAssertNotEqual(newest, earlier)
        for id in [newest, earlier, newest] {
            let took = await path { model.toggleReaction(chatId: chatId, messageId: id, emoji: "❤️") }
            XCTAssertEqual(took, .reaction, "message \(id)")
        }
    }

    /// Two quick taps on a chip are the chip's (on, then off again), not a heart for the
    /// message; a chip of another message does not stand in the way (tester report,
    /// 2026-11-16, "double reaction").
    func testADoubleTapWithATapOnThatMessagesChipInItIsNotAHeart() {
        defer { ChatGestures.chipTap = nil }
        XCTAssertFalse(ChatGestures.doubleTapOnChip(of: "m1", now: 100))
        ChatGestures.chipTap = ("m1", 100)
        XCTAssertTrue(ChatGestures.doubleTapOnChip(of: "m1", now: 100.3))
        XCTAssertFalse(ChatGestures.doubleTapOnChip(of: "m2", now: 100.3), "another message's double-tap is a heart")
        XCTAssertFalse(ChatGestures.doubleTapOnChip(of: "m1", now: 101), "a later double-tap on the bubble is")
    }
}
