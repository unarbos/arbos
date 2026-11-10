import XCTest

/// Tapping a "Messaged ◉ <agent>" notice pushes that agent's chat from the right with
/// swipe-back, and highlights the destination message
/// ([feedback iphone-messaged-notice-nav-2026-11-10]; Jacob, 2026-11-10).
///
/// Runs the fixture app (`ARBOS_FIXTURE=1`, Debug builds): no relay, deterministic screens.
final class MessagedNoticeNavUITests: XCTestCase {
    private let appear: TimeInterval = 90
    private let settle: TimeInterval = 20
    private let sourceChat = "fx-comms-main"
    private let destChat = "fx-affine-main"
    private let handoffId = "c3"
    private let destMessage = "fx-affine-from-comms"

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launch() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = [
            "ARBOS_FIXTURE": "1",
            "ARBOS_OPEN_CHAT": sourceChat,
        ]
        app.launch()
        XCTAssertTrue(app.buttons["chat-back:\(sourceChat)"].waitForExistence(timeout: appear),
                      "Comms's chat opened")
        return app
    }

    /// One finger from the very left edge to past the middle — the system's interactive pop.
    private func edgeSwipeBack(_ app: XCUIApplication, y: CGFloat = 0.45) {
        let from = app.coordinate(withNormalizedOffset: CGVector(dx: 0.0, dy: y))
        let to = app.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: y))
        from.press(forDuration: 0.05, thenDragTo: to, withVelocity: .fast, thenHoldForDuration: 0.05)
    }

    /// [feedback iphone-messaged-notice-nav-2026-11-10]
    func testMessagedNoticePushesDestinationAndHighlights() {
        let app = launch()
        let notice = app.descendants(matching: .any)["handoff-notice:\(handoffId)"].firstMatch
        XCTAssertTrue(notice.waitForExistence(timeout: settle),
                      "the Messaged ◉ Affine notice is on screen and marked tappable")
        notice.tap()

        let destBack = app.buttons["chat-back:\(destChat)"]
        XCTAssertTrue(destBack.waitForExistence(timeout: settle),
                      "Affine's chat was pushed from the right")
        // Source chat stays under the push (not replaced): its back control is covered.
        XCTAssertFalse(app.buttons["chat-back:\(sourceChat)"].isHittable,
                       "Comms stays under the pushed Affine chat")

        let bubble = app.descendants(matching: .any)["bubble:\(destMessage)"].firstMatch
        XCTAssertTrue(bubble.waitForExistence(timeout: settle),
                      "the landed message Bob/Comms sent is on screen")
        XCTAssertTrue(bubble.isHittable, "the landed message is visible in the viewport")

        edgeSwipeBack(app)
        let sourceBack = app.buttons["chat-back:\(sourceChat)"]
        XCTAssertTrue(sourceBack.waitForExistence(timeout: settle),
                      "edge-swipe returns to Comms")
        let gone = NSPredicate(format: "exists == false")
        let wait = XCTNSPredicateExpectation(predicate: gone, object: destBack)
        XCTAssertEqual(XCTWaiter().wait(for: [wait], timeout: settle), .completed,
                       "Affine left the stack after swipe-back")
    }
}
