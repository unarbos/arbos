import XCTest

/// Back gestures on every chat screen (Jacob, 2026-10-28: the system's interactive edge pop,
/// finger-tracking and cancellable, is the one owner of the edge; a swipe right anywhere
/// else is an animated "back"; there is no swipe-to-reply — Reply stays on long-press and in
/// the actions menu). Runs the fixture app (`ARBOS_FIXTURE=1`, Debug builds): no relay,
/// deterministic screens.
///
/// Screens are told apart by their back chevron: `chat-back:<chatId>` on a chat,
/// `server-back:<serverId>` on a server's channel list, the `Agents` / `People` filter on Home.
/// The composer field is `composer-field`, its reply quote `composer-reply-preview`, a
/// message bubble `bubble:<messageId>`.
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

    /// One finger from the very left edge to past the middle, at `y` (normalized).
    private func edgeSwipeBack(_ app: XCUIApplication, y: CGFloat = 0.45) {
        let from = app.coordinate(withNormalizedOffset: CGVector(dx: 0.0, dy: y))
        let to = app.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: y))
        from.press(forDuration: 0.05, thenDragTo: to, withVelocity: .fast, thenHoldForDuration: 0.05)
    }

    private func chatBack(_ app: XCUIApplication, _ chatId: String) -> XCUIElement {
        app.buttons["chat-back:\(chatId)"]
    }

    private var bubbleQuery: NSPredicate { NSPredicate(format: "identifier BEGINSWITH 'bubble:'") }

    /// The lowest bubble fully on screen (the newest message of the open chat).
    private func lastBubble(_ app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line) -> XCUIElement {
        let bubbles = app.descendants(matching: .any).matching(bubbleQuery)
        XCTAssertTrue(bubbles.firstMatch.waitForExistence(timeout: settle), "the transcript shows bubbles", file: file, line: line)
        let window = app.windows.firstMatch.frame
        let visible = bubbles.allElementsBoundByIndex.filter { $0.isHittable && $0.frame.maxY < window.maxY - 120 && $0.frame.minY > 120 }
        XCTAssertFalse(visible.isEmpty, "a bubble is hittable", file: file, line: line)
        return visible.last!
    }

    /// Tap the composer so the keyboard is up (what every sent message leaves behind).
    private func raiseKeyboard(_ app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line) {
        let field = app.descendants(matching: .any)["composer-field"]
        XCTAssertTrue(field.waitForExistence(timeout: settle), "the composer field is there", file: file, line: line)
        field.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: settle), "the keyboard came up", file: file, line: line)
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

    /// The keyboard goes down with the pop, not after it.
    private func assertKeyboardGone(_ app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line) {
        assertGone(app.keyboards.firstMatch, file: file, line: line)
    }

    // MARK: Edge back, keyboard down and up, per chat kind

    func testAgentChatEdgeSwipeGoesHome() {
        let app = launch(["ARBOS_OPEN_PROJECT": "fx-comms"])
        let back = chatBack(app, "fx-comms-main")
        XCTAssertTrue(back.waitForExistence(timeout: appear), "the Comms chat opened")
        edgeSwipeBack(app)
        assertGone(back)
        assertHome(app)
    }

    /// The regression (#1831): after a send the composer keeps focus; the edge pop must
    /// still go back, and the keyboard goes with it.
    func testAgentChatEdgeSwipeGoesHomeWithTheKeyboardUp() {
        let app = launch(["ARBOS_OPEN_PROJECT": "fx-comms"])
        XCTAssertTrue(chatBack(app, "fx-comms-main").waitForExistence(timeout: appear), "the Comms chat opened")
        raiseKeyboard(app)
        edgeSwipeBack(app)
        assertGone(chatBack(app, "fx-comms-main"))
        assertKeyboardGone(app)
        assertHome(app)
    }

    func testDMEdgeSwipeGoesHomeWithTheKeyboardUp() {
        let app = launch(["ARBOS_OPEN_PEOPLE": "fx-dm-unconst"])
        XCTAssertTrue(chatBack(app, "fx-dm-unconst").waitForExistence(timeout: appear), "the @unconst DM opened")
        raiseKeyboard(app)
        edgeSwipeBack(app)
        assertGone(chatBack(app, "fx-dm-unconst"))
        assertKeyboardGone(app)
        assertHome(app)
    }

    func testGroupChatEdgeSwipeGoesHome() {
        let app = launch(["ARBOS_OPEN_PEOPLE": "fx-group-launch"])
        let back = chatBack(app, "fx-group-launch")
        XCTAssertTrue(back.waitForExistence(timeout: appear), "the Launch week group opened")
        edgeSwipeBack(app)
        assertGone(back)
        assertHome(app)
    }

    func testGroupChatEdgeSwipeGoesHomeWithTheKeyboardUp() {
        let app = launch(["ARBOS_OPEN_PEOPLE": "fx-group-launch"])
        XCTAssertTrue(chatBack(app, "fx-group-launch").waitForExistence(timeout: appear), "the Launch week group opened")
        raiseKeyboard(app)
        edgeSwipeBack(app)
        assertGone(chatBack(app, "fx-group-launch"))
        assertKeyboardGone(app)
        assertHome(app)
    }

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

    func testServerChannelEdgeSwipeGoesBackWithTheKeyboardUp() {
        let app = launch(["ARBOS_FIXTURE_SERVERS": "1", "ARBOS_OPEN_SERVER": "fx-server-hq:#general"])
        XCTAssertTrue(chatBack(app, "fx-server-hq-general").waitForExistence(timeout: appear), "#general of Arbos HQ opened")
        raiseKeyboard(app)
        edgeSwipeBack(app)
        assertGone(chatBack(app, "fx-server-hq-general"))
        assertKeyboardGone(app)
        XCTAssertTrue(app.buttons["server-back:fx-server-hq"].waitForExistence(timeout: settle), "back from #general is the channel list")
    }

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

    func testJobTabEdgeSwipeGoesToTheAgentChatWithTheKeyboardUp() {
        let app = launch(["ARBOS_FIXTURE_JOBS": "3", "ARBOS_OPEN_PROJECT": "fx-comms",
                          "ARBOS_OPEN_CHAT": "fx-comms-main", "ARBOS_SHOW": "jobtab"])
        XCTAssertTrue(chatBack(app, "job:fx-job-surf").waitForExistence(timeout: appear), "the surf-schools job tab opened over Comms")
        raiseKeyboard(app)
        edgeSwipeBack(app)
        assertGone(chatBack(app, "job:fx-job-surf"))
        assertKeyboardGone(app)
        XCTAssertTrue(chatBack(app, "fx-comms-main").waitForExistence(timeout: settle), "back from a job tab is the agent's chat")
    }

    // MARK: The edge pop is the system's: interactive and cancellable

    /// A short edge drag released short of halfway cancels: the chat stays, settles back
    /// where it was, and the next edge swipe still pops. (Main's edge rule popped after
    /// 40 pt whatever the release — a threshold, not a finger-tracking transition.)
    func testAPartialEdgeSwipeReleasedCancelsCleanly() {
        let app = launch(["ARBOS_OPEN_PROJECT": "fx-comms"])
        let back = chatBack(app, "fx-comms-main")
        XCTAssertTrue(back.waitForExistence(timeout: appear), "the Comms chat opened")
        let restX = back.frame.minX
        let from = app.coordinate(withNormalizedOffset: CGVector(dx: 0.0, dy: 0.45))
        // 60 pt of a 393 pt screen: well under the halfway point; the hold kills the velocity.
        from.press(forDuration: 0.05, thenDragTo: from.withOffset(CGVector(dx: 60, dy: 0)), withVelocity: .slow, thenHoldForDuration: 0.6)
        sleep(1)
        XCTAssertTrue(back.exists, "the chat did not go back")
        XCTAssertEqual(back.frame.minX, restX, accuracy: 1, "the screen settled back where it was")
        XCTAssertTrue(back.isHittable, "the chat is usable again")
        XCTAssertFalse(app.descendants(matching: .any)["composer-reply-preview"].exists, "no reply started")
        edgeSwipeBack(app)
        assertGone(back)
        assertHome(app)
    }

    /// An edge swipe while the transcript is still decelerating from a flick still pops.
    func testEdgeSwipeWhileScrollingGoesBack() {
        let app = launch(["ARBOS_OPEN_PEOPLE": "fx-group-launch", "ARBOS_FIXTURE_LONG_ROWS": "120"])
        let back = chatBack(app, "fx-group-launch")
        XCTAssertTrue(back.waitForExistence(timeout: appear), "the Launch week group opened")
        let bubbles = app.descendants(matching: .any).matching(bubbleQuery)
        XCTAssertTrue(bubbles.firstMatch.waitForExistence(timeout: settle), "the transcript shows bubbles")
        let window = app.windows.firstMatch.frame
        let mid = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.6))
        // A fast flick up: the scroll view decelerates for a second or more.
        mid.press(forDuration: 0.02, thenDragTo: mid.withOffset(CGVector(dx: 0, dy: -window.height * 0.4)), withVelocity: .fast, thenHoldForDuration: 0)
        edgeSwipeBack(app)
        assertGone(back)
        assertHome(app)
    }

    // MARK: A swipe right away from the edge is also back; nothing replies

    /// Swipe right on a message bubble, away from the edge: the chat goes back (the
    /// full-screen rule); no reply starts, the composer stays unfocused.
    func testSwipeRightOnABubbleGoesBackAndStartsNoReply() {
        let app = launch(["ARBOS_OPEN_PEOPLE": "fx-group-launch"])
        let back = chatBack(app, "fx-group-launch")
        XCTAssertTrue(back.waitForExistence(timeout: appear), "the Launch week group opened")
        let bubbles = app.descendants(matching: .any).matching(bubbleQuery)
        XCTAssertTrue(bubbles.firstMatch.waitForExistence(timeout: settle), "the transcript shows bubbles")
        let window = app.windows.firstMatch.frame
        // A bubble in the lower half, clear of the composer; the drag starts on its left
        // part and stays inside the screen (a drag that leaves the screen is not a swipe).
        let bubble = bubbles.allElementsBoundByIndex.last { $0.frame.minY > window.midY && $0.frame.maxY < window.maxY - 160 }
        XCTAssertNotNil(bubble, "a bubble is on screen")
        let start = bubble!.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.5))
        let dx = min(180, window.maxX - 24 - (bubble!.frame.minX + bubble!.frame.width * 0.3))
        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: dx, dy: 0)), withVelocity: .fast, thenHoldForDuration: 0.05)
        assertGone(back)
        XCTAssertFalse(app.descendants(matching: .any)["composer-reply-preview"].exists, "no reply started")
        XCTAssertFalse(app.keyboards.firstMatch.exists, "the composer was not focused")
        assertHome(app)
    }

    /// Reply is still reachable: long-press a bubble → the actions menu shows Reply.
    func testLongPressStillOffersReply() {
        let app = launch(["ARBOS_OPEN_PROJECT": "fx-comms"])
        XCTAssertTrue(chatBack(app, "fx-comms-main").waitForExistence(timeout: appear), "the Comms chat opened")
        let bubble = lastBubble(app)
        bubble.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).press(forDuration: 0.9)
        XCTAssertTrue(app.descendants(matching: .any)["message-actions-menu"].waitForExistence(timeout: settle), "the actions menu opened")
        // The menu rows are buttons labelled by their item; the container carries the identifier.
        let reply = app.buttons.matching(NSPredicate(format: "identifier == 'message-menu-reply' OR label == 'Reply'")).firstMatch
        XCTAssertTrue(reply.waitForExistence(timeout: settle), "Reply is in the menu")
    }

    /// The edge zone is the edge pop's: a swipe from the edge at a bubble's height goes
    /// back and starts no reply.
    func testEdgeSwipeOverABubbleGoesBack() {
        let app = launch(["ARBOS_OPEN_PROJECT": "fx-comms"])
        let back = chatBack(app, "fx-comms-main")
        XCTAssertTrue(back.waitForExistence(timeout: appear), "the Comms chat opened")
        let bubble = lastBubble(app)
        let y = bubble.frame.midY / app.windows.firstMatch.frame.height
        edgeSwipeBack(app, y: y)
        assertGone(back)
        XCTAssertFalse(app.descendants(matching: .any)["composer-reply-preview"].exists, "no reply started")
        assertHome(app)
    }

    /// A vertical drag on a bubble is left to the scroll view: no keyboard, no back.
    func testVerticalDragOnABubbleDoesNotGoBack() {
        let app = launch(["ARBOS_OPEN_PEOPLE": "fx-group-launch", "ARBOS_FIXTURE_LONG_ROWS": "120"])
        let back = chatBack(app, "fx-group-launch")
        XCTAssertTrue(back.waitForExistence(timeout: appear), "the Launch week group opened")
        let bubbles = app.descendants(matching: .any).matching(bubbleQuery)
        XCTAssertTrue(bubbles.firstMatch.waitForExistence(timeout: settle), "the transcript shows bubbles")
        let window = app.windows.firstMatch.frame
        // A bubble in the lower half of the transcript, clear of the composer.
        let onBubble = bubbles.allElementsBoundByIndex.last { $0.frame.minY > window.midY && $0.frame.maxY < window.maxY - 200 }
        XCTAssertNotNil(onBubble, "a bubble is on screen")
        let start = onBubble!.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: 6, dy: -200)), withVelocity: .slow, thenHoldForDuration: 0.1)
        sleep(1)
        XCTAssertFalse(app.descendants(matching: .any)["composer-reply-preview"].exists, "no reply started")
        XCTAssertFalse(app.keyboards.firstMatch.exists, "the composer was not focused")
        XCTAssertTrue(back.exists, "the chat did not go back")
    }

    // MARK: Frame timing of the pop

    /// UIKit's navigation-transition signposts over the edge-swipe pop: duration, hitch time
    /// ratio, frame rate. Each iteration relaunches outside the measured window so only the
    /// pop is measured. Read the numbers in the xcodebuild log (`measured [...]`).
    func testEdgeSwipeBackTransitionTiming() {
        let options = XCTMeasureOptions()
        options.iterationCount = 5
        options.invocationOptions = [.manuallyStart, .manuallyStop]
        measure(metrics: [XCTOSSignpostMetric.navigationTransitionMetric, XCTClockMetric()], options: options) {
            let app = launch(["ARBOS_OPEN_PEOPLE": "fx-group-launch"])
            let back = chatBack(app, "fx-group-launch")
            XCTAssertTrue(back.waitForExistence(timeout: appear), "the Launch week group opened")
            sleep(1)
            startMeasuring()
            edgeSwipeBack(app)
            assertGone(back)
            assertHome(app)
            stopMeasuring()
            app.terminate()
        }
    }
}
