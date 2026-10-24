import XCTest
@testable import Arbos

/// Jacob, 2026-10-24 (feedback `sent-state-opacity-2026-10-24`): my own bubble says where it
/// is by its opacity, not by a caption — pending lowest, sent-but-unseen lighter, seen (a
/// peer's watermark covers it or an agent answered after it) full; failed is full with a mark.
final class DeliveryStateTests: XCTestCase {
    private let chat = "fx-dm-sam"
    private let t0: Int64 = 1_700_000_000_000

    private func msg(_ id: String, role: Role = .user, sender: String = "user:fx-me", status: MessageStatus = .done,
                     pending: Bool = false, at: Int64) -> Message {
        Message(id: id, chatId: chat, role: role, sender: sender, status: status, turn: nil, decision: nil, subagentId: nil,
                subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: at, completedAt: nil,
                pending: pending, parts: [.text(id: "\(id)-t", text: "fish?")], hiddenCount: 0, senderHandle: "me", senderName: "Me")
    }

    func testOpacityIsTheState() {
        XCTAssertEqual(Delivery.seen.opacity, 1)
        XCTAssertEqual(Delivery.failed.opacity, 1)
        XCTAssertEqual(Delivery.sent.opacity, Delivery.sentAlpha)
        XCTAssertEqual(Delivery.pending.opacity, Delivery.pendingAlpha)
        XCTAssertLessThan(Delivery.pendingAlpha, Delivery.sentAlpha)
        XCTAssertLessThan(Delivery.sentAlpha, 1)
    }

    func testUnseenIsSentAndAWatermarkMakesItSeen() {
        let list = [msg("a", at: t0), msg("b", at: t0 + 1_000), msg("c", at: t0 + 2_000)]
        for m in list {
            XCTAssertEqual(Delivery.of(m, in: list, receipts: [:]), .sent, "nobody opened the chat: lighter")
        }
        // Sam's watermark is "b": a and b are seen, c is still only sent.
        let receipts = ["sam": "b"]
        XCTAssertEqual(list.map { Delivery.of($0, in: list, receipts: receipts) }, [.seen, .seen, .sent])
    }

    func testAnAgentReplyAfterMineMeansSeen() {
        let list = [msg("a", at: t0), msg("r", role: .assistant, sender: "agent:fx-comms", at: t0 + 500), msg("b", at: t0 + 1_000)]
        XCTAssertEqual(Delivery.of(list[0], in: list, receipts: [:]), .seen)
        XCTAssertEqual(Delivery.of(list[2], in: list, receipts: [:]), .sent)
    }

    func testPendingAndFailedAreUnchangedStates() {
        let pending = msg("p", status: .queued, pending: true, at: t0)
        let failed = msg("f", status: .error, at: t0 + 1)
        let list = [pending, failed]
        XCTAssertEqual(Delivery.of(pending, in: list, receipts: ["sam": "f"]), .pending, "pending wins over any watermark")
        XCTAssertEqual(Delivery.of(failed, in: list, receipts: ["sam": "f"]), .failed, "failed wins over any watermark")
    }

    func testReadReceiptsStoreKeepsOneWatermarkPerPeer() {
        let store = ReadReceipts()
        store.note(["handle": "sam", "seen": "a"], chatId: chat)
        store.note(["handle": "sam", "seen": "b"], chatId: chat)
        store.note(["handle": "mira", "seen": ""], chatId: chat)
        store.note(["seen": "c"], chatId: chat)
        XCTAssertEqual(store.seen[chat], ["sam": "b"])
        store.clear()
        XCTAssertTrue(store.seen.isEmpty)
    }
}
