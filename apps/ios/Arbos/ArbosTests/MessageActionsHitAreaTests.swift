import XCTest
@testable import Arbos

/// The long-press menu's rows and the reaction capsule's cells are tappable over
/// their whole rect, and tile their container with no dead zone (feedback
/// iphone-bubble-menu-hit-area-2026-10-26: taps beside the icon / label fell through).
final class MessageActionsHitAreaTests: XCTestCase {
    private let width: CGFloat = 250

    private func assertRowsTile(_ items: [MessageMenuItem], file: StaticString = #filePath, line: UInt = #line) {
        let menu = MessageActionsMenuRows.compute(items: items, width: width)
        XCTAssertEqual(menu.rows.map(\.item), items, file: file, line: line)
        XCTAssertEqual(menu.rows.first?.frame.minY, 0, file: file, line: line)
        XCTAssertEqual(menu.rows.last?.frame.maxY, menu.height, file: file, line: line)
        for (i, row) in menu.rows.enumerated() {
            XCTAssertEqual(row.hitRect, row.frame, "\(row.item): the hit rect is the whole row", file: file, line: line)
            XCTAssertEqual(row.frame.minX, 0, file: file, line: line)
            XCTAssertEqual(row.frame.width, width, "\(row.item): full width", file: file, line: line)
            XCTAssertGreaterThanOrEqual(row.frame.height, MessageActionsMenuRows.rowHeight, file: file, line: line)
            if i + 1 < menu.rows.count {
                XCTAssertEqual(row.frame.maxY, menu.rows[i + 1].frame.minY,
                               "\(row.item) → \(menu.rows[i + 1].item): no gap, no overlap", file: file, line: line)
            }
            // The divider and the destructive band live inside the row they belong to.
            if let d = row.dividerRect { XCTAssertTrue(row.frame.contains(d), "\(row.item): divider inside the row", file: file, line: line) }
            XCTAssertTrue(row.frame.contains(row.labelRect), file: file, line: line)
            XCTAssertEqual(row.labelRect.height, MessageActionsMenuRows.rowHeight, file: file, line: line)
        }
        // Every point of the menu is inside exactly one row.
        var y: CGFloat = 0.25
        while y < menu.height {
            let hits = menu.rows.filter { $0.hitRect.minY <= y && y < $0.hitRect.maxY }
            XCTAssertEqual(hits.count, 1, "y=\(y) is inside \(hits.count) rows", file: file, line: line)
            y += 0.5
        }
    }

    func testStandardMenuRowsTileWithNoDeadZone() {
        assertRowsTile(MessageMenu.items)
    }

    func testEveryMenuShapeTiles() {
        assertRowsTile(MessageMenu.itemsWithThread)
        assertRowsTile(MessageMenu.items(forMessage: true, canEdit: true, canDelete: true, hasReplies: true))
        assertRowsTile(MessageMenu.items(forMessage: true, canEdit: true, canDelete: false))
        assertRowsTile(MessageMenu.copyOnly)
    }

    func testDestructiveBandIsPartOfTheDeleteRow() {
        let menu = MessageActionsMenuRows.compute(items: MessageMenu.items, width: width)
        let delete = menu.rows.last!
        XCTAssertEqual(delete.item, .delete)
        XCTAssertEqual(delete.bandAbove, MessageActionsMenuRows.destructiveBand)
        XCTAssertEqual(delete.frame.height, MessageActionsMenuRows.rowHeight + MessageActionsMenuRows.destructiveBand)
        XCTAssertEqual(delete.labelRect.minY, delete.frame.minY + MessageActionsMenuRows.destructiveBand)
        // No hairline between Reply in DM and the band.
        XCTAssertFalse(menu.rows[menu.rows.count - 2].dividerBelow)
        XCTAssertTrue(menu.rows[0].dividerBelow)
        // Copy-only: no band, no divider.
        let solo = MessageActionsMenuRows.compute(items: MessageMenu.copyOnly, width: width)
        XCTAssertEqual(solo.rows[0].bandAbove, 0)
        XCTAssertFalse(solo.rows[0].dividerBelow)
        XCTAssertEqual(solo.height, MessageActionsMenuRows.rowHeight)
    }

    /// The overlay's layout uses the same height the rows draw (five rows + the band = 228 pt,
    /// what `MessageActionsLayoutTests` fits against the keyboard).
    func testMenuHeightMatchesTheRows() {
        let menu = MessageActionsMenuRows.compute(items: MessageMenu.items, width: width)
        XCTAssertEqual(menu.height, 5 * 44 + 8)
        let longest = MessageActionsMenuRows.compute(items: MessageMenu.items(forMessage: true, canEdit: true, canDelete: true, hasReplies: true), width: width)
        XCTAssertEqual(longest.height, 7 * 44 + 8)
    }

    func testCapsuleCellsTileTheCapsule() {
        let cells = ReactionCapsule.cellFrames
        XCTAssertEqual(cells.count, ReactionCapsule.palette.count + 1)
        XCTAssertEqual(cells.first?.minX, ReactionCapsule.padding)
        XCTAssertEqual(cells.last?.maxX, ReactionCapsule.width - ReactionCapsule.padding)
        for (i, c) in cells.enumerated() {
            XCTAssertEqual(c.size, CGSize(width: ReactionCapsule.cell, height: ReactionCapsule.cell))
            XCTAssertEqual(c.minY, ReactionCapsule.padding)
            XCTAssertEqual(c.maxY, ReactionCapsule.height - ReactionCapsule.padding)
            if i + 1 < cells.count { XCTAssertEqual(c.maxX, cells[i + 1].minX, "cells \(i) → \(i + 1) touch") }
        }
        // A cell is wider than the emoji glyph it holds: the margin is tappable too.
        XCTAssertGreaterThan(ReactionCapsule.cell, ReactionCapsule.emojiSize)
    }
}
