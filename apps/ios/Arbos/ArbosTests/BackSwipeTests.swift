import XCTest
@testable import Arbos

/// Horizontal gesture rules on the chat screens (#1831, then Jacob's 2026-10-26 iMessage
/// rule): a drag from the edge zone pops and is never blocked by screen state; the edge
/// zone belongs to it alone; a swipe right on a bubble away from the edge replies, decided
/// once at 12 pt so vertical scroll is untouched; the full-screen swipe right is "back"
/// only outside the edge zone, the composer bar and a bubble swipe.
final class BackSwipeTests: XCTestCase {
    private let far = CGPoint(x: 200, y: 400)

    // MARK: Edge pop

    func testADragFromTheEdgeZonePopsOnceItHasMovedRight() {
        let edge = CGPoint(x: 2, y: 400)
        XCTAssertTrue(BackSwipe.startsAtEdge(edge))
        XCTAssertFalse(BackSwipe.startsAtEdge(CGPoint(x: BackSwipe.edgeZoneWidth, y: 400)))
        XCTAssertTrue(BackSwipe.edgePops(translation: CGSize(width: BackSwipe.edgePopDistance, height: 5)))
        XCTAssertTrue(BackSwipe.edgePops(translation: CGSize(width: 200, height: -60)), "a long edge pan may wander")
        XCTAssertFalse(BackSwipe.edgePops(translation: CGSize(width: BackSwipe.edgePopDistance - 1, height: 0)), "too short")
        XCTAssertFalse(BackSwipe.edgePops(translation: CGSize(width: 50, height: 80)), "more down than right: a scroll from the edge")
        XCTAssertFalse(BackSwipe.edgePops(translation: CGSize(width: -50, height: 0)), "left")
    }

    func testTheEdgePopTakesNoScreenStateIntoAccount() {
        // Keyboard up, mid-edit, finger over the composer bar or over a bubble swipe: an
        // edge drag still pops. #1554's focus gate is what this replaces.
        let edge = CGPoint(x: 2, y: 800)
        XCTAssertTrue(BackSwipe.isBackSwipe(translation: CGSize(width: 60, height: 0), velocity: .zero,
                                            start: edge, composerTop: 760, bubbleSwipeActive: true))
        XCTAssertFalse(BackSwipe.isBackSwipe(translation: CGSize(width: 30, height: 0), velocity: CGSize(width: 2000, height: 0),
                                             start: edge, composerTop: 760, bubbleSwipeActive: false), "an edge drag pops by distance, not speed")
    }

    // MARK: Full-screen back swipe

    func testAClearRightPanOnTheTranscriptIsBack() {
        XCTAssertTrue(BackSwipe.isBackSwipe(translation: CGSize(width: 90, height: 10),
                                            velocity: CGSize(width: 800, height: 40),
                                            start: far, composerTop: 760, bubbleSwipeActive: false))
        // Slow but long is also back.
        XCTAssertTrue(BackSwipe.isBackSwipe(translation: CGSize(width: 130, height: 20),
                                            velocity: CGSize(width: 10, height: 30),
                                            start: far, composerTop: 760, bubbleSwipeActive: false))
        // No composer on the screen (server channel list).
        XCTAssertTrue(BackSwipe.isBackSwipe(translation: CGSize(width: 200, height: 0),
                                            velocity: CGSize(width: 1200, height: 0),
                                            start: far, composerTop: nil, bubbleSwipeActive: false))
    }

