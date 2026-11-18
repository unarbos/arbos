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

    // MARK: A hold that slides to its choice

    /// A point over a capsule cell or a menu row is that cell or row; the band above Delete
    /// and the space between the two groups are nothing.
    func testAHeldFingerIsOverTheCellOrRowUnderIt() {
        let rows = MessageActionsMenuRows.compute(items: [.copy, .reply, .delete], width: 250)
        let capsule = CGPoint(x: 16, y: 100), menu = CGPoint(x: 16, y: 300)
        func at(_ x: CGFloat, _ y: CGFloat) -> MessageActionsTarget? {
            MessageActionsTarget.at(CGPoint(x: x, y: y), capsule: capsule, menu: menu, rows: rows)
        }
        let cell = ReactionCapsule.cell, pad = ReactionCapsule.padding
        XCTAssertEqual(at(16 + pad + 1, 100 + pad + 1), .cell(0))
        XCTAssertEqual(at(16 + pad + cell * CGFloat(ReactionCapsule.palette.count) + 1, 100 + pad + 1),
                       .cell(ReactionCapsule.palette.count), "the add-emoji control is the last cell")
        XCTAssertEqual(at(20, 301), .row(.copy))
        XCTAssertEqual(at(265, 300 + MessageActionsMenuRows.rowHeight + 1), .row(.reply))
        let deleteTop = 300 + 2 * MessageActionsMenuRows.rowHeight
        XCTAssertNil(at(20, deleteTop + 1), "the band above Delete is not Delete")
        XCTAssertEqual(at(20, deleteTop + MessageActionsMenuRows.destructiveBand + 1), .row(.delete))
        XCTAssertNil(at(20, 200), "between the capsule and the menu")
        XCTAssertNil(at(300, 301), "right of the menu")
    }

    // MARK: A tap that closes the menu

    /// A tap closes the menu everywhere but on the capsule and the menu: on the blur, on the
    /// lifted message, and on the part of a long message that runs on beside or under the
    /// menu, where a link used to take the tap (tester reports, 2026-11-13 and 2026-11-14).
    func testATapOutsideTheCapsuleAndTheMenuDismisses() {
        let capsule = CGRect(x: 16, y: 67, width: ReactionCapsule.width, height: ReactionCapsule.height)
        let menu = CGRect(x: 16, y: 582, width: 250, height: 228)
        func closes(_ x: CGFloat, _ y: CGFloat, menu: CGRect?) -> Bool {
            MessageActionsTarget.dismisses(CGPoint(x: x, y: y), capsule: capsule, menu: menu)
        }
        XCTAssertTrue(closes(200, 300, menu: menu), "on the lifted message")
        XCTAssertTrue(closes(340, 700, menu: menu), "bottom right, beside the menu")
        XCTAssertTrue(closes(8, 700, menu: menu), "left of the menu")
        XCTAssertTrue(closes(200, 30, menu: menu), "above the capsule")
        XCTAssertFalse(closes(100, 600, menu: menu), "a menu row")
        XCTAssertFalse(closes(100, 582 + 3 * 44 + 4, menu: menu), "the band above Delete is the menu's")
        XCTAssertFalse(closes(18, 69, menu: menu), "the capsule's padding")
        XCTAssertTrue(closes(100, 600, menu: nil), "the emoji keyboard took the menu's place")
    }

    // MARK: The sender's name on a lifted bubble

    /// A bubble that continues a run has no name in the transcript; lifted it gets one. One
    /// that has its name, and my own, are left as they are (tester report, 2026-11-17).
    func testALiftedBubbleIsNamedOnlyWhenTheRowIsNot() {
        func row(_ kind: RowKind, role: Role = .project) -> Row { Row(id: "r", messageId: "m", role: role, kind: kind) }
        let unnamed = row(.other(text: "hi", handle: "mira", avatar: nil, named: false, quote: nil))
        XCTAssertEqual(unnamed.namedForHold?.kind, .other(text: "hi", handle: "mira", avatar: nil, named: true, quote: nil))
        XCTAssertEqual(unnamed.namedForHold?.id, "r")
        XCTAssertNil(row(.other(text: "hi", handle: "mira", avatar: nil, named: true, quote: nil)).namedForHold)
        XCTAssertNil(row(.user(text: "hi", delivery: .delivered, quote: nil), role: .user).namedForHold)
        let agent = row(.member(blocks: [], live: false, handle: "scout", name: "Scout", avatar: nil, named: false, agentId: "p1", quote: nil))
        XCTAssertEqual(agent.namedForHold?.kind,
                       .member(blocks: [], live: false, handle: "scout", name: "Scout", avatar: nil, named: true, agentId: "p1", quote: nil))
    }

    /// A finger that never left where it pressed chooses nothing when it lifts; one that
    /// slid away lifts where it is.
    @MainActor
    func testAHoldChoosesOnlyAfterItSlides() {
        let finger = HeldFinger()
        finger.press(at: CGPoint(x: 100, y: 100))
        finger.move(to: CGPoint(x: 104, y: 103))
        XCTAssertNil(finger.point, "still holding")
        finger.lift()
        XCTAssertNil(finger.lifted, "a hold that lifts in place leaves the menu open")

        finger.press(at: CGPoint(x: 100, y: 100))
        finger.move(to: CGPoint(x: 100, y: 140))
        finger.move(to: CGPoint(x: 101, y: 101))
        XCTAssertEqual(finger.point, CGPoint(x: 101, y: 101), "once it has slid it is followed, even back")
        finger.lift()
        XCTAssertEqual(finger.lifted, CGPoint(x: 101, y: 101))
        XCTAssertFalse(finger.isDown)
    }
}
