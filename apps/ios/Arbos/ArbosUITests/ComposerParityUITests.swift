import UIKit
import XCTest

/// Grok parity for the composer, on the screen rather than in the arithmetic
/// (Jacob, 2026-11-09). The pure rules are in `ComposerParityTests`; these are the claims that
/// only the laid-out box can answer, including the one Jacob has now asked for twice.
///
/// `testTheBoxVisiblyLightsUnderAHoldingFinger` is the regression guard for the touch-and-hold
/// highlight ([feedback composer-hold-highlight-2026-11-09], Jacob 2026-11-09 11:55;
/// [feedback attachment-in-composer-2026-11-09] is the staged picture). It went missing once
/// already in a way worth
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

    /// The mean brightness of a rectangle of the screen, 0–255.
    ///
    /// The screenshot is in pixels and an element's frame is in points, so the frame is scaled
    /// by the ratio between the two rather than by an assumed device scale.
    private func brightness(_ app: XCUIApplication, of rect: CGRect) -> Double {
        guard let shot = app.screenshot().image.cgImage else { return 0 }
        let scale = CGFloat(shot.width) / app.windows.firstMatch.frame.width
        let box = CGRect(x: rect.minX * scale, y: rect.minY * scale,
                         width: rect.width * scale, height: rect.height * scale)
        guard let piece = shot.cropping(to: box) else { return 0 }
        let width = piece.width, height = piece.height
        guard width > 0, height > 0 else { return 0 }
        var grey = [UInt8](repeating: 0, count: width * height)
        guard let canvas = CGContext(data: &grey, width: width, height: height,
                                     bitsPerComponent: 8, bytesPerRow: width,
                                     space: CGColorSpaceCreateDeviceGray(),
                                     bitmapInfo: CGImageAlphaInfo.none.rawValue) else { return 0 }
        canvas.draw(piece, in: CGRect(x: 0, y: 0, width: width, height: height))
        return grey.reduce(0.0) { $0 + Double($1) } / Double(width * height)
    }

    /// The light has to be *seen*, not merely drawn.
    ///
    /// The first cut of this test asked whether the lit overlay existed, and it passed while
    /// Jacob measured the actual change in the pill at a mean of about +2.7 out of 255 — a
    /// highlight that is present in the view tree and invisible in a still. So this measures
    /// the pill itself, before and during, and holds the light to a delta a person can see.
    ///
    /// Magnitude rather than sign: the wash is drawn in the theme's text colour, so it
    /// brightens a dark pill and deepens a light one. A white wash on a pale pill cannot be
    /// seen, so "brighter" is not the invariant — "clearly different under the finger" is, and
    /// that holds whichever appearance the simulator happens to be in.
    func testTheBoxVisiblyLightsUnderAHoldingFinger() {
        let app = openChat()
        let box = field(app)
        XCTAssertTrue(box.waitForExistence(timeout: appear), "the composer is on screen")

        let lit = app.descendants(matching: .any)["input-hold-highlight"].firstMatch
        XCTAssertFalse(lit.exists, "nothing is lit before a finger is anywhere near it")

        // Focus first, and only then measure. A press on an unfocused field raises the
        // keyboard, which lifts the composer by the keyboard's height — the two samples would
        // then cover different parts of the screen and the delta would mean nothing.
        box.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 10), "the keyboard is up")
        Thread.sleep(forTimeInterval: 1)
        let pill = box.frame
        let resting = brightness(app, of: pill)
        // Held well past the arming delay, and the press is what raises the system's own
        // Paste / AutoFill menu too — the highlight must not take that touch away. The light
        // outlasts the lift on purpose, which is what makes it measurable from a still at all.
        box.press(forDuration: 1.2)
        XCTAssertTrue(lit.waitForExistence(timeout: 5),
                      "touch-and-hold lights the box the way Grok's does")

        let held = brightness(app, of: pill)
        let delta = abs(held - resting)
        XCTAssertGreaterThan(delta, 12,
                             "the light is too faint to see: the pill read \(resting) at rest "
                             + "and \(held) under the finger, a delta of \(delta). Jacob "
                             + "measured the cut that failed review at about 2.7.")
    }
}