    func testScrollsLeftPansAndShortDragsAreNotBack() {
        XCTAssertFalse(BackSwipe.isBackSwipe(translation: CGSize(width: 20, height: 300),
                                             velocity: CGSize(width: 10, height: 900),
                                             start: far, composerTop: 760, bubbleSwipeActive: false), "vertical scroll")
        XCTAssertFalse(BackSwipe.isBackSwipe(translation: CGSize(width: -120, height: 0),
                                             velocity: CGSize(width: -900, height: 0),
                                             start: far, composerTop: 760, bubbleSwipeActive: false), "time reveal (drag left)")
        XCTAssertFalse(BackSwipe.isBackSwipe(translation: CGSize(width: 60, height: 5),
                                             velocity: CGSize(width: 300, height: 0),
                                             start: far, composerTop: 760, bubbleSwipeActive: false), "too short")
        XCTAssertFalse(BackSwipe.isBackSwipe(translation: CGSize(width: 90, height: 85),
                                             velocity: CGSize(width: 300, height: 290),
                                             start: far, composerTop: 760, bubbleSwipeActive: false), "diagonal")
    }

    func testADragThatStartsInTheComposerBarIsNeverBack() {
        // Selecting text in the field or sliding the mic: a right pan that begins at or
        // below the bar's top edge, however fast or long.
        let fast = CGSize(width: 1500, height: 0), long = CGSize(width: 200, height: 0)
        XCTAssertFalse(BackSwipe.isBackSwipe(translation: long, velocity: fast, start: CGPoint(x: 200, y: 760), composerTop: 760, bubbleSwipeActive: false))
        XCTAssertFalse(BackSwipe.isBackSwipe(translation: long, velocity: fast, start: CGPoint(x: 200, y: 800), composerTop: 760, bubbleSwipeActive: false))
        // Just above the bar is the transcript: back.
        XCTAssertTrue(BackSwipe.isBackSwipe(translation: long, velocity: fast, start: CGPoint(x: 200, y: 759), composerTop: 760, bubbleSwipeActive: false))
    }

    /// The UIKit edge recognizer (iphone-scroll-perf) decides at 6 pt, before the scroll view's
    /// own ~10 pt pan: a start that is at least as rightward as it is vertical claims the touch
    /// (the shape `edgePops` asks for at the end, so the pop never loses to the transcript);
    /// a leftward or clearly vertical start fails at once (a scroll from the edge zone stays the
    /// transcript's, as under #1985); anything between waits for the next point.
    func testAnEdgeDragClaimsEarlyAndFailsFastOnVerticalMotion() {
        XCTAssertEqual(BackSwipe.edgeLockDistance, 6)
        XCTAssertEqual(BackSwipe.edgeLock(translation: CGSize(width: 6, height: 0)), .claimed)
        XCTAssertEqual(BackSwipe.edgeLock(translation: CGSize(width: 5, height: 5)), .claimed, "as much right as down: the pop's")
        XCTAssertEqual(BackSwipe.edgeLock(translation: CGSize(width: 4, height: 6)), .undecided, "more down than right, not yet clearly a scroll")
        XCTAssertEqual(BackSwipe.edgeLock(translation: CGSize(width: 2, height: 8)), .declined, "twice as much down as right: a scroll")
        XCTAssertEqual(BackSwipe.edgeLock(translation: CGSize(width: 0, height: -9)), .declined, "straight up")
        XCTAssertEqual(BackSwipe.edgeLock(translation: CGSize(width: -6, height: 0)), .declined, "left")
        XCTAssertEqual(BackSwipe.edgeLock(translation: CGSize(width: 3, height: 5)), .undecided, "not yet clear")
    }

    /// The one transcript recognizer finds the row under the finger by its reported frame and
    /// only replies on rows that can be replied to.
    func testTheRowUnderTheFingerIsFoundByFrame() {
        let index = RowFrameIndex()
        index.set("a", frame: CGRect(x: 0, y: 0, width: 300, height: 40), canReply: true)
        index.set("sys", frame: CGRect(x: 0, y: 40, width: 300, height: 20), canReply: false)
        index.set("b", frame: CGRect(x: 0, y: 60, width: 300, height: 40), canReply: true)
        XCTAssertEqual(index.replyableRow(at: CGPoint(x: 10, y: 10)), "a")
        XCTAssertNil(index.replyableRow(at: CGPoint(x: 10, y: 50)), "a system line never replies")
        XCTAssertEqual(index.replyableRow(at: CGPoint(x: 299, y: 99)), "b")
        XCTAssertNil(index.replyableRow(at: CGPoint(x: 10, y: 200)), "between / below the rows")
        index.remove("b")
        XCTAssertNil(index.replyableRow(at: CGPoint(x: 10, y: 70)), "a row that left the window")
    }

