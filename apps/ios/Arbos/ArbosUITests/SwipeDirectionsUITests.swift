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
    private static let group = "fx-group-launch"

    /// The Comms agent chat, used for the own-message cases. Every `.user` row of a non-people
    /// chat is the signed-in user's, so `c1` (long) and `c5` ("Hey") are unambiguously mine and
    /// render trailing-aligned — no fixture flag and no guessing which member sent what.
    private static let comms = "fx-comms-main"

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launchGroup() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1", "ARBOS_OPEN_PEOPLE": Self.group]
        app.launch()
        XCTAssertTrue(chatBack(app).waitForExistence(timeout: appear), "the Launch week group opened")
        return app
    }

    private func launchComms() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1", "ARBOS_OPEN_PROJECT": "fx-comms"]
        app.launch()
        XCTAssertTrue(chatBack(app, Self.comms).waitForExistence(timeout: appear), "the Comms chat opened")
        return app
    }

    private func chatBack(_ app: XCUIApplication, _ chatId: String = SwipeDirectionsUITests.group) -> XCUIElement {
        app.buttons["chat-back:\(chatId)"]
    }

    private func replyPreview(_ app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any).matching(identifier: "composer-reply-preview").firstMatch
    }

    /// The element published as `bubble:<id>`, resolved to exactly one.
    ///
    /// A row that draws several accessibility elements publishes the identifier on each of them
    /// — an album carries one per attachment — and the plain `query[id]` subscript raises
    /// "Multiple matching elements found" as soon as anything reads a frame or hittability.
    /// That ambiguity, not the gesture, is why every `g2b` case in this file used to fail with
    /// "never came into reach": the loop below asked `isHittable`, never got an answer, and
    /// scrolled past a bubble that was on screen the whole time.
    ///
    /// Of the matches, take the largest by area: that is the row, and the rest are the pieces
    /// drawn inside it. Some of those pieces have no size at all, and XCUITest answers a
    /// hittability question about one of them with "Activation point invalid".
    private func bubbleElement(_ app: XCUIApplication, _ id: String) -> XCUIElement {
        let matches = app.descendants(matching: .any).matching(identifier: "bubble:\(id)")
        let all = matches.allElementsBoundByIndex
        guard all.count > 1 else { return matches.firstMatch }
        // Say it out loud, in the log, every time. Picking a winner in silence is how this
        // cost a day: the suite reported "never came into reach" and the gesture took the
        // blame, with nothing anywhere to show that the identifier was the ambiguous part.
        print("bubble:\(id) matches \(all.count) elements — taking the largest")
        return all.max { $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height }
            ?? matches.firstMatch
    }

    /// Wait until a row has stopped moving.
    ///
    /// A scroll does not end when the finger lifts, and `revealBubble` can hand back a bubble
    /// that is in the right place but still decelerating. A swipe delivered into a moving
    /// scroll view belongs to the scroll view, so the gesture under test never happens and the
    /// failure is recorded against the app.
    ///
    /// It fails rather than falling through on timeout. A settle that gives up quietly is the
    /// same defect in new clothes: the swipe still lands mid-scroll, and the app is still the
    /// one blamed for it.
    private func settleRow(_ element: XCUIElement, timeout: TimeInterval = 4,
                           file: StaticString = #filePath, line: UInt = #line) {
        var previous = CGRect.null
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            let current = element.frame
            if !current.isEmpty, current == previous { return }
            previous = current
            Thread.sleep(forTimeInterval: 0.1)
        }
        XCTFail("\(element.identifier) was still moving after \(Int(timeout))s", file: file, line: line)
    }

    /// Scroll `bubble:<id>` clear of the header and the composer, let it come to rest, and
    /// return it.
    ///
    /// Each pass drags by the offset that would centre the bubble rather than by a fixed third
    /// of a screen. A fixed step is both coarser and blinder: it has to guess a direction, and
    /// it can be wider than the band a tall row has to land in and step straight over it. `g6`
    /// is a 526 pt screenshot in an 874 pt window, which leaves it a 188 pt band to land in
    /// against a 288 pt step — measured at y = −31 on one pass and y = +248 on the next, over
    /// the top of the only positions that would have counted.
    ///
    /// Centring also subsumes the direction question. A chat does not always open at its newest
    /// message — the Comms chat opens part-way up, so `c5` starts below the viewport — and the
    /// sign of the offset points the drag the right way without being told.
    private func revealBubble(_ app: XCUIApplication, _ id: String,
                              file: StaticString = #filePath, line: UInt = #line) -> XCUIElement {
        let window = app.windows.firstMatch.frame
        var previous = CGRect.null
        for _ in 0..<12 {
            // Resolved every pass, not once up front: which element is the largest match only
            // settles after the list has built the row.
            let bubble = bubbleElement(app, id)
            let frame = bubble.frame
            // Size before hittability. A degenerate match has no activation point, and asking
            // whether it is hittable throws rather than answering false.
            if bubble.exists, frame.width > 1, frame.height > 1, bubble.isHittable {
                let clear = frame.midY > window.height * 0.3 && frame.maxY < window.maxY - 160
                // The transcript can simply run out of room. `c5` is the second-newest message
                // in the Comms chat, so nothing lifts it a full 160 pt clear of the composer.
                // Once a drag stops moving the bubble, "fully on screen and clear of the
                // composer" is as good as the position gets, and it is enough to swipe.
                let stuck = frame == previous && frame.minY > window.minY
                    && frame.maxY < window.maxY - 110
                if clear || stuck {
                    settleRow(bubble, file: file, line: line)
                    return bubble
                }
                previous = frame
            }
            // An unbuilt row has no frame, which centres as a downward drag — the right
            // default, since the history is where unbuilt rows are.
            let reach = window.height * 0.4
            let step = max(-reach, min(reach, window.midY - frame.midY))
            let mid = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.45))
            mid.press(forDuration: 0.05, thenDragTo: mid.withOffset(CGVector(dx: 0, dy: step)),
                      withVelocity: .slow, thenHoldForDuration: 0.1)
            usleep(400_000)
        }
        XCTFail("bubble:\(id) never came into reach", file: file, line: line)
        return bubbleElement(app, id)
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
        // Print what the app is actually given. XCUITest clamps a destination to the screen in
        // silence, so a test can ask for 180 pt, deliver 45, and then report the app refusing
        // to go back — the exact misreading that cost `BackSwipeUITests` a day. The arithmetic
        // above keeps the destination on screen; this line is what lets a reader check that
        // from the log instead of re-deriving it from a frame.
        print("swipe right on \(bubble.identifier): x \(Int(startX)) → \(Int(startX + distance)) " +
              "of \(Int(window.maxX)), distance \(Int(distance))")
        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: distance, dy: 0)),
                    withVelocity: .fast, thenHoldForDuration: 0.05)
    }

    private func assertWentBack(_ app: XCUIApplication, _ chatId: String = SwipeDirectionsUITests.group,
                                file: StaticString = #filePath, line: UInt = #line) {
        let gone = NSPredicate(format: "exists == false")
        let wait = XCTNSPredicateExpectation(predicate: gone, object: chatBack(app, chatId))
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

    // MARK: Your own messages reply too, and still go back

    /// Affinity, 2026-10-31: swipe left on **any** message bubble replies to it, your own
    /// included — not only other people's.
    ///
    /// The four cases above all happen to swipe incoming bubbles (`g3`, `g2b` and `g6` are other
    /// members', `g8` is the agent's), so until now nothing proved the outgoing side. The app
    /// never restricted it — `isMessageRow` admits `.user` rows and never looks at the sender —
    /// but "never restricted it" is not the same claim as "a test holds it there".
    func testSwipeLeftOnMyOwnBubbleReplies() {
        let app = launchComms()
        // `c5` is "Hey": short, mine, and hard against the trailing edge.
        swipeLeft(on: revealBubble(app, "c5"), app)
        XCTAssertTrue(replyPreview(app).waitForExistence(timeout: settle), "my own message's reply quote is in the composer")
        XCTAssertTrue(chatBack(app, Self.comms).exists, "a left swipe never goes back")
    }

    /// The same bubble from the other direction, which is the half worth guarding: making reply
    /// reach outgoing bubbles must not let it take the back gesture with it.
    func testSwipeRightOnMyOwnBubbleGoesBackAndOpensNoReply() {
        let app = launchComms()
        // `c1` is mine and long, so it is wide enough to hold a 180 pt rightward drag on screen.
        // `c5` is not: from a bubble that narrow the finger reaches the screen edge first, and
        // XCUITest would clamp the drag and report it as the app refusing to go back.
        swipeRight(on: revealBubble(app, "c1"), app)
        assertWentBack(app, Self.comms)
        XCTAssertFalse(replyPreview(app).exists, "a right swipe on my own message never replies")
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
