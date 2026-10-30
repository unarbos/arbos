import XCTest
@testable import Arbos

/// The ↓ button's landing window (jump-to-latest-instant-ios-2026-10-25): from deep
/// history it re-cuts the view tree to one opening page instead of rebuilding the
/// whole 360-row window; within the tail window it leaves the rows alone so the
/// short animated hop stays.
final class JumpToLatestWindowTests: XCTestCase {
    func testDeepInHistoryLandsOnOneOpeningPage() {
        let landing = ChatSession.windowAfterJump(count: ChatSession.windowMax, skipEnd: 600)
        XCTAssertEqual(landing.count, ChatSession.windowInitial)
        XCTAssertEqual(landing.skipEnd, 0)
    }

    func testAnySlideIntoHistoryCountsAsDeep() {
        let landing = ChatSession.windowAfterJump(count: 350, skipEnd: 30)
        XCTAssertEqual(landing.count, ChatSession.windowInitial)
        XCTAssertEqual(landing.skipEnd, 0)
    }

    func testWithinTheTailWindowNothingIsRebuilt() {
        let landing = ChatSession.windowAfterJump(count: 200, skipEnd: 0)
        XCTAssertEqual(landing.count, 200)
        XCTAssertEqual(landing.skipEnd, 0)
    }

    func testAnOversizedTailWindowIsCappedNotRecut() {
        let landing = ChatSession.windowAfterJump(count: ChatSession.windowMax + 40, skipEnd: 0)
        XCTAssertEqual(landing.count, ChatSession.windowMax)
        XCTAssertEqual(landing.skipEnd, 0)
    }
}
