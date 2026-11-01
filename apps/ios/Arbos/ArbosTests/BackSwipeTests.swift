import XCTest
@testable import Arbos

/// Horizontal gesture rules on the chat screens. The system's interactive edge pop owns the
/// edge; away from it the direction decides (Jacob, 2026-10-30: "swiping to the right to go
/// back and swiping to the left to reply so those should not be interfering with each other").
/// The edge rule itself is UIKit's — these pin what our own recognizer leaves alone, what it
/// claims, and above all that neither direction can ever do the other's job.
final class BackSwipeTests: XCTestCase {
    private let far = CGPoint(x: 200, y: 400)

    func testTheEdgeZoneBelongsToTheSystemPop() {
        XCTAssertTrue(BackSwipe.startsAtEdge(CGPoint(x: 0, y: 400)))
        XCTAssertTrue(BackSwipe.startsAtEdge(CGPoint(x: BackSwipe.edgeZoneWidth - 1, y: 400)))
        XCTAssertFalse(BackSwipe.startsAtEdge(CGPoint(x: BackSwipe.edgeZoneWidth, y: 400)))
        // Whatever the drag does, a start in the edge zone is never the full-screen back's.
        XCTAssertEqual(BackSwipe.fullScreenLock(translation: CGSize(width: 200, height: 0), start: CGPoint(x: 10, y: 400), composerTop: nil), .declined)
        XCTAssertEqual(BackSwipe.fullScreenLock(translation: CGSize(width: 12, height: 0), start: CGPoint(x: 23, y: 400), composerTop: nil), .declined)
    }

    func testTheFullScreenBackDecidesOnceAtTwelvePoints() {
        XCTAssertEqual(BackSwipe.lockDistance, 12)
        XCTAssertEqual(BackSwipe.fullScreenLock(translation: CGSize(width: 6, height: 4), start: far, composerTop: nil), .undecided, "under 12 pt: wait")
        XCTAssertEqual(BackSwipe.fullScreenLock(translation: CGSize(width: 12, height: 0), start: far, composerTop: nil), .claimed)
        XCTAssertEqual(BackSwipe.fullScreenLock(translation: CGSize(width: 10, height: -8), start: far, composerTop: nil), .claimed, "mostly right")
        XCTAssertEqual(BackSwipe.fullScreenLock(translation: CGSize(width: 2, height: 30), start: far, composerTop: nil), .declined, "scroll down")
        XCTAssertEqual(BackSwipe.fullScreenLock(translation: CGSize(width: 3, height: -30), start: far, composerTop: nil), .declined, "scroll up")
        XCTAssertEqual(BackSwipe.fullScreenLock(translation: CGSize(width: 9, height: 9), start: far, composerTop: nil), .declined, "diagonal, not clearly right")
        XCTAssertEqual(BackSwipe.fullScreenLock(translation: CGSize(width: -30, height: 0), start: far, composerTop: nil), .declined, "left = time peek")
    }

    func testTheComposerBarIsNeverBack() {
        let top: CGFloat = 760
        XCTAssertEqual(BackSwipe.fullScreenLock(translation: CGSize(width: 40, height: 0), start: CGPoint(x: 200, y: 760), composerTop: top), .declined)
        XCTAssertEqual(BackSwipe.fullScreenLock(translation: CGSize(width: 40, height: 0), start: CGPoint(x: 200, y: 800), composerTop: top), .declined)
        XCTAssertEqual(BackSwipe.fullScreenLock(translation: CGSize(width: 40, height: 0), start: CGPoint(x: 200, y: 759), composerTop: top), .claimed, "just above the bar")
        XCTAssertEqual(BackSwipe.fullScreenLock(translation: CGSize(width: 40, height: 0), start: CGPoint(x: 200, y: 800), composerTop: nil), .claimed, "no bar on this screen")
    }

