import XCTest
@testable import Arbos

/// The ↓ button's landing (jump-to-latest-instant-ios-2026-10-25): from deep history it
/// lands at once instead of animating through every row between; near the tail the short
/// animated hop stays.
final class JumpToLatestWindowTests: XCTestCase {
    func testDeepInHistoryLandsAtOnce() {
        XCTAssertTrue(ChatSession.jumpLandsAtOnce(rowsBelow: ChatSession.jumpHopRows, skipEnd: 600))
    }

    func testAnySlideIntoHistoryCountsAsDeep() {
        XCTAssertTrue(ChatSession.jumpLandsAtOnce(rowsBelow: 20, skipEnd: 30))
    }

    func testNearTheTailTheHopIsAnimated() {
        XCTAssertFalse(ChatSession.jumpLandsAtOnce(rowsBelow: 200, skipEnd: 0))
    }

    func testMoreRowsBelowThanAHopCoversLandsAtOnce() {
        XCTAssertTrue(ChatSession.jumpLandsAtOnce(rowsBelow: ChatSession.jumpHopRows + 40, skipEnd: 0))
    }
}
