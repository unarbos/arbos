import SwiftUI
import XCTest
@testable import Arbos

/// Tester bug ([feedback reaction-chip-anchor-ios-2026-10-29], Scout, Arbonites): reaction
/// chips must sit under their own bubble, on its leading edge. Someone else's file or video
/// card in a people chat sits after the avatar lane, but its chips started at the screen edge.
/// Every row that carries reactions must start its chips where the layout puts the bubble:
/// the avatar lane for another person's or an agent's row in a people chat (grouped
/// continuations included), 0 for mine and for the 1:1 agent chat.
@MainActor
final class ReactionChipAnchorTests: XCTestCase {
    private let chat = "fx-group-arbonites"
    private let me = "user:fx-me"
    private let t0: Int64 = 1_700_000_000_000
    private let inset: CGFloat = 16
    private let pdf = Attachment(name: "plan.pdf", mime: "application/pdf", blobRef: "blob-pdf", kind: "file")
    private let video = Attachment(name: "clip.mov", mime: "video/quicktime", blobRef: "blob-mov", kind: "video")
    private let picture = Attachment(name: "IMG_1.jpeg", mime: "image/jpeg", blobRef: "blob-jpg", kind: "image")

    private func reacted(_ m: Message) -> Message {
        var m = m
        m.reactions = [MessageReaction(emoji: "🥹", senders: ["user:fx-sam"])]
        return m
    }

    private func person(_ i: Int, _ sender: String, _ parts: [Part]) -> Message {
        reacted(Message(id: "m\(i)", chatId: chat, role: .user, sender: sender, status: .done, turn: nil, decision: nil,
                        subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil,
                        createdAt: t0 + Int64(i) * 10_000, completedAt: nil, pending: false, parts: parts,
                        hiddenCount: 0, senderHandle: sender == me ? "me" : "ben", senderName: sender == me ? "Me" : "Ben"))
    }

    private func agent(_ i: Int, _ parts: [Part]) -> Message {
        reacted(Message(id: "m\(i)", chatId: chat, role: .assistant, sender: "agent:fx-comms", status: .done, turn: .reply,
                        decision: "reply", subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil,
                        createdAt: t0 + Int64(i) * 10_000, completedAt: nil, pending: false, parts: parts,
                        hiddenCount: 0, senderHandle: "comms", senderName: "Comms"))
    }

    private func text(_ i: Int) -> Part { .text(id: "m\(i)-t", text: "line \(i)") }
    private func file(_ i: Int, _ a: Attachment) -> Part { .attachment(id: "m\(i)-a", attachment: a) }

    private func chipRows(_ messages: [Message], people: Bool) -> [Row] {
        Transcript.rows(messages, projectId: people ? "people:\(chat)" : "p1", me: me, people: people,
                        multiParty: people)
            .filter { !$0.reactions.isEmpty }
    }

    func testChipsStartOnTheBubblesLeadingEdgeInAPeopleGroup() {
        let lane = Theme.memberLane(composerInset: inset)
        let ben = "user:fx-ben"
        let messages = [
            person(0, ben, [text(0)]),
            person(1, ben, [text(1)]),
            person(2, ben, [file(2, pdf)]),
            person(3, ben, [file(3, video)]),
            person(4, ben, [file(4, picture)]),
            agent(5, [text(5)]),
            agent(6, [file(6, pdf)]),
            person(7, me, [text(7)]),
            person(8, me, [file(8, pdf)]),
        ]
        let rows = chipRows(messages, people: true)
        XCTAssertEqual(rows.map(\.messageId), messages.map(\.id), "every message carries one chips row")
        XCTAssertFalse(rows[1].showsAvatar, "Ben's second line continues his run (empty avatar column)")
        for row in rows {
            let mine = row.messageId == "m7" || row.messageId == "m8"
            XCTAssertEqual(RowView.reactionLeading(for: row.kind, composerInset: inset), mine ? 0 : lane,
                           "\(row.messageId) (\(row.kind)): chips must start where its bubble does")
        }
    }

    func testChipsStartAtTheEdgeInTheAgentChat() {
        let messages = [agent(0, [text(0)]), agent(1, [file(1, pdf)]), person(2, me, [text(2)])]
        for row in chipRows(messages, people: false) {
            XCTAssertEqual(RowView.reactionLeading(for: row.kind, composerInset: inset), 0,
                           "\(row.messageId): no avatar lane in a 1:1 agent chat")
        }
    }
}