    func testTheFullScreenBackPopsOnAClearRightPan() {
        XCTAssertTrue(BackSwipe.fullScreenPops(translation: CGSize(width: 90, height: 10), velocity: CGSize(width: 900, height: 50)))
        XCTAssertTrue(BackSwipe.fullScreenPops(translation: CGSize(width: 130, height: 20), velocity: CGSize(width: 10, height: 400)), "long enough even when the hand drifts at the end")
        XCTAssertTrue(BackSwipe.fullScreenPops(translation: CGSize(width: 200, height: 0), velocity: .zero))
        XCTAssertFalse(BackSwipe.fullScreenPops(translation: CGSize(width: 60, height: 5), velocity: CGSize(width: 2000, height: 0)), "too short")
        XCTAssertFalse(BackSwipe.fullScreenPops(translation: CGSize(width: 90, height: 85), velocity: CGSize(width: 900, height: 0)), "not horizontal enough")
        XCTAssertFalse(BackSwipe.fullScreenPops(translation: CGSize(width: -120, height: 0), velocity: .zero), "left")
        XCTAssertFalse(BackSwipe.fullScreenPops(translation: CGSize(width: 90, height: 10), velocity: CGSize(width: 100, height: 400)), "short and faster vertically: a scroll that wandered")
    }

    // MARK: The direction split (Jacob, 2026-10-30)

    /// The guarantee, stated as two assertions over the same translations: a rightward pan can
    /// never reply, and a leftward pan can never go back. Everything else about the two
    /// gestures is detail; this is the part that must not regress.
    func testTheTwoDirectionsCanNeverDoEachOthersJob() {
        for dx in stride(from: CGFloat(12), through: 400, by: 17) {
            let right = CGSize(width: dx, height: 0)
            let left = CGSize(width: -dx, height: 0)
            XCTAssertEqual(BubbleSwipe.lock(translation: right), .declined, "right must never reply (dx=\(dx))")
            XCTAssertFalse(BackSwipe.fullScreenPops(translation: left, velocity: CGSize(width: -2000, height: 0)),
                           "left must never go back (dx=-\(dx))")
            // And the one each direction IS for.
            XCTAssertEqual(BubbleSwipe.lock(translation: left), .claimed, "left replies (dx=-\(dx))")
        }
        XCTAssertTrue(BackSwipe.fullScreenPops(translation: CGSize(width: 200, height: 0), velocity: .zero), "right goes back")
    }

    /// The reply swipe decides on the same terms as the back swipe, mirrored: 12 pt, and a pan
    /// more than 1.15× as wide as tall. A vertical pan is the scroll view's in both directions.
    func testTheReplySwipeDecidesOnceAtTwelvePoints() {
        XCTAssertEqual(BubbleSwipe.lockDistance, BackSwipe.lockDistance, "one lock distance for both directions")
        XCTAssertEqual(BubbleSwipe.lock(translation: CGSize(width: -6, height: 4)), .undecided, "under 12 pt: wait")
        XCTAssertEqual(BubbleSwipe.lock(translation: CGSize(width: -12, height: 0)), .claimed)
        XCTAssertEqual(BubbleSwipe.lock(translation: CGSize(width: -10, height: 8)), .claimed, "mostly left")
        XCTAssertEqual(BubbleSwipe.lock(translation: CGSize(width: -2, height: 30)), .declined, "scroll down")
        XCTAssertEqual(BubbleSwipe.lock(translation: CGSize(width: -3, height: -30)), .declined, "scroll up")
        XCTAssertEqual(BubbleSwipe.lock(translation: CGSize(width: -9, height: 9)), .declined, "diagonal, not clearly left")
    }

