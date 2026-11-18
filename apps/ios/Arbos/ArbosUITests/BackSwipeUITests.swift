import XCTest

/// Back gestures on every chat screen (Jacob, 2026-10-28: the system's interactive edge pop,
/// finger-tracking and cancellable, is the one owner of the edge; a swipe right anywhere
/// else is an animated "back"; there is no swipe-right-to-reply — Reply is on long-press, in
/// the actions menu, and a swipe LEFT on a bubble, tester reports 2026-11-13). Runs the fixture app (`ARBOS_FIXTURE=1`, Debug builds): no relay,
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

    /// Wait until the transcript has stopped moving. A chat opens by scrolling to its newest
    /// message, and the first bubble exists before that scroll ends: a frame read mid-scroll
    /// says where a row was, not where the finger will land.
    private func settleTranscript(_ app: XCUIApplication, timeout: TimeInterval = 8) {
        let bubbles = app.descendants(matching: .any).matching(bubbleQuery)
        var previous = CGRect.null
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            let current = bubbles.firstMatch.frame
            if !current.isEmpty, current == previous { return }
            previous = current
            Thread.sleep(forTimeInterval: 0.25)
        }
    }

    /// The lowest fully visible bubble with room to swipe `distance` right from 44 pt inside its
    /// leading edge without the finger leaving the screen, and that start point.
    ///
    /// XCUITest clamps a drag destination to the screen silently: 180 pt asked for from the
    /// centre of a short own bubble against the trailing edge reaches the app as about 45 pt,
    /// under the 70 pt `BackSwipe.fullScreenPops` needs, and the test then blames the app
    /// ("the chat did not go back") for a gesture it never delivered.
    private func bubbleWithRoomToSwipeRight(_ app: XCUIApplication, distance: CGFloat,
                                            file: StaticString = #filePath, line: UInt = #line) -> XCUICoordinate {
        let bubbles = app.descendants(matching: .any).matching(bubbleQuery)
        XCTAssertTrue(bubbles.firstMatch.waitForExistence(timeout: settle), "the transcript shows bubbles", file: file, line: line)
        settleTranscript(app)
        let window = app.windows.firstMatch.frame
        let inset: CGFloat = 44
        let rightLimit = window.maxX - 8
        // The start is clear of the 24 pt edge zone (the system pop's) and the end of the screen's edge.
        let roomy = bubbles.allElementsBoundByIndex.filter {
            $0.isHittable && $0.frame.maxY < window.maxY - 120 && $0.frame.minY > 120
                && $0.frame.minX + inset > 32 && $0.frame.minX + inset + distance < rightLimit
        }
        XCTAssertFalse(roomy.isEmpty, "a bubble has room for a \(Int(distance)) pt swipe in a \(Int(window.width)) pt screen",
                       file: file, line: line)
        let bubble = roomy.last!
        XCTAssertLessThan(bubble.frame.minX + inset + distance, rightLimit,
                          "the \(Int(distance)) pt drag would be clamped at the screen edge and never reach the app",
                          file: file, line: line)
        return bubble.coordinate(withNormalizedOffset: CGVector(dx: 0, dy: 0.5)).withOffset(CGVector(dx: inset, dy: 0))
    }

    /// Tap the composer so the keyboard is up (what every sent message leaves behind).
    private func raiseKeyboard(_ app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line) {
        let field = app.descendants(matching: .any)["composer-field"]
        XCTAssertTrue(field.waitForExistence(timeout: settle), "the composer field is there", file: file, line: line)
        field.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: settle), "the keyboard came up", file: file, line: line)
    }

    private func assertHome(_ app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertTrue(app.buttons["home-account"].waitForExistence(timeout: settle),
                      "Home's account circle is on screen", file: file, line: line)
        XCTAssertFalse(app.buttons["Agents"].exists, "the list filter is gone", file: file, line: line)
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
        let app = launch(["ARBOS_OPEN_PROJECT": "fx-comms"])
        let back = chatBack(app, "fx-comms-main")
        XCTAssertTrue(back.waitForExistence(timeout: appear), "the Comms chat opened")
        let start = bubbleWithRoomToSwipeRight(app, distance: 180)
        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: 180, dy: 0)), withVelocity: .fast, thenHoldForDuration: 0.05)
        assertGone(back)
        XCTAssertFalse(app.descendants(matching: .any)["composer-reply-preview"].exists, "no reply started")
        XCTAssertFalse(app.keyboards.firstMatch.exists, "the composer was not focused")
        assertHome(app)
    }

    // MARK: A swipe left on a bubble replies; off a bubble it is the time peek

    /// My newest bubble on screen: the bubble itself, not its full-width row.
    private func ownBubble(_ app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line) -> XCUIElement {
        let own = app.descendants(matching: .any).matching(NSPredicate(format: "identifier BEGINSWITH 'own-bubble-'"))
        XCTAssertTrue(own.firstMatch.waitForExistence(timeout: settle), "the transcript shows a bubble of mine", file: file, line: line)
        settleTranscript(app)
        let window = app.windows.firstMatch.frame
        let visible = own.allElementsBoundByIndex.filter { $0.isHittable && $0.frame.maxY < window.maxY - 120 && $0.frame.minY > 120 }
        XCTAssertFalse(visible.isEmpty, "a bubble of mine is hittable", file: file, line: line)
        return visible.last!
    }

    private func openComms() -> XCUIApplication {
        let app = launch(["ARBOS_OPEN_PROJECT": "fx-comms", "ARBOS_SWIPE_TRACE": "1"])
        XCTAssertTrue(chatBack(app, "fx-comms-main").waitForExistence(timeout: appear), "the Comms chat opened")
        return app
    }

    private func openGroup() -> XCUIApplication {
        let app = launch(["ARBOS_OPEN_PEOPLE": "fx-group-launch", "ARBOS_SWIPE_TRACE": "1"])
        XCTAssertTrue(chatBack(app, "fx-group-launch").waitForExistence(timeout: appear), "the Launch week group opened")
        settleTranscript(app)
        return app
    }

    /// Sam's picture with the caption "Hero for the landing page, v2." (fixture message `g7`),
    /// by its id and not by where it sits: the full-width row, bubble after the avatar lane.
    private func heroRow(_ app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line) -> XCUIElement {
        let row = app.descendants(matching: .any)["bubble:g7"].firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: settle) && row.isHittable, "Sam's picture message is on screen", file: file, line: line)
        return row
    }

    private func dragLeft(from start: XCUICoordinate, _ distance: CGFloat, press: TimeInterval = 0.05) {
        start.press(forDuration: press, thenDragTo: start.withOffset(CGVector(dx: -distance, dy: 0)), withVelocity: .slow, thenHoldForDuration: 0.1)
    }

    /// The composer quotes a message that says `text`, has focus, and the chat is still the screen.
    private func assertReplying(_ app: XCUIApplication, to text: String, file: StaticString = #filePath, line: UInt = #line) {
        let preview = app.descendants(matching: .any)["composer-reply-preview"]
        XCTAssertTrue(preview.waitForExistence(timeout: settle), "the composer quotes a message", file: file, line: line)
        XCTAssertTrue(preview.label.contains(text), "it quotes \"\(text)\": \(preview.label)", file: file, line: line)
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: settle), "the composer took focus", file: file, line: line)
        assertOnlyTheChat(app, file: file, line: line)
    }

    private func assertNoReply(_ app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line) {
        sleep(1)
        XCTAssertFalse(app.descendants(matching: .any)["composer-reply-preview"].exists, "no reply started", file: file, line: line)
        XCTAssertFalse(app.keyboards.firstMatch.exists, "the composer was not focused", file: file, line: line)
    }

    /// Nothing opened over the chat (a picture, a profile page) and the app did not leave for a link.
    private func assertOnlyTheChat(_ app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line) {
        sleep(1)
        XCTAssertEqual(app.state, .runningForeground, "no link left the app", file: file, line: line)
        XCTAssertTrue(app.descendants(matching: .any)["chat-search-button"].firstMatch.isHittable, "nothing opened over the chat", file: file, line: line)
    }

    /// What the last left swipe was, from the rig readout: `reply` or `peek`.
    private func leftSwipe(_ app: XCUIApplication) -> [String] {
        let readout = app.descendants(matching: .any)["back-swipe-trace"].firstMatch
        guard readout.waitForExistence(timeout: settle) else { return [] }
        return ((readout.value as? String) ?? "").split(separator: " ").map(String.init)
    }

    /// Drag left from the middle of my bubble, past the threshold: the composer quotes that
    /// message and takes focus, and the chat stays.
    func testSwipeLeftOnABubbleReplies() {
        let app = openComms()
        let bubble = ownBubble(app)
        let said = String((bubble.label.isEmpty ? bubble.staticTexts.firstMatch.label : bubble.label).prefix(12))
        XCTAssertFalse(said.isEmpty, "the bubble's words are readable")
        dragLeft(from: bubble.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)), 120)
        assertReplying(app, to: said)
        XCTAssertTrue(leftSwipe(app).contains("reply"))
    }

    /// The same on someone else's message in a group, started on its caption.
    func testSwipeLeftOnAnotherPersonsBubbleReplies() {
        let app = openGroup()
        let row = heroRow(app)
        // 100 pt in and 20 pt up from the row's bottom-leading corner: past the avatar lane, on the caption.
        dragLeft(from: row.coordinate(withNormalizedOffset: CGVector(dx: 0, dy: 1)).withOffset(CGVector(dx: 100, dy: -20)), 80)
        assertReplying(app, to: "Hero for the landing page")
    }

    /// Started on a picture and let go just past the threshold: a reply, and the lift does
    /// not open the viewer.
    func testSwipeLeftFromAPictureRepliesAndOpensNoViewer() {
        let app = openGroup()
        // 150 pt in at the row's middle height: on the picture.
        dragLeft(from: heroRow(app).coordinate(withNormalizedOffset: CGVector(dx: 0, dy: 0.5)).withOffset(CGVector(dx: 150, dy: 0)), 62)
        assertReplying(app, to: "Hero for the landing page")
    }

    /// Started on a code block's Copy button and let go just past the threshold: a reply,
    /// and the lift does not copy.
    func testSwipeLeftFromACodeCopyButtonRepliesAndCopiesNothing() {
        let app = launch(["ARBOS_OPEN_PEOPLE": "fx-group-launch", "ARBOS_FIXTURE_LONG_ROWS": "120", "ARBOS_FIXTURE_LONG_RICH": "1"])
        XCTAssertTrue(chatBack(app, "fx-group-launch").waitForExistence(timeout: appear), "the Launch week group opened")
        settleTranscript(app)
        let window = app.windows.firstMatch.frame
        let copy = app.descendants(matching: .any).matching(NSPredicate(format: "label == 'Copy code'")).firstMatch
        // Into history until a code card's Copy button is clear of the header and the composer.
        var tries = 0
        while tries < 30, !(copy.exists && copy.isHittable && copy.frame.minY > 160 && copy.frame.maxY < window.maxY - 200) {
            let from = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.35))
            from.press(forDuration: 0.05, thenDragTo: from.withOffset(CGVector(dx: 0, dy: 220)), withVelocity: .slow, thenHoldForDuration: 0.3)
            tries += 1
        }
        let seen = app.descendants(matching: .any).matching(bubbleQuery).allElementsBoundByIndex.map(\.identifier)
        XCTAssertTrue(copy.exists && copy.isHittable, "a code card's Copy button is on screen after \(tries) drags; rows: \(seen); cards: \(app.descendants(matching: .any).matching(identifier: "code-card").count)")
        // First that this read sees a copy at all: a plain tap says Copied, for 1.5 s.
        let saysCopied = app.descendants(matching: .any).matching(NSPredicate(format: "label == 'Copied'")).firstMatch
        copy.tap()
        XCTAssertTrue(saysCopied.exists, "a tap copies, and the test can see it")
        sleep(3)
        XCTAssertTrue(copy.exists && copy.isHittable, "the button is back to Copy code")
        dragLeft(from: copy.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)), 62)
        // Read at once: the button says Copied for 1.5 s.
        let copied = app.descendants(matching: .any).matching(NSPredicate(format: "label == 'Copied'")).firstMatch.exists
        XCTAssertFalse(copied, "the lift did not copy the code")
        XCTAssertTrue(app.descendants(matching: .any)["composer-reply-preview"].waitForExistence(timeout: settle), "the composer quotes the message")
    }

    /// Hold a bubble into its menu, then slide the same finger left along it and lift: that
    /// finger is choosing in the menu, it is not a reply swipe.
    func testAHoldIntoTheMenuThatSlidesLeftStartsNoReply() {
        let app = openComms()
        dragLeft(from: ownBubble(app).coordinate(withNormalizedOffset: CGVector(dx: 0.8, dy: 0.5)), 80, press: 0.9)
        assertNoReply(app)
        XCTAssertFalse(leftSwipe(app).contains("reply"))
    }

    /// Let go short of the threshold: nothing happens.
    func testAShortSwipeLeftOnABubbleStartsNoReply() {
        let app = openComms()
        dragLeft(from: ownBubble(app).coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)), 30)
        assertNoReply(app)
        XCTAssertTrue(chatBack(app, "fx-comms-main").exists, "the chat did not go back")
    }

    /// The same drag from the empty side of my bubble's row is still the time peek, and no reply.
    func testSwipeLeftBesideABubbleIsTheTimePeek() {
        let app = openComms()
        let bubble = ownBubble(app)
        let window = app.windows.firstMatch.frame
        // Mid-way between the screen's centre and the bubble's leading edge, clear of both.
        let x = (window.midX + bubble.frame.minX) / 2
        XCTAssertGreaterThan(bubble.frame.minX - x, 24, "the row has an empty side to start on")
        dragLeft(from: app.coordinate(withNormalizedOffset: .zero).withOffset(CGVector(dx: x, dy: bubble.frame.midY)), 120)
        assertNoReply(app)
        let trace = leftSwipe(app)
        XCTAssertEqual(trace.prefix(2), ["start", "peek"], "the left swipe was the time peek")
        XCTAssertTrue(trace.contains { $0.hasPrefix("time:") && $0.contains(where: \.isNumber) }, "a message's time came on screen: \(trace)")
        XCTAssertTrue(chatBack(app, "fx-comms-main").exists, "the chat did not go back")
    }

    /// A vertical drag that starts on a bubble still scrolls the transcript.
    func testVerticalDragOnABubbleStillScrolls() {
        let app = launch(["ARBOS_OPEN_PEOPLE": "fx-group-launch", "ARBOS_FIXTURE_LONG_ROWS": "120"])
        XCTAssertTrue(chatBack(app, "fx-group-launch").waitForExistence(timeout: appear), "the Launch week group opened")
        settleTranscript(app)
        let window = app.windows.firstMatch.frame
        let bubble = app.descendants(matching: .any).matching(bubbleQuery).allElementsBoundByIndex.last {
            $0.isHittable && $0.frame.midY > window.height * 0.3 && $0.frame.midY < window.height * 0.5
        }
        XCTAssertNotNil(bubble, "a bubble sits in the upper middle of the screen to drag on")
        // Found again by its id afterwards: a query by position would name another row.
        let id = bubble!.identifier
        let before = bubble!.frame.midY
        let start = bubble!.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: -6, dy: 200)), withVelocity: .slow, thenHoldForDuration: 0.1)
        sleep(1)
        XCTAssertGreaterThan(app.descendants(matching: .any)[id].firstMatch.frame.midY - before, 50, "the transcript followed the finger down")
        XCTAssertFalse(app.descendants(matching: .any)["composer-reply-preview"].exists, "no reply started")
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
        settleTranscript(app)
        let window = app.windows.firstMatch.frame
        // A bubble whose middle is mid-screen: room for the 200 pt drag up, clear of the composer.
        // Not "wholly in the lower half": tall rows rarely fit there, and layout changes move them.
        let onBubble = bubbles.allElementsBoundByIndex.last {
            $0.isHittable && $0.frame.midY > window.height * 0.3 && $0.frame.midY < window.height * 0.7
        }
        XCTAssertNotNil(onBubble, "a bubble sits in the middle of the screen to drag on")
        // A drag clamped at the top edge would be shorter than asked, and this test would pass on less of a drag.
        XCTAssertGreaterThan(onBubble!.frame.midY - 200, window.minY + 8, "the 200 pt drag up stays on screen")
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
