import XCTest
@testable import Arbos

/// The reader's place survives the keyboard going away under their finger
/// ([feedback typing-anchor-2026-11-08], runs 42545157179 / 42545159346: the head leg of
/// the history pair came back at the live bottom while the base leg stayed up in history,
/// off the same drag from the same place).
///
/// The transcript measures how far it is from the end as
/// `contentSize − contentOffset − containerSize`, which is a distance from the container's
/// bottom edge. `.scrollDismissesKeyboard(.interactively)` retracts the keyboard under the
/// finger, so the container grows by the keyboard's whole height during the one gesture
/// that takes the reader away from the end — and the raw number falls by that height while
/// the reader is going the other way.
///
/// It only became visible when the presence row moved inside the transcript, because a row
/// that lives in the transcript changes the content size while it works, and a growth while
/// `pinned` re-sticks to the end. The latch was always wrong; it had nothing to fire it.
final class TranscriptScrollLatchTests: XCTestCase {
    /// iPhone 17 portrait, the height `BottomBarFadeTests` already models.
    private let keyboard: CGFloat = 336

    func testADragWithNoKeyboardInItReadsExactlyAsItAlwaysDid() {
        for sampled in [CGFloat(0), 60, 119, 120, 121, 400] {
            let held = TranscriptScrollLatch.distance(sampled: sampled, container: 874, gestureStart: 874)
            XCTAssertEqual(held, sampled, "a container that did not move corrects by nothing")
            XCTAssertEqual(TranscriptScrollLatch.away(distance: held), sampled > 120)
        }
    }

    /// The regression. A reader drags 300 pt up into history with the keyboard open; the
    /// same drag retracts the keyboard, so the container grows 336 pt under them.
    func testTheKeyboardRetractingUnderTheFingerDoesNotReadAsArrivingAtTheEnd() {
        let dragged: CGFloat = 300
        let withKeyboard: CGFloat = 874 - 336
        // What the scroll view reports once the keyboard has gone: the same content, a
        // taller container, so 36 pt *past* the end.
        let sampled = dragged - keyboard
        XCTAssertEqual(sampled, -36)
        XCTAssertFalse(TranscriptScrollLatch.away(distance: sampled),
                       "the raw sample is the bug: a reader 300 pt into history reads as pinned")

        let held = TranscriptScrollLatch.distance(sampled: sampled, container: 874, gestureStart: withKeyboard)
        XCTAssertEqual(held, dragged, "measured against the gesture's own container, it is the 300 pt drag")
        XCTAssertTrue(TranscriptScrollLatch.away(distance: held), "so the reader stays away from the end")
    }

    /// A reader who really did come back to the end during the same gesture still latches,
    /// so the correction cannot be used to strand someone at the bottom.
    func testComingBackToTheEndDuringTheSameGestureStillCounts() {
        let withKeyboard: CGFloat = 874 - 336
        let held = TranscriptScrollLatch.distance(sampled: -336, container: 874, gestureStart: withKeyboard)
        XCTAssertEqual(held, 0, "no content between them and the end")
        XCTAssertFalse(TranscriptScrollLatch.away(distance: held))
    }

    /// The threshold is the one the transcript has always used; this suite is about the
    /// measurement reaching it honestly, not about moving it.
    func testTheThresholdIsUnchanged() {
        XCTAssertEqual(TranscriptScrollLatch.awayThreshold, 120)
        XCTAssertFalse(TranscriptScrollLatch.away(distance: 120))
        XCTAssertTrue(TranscriptScrollLatch.away(distance: 120.5))
    }
}
