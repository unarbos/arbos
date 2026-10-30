import XCTest

/// The two chat gestures, told apart by direction only (Jacob, 2026-10-30: "you should be
/// swiping to the right to go back and swiping to the left to reply so those should not be
/// interfering with each other"):
///
/// - right anywhere → back to the chats list, including on a message bubble;
/// - left on a message → reply, on every message type;
/// - neither ever does the other's job, and a vertical drag is still a scroll.
///
/// These are written to fail on the build Jacob reported from (`7bbfaf425`), not merely to pass
/// on today's main — the structural gap that let this reach him is that nothing ever tests the
/// commit a tester is running. On that build a RIGHT swipe on a bubble replied and a LEFT swipe
/// peeked the message times, so `testSwipeRightOnABubbleGoesBackAndOpensNoReply` and every
/// `SwipeLeft…Replies` test below go red against it.
///
/// `BackSwipeUITests` owns the edge pop and the back gesture per chat kind; this file owns the
/// direction split. Screens are told apart by their back chevron (`chat-back:<chatId>`), a
/// bubble is `bubble:<messageId>`, and the composer's reply quote is `composer-reply-preview`.
final class SwipeDirectionsUITests: XCTestCase {
    /// The shared Mac runs the gate and releases at the same time; waits are generous.
    private let appear: TimeInterval = 90
    private let settle: TimeInterval = 20