    /// The bubble follows the finger 1:1 to the reveal width, then rubber-bands, and arms at
    /// 70%. `reveal` takes the raw (negative) translation and returns a positive distance.
    func testTheReplyRevealFollowsTheFingerThenRubberBands() {
        XCTAssertEqual(BubbleSwipe.reveal(for: 0), 0)
        XCTAssertEqual(BubbleSwipe.reveal(for: -30), 30, "1:1")
        XCTAssertEqual(BubbleSwipe.reveal(for: -BubbleSwipe.revealWidth), BubbleSwipe.revealWidth)
        XCTAssertEqual(BubbleSwipe.reveal(for: -(BubbleSwipe.revealWidth + 50)), BubbleSwipe.revealWidth + 10, "a fifth of the excess")
        XCTAssertEqual(BubbleSwipe.reveal(for: -1000), BubbleSwipe.revealWidth + 24, "capped")
        XCTAssertEqual(BubbleSwipe.reveal(for: 200), 0, "a rightward pan reveals no reply arrow")

        XCTAssertFalse(BubbleSwipe.triggers(reveal: BubbleSwipe.revealWidth * 0.69))
        XCTAssertTrue(BubbleSwipe.triggers(reveal: BubbleSwipe.revealWidth * 0.7))
        XCTAssertTrue(BubbleSwipe.triggers(reveal: BubbleSwipe.revealWidth))
    }

    // MARK: Row lookup under the finger

    /// The album bug (DaWizzaa, message 01M670FVEYHXTG2RQBN9PF6G0N): a tall picture or album
    /// row overlaps its neighbours, and the lookup used to take whichever row a dictionary
    /// yielded first and give up if that one could not be replied to. The repliable row under
    /// the finger must be found whatever order the rows were written in.
    func testATallRowIsFoundUnderANonRepliableNeighbour() {
        // A date separator whose frame spans the album's top, written first and last in turn.
        for separatorFirst in [true, false] {
            let index = RowFrameIndex()
            let album = CGRect(x: 0, y: 100, width: 360, height: 300)
            let separator = CGRect(x: 0, y: 80, width: 360, height: 40)
            if separatorFirst {
                index.set("sep", frame: separator, canReply: false)
                index.set("album", frame: album, canReply: true)
            } else {
                index.set("album", frame: album, canReply: true)
                index.set("sep", frame: separator, canReply: false)
            }
            // A point inside both frames: the album is the answer either way.
            XCTAssertEqual(index.replyableRow(at: CGPoint(x: 180, y: 110)), "album",
                           "separatorFirst=\(separatorFirst)")
            // And a point only the album holds.
            XCTAssertEqual(index.replyableRow(at: CGPoint(x: 180, y: 300)), "album")
        }
    }

    /// Overlapping repliable rows: the innermost (smallest) one is the one the finger is on.
    func testTheInnermostRepliableRowWins() {
        let index = RowFrameIndex()
        index.set("outer", frame: CGRect(x: 0, y: 0, width: 360, height: 400), canReply: true)
        index.set("inner", frame: CGRect(x: 20, y: 150, width: 200, height: 60), canReply: true)
        XCTAssertEqual(index.replyableRow(at: CGPoint(x: 100, y: 180)), "inner")
        XCTAssertEqual(index.replyableRow(at: CGPoint(x: 300, y: 180)), "outer", "outside the inner frame")
    }

    /// Nothing repliable under the finger: the drag is the time peek's, not a reply's.
    func testNoRepliableRowMeansNoReply() {
        let index = RowFrameIndex()
        index.set("sep", frame: CGRect(x: 0, y: 0, width: 360, height: 40), canReply: false)
        XCTAssertNil(index.replyableRow(at: CGPoint(x: 100, y: 20)), "a system row is not repliable")
        XCTAssertNil(index.replyableRow(at: CGPoint(x: 100, y: 900)), "empty space")

        // A row that stops being repliable (a message deleted under the finger) is dropped.
        index.set("m", frame: CGRect(x: 0, y: 50, width: 360, height: 40), canReply: true)
        XCTAssertEqual(index.replyableRow(at: CGPoint(x: 100, y: 60)), "m")
        index.set("m", frame: CGRect(x: 0, y: 50, width: 360, height: 40), canReply: false)
        XCTAssertNil(index.replyableRow(at: CGPoint(x: 100, y: 60)))
        index.set("m", frame: CGRect(x: 0, y: 50, width: 360, height: 40), canReply: true)
        index.remove("m")
        XCTAssertNil(index.replyableRow(at: CGPoint(x: 100, y: 60)), "a row that left the tree")
    }
}
