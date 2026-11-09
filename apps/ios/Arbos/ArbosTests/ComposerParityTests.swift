import XCTest
@testable import Arbos

/// The composer's Grok-parity rules, as rules rather than as pixels
/// (Jacob, 2026-11-09; [feedback attachment-in-composer-2026-11-09] for the staged picture).
final class ComposerParityTests: XCTestCase {
    func testTheMicStaysWhileYouType() {
        // Ours used to drop the mic on the first character, which took the voice memo away
        // from anyone who started typing and changed their mind. Grok keeps it beside the
        // arrow, so there is no state that hides it.
        XCTAssertTrue(ComposerTrailing.micShown())
    }

    func testTheControlsMoveToTheirOwnRowOnceTheTextWraps() {
        XCTAssertFalse(ComposerLayout.stacked(wasStacked: false, lines: 1, empty: false, attachments: 0),
                       "one line keeps the controls beside the text")
        XCTAssertTrue(ComposerLayout.stacked(wasStacked: false, lines: 2, empty: false, attachments: 0),
                      "a second line puts them on their own row")
    }

    func testTheRowIsHeldUntilTheDraftIsEmpty() {
        // Stacking hands the text back the width the controls were using, so a line that
        // wrapped beside them may fit without them. Re-deciding on that would unstack, re-wrap
        // and stack again for any text near the boundary.
        XCTAssertTrue(ComposerLayout.stacked(wasStacked: true, lines: 1, empty: false, attachments: 0),
                      "a wrapped composer stays stacked while there is still a draft")
        XCTAssertFalse(ComposerLayout.stacked(wasStacked: true, lines: 1, empty: true, attachments: 0),
                       "and comes back to one row when the draft is emptied")
    }

    func testAStagedPictureAlwaysMakesTheTallCard() {
        XCTAssertTrue(ComposerLayout.stacked(wasStacked: false, lines: 1, empty: true, attachments: 1),
                      "the thumbnail needs the width even with nothing typed")
    }

    func testTheBadgeHangsOffTheThumbnailsCorner() {
        // Half in, half out: inside the picture it covers the part being checked, fully
        // outside it collides with the box's rounded corner.
        XCTAssertEqual(ComposerAttachment.overhang, ComposerAttachment.badge / 2)
    }

    func testTheMicSitsOnTheSameDiscAsThePlus() {
        XCTAssertEqual(Theme.composerDisc, Theme.actionPillHeight)
    }

    func testTheDiscIsAFillRatherThanMoreOfThePillsGlass() {
        // The + already drew a `ControlSurface`, which on Glass *is* the pill's clear glass —
        // so it drew a disc nobody could see. A fill is the whole point.
        XCTAssertGreaterThan(ComposerDisc.fill, 0)
    }

    func testAWrappedCountComesFromTheHeightTheTextWants() {
        let insets = UIEdgeInsets(top: 10, left: 0, bottom: 10, right: 0)
        XCTAssertEqual(MentionTextView.lines(fit: 42, line: 22, insets: insets), 1)
        XCTAssertEqual(MentionTextView.lines(fit: 64, line: 22, insets: insets), 2)
        XCTAssertEqual(MentionTextView.lines(fit: 0, line: 0, insets: insets), 1,
                       "a font with no line height must not divide by zero")
    }

    func testTheBoxLightsOnlyForAFingerThatRests() {
        XCTAssertFalse(HoldHighlight.lights(held: 0.1, drift: 0), "a tap lights nothing")
        XCTAssertTrue(HoldHighlight.lights(held: HoldHighlight.delay, drift: 0),
                      "a rest at the threshold lights")
        XCTAssertFalse(HoldHighlight.lights(held: 1, drift: HoldHighlight.drift + 1),
                       "a finger that wandered is scrolling or selecting, not holding")
    }

    func testTheWashCoversAOneLinePillsHeight() {
        // A radius under the box's height would read as a spotlight with an edge rather than
        // Grok's wash.
        let pill = CGSize(width: 320, height: Theme.composerHeight)
        XCTAssertGreaterThan(HoldHighlight.radius(of: pill), pill.height)
    }
}
