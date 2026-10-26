import XCTest

/// Swipe right from the left edge (the system interactive pop) goes back one level on every
/// chat screen (feedback `iphone-back-swipe-restore-2026-10-25`, #1831). Runs the fixture app
/// (`ARBOS_FIXTURE=1`, Debug builds): no relay, deterministic screens.
///
/// Screens are told apart by their back chevron: `chat-back:<chatId>` on a chat,
/// `server-back:<serverId>` on a server's channel list, the `Agents` / `People` filter on Home.
final class BackSwipeUITests: XCTestCase {
    /// The shared Mac runs the gate and releases at the same time; waits are generous.
    private let appear: TimeInterval = 90
    private let settle: TimeInterval = 20

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launch(_ env: [String: String]) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1"].merging(env) { _, new in new }
        app.launch()
        return app
    }

    /// One finger from the very left edge to past the middle: the edge-pan band is ~20 pt.
    private func edgeSwipeBack(_ app: XCUIApplication) {
        let from = app.coordinate(withNormalizedOffset: CGVector(dx: 0.0, dy: 0.45))
        let to = app.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.45))
        from.press(forDuration: 0.05, thenDragTo: to, withVelocity: .fast, thenHoldForDuration: 0.05)
    }

    private func chatBack(_ app: XCUIApplication, _ chatId: String) -> XCUIElement {
        app.buttons["chat-back:\(chatId)"]
    }

    private func assertHome(_ app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertTrue(app.buttons["Agents"].waitForExistence(timeout: settle), "Home's list filter is on screen", file: file, line: line)
        XCTAssertTrue(app.buttons["People"].exists, file: file, line: line)
    }

    private func assertGone(_ element: XCUIElement, file: StaticString = #filePath, line: UInt = #line) {
        let gone = NSPredicate(format: "exists == false")
        let wait = XCTNSPredicateExpectation(predicate: gone, object: element)
        XCTAssertEqual(XCTWaiter().wait(for: [wait], timeout: settle), .completed, "\(element) left the screen", file: file, line: line)
    }

    // MARK: Agent 1:1

    func testAgentChatEdgeSwipeGoesHome() {
        let app = launch(["ARBOS_OPEN_PROJECT": "fx-comms"])
        let back = chatBack(app, "fx-comms-main")
        XCTAssertTrue(back.waitForExistence(timeout: appear), "the Comms chat opened")
        edgeSwipeBack(app)
        assertGone(back)
        assertHome(app)
    }

    /// The regression: after a send the composer keeps focus, and #1554 turned the edge
    /// pop off while it was focused — so no chat could be left by swipe once typed in.
    func testAgentChatEdgeSwipeGoesHomeWithTheKeyboardUp() {
        let app = launch(["ARBOS_OPEN_PROJECT": "fx-comms", "ARBOS_SHOW": "keyboard"])
        let back = chatBack(app, "fx-comms-main")
        XCTAssertTrue(back.waitForExistence(timeout: appear), "the Comms chat opened")
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: settle), "the composer is focused")
        edgeSwipeBack(app)
        assertGone(back)
        assertHome(app)
    }

    // MARK: Group

    func testGroupChatEdgeSwipeGoesHome() {
        let app = launch(["ARBOS_OPEN_PEOPLE": "fx-group-launch"])
        let back = chatBack(app, "fx-group-launch")
        XCTAssertTrue(back.waitForExistence(timeout: appear), "the Launch week group opened")
        edgeSwipeBack(app)
        assertGone(back)
        assertHome(app)
    }

    // MARK: Server

    /// A channel chat sits over its server's channel list: one swipe per level.
    func testServerChannelEdgeSwipeGoesToTheChannelListThenHome() {
        let app = launch(["ARBOS_FIXTURE_SERVERS": "1", "ARBOS_OPEN_SERVER": "fx-server-hq:#general"])
        let channelBack = chatBack(app, "fx-server-hq-general")
        XCTAssertTrue(channelBack.waitForExistence(timeout: appear), "#general of Arbos HQ opened")
        edgeSwipeBack(app)
        assertGone(channelBack)
        let listBack = app.buttons["server-back:fx-server-hq"]
        XCTAssertTrue(listBack.waitForExistence(timeout: settle), "the server's channel list is one level up")
        edgeSwipeBack(app)
        assertGone(listBack)
        assertHome(app)
    }

    /// Jacob's 23:54 report was a server channel (#general) with the keyboard up: same gate,
    /// same fix — the channel screen itself, not only the channel list.
    func testServerChannelEdgeSwipeGoesBackWithTheKeyboardUp() {
        let app = launch(["ARBOS_FIXTURE_SERVERS": "1", "ARBOS_OPEN_SERVER": "fx-server-hq:#general",
                          "ARBOS_SHOW": "keyboard"])
        let channelBack = chatBack(app, "fx-server-hq-general")
        XCTAssertTrue(channelBack.waitForExistence(timeout: appear), "#general of Arbos HQ opened")
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: settle), "the channel composer is focused")
        edgeSwipeBack(app)
        assertGone(channelBack)
        XCTAssertTrue(app.buttons["server-back:fx-server-hq"].waitForExistence(timeout: settle), "back from #general is the channel list")
    }

    // MARK: Job tab

    /// A job tab (#1801) is a chat screen pushed over its agent's chat: back lands on the agent.
    func testJobTabEdgeSwipeGoesToTheAgentChatThenHome() {
        let app = launch(["ARBOS_FIXTURE_JOBS": "3", "ARBOS_OPEN_PROJECT": "fx-comms",
                          "ARBOS_OPEN_CHAT": "fx-comms-main", "ARBOS_SHOW": "jobtab"])
        let jobBack = chatBack(app, "job:fx-job-surf")
        XCTAssertTrue(jobBack.waitForExistence(timeout: appear), "the surf-schools job tab opened over Comms")
        edgeSwipeBack(app)
        assertGone(jobBack)
        let agentBack = chatBack(app, "fx-comms-main")
        XCTAssertTrue(agentBack.waitForExistence(timeout: settle), "back from a job tab is the agent's chat")
        edgeSwipeBack(app)
        assertGone(agentBack)
        assertHome(app)
    }
}