    func testTheEdgeZoneBelongsToTheEdgePopAlone() {
        // A bubble never starts a reply from the edge zone; one point outside it does.
        XCTAssertEqual(BubbleSwipe.lock(translation: CGSize(width: 40, height: 2), startX: 10), .declined)
        XCTAssertEqual(BubbleSwipe.lock(translation: CGSize(width: 40, height: 2), startX: BackSwipe.edgeZoneWidth - 1), .declined)
        XCTAssertEqual(BubbleSwipe.lock(translation: CGSize(width: 40, height: 2), startX: BackSwipe.edgeZoneWidth), .reply)
    }

    func testABubbleSwipeOwnsItsDragSoTheScreenDoesNotGoBack() {
        XCTAssertFalse(BackSwipe.isBackSwipe(translation: CGSize(width: 200, height: 0),
                                             velocity: CGSize(width: 1500, height: 0),
                                             start: far, composerTop: 760, bubbleSwipeActive: true))
    }

    // MARK: Bubble swipe → reply

    func testABubbleSwipeDecidesOnceAtTwelvePoints() {
        XCTAssertEqual(BubbleSwipe.lock(translation: CGSize(width: 6, height: 4), startX: 100), .undecided, "under 12 pt nothing is decided")
        XCTAssertEqual(BubbleSwipe.lock(translation: CGSize(width: 12, height: 0), startX: 100), .reply)
        XCTAssertEqual(BubbleSwipe.lock(translation: CGSize(width: 10, height: -8), startX: 100), .reply, "mostly right, a little up")
    }

    func testVerticalScrollAndLeftPansNeverBecomeAReply() {
        XCTAssertEqual(BubbleSwipe.lock(translation: CGSize(width: 2, height: 30), startX: 100), .declined, "scroll down")
        XCTAssertEqual(BubbleSwipe.lock(translation: CGSize(width: 3, height: -30), startX: 100), .declined, "scroll up")
        XCTAssertEqual(BubbleSwipe.lock(translation: CGSize(width: 9, height: 9), startX: 100), .declined, "diagonal, not clearly horizontal")
        XCTAssertEqual(BubbleSwipe.lock(translation: CGSize(width: -30, height: 0), startX: 100), .declined, "left = time reveal")
    }

    func testTheBubbleFollowsTheFingerThenRubberBands() {
        XCTAssertEqual(BubbleSwipe.reveal(for: 0), 0)
        XCTAssertEqual(BubbleSwipe.reveal(for: -40), 0, "never left")
        XCTAssertEqual(BubbleSwipe.reveal(for: 30), 30, "1:1 inside the reveal width")
        XCTAssertEqual(BubbleSwipe.reveal(for: BubbleSwipe.revealWidth), BubbleSwipe.revealWidth)
        XCTAssertEqual(BubbleSwipe.reveal(for: BubbleSwipe.revealWidth + 50), BubbleSwipe.revealWidth + 10, accuracy: 0.001, "a fifth of the excess")
        XCTAssertEqual(BubbleSwipe.reveal(for: 1000), BubbleSwipe.revealWidth + 24, "capped")
    }

    func testTheReplyArmsAtTheThreshold() {
        let threshold = BubbleSwipe.revealWidth * BubbleSwipe.triggerFraction
        XCTAssertFalse(BubbleSwipe.triggers(reveal: threshold - 1))
        XCTAssertTrue(BubbleSwipe.triggers(reveal: threshold))
        XCTAssertTrue(BubbleSwipe.triggers(reveal: BubbleSwipe.revealWidth + 24))
    }
}
