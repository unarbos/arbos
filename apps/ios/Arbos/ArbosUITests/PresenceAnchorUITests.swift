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

    private func presence(_ app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any)["presence-row"].firstMatch
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

        let transcript = app.scrollViews.firstMatch
        for _ in 0..<4 { transcript.swipeDown() }
        // The jump-to-latest disc only shows when the reader is away from the end, so it is
        // the app's own word for "this test actually scrolled up".
        let jump = app.descendants(matching: .any)["chat-jump-to-latest"].firstMatch
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

        let transcript = app.scrollViews.firstMatch
        for _ in 0..<4 { transcript.swipeDown() }

        let jump = app.descendants(matching: .any)["chat-jump-to-latest"].firstMatch
        XCTAssertTrue(jump.waitForExistence(timeout: 15),
                      "the drag took the reader into history, keyboard or no keyboard")

        // The yank does not arrive with the drag, it arrives with the next growth of the
        // transcript — which is why a screenshot taken straight after the drag can miss it.
        // The working row changes size on its own, so waiting is all this needs.
        Thread.sleep(forTimeInterval: 4)
        shot(app, "history-after-the-keyboard-went-down")
        XCTAssertTrue(jump.exists,
                      "the transcript must not re-stick to the end under a reader who left it")

        let top = composerTop(app)
        let row = presence(app)
        XCTAssertTrue(!row.exists || row.frame.minY >= top,
                      "and the row stays gone while they are up here: frame \(row.frame), composer top \(top)")
    }
}
