import XCTest

/// What `.defaultScrollAnchor(.bottom)` buys us, pinned before anyone changes it.
///
/// The transcript keeps a reader at the end through a resize because the scroll view's
/// bottom edge is anchored (`ChatView.swift`, the `.defaultScrollAnchor(.bottom)` on the
/// transcript). That one modifier is load-bearing for three separate behaviours, and only
/// the first of them has a test: `TranscriptFirstFrameUITests` asserts a chat opens on its
/// newest row ("the newest row starts inside the window"). The other two are here.
///
/// They are deliberately written *before* the anchor is touched for #2920, and they must be
/// green before and after. A test written after a change can only say the change did what
/// its author meant; one that was already green says the change took nothing away. The
/// failure they exist to catch is a reader who stops being carried to the end when the
/// container resizes — a chat that opens mid-history, or one that strands the newest
/// message below the fold when the keyboard moves.
///
/// The assertion is relative, not absolute: the newest bubble's bottom against the
/// composer's top *in the same state*. The keyboard moves both, so a fixed coordinate would
/// encode the keyboard's height and break on any device, while the relation holds in every
/// state the reader can be at the end in.
final class TranscriptEndAnchorUITests: XCTestCase {
    private let appear: TimeInterval = 90

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    /// `fx-comms-main`: the agent is mid-turn, so the transcript grows on its own while the
    /// test watches. That growth is what the new-row guard needs and it needs no send.
    private func openWorkingChat() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = [
            "ARBOS_FIXTURE": "1",
            "ARBOS_NO_STORE": "1",
            "ARBOS_OPEN_PROJECT": "fx-comms",
            "ARBOS_OPEN_CHAT": "fx-comms-main",
        ]
        app.launch()
        let back = app.descendants(matching: .any)["chat-back:fx-comms-main"].firstMatch
        XCTAssertTrue(back.waitForExistence(timeout: appear), "the fixture chat is open")
        return app
    }

    private func bubbles(_ app: XCUIApplication) -> [XCUIElement] {
        app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier BEGINSWITH %@", "bubble:"))
            .allElementsBoundByIndex
    }

    /// The composer's top edge in the state it is in right now.
    ///
    /// `composer-field` rather than `composer-plus`: the field is in every chat kind, where
    /// the `+` is hidden in channels, and it keeps its identity through the composer's own
    /// layout changes.
    private func composerTop(_ app: XCUIApplication) -> CGFloat {
        let field = app.textViews["composer-field"]
        XCTAssertTrue(field.waitForExistence(timeout: appear), "the composer is on screen")
        return field.frame.minY
    }

    /// The reader is at the end: the newest bubble is drawn, and all of it is above the
    /// composer rather than pushed under it or below the fold.
    ///
    /// Scrolling up moves content *down*, so a transcript that has slipped off the end puts
    /// the newest bubble's bottom past the composer's top — which is what this catches, and
    /// it catches a slip of a few points, not only a whole screen.
    private func assertAtTheEnd(_ app: XCUIApplication, _ state: String,
                                file: StaticString = #filePath, line: UInt = #line) {
        let frames = bubbles(app).map(\.frame).filter { $0.height > 0 }
        XCTAssertFalse(frames.isEmpty, "\(state): the transcript has rows", file: file, line: line)
        guard let newest = frames.max(by: { $0.maxY < $1.maxY }) else { return }
        let top = composerTop(app)
        XCTAssertLessThanOrEqual(newest.maxY, top + 1,
                                 "\(state): the newest row ends above the composer, so the reader "
                                 + "is at the end — newest \(newest), composer top \(top)",
                                 file: file, line: line)
        let window = app.windows.firstMatch.frame
        XCTAssertGreaterThan(newest.maxY, window.minY,
                             "\(state): the newest row is on screen rather than off the top — "
                             + "newest \(newest) in \(window)",
                             file: file, line: line)
    }

    private func raiseKeyboard(_ app: XCUIApplication) {
        let field = app.textViews["composer-field"]
        XCTAssertTrue(field.waitForExistence(timeout: appear), "the composer is on screen")
        field.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 15), "the keyboard is up")
    }

    /// A content growth under a reader who is already at the end must carry them with it.
    ///
    /// This is the `onStick` path: the reporter holds `pinned` while the reader is at the
    /// end, and a growth re-sticks the transcript. The streaming row rewrites itself about
    /// once a second, so the wait is the whole mechanism — nothing here has to send.
    func testAGrowingRowAtTheEndKeepsTheReaderAtTheEnd() {
        let app = openWorkingChat()
        assertAtTheEnd(app, "just opened")
        // Several ticks of the streaming row, so this is a real sequence of growths rather
        // than whichever single frame the first look happened to catch.
        Thread.sleep(forTimeInterval: 6)
        assertAtTheEnd(app, "after the row grew under them")
    }

    /// The container shrinking under a reader at the end must keep them at the end.
    ///
    /// The keyboard coming up takes roughly its own height out of the container. Anchored at
    /// the bottom the end stays put and older rows leave off the top, which is what
    /// `TranscriptSpacingUITests` observes in passing when it says raising the keyboard
    /// scrolls rows off.
    func testTheKeyboardComingUpAtTheEndKeepsTheReaderAtTheEnd() {
        let app = openWorkingChat()
        assertAtTheEnd(app, "keyboard down")
        raiseKeyboard(app)
        assertAtTheEnd(app, "keyboard up")
    }

    /// An element's frame once it has held still across two reads, so a row is measured where
    /// it came to rest and not where a scroll or the keyboard was carrying it.
    private func restingFrame(_ element: XCUIElement) -> CGRect {
        var last = element.frame
        for _ in 0..<20 {
            usleep(300_000)
            let now = element.frame
            if now == last { return now }
            last = now
        }
        return last
    }

    private func openLaunchWeek(_ extra: [String: String] = [:]) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1", "ARBOS_NO_STORE": "1",
                                 "ARBOS_OPEN_PEOPLE": "fx-group-launch"].merging(extra) { $1 }
        app.launch()
        XCTAssertTrue(app.textViews["composer-field"].waitForExistence(timeout: appear),
                      "the fixture group is open")
        return app
    }

    /// The container shrinking under a reader who has scrolled up must leave them where they
    /// are (tester report, 2026-11-14: "I'd prefer it just stays where you were. Would make it
    /// easier to type a reply to someone while re-reading someone's message").
    ///
    /// The other half of the case above, and the two differ only in where the reader was when
    /// the keyboard came up. Focusing the composer used to send everyone to the newest row,
    /// so the row asserted on here left the screen.
    func testTheKeyboardComingUpInHistoryLeavesTheReaderWhereTheyAre() {
        let app = openLaunchWeek()
        let window = app.windows.firstMatch
        window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.35))
            .press(forDuration: 0.1, thenDragTo: window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.8)))

        // A row that starts under the header and above the middle of the window, so it is
        // clear of where the composer and the keyboard will be.
        let header = app.descendants(matching: .any)["chat-search-button"].firstMatch.frame
        let rows = bubbles(app)
        // The drag's own deceleration: once one row is at rest they all are.
        if let any = rows.first { _ = restingFrame(any) }
        guard let row = rows.first(where: { $0.frame.minY > header.maxY && $0.frame.minY < window.frame.midY }) else {
            return XCTFail("a row starts in the upper half of the screen after scrolling up")
        }
        let id = row.identifier
        let before = row.frame

        raiseKeyboard(app)

        let after = app.descendants(matching: .any)[id].firstMatch
        XCTAssertTrue(after.exists, "the row the reader was on is still drawn")
        XCTAssertEqual(restingFrame(after).minY, before.minY, accuracy: 2,
                       "and it has not moved: the reader keeps their place when the keyboard opens")
    }

    /// A reader taken to an old message by a reveal (a quote tap, a search result) has not
    /// touched the transcript, so nothing they did says they left the end, and the message is
    /// older than the row window, so the window has slid onto it and the end of what is drawn
    /// is not the newest message. Opening the keyboard there must leave them on the message,
    /// which is the reply-to-what-I-am-reading case again, and scrolling down with the
    /// keyboard up must still arrive at the real newest message.
    ///
    /// Before, the reader was thrown fifteen rows down to the end of the slid window (the
    /// first assertion). The second half is a guard and did hold before: the window comes
    /// back as soon as the reader scrolls to its end.
    func testAfterARevealTheKeyboardKeepsThePlaceAndTheNewestRowCanStillBeReached() {
        let app = openLaunchWeek(["ARBOS_FIXTURE_LONG_ROWS": "420", "ARBOS_SHOW": "reveal-old",
                                  "ARBOS_TRACE_REVEAL": "1"])
        // `<message id> passes=… arrived=…` once the reveal's corrections have stopped.
        let probe = app.descendants(matching: .any)["reveal-probe"].firstMatch
        let landed = XCTNSPredicateExpectation(predicate: NSPredicate(format: "value CONTAINS %@", "arrived="),
                                               object: probe)
        XCTAssertEqual(XCTWaiter().wait(for: [landed], timeout: appear), .completed,
                       "the rig's reveal has landed — probe \(String(describing: probe.value))")
        let id = String((probe.value as? String ?? "").prefix { $0 != " " })
        let revealed = app.descendants(matching: .any)["bubble:\(id)"].firstMatch
        XCTAssertTrue(revealed.exists, "the revealed message \(id) is drawn")
        let before = restingFrame(revealed)

        raiseKeyboard(app)
        XCTAssertEqual(restingFrame(revealed).minY, before.minY, accuracy: 2,
                       "the reader stays on the message they were taken to when the keyboard opens")

        // The fixture's own last message, after all the padded history.
        let newest = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label CONTAINS %@", "1,284 signups"))
            .firstMatch
        let window = app.windows.firstMatch
        for _ in 0..<15 where !(newest.exists && newest.frame.maxY <= composerTop(app) + 1) {
            window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.45))
                .press(forDuration: 0.05, thenDragTo: window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.12)))
        }
        XCTAssertTrue(newest.exists, "scrolling down with the keyboard up reaches the newest message")
        assertAtTheEnd(app, "scrolled back down with the keyboard up")
    }

    /// The container *growing* under a reader at the end must also keep them at the end.
    ///
    /// This is the direction #2920 changes, and the reason it is a separate case from the one
    /// above: the fix has to stop the anchor dragging a reader who left the end back to it,
    /// without stopping it carrying a reader who never left. Those two differ only in where
    /// the reader was when the container grew, so the guard has to state the second half
    /// explicitly or a fix could satisfy the drag case by abandoning this one.
    ///
    /// The dismissal is a tap on empty chrome, which the transcript handles ("Tap empty chat
    /// chrome to dismiss the keyboard"), and not a drag: a drag is the gesture
    /// `PresenceAnchorUITests` tests and it takes the reader off the end on purpose, which is
    /// the opposite of what this case needs.
    func testTheKeyboardGoingDownAtTheEndKeepsTheReaderAtTheEnd() {
        let app = openWorkingChat()
        raiseKeyboard(app)
        assertAtTheEnd(app, "keyboard up")

        // The empty point is computed from the presence row rather than guessed. That row is
        // left-aligned and only as wide as its words, so the transcript's full width beside it
        // is chrome with nothing tappable in it — no bubble, no avatar, no link. A fixed
        // normalized coordinate would be a guess about which row the fixture put at that
        // height, and would start tapping bubbles the moment the fixture changed.
        //
        // The row is not unconditional, so the derivation is checked rather than assumed:
        // `presenceRow` draws only while `presenceParticipants` is non-empty, which for an
        // agent chat means `AppModel.isStreaming`, which caps at 15 minutes from
        // `liveStreamAt`. `fx-comms-main` stages its stream about 11.9 minutes before launch,
        // so the row is there with roughly three minutes to spare and the clock restarts every
        // launch. That is a guarantee with a margin, not a standing one: if someone moves the
        // fixture's timestamps the row goes, and these two assertions are what makes that a
        // loud failure here instead of a tap into empty space that quietly stops dismissing
        // the keyboard. A guard that can go green without dismissing is no guard.
        let row = app.descendants(matching: .any)["presence-row"].firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: appear),
                      "the tap point is derived from the presence row, so the row has to be here")
        let anchor = row.frame
        XCTAssertGreaterThan(anchor.height, 0,
                             "the presence row has a real frame to derive the tap point from — got \(anchor)")

        let window = app.windows.firstMatch
        let chrome = window.coordinate(withNormalizedOffset: .zero)
            .withOffset(CGVector(dx: window.frame.maxX - 6, dy: anchor.midY))
        chrome.tap()

        // The dismissal is asserted, not assumed: without this the case could pass by never
        // resizing the container at all, which is the one thing it exists to exercise.
        let gone = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"),
                                             object: app.keyboards.firstMatch)
        XCTAssertEqual(XCTWaiter().wait(for: [gone], timeout: 15), .completed,
                       "the tap on empty chrome dropped the keyboard")
        assertAtTheEnd(app, "keyboard down again")
    }
}
