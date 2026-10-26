import XCTest
@testable import Arbos

/// Back-gesture rules (feedback `iphone-back-swipe-restore-2026-10-25`, #1831): the system edge
/// pop is never blocked by screen state — only by having nothing to pop to or a running
/// transition — and the full-screen swipe-right counts as "back" for a clear right pan that did
/// not start in the composer bar. Before this, both were gated on the composer's focus and the
/// keyboard being up (as it is after every send) left every chat without a back swipe.
final class BackSwipeTests: XCTestCase {
    func testEdgePopBeginsWheneverTheStackHasSomethingToPopTo() {
        XCTAssertTrue(BackSwipe.popAllowed(stackDepth: 2, transitioning: false), "chat over Home")
        XCTAssertTrue(BackSwipe.popAllowed(stackDepth: 3, transitioning: false), "job tab over the agent chat")
        XCTAssertFalse(BackSwipe.popAllowed(stackDepth: 1, transitioning: false), "Home has nothing to pop to")
        XCTAssertFalse(BackSwipe.popAllowed(stackDepth: 2, transitioning: true), "never mid-push or mid-pop")
    }

    func testEdgePopTakesNoScreenStateIntoAccount() {
        // The rule has no focus / draft / edit input at all: a keyboard up after a send
        // cannot block it, which is what #1554's gate did.
        let signature: (Int, Bool) -> Bool = BackSwipe.popAllowed(stackDepth:transitioning:)
        XCTAssertTrue(signature(2, false))
    }

    func testAClearRightPanOnTheTranscriptIsBack() {
        XCTAssertTrue(BackSwipe.isBackSwipe(translation: CGSize(width: 90, height: 10),
                                            velocity: CGSize(width: 800, height: 40),
                                            startY: 400, composerTop: 760))
        // Slow but long is also back.
        XCTAssertTrue(BackSwipe.isBackSwipe(translation: CGSize(width: 130, height: 20),
                                            velocity: CGSize(width: 10, height: 30),
                                            startY: 400, composerTop: 760))
        // From the very edge, with no composer on the screen (server channel list).
        XCTAssertTrue(BackSwipe.isBackSwipe(translation: CGSize(width: 200, height: 0),
                                            velocity: CGSize(width: 1200, height: 0),
                                            startY: 400, composerTop: nil))
    }

    func testScrollsLeftPansAndShortDragsAreNotBack() {
        XCTAssertFalse(BackSwipe.isBackSwipe(translation: CGSize(width: 20, height: 300),
                                             velocity: CGSize(width: 10, height: 900),
                                             startY: 400, composerTop: 760), "vertical scroll")
        XCTAssertFalse(BackSwipe.isBackSwipe(translation: CGSize(width: -120, height: 0),
                                             velocity: CGSize(width: -900, height: 0),
                                             startY: 400, composerTop: 760), "time reveal (drag left)")
        XCTAssertFalse(BackSwipe.isBackSwipe(translation: CGSize(width: 60, height: 5),
                                             velocity: CGSize(width: 300, height: 0),
                                             startY: 400, composerTop: 760), "too short")
        XCTAssertFalse(BackSwipe.isBackSwipe(translation: CGSize(width: 90, height: 85),
                                             velocity: CGSize(width: 300, height: 290),
                                             startY: 400, composerTop: 760), "diagonal")
    }

    func testADragThatStartsInTheComposerBarIsNeverBack() {
        // Selecting text in the field or sliding the mic: a right pan that begins at or
        // below the bar's top edge, however fast or long.
        XCTAssertFalse(BackSwipe.isBackSwipe(translation: CGSize(width: 200, height: 0),
                                             velocity: CGSize(width: 1500, height: 0),
                                             startY: 760, composerTop: 760))
        XCTAssertFalse(BackSwipe.isBackSwipe(translation: CGSize(width: 200, height: 0),
                                             velocity: CGSize(width: 1500, height: 0),
                                             startY: 800, composerTop: 760))
        // Just above the bar is the transcript: back.
        XCTAssertTrue(BackSwipe.isBackSwipe(translation: CGSize(width: 200, height: 0),
                                            velocity: CGSize(width: 1500, height: 0),
                                            startY: 759, composerTop: 760))
    }
}
