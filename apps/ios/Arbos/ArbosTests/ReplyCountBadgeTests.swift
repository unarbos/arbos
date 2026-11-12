import SwiftUI
import XCTest
@testable import Arbos

/// Reply-count digit in a bubble's top-trailing corner
/// ([feedback ADc6sZtZP2moU0knIJ_ifHs], Jacob 2026-11-10: "2 in the corner is too close
/// to the corner placement is wrong"). The digit must sit clear of the continuous
/// `Theme.bubbleRadius` curve — the old 6/8 padding sat inside it.
final class ReplyCountBadgeTests: XCTestCase {
    func testReplyCountClearsTheBubbleCorner() {
        XCTAssertEqual(RowView.replyCountTop, 10)
        XCTAssertEqual(RowView.replyCountTrailing, 12)
        // The broken placement was 6 top / 8 trailing; both must stay larger.
        XCTAssertGreaterThan(RowView.replyCountTop, 6)
        XCTAssertGreaterThan(RowView.replyCountTrailing, 8)
        // Clear at least half the continuous corner so the digit is not in the curve.
        XCTAssertGreaterThanOrEqual(RowView.replyCountTop, Theme.bubbleRadius / 2)
        XCTAssertGreaterThanOrEqual(RowView.replyCountTrailing, Theme.bubbleRadius / 2)
    }
}
