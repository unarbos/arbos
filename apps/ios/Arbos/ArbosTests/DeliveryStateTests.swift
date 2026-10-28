import XCTest
@testable import Arbos

/// Jacob, 2026-10-24 (feedback `sent-state-opacity-2026-10-24`) and 2026-10-25 (feedback
/// `sent-full-when-delivered-2026-10-25`): my own bubble says where it is by its opacity, not
/// by a caption — pending dimmed, delivered full at once (read receipts, agent replies and
/// group membership never change it), failed full with a red "! Retry".
final class DeliveryStateTests: XCTestCase {
    private let chat = "fx-group-launch"
    private let t0: Int64 = 1_700_000_000_000

    private func msg(_ id: String, role: Role = .user, sender: String = "user:fx-me", status: MessageStatus = .done,
                     pending: Bool = false, at: Int64) -> Message {
        Message(id: id, chatId: chat, role: role, sender: sender, status: status, turn: nil, decision: nil, subagentId: nil,
                subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: at, completedAt: nil,
                pending: pending, parts: [.text(id: "\(id)-t", text: "I did lol")], hiddenCount: 0, senderHandle: "me", senderName: "Me")
    }

    func testOnlyPendingIsDimmed() {
        XCTAssertEqual(Delivery.delivered.opacity, 1)
        XCTAssertEqual(Delivery.failed.opacity, 1)
        XCTAssertEqual(Delivery.pending.opacity, Delivery.pendingAlpha)
        XCTAssertLessThan(Delivery.pending.opacity, Delivery.delivered.opacity)
    }

    func testAnAckedBubbleIsDeliveredWhateverFollows() {
        // Jacob's Arbonites shape (2026-10-25): mine, mine, an agent reply, mine — no receipts.
        let a = msg("a", at: t0), b = msg("b", at: t0 + 1_000), c = msg("c", at: t0 + 3_000)
        for m in [a, b, c] { XCTAssertEqual(Delivery.of(m), .delivered, "\(m.id): full style at once") }
    }

    func testPendingAndFailedAreTheOnlyOtherStates() {
        XCTAssertEqual(Delivery.of(msg("p", status: .queued, pending: true, at: t0)), .pending)
        XCTAssertEqual(Delivery.of(msg("f", status: .error, at: t0 + 1)), .failed)
    }

    func testRowsCarryTheStateOnMyBubblesOnly() {
        let mine = msg("a", at: t0)
        let other = msg("o", sender: "user:fx-u3", at: t0 + 1_000)
        let pending = msg("p", status: .queued, pending: true, at: t0 + 2_000)
        let rows = Transcript.rows([mine, other, pending], projectId: "people:\(chat)", me: "user:fx-me", people: true, multiParty: true)
        let states: [Delivery?] = rows.compactMap { row in
            switch row.kind {
            case .user(_, let delivery, _, _): return delivery
            case .other: return .some(nil)
            default: return nil
            }
        }
        XCTAssertEqual(states, [.delivered, nil, .pending])
    }
}
