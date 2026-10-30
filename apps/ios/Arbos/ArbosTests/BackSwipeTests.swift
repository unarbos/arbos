import XCTest
@testable import Arbos

/// Horizontal gesture rules on the chat screens (Jacob, 2026-10-28: the system's
/// interactive edge pop is the one owner of the edge; a swipe right anywhere else is an
/// animated "back"; there is no swipe-to-reply). The edge rule itself is UIKit's — these
/// pin what our own recognizer leaves alone and what it claims.
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
}
