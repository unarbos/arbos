import SwiftUI
import XCTest
@testable import Arbos

/// The transcript never comes back blank while the chat has rows
/// ([feedback iphone-chat-empties-after-screenshot-2026-10-28]). Two rules: the window slice
/// the view tree holds is never empty, and a return (the feedback sheet a screenshot opened is
/// gone, the scene is active again) that finds nothing drawn re-anchors the reader where
/// they were — the tail when following it, else the realised row nearest the viewport, kept
/// in place.
final class TranscriptReanchorTests: XCTestCase {
    private let rows = Array(0..<500)

    // MARK: Window slice

    func testTailWindowIsTheNewestRows() {
        let w = TranscriptReanchor.window(rows, count: 80, skipEnd: 0, fallback: 80)
        XCTAssertEqual(w.count, 80)
        XCTAssertEqual(w.last, 499)
    }

    func testSlidWindowDropsTheNewestRows() {
        let w = TranscriptReanchor.window(rows, count: 360, skipEnd: 100, fallback: 80)
        XCTAssertEqual(w.count, 360)
        XCTAssertEqual(w.last, 399)
        XCTAssertEqual(w.first, 40)
    }

    func testASlidePastTheEndReseedsToTheTail() {
        // A window that slid 600 into a chat that now has 500 rows (rows left, chat replaced).
        let w = TranscriptReanchor.window(rows, count: 360, skipEnd: 600, fallback: 80)
        XCTAssertFalse(w.isEmpty)
        XCTAssertEqual(w.last, 499)
    }

    func testASlideExactlyToTheEndKeepsRows() {
        let w = TranscriptReanchor.window(rows, count: 80, skipEnd: 500, fallback: 80)
        XCTAssertFalse(w.isEmpty)
        XCTAssertEqual(w.last, 499)
    }

    func testAZeroOrNegativeCountFallsBackToOnePage() {
        XCTAssertEqual(TranscriptReanchor.window(rows, count: 0, skipEnd: 0, fallback: 80).count, 80)
        XCTAssertEqual(TranscriptReanchor.window(rows, count: -5, skipEnd: 20, fallback: 80).last, 479)
    }

    func testNoRowsIsTheOnlyEmptyWindow() {
        XCTAssertTrue(TranscriptReanchor.window([Int](), count: 80, skipEnd: 0, fallback: 80).isEmpty)
        XCTAssertEqual(TranscriptReanchor.window([7], count: 80, skipEnd: 3, fallback: 80), [7])
    }

    func testAShortChatShowsEverything() {
        let w = TranscriptReanchor.window(Array(0..<12), count: 80, skipEnd: 0, fallback: 80)
        XCTAssertEqual(w, Array(0..<12))
    }

    // MARK: Verdict after a return

    private let viewport = CGRect(x: 0, y: 1000, width: 390, height: 800)
    private func realized(_ pairs: [(String, CGFloat)], height: CGFloat = 60) -> [TranscriptReanchor.Realized] {
        pairs.map { TranscriptReanchor.Realized(id: $0.0, frame: CGRect(x: 0, y: $0.1, width: 390, height: height)) }
    }

    func testARowInTheViewportIsDrawn() {
        let v = TranscriptReanchor.verdict(rowsEmpty: false, pinned: true,
                                           realized: realized([("a", 900), ("b", 1300), ("c", 1700)]),
                                           visibleRect: viewport, headerInset: 64, footerInset: 100)
        XCTAssertEqual(v, .drawn)
    }

    func testARowOnlyUnderTheHeaderOrComposerIsNotDrawn() {
        // "a" ends inside the header band; "c" starts under the composer.
        let v = TranscriptReanchor.verdict(rowsEmpty: false, pinned: true,
                                           realized: realized([("a", 990), ("c", 1760)], height: 50),
                                           visibleRect: viewport, headerInset: 64, footerInset: 100)
        XCTAssertEqual(v, .toTail)
    }