    /// The Launch week group: a text bubble (`g3`), a five-attachment album with a caption
    /// (`g2b` — DaWizzaa's shape), a tall portrait screenshot (`g6`) and an agent's
    /// picture-only message (`g8`).
    private let group = "fx-group-launch"

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launchGroup() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1", "ARBOS_OPEN_PEOPLE": group]
        app.launch()
        XCTAssertTrue(chatBack(app).waitForExistence(timeout: appear), "the Launch week group opened")
        return app
    }

    private func chatBack(_ app: XCUIApplication) -> XCUIElement { app.buttons["chat-back:\(group)"] }

    private func replyPreview(_ app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any)["composer-reply-preview"]
    }

    /// Scroll `bubble:<id>` into the middle of the transcript and return it. The chat opens at
    /// the newest message, so the older media rows need dragging down to reach.
    private func revealBubble(_ app: XCUIApplication, _ id: String,
                              file: StaticString = #filePath, line: UInt = #line) -> XCUIElement {
        let bubble = app.descendants(matching: .any)["bubble:\(id)"]
        let window = app.windows.firstMatch.frame
        for _ in 0..<12 {
            if bubble.exists, bubble.isHittable,
               bubble.frame.midY > window.height * 0.3, bubble.frame.maxY < window.maxY - 160 {
                return bubble
            }
            // Drag down from mid-screen: back through the history, one third of a screen.
            let mid = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.45))
            mid.press(forDuration: 0.05, thenDragTo: mid.withOffset(CGVector(dx: 0, dy: window.height * 0.33)),
                      withVelocity: .slow, thenHoldForDuration: 0.1)
            usleep(400_000)
        }
        XCTFail("bubble:\(id) never came into reach", file: file, line: line)
        return bubble
    }

    /// Swipe left across `bubble`, starting near its trailing edge so the finger has room to
    /// travel without leaving the screen. Returns the distance actually swiped.
    @discardableResult
    private func swipeLeft(on bubble: XCUIElement, _ app: XCUIApplication, want: CGFloat = 110,
                           file: StaticString = #filePath, line: UInt = #line) -> CGFloat {
        let start = bubble.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5))
        // Keep 32 pt clear of the left screen edge: the first 24 pt are the system pop's.
        let room = bubble.frame.minX + bubble.frame.width * 0.9 - 32
        let distance = min(want, room)
        XCTAssertGreaterThan(distance, 60, "the bubble has room for a left swipe", file: file, line: line)
        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: -distance, dy: 0)),
                    withVelocity: .fast, thenHoldForDuration: 0.05)
        return distance
    }

    /// Swipe right across `bubble`, starting near its leading edge (but clear of the edge zone,
    /// which belongs to the system pop) so a long rightward drag stays on screen.
    private func swipeRight(on bubble: XCUIElement, _ app: XCUIApplication, want: CGFloat = 180,
                           file: StaticString = #filePath, line: UInt = #line) {
        let window = app.windows.firstMatch.frame
        let startX = max(bubble.frame.minX + 8, 40)
        let start = bubble.coordinate(withNormalizedOffset: CGVector(dx: 0, dy: 0.5))
            .withOffset(CGVector(dx: startX - bubble.frame.minX, dy: 0))
        let distance = min(want, window.maxX - 8 - startX)
        XCTAssertGreaterThan(distance, 80, "the bubble has room for a right swipe", file: file, line: line)
        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: distance, dy: 0)),
                    withVelocity: .fast, thenHoldForDuration: 0.05)
    }

    private func assertWentBack(_ app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line) {
        let gone = NSPredicate(format: "exists == false")
        let wait = XCTNSPredicateExpectation(predicate: gone, object: chatBack(app))
        XCTAssertEqual(XCTWaiter().wait(for: [wait], timeout: settle), .completed,
                       "the chat went back to the list", file: file, line: line)
        XCTAssertTrue(app.buttons["People"].waitForExistence(timeout: settle), "Home is on screen", file: file, line: line)
    }

    // MARK: Left on a message replies — every message type

    /// A plain text bubble.
    func testSwipeLeftOnATextBubbleReplies() {
        let app = launchGroup()
        swipeLeft(on: revealBubble(app, "g3"), app)
        XCTAssertTrue(replyPreview(app).waitForExistence(timeout: settle), "the composer shows the reply quote")
        XCTAssertTrue(chatBack(app).exists, "a left swipe never goes back")
    }

    /// The album: five attachments and a caption, the shape of DaWizzaa's two-screenshot
    /// message (01M670FVEYHXTG2RQBN9PF6G0N). A tall row overlaps its neighbours, which is what
    /// the old `RowFrameIndex.replyableRow(at:)` got wrong — it took whichever row a dictionary
    /// yielded first and gave up when that one was not repliable, so album and picture bubbles
    /// missed their reply far more often than a line of text did.
    func testSwipeLeftOnAnAlbumBubbleReplies() {
        let app = launchGroup()
        swipeLeft(on: revealBubble(app, "g2b"), app)
        XCTAssertTrue(replyPreview(app).waitForExistence(timeout: settle), "the album's reply quote is in the composer")
        XCTAssertTrue(chatBack(app).exists, "a left swipe never goes back")
    }

    /// A tall portrait screenshot with a long caption.
    func testSwipeLeftOnAPictureBubbleReplies() {
        let app = launchGroup()
        swipeLeft(on: revealBubble(app, "g6"), app)
        XCTAssertTrue(replyPreview(app).waitForExistence(timeout: settle), "the picture's reply quote is in the composer")
        XCTAssertTrue(chatBack(app).exists, "a left swipe never goes back")
    }

    /// A picture-only message with no words at all: the quote still has something to show, and
    /// the swipe still lands on the row rather than on the picture's tap target.
    func testSwipeLeftOnAPictureOnlyBubbleReplies() {
        let app = launchGroup()
        swipeLeft(on: revealBubble(app, "g8"), app)
        XCTAssertTrue(replyPreview(app).waitForExistence(timeout: settle), "the reply quote is in the composer")
        // A left drag on a picture must not be read as a tap that opens the viewer.
        XCTAssertFalse(app.descendants(matching: .any)["media-viewer"].exists, "the viewer stayed shut")
    }

    // MARK: Right is back, and never a reply

    /// The collision Jacob reported, from the other side: a right swipe that starts on a bubble
    /// goes back and starts no reply. On `7bbfaf425` this swipe replied instead.
    func testSwipeRightOnABubbleGoesBackAndOpensNoReply() {
        let app = launchGroup()
        swipeRight(on: revealBubble(app, "g3"), app)
        assertWentBack(app)
        XCTAssertFalse(replyPreview(app).exists, "a right swipe never replies")
    }

    /// The same on an album, where the old row lookup was least reliable.
    func testSwipeRightOnAnAlbumBubbleGoesBackAndOpensNoReply() {
        let app = launchGroup()
        swipeRight(on: revealBubble(app, "g2b"), app)
        assertWentBack(app)
        XCTAssertFalse(replyPreview(app).exists, "a right swipe never replies")
    }

    /// "Back" takes a deliberate swipe, not a flick: 50 pt is under the 70 pt rule, so the chat
    /// stays. On `7bbfaf425` this armed a reply (the arrow fired at 39 pt) — the test is here to
    /// catch a threshold quietly dropping back to that build's 40 pt.
    func testAShortRightSwipeOnABubbleNeitherGoesBackNorReplies() {
        let app = launchGroup()
        let bubble = revealBubble(app, "g3")
        let start = bubble.coordinate(withNormalizedOffset: CGVector(dx: 0, dy: 0.5))
            .withOffset(CGVector(dx: max(8, 40 - bubble.frame.minX), dy: 0))
        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: 50, dy: 0)),
                    withVelocity: .slow, thenHoldForDuration: 0.5)
        sleep(1)
        XCTAssertTrue(chatBack(app).exists, "50 pt is not a back swipe")
        XCTAssertFalse(replyPreview(app).exists, "and it is certainly not a reply")
    }

    // MARK: Vertical scrolling is untouched

    /// A vertical drag on an album is the scroll view's: no reply, no back, no keyboard. The
    /// album is the row most likely to swallow a pan, since it is the tallest.
    func testVerticalDragOnAnAlbumNeitherRepliesNorGoesBack() {
        let app = launchGroup()
        let bubble = revealBubble(app, "g2b")
        let start = bubble.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: 6, dy: -200)),
                    withVelocity: .slow, thenHoldForDuration: 0.1)
        sleep(1)
        XCTAssertFalse(replyPreview(app).exists, "no reply started")
        XCTAssertFalse(app.keyboards.firstMatch.exists, "the composer was not focused")
        XCTAssertTrue(chatBack(app).exists, "the chat did not go back")
    }

    /// The transcript still scrolls: the same vertical drag moves the content.
    func testTheTranscriptStillScrolls() {
        let app = launchGroup()
        let bubble = revealBubble(app, "g2b")
        let before = bubble.frame.midY
        let start = bubble.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: 0, dy: -160)),
                    withVelocity: .slow, thenHoldForDuration: 0.1)
        sleep(1)
        XCTAssertLessThan(bubble.frame.midY, before - 60, "the album moved up with the scroll")
    }
}
