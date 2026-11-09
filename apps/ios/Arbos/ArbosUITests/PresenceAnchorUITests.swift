import XCTest

/// The presence row belongs to the end of the transcript, not to the bar over it
/// ([feedback typing-anchor-2026-11-08]; Jacob: "the agent typing and or user is typing
/// scrolls with you which covers up the chat bubbles as you scroll up … Grok does not do
/// that. it stays typing down at the actual location where the typing is occurring").
///
/// Pinned to the bar the row stayed on screen while the reader went up into history, and
/// since it draws no background of its own it landed as a second line of text across
/// whichever old bubble was under it. These two tests are the before/after: at the live
/// bottom the row sits over the composer as it always did, and up in history it is gone.
///
/// Agent presence is what the fixture can stage (`fx-comms-main` runs a turn, so the row
/// reads "Comms is working"). Human typing is the same row: `presenceParticipants` merges
/// typing handles and working agents into one list for one `PresenceRow`, so there is no
/// second placement that could still be pinned.
final class PresenceAnchorUITests: XCTestCase {
    private let appear: TimeInterval = 90

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    /// The drag case asks whether a reader keeps their place in history after the keyboard goes
    /// away. That is only a question the transcript can answer if, once the keyboard is down,
    /// there is somewhere to stand: dismissing it hands the viewport the keyboard's whole height
    /// back, and `fx-comms-main` is six messages. If the settled transcript cannot hold a
    /// position further from the end than the app's own `awayThreshold`, the drag case is
    /// unsatisfiable by construction and its failure says nothing about any anchor.
    ///
    /// Measured here rather than argued about, because the two readings of a failing drag case —
    /// "it was pulled back" and "there was nowhere left to stand" — produce the same single
    /// number and legs are an hour each.
    func testTheFixtureIsDeepEnoughForTheDragCaseToMeanAnything() {
        let app = openWorkingChat()
        XCTAssertTrue(presence(app).waitForExistence(timeout: appear), "the fixture chat is up")
        record(app, "at rest at the live bottom, keyboard down")

        // No keyboard anywhere in this case: this is the state the reader is left in once their
        // drag has dismissed it, and the question is only how much transcript it leaves behind.
        for _ in 0..<3 { dragIntoHistory(app, endingAt: 0.80) }
        // Long enough for any rubber-band to spring back, so what is measured is a position the
        // transcript will actually hold rather than the far end of an overscroll.
        Thread.sleep(forTimeInterval: 2)
        record(app, "settled as far back as it goes, keyboard down")

        let top = composerTop(app)
        guard let newest = bubbles(app).max(by: { $0.frame.maxY < $1.frame.maxY }) else {
            return XCTFail("the transcript has rows")
        }
        XCTAssertGreaterThan(newest.frame.maxY, top + 120,
                             "with the keyboard down the transcript must be able to settle more "
                             + "than the 120 pt `awayThreshold` from its end, or no reader can be "
                             + "in history here at all — newest \(newest.id) \(newest.frame), "
                             + "composer top \(top)")
    }

