import XCTest
@testable import Arbos

/// [feedback iphone-messaged-notice-nav-2026-11-10] — tapping a "Messaged ◉ X" notice
/// pushes the destination chat (swipe-back) and reveals the landed message when known.
@MainActor
final class MessagedNoticeNavTests: XCTestCase {
    private func model() -> AppModel {
        let m = AppModel()
        m.loadFixture()
        return m
    }

    func testHandoffWithDestinationIdsPushesAndArmsReveal() {
        let m = model()
        m.path = [.chat(chatId: "fx-comms-main", projectId: "fx-comms")]
        XCTAssertTrue(m.canOpenHandoff(fromChat: "fx-comms-main", messageId: "c3"))
        XCTAssertTrue(m.openHandoff(fromChat: "fx-comms-main", messageId: "c3"))
        XCTAssertEqual(m.path, [
            .chat(chatId: "fx-comms-main", projectId: "fx-comms"),
            .chat(chatId: "fx-affine-main", projectId: "fx-affine"),
        ], "the destination is pushed over the source chat for swipe-back")
        XCTAssertEqual(m.pendingReveal, MessageReveal(chatId: "fx-affine-main", messageId: "fx-affine-from-comms"))
    }

    func testOldHandoffWithoutIdsStillOpensTheTargetAgent() {
        let m = model()
        m.path = [.chat(chatId: "fx-comms-main", projectId: "fx-comms")]
        // Strip destination ids; leave toProjectId so the target agent is still known.
        var list = m.messages(of: "fx-comms-main")
        guard let i = list.firstIndex(where: { $0.id == "c3" }) else {
            return XCTFail("fixture hand-off c3")
        }
        list[i].agentMessage = AgentMessageRef(id: "am-old", toProjectId: "fx-affine", toName: "Affine",
                                               status: "delivered")
        m.messages["fx-comms-main"] = list
        // Also clear the landed copy so a scan cannot find it.
        m.messages["fx-affine-main"] = m.messages(of: "fx-affine-main").filter { $0.id != "fx-affine-from-comms" }

        XCTAssertTrue(m.openHandoff(fromChat: "fx-comms-main", messageId: "c3"))
        XCTAssertEqual(m.path.last, .chat(chatId: "fx-affine-main", projectId: "fx-affine"))
        XCTAssertNil(m.pendingReveal, "without a destination message id there is nothing to highlight")
    }

    func testScanFindsLandedMessageByDeliveryId() {
        let m = model()
        m.path = [.chat(chatId: "fx-comms-main", projectId: "fx-comms")]
        var list = m.messages(of: "fx-comms-main")
        guard let i = list.firstIndex(where: { $0.id == "c3" }) else {
            return XCTFail("fixture hand-off c3")
        }
        // Destination ids absent; delivery id still matches the landed Affine row.
        list[i].agentMessage = AgentMessageRef(id: "am-c3", toProjectId: "fx-affine", toName: "Affine",
                                               status: "delivered")
        m.messages["fx-comms-main"] = list

        XCTAssertTrue(m.openHandoff(fromChat: "fx-comms-main", messageId: "c3"))
        XCTAssertEqual(m.pendingReveal, MessageReveal(chatId: "fx-affine-main", messageId: "fx-affine-from-comms"))
    }

    func testHandoffRowBuildsATappableSystemLine() {
        let m = model()
        let rows = Transcript.rows(m.messages(of: "fx-comms-main"), projectId: "fx-comms")
        let handoff = rows.first { $0.id == "c3#handoff" }
        XCTAssertNotNil(handoff, "the fixture hand-off draws a system line")
        if case .system(_, let text, _) = handoff?.kind {
            XCTAssertTrue(text.contains("Messaged") || text.contains("messages with"), text)
        } else {
            XCTFail("expected a system hand-off row")
        }
    }
}
