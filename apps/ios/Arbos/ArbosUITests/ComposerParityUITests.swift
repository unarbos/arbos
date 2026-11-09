import XCTest

/// Grok parity for the composer, on the screen rather than in the arithmetic
/// (Jacob, 2026-11-09). The pure rules are in `ComposerParityTests`; these are the three
/// claims that only the laid-out box can answer, plus the one Jacob has now asked for twice.
///
/// `testTheBoxLightsUnderAHoldingFinger` is the regression guard for the touch-and-hold
/// highlight ([feedback attachment-in-composer-2026-11-09] is the staged picture; the
/// highlight is Jacob, 2026-11-09 11:55). It went missing once already in a way worth
/// knowing about: `ControlSurface` has always taken a `pressed` flag and `Theme.controlPill`
/// has always plumbed it, but nothing ever passed `true`. A test that only checked the flag
/// existed would have passed throughout. This one holds a finger on the real field and looks
/// for the lit overlay, so the highlight has to actually be driven.
final class ComposerParityUITests: XCTestCase {
    private let appear: TimeInterval = 90

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func openChat() -> XCUIApplication {
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

    private func field(_ app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any)["composer-field"].firstMatch
    }

    private func plus(_ app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any)["composer-plus"].firstMatch
    }

    private func mic(_ app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any)["composer-mic"].firstMatch
    }

    /// Enough characters to wrap several times at any phone width.
    private let longDraft = String(repeating: "composer parity ", count: 14)

    func testTheMicIsStillThereWhileYouType() {
        let app = openChat()
        let box = field(app)
        XCTAssertTrue(box.waitForExistence(timeout: appear), "the composer is on screen")
        XCTAssertTrue(mic(app).exists, "the mic is there before anything is typed")

        box.tap()
        box.typeText("hello")
        XCTAssertTrue(mic(app).waitForExistence(timeout: 5),
                      "Grok keeps the mic beside the send arrow while you type")
    }

    func testTheControlsStayAtTheBottomWhileTheTextGrowsAboveThem() {
        let app = openChat()
        let box = field(app)
        XCTAssertTrue(box.waitForExistence(timeout: appear), "the composer is on screen")
        box.tap()

        let before = plus(app).frame
        let topBefore = box.frame.minY
        box.typeText(longDraft)

        // The composer is pinned to the bottom of the screen and grows upward, so a `+` on
        // the bottom row hardly moves while the box's top edge climbs. Centred against the
        // text — what ours did — it would rise with the box by about half that climb.
        let after = plus(app).frame
        let climb = topBefore - box.frame.minY
        XCTAssertGreaterThan(climb, 40,
                             "the field grew: top was \(topBefore), now \(box.frame.minY)")
        XCTAssertLessThan(abs(after.midY - before.midY), climb / 4,
                          "the + stayed on the bottom row — was \(before), now \(after), "
                          + "the box climbed \(climb)")
    }

    /// [feedback attachment-in-composer-2026-11-09]
    ///
    /// A staged picture previews inside the composer — a big rounded thumbnail of the picture
    /// itself with a ✕ on its corner, the ordinary placeholder below it and the controls still
    /// on the bottom row — instead of the capsule chip with a filename that sat above the box.
    func testAStagedPicturePreviewsInsideTheComposer() {
        let app = XCUIApplication()
        app.launchEnvironment = [
            "ARBOS_FIXTURE": "1",
            "ARBOS_NO_STORE": "1",
            "ARBOS_OPEN_PROJECT": "fx-comms",
            "ARBOS_OPEN_CHAT": "fx-comms-main",
            "ARBOS_FIXTURE_COMPOSER_ATTACHMENT": "1",
        ]
        app.launch()

        let thumb = app.descendants(matching: .any)["composer-attachment"].firstMatch
        XCTAssertTrue(thumb.waitForExistence(timeout: appear), "the picture previews in the box")
        XCTAssertGreaterThan(thumb.frame.height, 80,
                             "a preview of the picture, not the old 28 pt chip: \(thumb.frame)")

        let box = field(app)
        XCTAssertTrue(box.exists, "the field is still there under the thumbnail")
        XCTAssertGreaterThan(box.frame.minY, thumb.frame.minY,
                             "the placeholder sits below the picture — thumbnail \(thumb.frame), "
                             + "field \(box.frame)")
        XCTAssertGreaterThan(plus(app).frame.midY, box.frame.minY,
                             "the controls stay on the bottom row of the grown card")

        let remove = app.descendants(matching: .any)["composer-remove-attachment"].firstMatch
        XCTAssertTrue(remove.exists, "the ✕ badge is on the thumbnail's corner")
        XCTAssertGreaterThan(remove.frame.midX, thumb.frame.midX, "top-right of the picture")
        XCTAssertLessThan(remove.frame.midY, thumb.frame.midY, "top-right of the picture")

        remove.tap()
        XCTAssertTrue(thumb.waitForNonExistence(timeout: 5), "the ✕ unstages the picture")
    }

    func testTheBoxLightsUnderAHoldingFinger() {
        let app = openChat()
        let box = field(app)
        XCTAssertTrue(box.waitForExistence(timeout: appear), "the composer is on screen")

        let lit = app.descendants(matching: .any)["input-hold-highlight"].firstMatch
        XCTAssertFalse(lit.exists, "nothing is lit before a finger is anywhere near it")

        // Held well past the arming delay, and the press is what raises the system's own
        // Paste / AutoFill menu too — the highlight must not take that touch away.
        box.press(forDuration: 1.2)
        XCTAssertTrue(lit.waitForExistence(timeout: 5),
                      "touch-and-hold lights the box the way Grok's does")
    }
}