    private func openWorkingChat() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = [
            "ARBOS_FIXTURE": "1",
            "ARBOS_NO_STORE": "1",
            // `openChat` is only read inside the `openProject` branch, so both are needed.
            "ARBOS_OPEN_PROJECT": "fx-comms",
            "ARBOS_OPEN_CHAT": "fx-comms-main",
        ]
        app.launch()
        let back = app.descendants(matching: .any)["chat-back:fx-comms-main"].firstMatch
        XCTAssertTrue(back.waitForExistence(timeout: appear), "the fixture chat is open")
        return app
    }

    private func shot(_ app: XCUIApplication, _ name: String) {
        let a = XCTAttachment(screenshot: app.screenshot())
        a.name = name
        a.lifetime = .keepAlways
        add(a)
    }

    /// `BackSwipeUITests`' idiom: an element that has to leave needs a predicate wait, since
    /// `exists` on its own only says what was true when it was read.
    private func assertGone(_ element: XCUIElement, _ message: String,
                            file: StaticString = #filePath, line: UInt = #line) {
        let gone = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: element)
        XCTAssertEqual(XCTWaiter().wait(for: [gone], timeout: 15), .completed, message, file: file, line: line)
    }

    private func presence(_ app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any)["presence-row"].firstMatch
    }

    /// The app's own word for "the reader is away from the end".
    ///
    /// Two affordances share that slot and they are mutually exclusive: `newMessagesPill`
    /// when rows have landed below, and the bare chevron when they have not. This fixture
    /// is the second case — the streaming row keeps its id while it works, so nothing counts
    /// as new below and the pill never comes. `chat-jump-to-latest` names the pill only, so
    /// waiting on it here waits for something this chat cannot show.
    private func awayMarker(_ app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any)["chat-scroll-to-bottom"].firstMatch
    }

    /// One deliberate drag rather than a flick.
    ///
    /// `swipeDown()` throws the transcript and leaves where it comes to rest to the
    /// deceleration. On a six-message fixture that is a short run-up, and on a busy runner
    /// the resting place stops being the app's answer and becomes the machine's — run
    /// 42540143057 failed `testTheRowScrollsAwayWithTheEndAndComesBack` on exactly that,
    /// while the photographs of the same behaviour passed. `thenHoldForDuration` stops the
    /// finger before it lifts, so the release carries no momentum (the idiom
    /// `ChatSwipeTrackingUITests` already uses) and the transcript rests where the drag
    /// put it.
    ///
    /// It starts below the header, and `end` says where it finishes as a fraction of the
    /// window. The keyboard case has to finish inside the keyboard's own band or there is
    /// nothing for `.scrollDismissesKeyboard(.interactively)` to dismiss; the keyboard-down
    /// case has to stop short of the composer or the drag never reaches the transcript.
    private func dragIntoHistory(_ app: XCUIApplication, endingAt end: CGFloat) {
        let from = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.25))
        let to = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: end))
        from.press(forDuration: 0.05, thenDragTo: to, withVelocity: .slow, thenHoldForDuration: 0.4)
    }

    private func composerTop(_ app: XCUIApplication) -> CGFloat {
        let plus = app.descendants(matching: .any)["composer-plus"].firstMatch
        XCTAssertTrue(plus.waitForExistence(timeout: appear), "the composer is on screen")
        return plus.frame.minY
    }

    /// At the live bottom nothing moved: the row still rests over the composer.
    func testTheRowRestsOverTheComposerAtTheLiveBottom() {
        let app = openWorkingChat()
        let row = presence(app)
        XCTAssertTrue(row.waitForExistence(timeout: appear), "the agent is working, so the row shows")
        shot(app, "presence-at-the-live-bottom")
        XCTAssertLessThanOrEqual(row.frame.maxY, composerTop(app) + 1,
                                 "the row sits above the composer's top edge, not over it")
    }

    /// The bug: up in history the row used to stay on screen, over the old bubbles. It is
    /// the transcript's last child now, so it leaves with the end it describes.
    func testTheRowScrollsAwayWithTheEndAndComesBack() {
        let app = openWorkingChat()
        let row = presence(app)
        XCTAssertTrue(row.waitForExistence(timeout: appear), "the agent is working, so the row shows")

        // Stops short of the composer, which would otherwise take the drag.
        dragIntoHistory(app, endingAt: 0.80)
        let jump = awayMarker(app)
        XCTAssertTrue(jump.waitForExistence(timeout: 15), "the reader is up in history")
        shot(app, "presence-scrolled-up-in-history")

        // Gone, or at least no longer drawn over the transcript the reader is looking at.
        let top = composerTop(app)
        XCTAssertTrue(!row.exists || row.frame.minY >= top,
                      "the row must not float over the old bubbles: frame \(row.frame), composer top \(top)")

        // And it comes back by itself when the reader returns to the end.
        jump.tap()
        XCTAssertTrue(row.waitForExistence(timeout: 15), "back at the end, the row is back")
        XCTAssertLessThanOrEqual(row.frame.maxY, composerTop(app) + 1,
                                 "and it rests over the composer again")
        shot(app, "presence-back-at-the-live-bottom")
    }

    /// Scrolling up with the keyboard open, which is one gesture and not two: the drag
    /// dismisses the keyboard (`.scrollDismissesKeyboard(.interactively)`) *and* carries the
    /// reader into history, so the container grows by the keyboard's height while they are
    /// leaving the end.
    ///
    /// The reader has to keep the place they dragged to. Measured from the live container,
    /// that one gesture reads as an arrival at the end rather than a departure from it, which
    /// latches `pinned`; and once the presence row lives in the transcript, the next time it
    /// changes size the transcript re-sticks to the end and the reader is back at the bottom
    /// without having asked. Runs 42545157179 / 42545159346 caught exactly that, as a history
    /// pair whose two legs came to rest in different places off the same drag.
    func testTheReaderKeepsTheirPlaceWhenTheirOwnDragDismissesTheKeyboard() {
        let app = openWorkingChat()
        XCTAssertTrue(presence(app).waitForExistence(timeout: appear), "the agent is working")

        // Focused the way a reader focuses it. The launch-time draft hook sets `composerFocused`
        // during setup, which is not the same path and does not raise the keyboard the same way.
        let placeholder = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label CONTAINS %@", "Ask Comms")).firstMatch
        XCTAssertTrue(placeholder.waitForExistence(timeout: appear), "the composer is on screen")
        placeholder.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 15), "the keyboard is up")
        shot(app, "keyboard-up-at-the-live-bottom")
        record(app, "keyboard up, before the drag")

        // Ends deep in the keyboard's band so the one gesture both carries the reader into
        // history and dismisses the keyboard, which is the pairing under test. It crosses
        // ~320 pt of transcript before it reaches the keyboard at all, so the reader is
        // well past the 120 pt the app calls "away" before the dismissal even starts.
        dragIntoHistory(app, endingAt: 0.95)
        assertGone(app.keyboards.firstMatch,
                   "the drag dismissed the keyboard, so this is the gesture under test")

        // Corroboration, not proof. The chevron is driven by `awayFromBottom`, so it says
        // what the app believes rather than where the transcript is — and run 42558663139
        // had it up while the transcript was back at the live bottom. Everything that
        // decides this case is measured below; this only says the drag registered at all.
        let jump = awayMarker(app)
        XCTAssertTrue(jump.waitForExistence(timeout: 15),
                      "the drag registered as a departure from the end")
        assertInHistory(app, "straight after the drag")

        // The yank does not arrive with the drag, it arrives with the next growth of the
        // transcript — which is why a screenshot taken straight after the drag can miss it.
        // The working row changes size on its own, so waiting is all this needs.
        let landmark = topBubble(app)
        Thread.sleep(forTimeInterval: 4)
        shot(app, "history-after-the-keyboard-went-down")

        // The transcript's own position, not the chevron's existence.
        assertInHistory(app, "after the transcript grew under them")
        if let landmark, let now = bubble(app, landmark.id) {
            XCTAssertEqual(now.minY, landmark.frame.minY, accuracy: 2,
                           "\(landmark.id) is where the reader left it — was \(landmark.frame), now \(now)")
        }

        let top = composerTop(app)
        let row = presence(app)
        XCTAssertTrue(!row.exists || row.frame.minY >= top,
                      "and the row stays gone while they are up here: frame \(row.frame), composer top \(top)")
    }

    private func bubbles(_ app: XCUIApplication) -> [(id: String, frame: CGRect)] {
        app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier BEGINSWITH %@", "bubble:"))
            .allElementsBoundByIndex
            .map { (id: $0.identifier, frame: $0.frame) }
            .filter { $0.frame.height > 0 }
    }

    private func bubble(_ app: XCUIApplication, _ id: String) -> CGRect? {
        bubbles(app).first { $0.id == id }?.frame
    }

    /// Every rendered row, the composer, the keyboard and the window on one line, recorded as an
    /// activity so it reaches `xcodebuild-test.log` whether the case passes or fails.
    ///
    /// A failure in `assertInHistory` is one number, and one number cannot distinguish the two
    /// things that produce it: a transcript that was yanked back to the end, and a transcript
    /// with too little history to be anywhere else. `fx-comms-main` is six messages, and the
    /// viewport grows by the keyboard's whole height when the keyboard goes away — so "there was
    /// nowhere left to stand" is a real candidate and has to be ruled in or out by measurement
    /// rather than argued about.
    private func record(_ app: XCUIApplication, _ state: String) {
        let rows = bubbles(app).sorted { $0.frame.minY < $1.frame.minY }
        let listed = rows.map { "\($0.id) \(Int($0.frame.minY))…\(Int($0.frame.maxY))" }
            .joined(separator: ", ")
        let span = rows.isEmpty ? "none" : "\(Int(rows[0].frame.minY))…\(Int(rows[rows.count - 1].frame.maxY))"
        let row = presence(app)
        let presenceText = row.exists ? "\(Int(row.frame.minY))…\(Int(row.frame.maxY))" : "absent"
        XCTContext.runActivity(named: "geometry — \(state)") { _ in
            let plus = app.descendants(matching: .any)["composer-plus"].firstMatch
            let top = plus.exists ? String(Int(plus.frame.minY)) : "?"
            XCTContext.runActivity(named: "window \(Int(app.windows.firstMatch.frame.height)), "
                                   + "composer top \(top), keyboard "
                                   + "\(app.keyboards.firstMatch.exists ? "up" : "down"), "
                                   + "rows span \(span), presence \(presenceText)") { _ in }
            XCTContext.runActivity(named: "rows [\(listed)]") { _ in }
        }
    }

    /// The highest bubble on screen, kept with its identifier so the comparison later is
    /// against the same row rather than whichever row is topmost by then — lazy rows join
    /// and leave the tree, so position alone does not name a bubble.
    ///
    /// Growth happens at the end, below this one, so a reader who keeps their place keeps it
    /// exactly where it was.
    private func topBubble(_ app: XCUIApplication) -> (id: String, frame: CGRect)? {
        bubbles(app).filter { $0.frame.maxY > 0 }.min { $0.frame.minY < $1.frame.minY }
    }

    /// The reader is in history: the end of the transcript is not in view, so the newest
    /// bubble sits below the composer's top edge rather than resting above it.
    ///
    /// This is the complement of `testTheRowRestsOverTheComposerAtTheLiveBottom`'s assertion,
    /// and it is deliberately about the transcript rather than about any indicator over it.
    private func assertInHistory(_ app: XCUIApplication, _ state: String,
                                 file: StaticString = #filePath, line: UInt = #line) {
        record(app, state)
        let top = composerTop(app)
        let frames = bubbles(app)
        XCTAssertFalse(frames.isEmpty, "\(state): the transcript has rows", file: file, line: line)
        guard let newest = frames.max(by: { $0.frame.maxY < $1.frame.maxY }) else { return }
        XCTAssertGreaterThan(newest.frame.maxY, top + 1,
                             "\(state): the reader is up in history, so the newest row is below "
                             + "the fold rather than resting above the composer — newest "
                             + "\(newest.id) \(newest.frame), composer top \(top)",
                             file: file, line: line)
    }
}