    func testNothingRealisedAtTheTailLandsOnTheTail() {
        let v = TranscriptReanchor.verdict(rowsEmpty: false, pinned: true, realized: [],
                                           visibleRect: viewport, headerInset: 64, footerInset: 100)
        XCTAssertEqual(v, .toTail)
    }

    func testNothingRealisedInHistoryStillLandsOnTheTail() {
        // Better a known place than a blank screen: the history position is gone with the rows.
        let v = TranscriptReanchor.verdict(rowsEmpty: false, pinned: false, realized: [],
                                           visibleRect: viewport, headerInset: 64, footerInset: 100)
        XCTAssertEqual(v, .toTail)
    }

    func testAnEmptyChatHasNothingToRecover() {
        let v = TranscriptReanchor.verdict(rowsEmpty: true, pinned: true, realized: [],
                                           visibleRect: viewport, headerInset: 64, footerInset: 100)
        XCTAssertEqual(v, .drawn)
    }

    func testHistoryWithRealisedRowsAboveTheViewportComesBackAtTheTop() {
        let v = TranscriptReanchor.verdict(rowsEmpty: false, pinned: false,
                                           realized: realized([("far", 100), ("near", 700)]),
                                           visibleRect: viewport, headerInset: 64, footerInset: 100)
        XCTAssertEqual(v, .toRow(id: "near", anchor: .top))
    }

    func testHistoryWithRealisedRowsBelowTheViewportComesBackAtTheBottom() {
        let v = TranscriptReanchor.verdict(rowsEmpty: false, pinned: false,
                                           realized: realized([("near", 1900), ("far", 2600)]),
                                           visibleRect: viewport, headerInset: 64, footerInset: 100)
        XCTAssertEqual(v, .toRow(id: "near", anchor: .bottom))
    }

    func testARowOverlappingOnlyTheHeaderIsKeptExactlyWhereItIs() {
        // Overlaps the viewport (under the header) but not its readable part: it is scrolled
        // to with the anchor that leaves it where it is, so nothing visibly jumps.
        let row = CGRect(x: 0, y: 980, width: 390, height: 60)
        let v = TranscriptReanchor.verdict(rowsEmpty: false, pinned: false,
                                           realized: [TranscriptReanchor.Realized(id: "edge", frame: row)],
                                           visibleRect: viewport, headerInset: 64, footerInset: 100)
        guard case .toRow(let id, let anchor) = v else { return XCTFail("expected toRow, got \(v)") }
        XCTAssertEqual(id, "edge")
        XCTAssertEqual(anchor, TranscriptReanchor.keepingAnchor(row: row, viewport: viewport))
    }

    func testKeepingAnchorLeavesTheRowWhereItIs() {
        // Row top 200 pt below the viewport top, 60 tall, viewport 800: a = 200 / 740.
        let row = CGRect(x: 0, y: 1200, width: 390, height: 60)
        let a = TranscriptReanchor.keepingAnchor(row: row, viewport: viewport)
        XCTAssertEqual(a.y, 200 / 740, accuracy: 0.0001)
        // After scrollTo(anchor: a): row.minY' - viewport.minY' = a * (H - h) = 200.
        XCTAssertEqual(a.y * (viewport.height - row.height), 200, accuracy: 0.001)
        // Rows taller than the viewport pin to the top.
        XCTAssertEqual(TranscriptReanchor.keepingAnchor(row: CGRect(x: 0, y: 900, width: 390, height: 900), viewport: viewport), .top)
    }

    func testWithoutGeometryRealisedRowsCount() {
        XCTAssertEqual(TranscriptReanchor.verdict(rowsEmpty: false, pinned: false, realized: realized([("a", 0)]),
                                                  visibleRect: nil, headerInset: 64, footerInset: 100), .drawn)
        XCTAssertEqual(TranscriptReanchor.verdict(rowsEmpty: false, pinned: false, realized: [],
                                                  visibleRect: nil, headerInset: 64, footerInset: 100), .toTail)
    }
}
