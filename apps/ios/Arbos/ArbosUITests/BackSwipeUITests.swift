import XCTest

/// Horizontal gestures on every chat screen (#1831; Jacob's 2026-10-26 iMessage rule):
/// a swipe from the left edge goes back one level, keyboard open or closed; a swipe
/// right on a bubble away from the edge replies to it; the edge wins over the bubble;
/// vertical scroll is untouched. Runs the fixture app (`ARBOS_FIXTURE=1`, Debug builds):
/// no relay, deterministic screens.
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

    // MARK: Bubble swipe → reply

    /// Swipe right on a message, away from the edge: the composer opens with that
    /// message's quote and the keyboard; the chat stays.
    func testSwipeRightOnABubbleReplies() {
        let app = launch(["ARBOS_OPEN_PROJECT": "fx-comms"])
        let back = chatBack(app, "fx-comms-main")
        XCTAssertTrue(back.waitForExistence(timeout: appear), "the Comms chat opened")
        let bubble = lastBubble(app)
        let start = bubble.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        let end = start.withOffset(CGVector(dx: 150, dy: 0))
        start.press(forDuration: 0.05, thenDragTo: end, withVelocity: .slow, thenHoldForDuration: 0.1)
        let quote = app.descendants(matching: .any)["composer-reply-preview"]
        XCTAssertTrue(quote.waitForExistence(timeout: settle), "the reply quote is in the composer")
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: settle), "the composer is focused")
        XCTAssertTrue(back.exists, "the chat did not go back")
    }

    /// An own bubble that is itself a reply (quote block above the words): the swipe must
    /// start the reply from the quote line too, not only from the words under it —
    /// DaWizzaa's report, [feedback swipe-reply-own-quoted-2026-10-28]. The tap on the
    /// quote (jump to the original) is a SwiftUI `Button`; its press must not swallow a
    /// horizontal pan before the transcript's swipe recognizer sees it.
    func testSwipeRightOnAnOwnQuotedBubbleRepliesFromTheQuoteLine() {
        let app = launch(["ARBOS_OPEN_PEOPLE": "fx-dm-unconst"])
        let back = chatBack(app, "fx-dm-unconst")
        XCTAssertTrue(back.waitForExistence(timeout: appear), "the @unconst DM opened")
        let quote = app.descendants(matching: .any)["bubble:d4"].descendants(matching: .any)["reply-quote"]
        XCTAssertTrue(quote.waitForExistence(timeout: settle), "my reply to unconst shows its quote block")
        swipeRight(from: quote.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)))
        let preview = app.descendants(matching: .any)["composer-reply-preview"]
        XCTAssertTrue(preview.waitForExistence(timeout: settle), "the reply quote is in the composer")
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: settle), "the composer is focused")
        XCTAssertTrue(back.exists, "the chat did not go back")
        // The swipe is a swipe, not a tap: the quoted original must not be revealed (tinted)
        // on release — that was the quote `Button` firing after the pan.
        XCTAssertFalse(app.descendants(matching: .any)["bubble:d3"].isSelected, "the swipe did not also fire the quote's jump")
    }

    /// Same bubble, finger on the words under the quote.
    func testSwipeRightOnAnOwnQuotedBubbleRepliesFromTheWords() {
        let app = launch(["ARBOS_OPEN_PEOPLE": "fx-dm-unconst"])
        let back = chatBack(app, "fx-dm-unconst")
        XCTAssertTrue(back.waitForExistence(timeout: appear), "the @unconst DM opened")
        let bubble = app.descendants(matching: .any)["bubble:d4"]
        XCTAssertTrue(bubble.waitForExistence(timeout: settle), "my reply to unconst is on screen")
        let quote = bubble.descendants(matching: .any)["reply-quote"]
        XCTAssertTrue(quote.waitForExistence(timeout: settle), "it shows its quote block")
        // Halfway between the quote line's bottom and the bubble's bottom: on the words.
        let y = (quote.frame.maxY + bubble.frame.maxY) / 2
        let start = app.coordinate(withNormalizedOffset: .zero)
            .withOffset(CGVector(dx: quote.frame.midX, dy: y))
        swipeRight(from: start)
        XCTAssertTrue(app.descendants(matching: .any)["composer-reply-preview"].waitForExistence(timeout: settle),
                      "the reply quote is in the composer")
        XCTAssertTrue(back.exists, "the chat did not go back")
    }

    /// A tap on the quote block of my own reply is still the quote's: the original (d3)
    /// is revealed — it reads as selected while it is tinted — and no reply starts.
    func testTapOnTheQuoteOfAnOwnReplyRevealsTheOriginalAndStartsNoReply() {
        let app = launch(["ARBOS_OPEN_PEOPLE": "fx-dm-unconst"])
        let back = chatBack(app, "fx-dm-unconst")
        XCTAssertTrue(back.waitForExistence(timeout: appear), "the @unconst DM opened")
        let quote = app.descendants(matching: .any)["bubble:d4"].descendants(matching: .any)["reply-quote"]
        XCTAssertTrue(quote.waitForExistence(timeout: settle), "my reply to unconst shows its quote block")
        XCTAssertTrue(quote.isHittable, "the quote block takes taps")
        let original = app.descendants(matching: .any)["bubble:d3"]
        XCTAssertFalse(original.isSelected, "the original is not tinted before the tap")
        quote.tap()
        let tinted = NSPredicate(format: "isSelected == true")
        let wait = XCTNSPredicateExpectation(predicate: tinted, object: original)
        XCTAssertEqual(XCTWaiter().wait(for: [wait], timeout: 1.4), .completed, "the quoted line is revealed (tinted) by the tap")
        XCTAssertFalse(app.descendants(matching: .any)["composer-reply-preview"].exists, "a tap on the quote starts no reply")
        XCTAssertFalse(app.keyboards.firstMatch.exists, "a tap on the quote does not focus the composer")
        XCTAssertTrue(back.exists, "the chat did not go back")
    }

    /// A slow 150 pt pan to the right from `start` — the reply swipe's shape.
    private func swipeRight(from start: XCUICoordinate) {
        let end = start.withOffset(CGVector(dx: 150, dy: 0))
        start.press(forDuration: 0.05, thenDragTo: end, withVelocity: .slow, thenHoldForDuration: 0.1)
    }

    /// The edge zone is the edge pop's: a swipe from the edge at a bubble's height goes
    /// back and starts no reply.
    func testEdgeSwipeOverABubbleGoesBackNotReply() {
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

    /// A vertical drag on a bubble is left to the scroll view: no reply, no keyboard, no
    /// back. Until iphone-scroll-perf (2026-10-27) XCUI's pans never scrolled this transcript
    /// — the per-bubble reply drag kept the scroll view's pan from beginning — so this test
    /// passed trivially; now the drag scrolls, and the pan stays inside the transcript (up,
    /// 200 pt) so its end never lands on the composer. `BackSwipeTests` pins the axis rule,
    /// `ScrollPerfUITests` measures the pans.
    func testVerticalDragOnABubbleDoesNotReplyOrGoBack() {
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
}
