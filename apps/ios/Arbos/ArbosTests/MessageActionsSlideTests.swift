import XCTest
@testable import Arbos

/// Sliding the thumb out of a long press onto a menu row or a quick reaction and
/// lifting to pick it, in one gesture (feedback iphone-bubble-menu-slide-to-pick-2026-11-05,
/// Affinity 11:28 CR: "today you cannot").
///
/// Geometry only, no simulator — the same form as `MessageActionsHitAreaTests`. The
/// case worth the file is the one in the middle: the capsule and the menu are two
/// separate rects, and under `.menuBelow` the held bubble sits between them, so a
/// single slide from an emoji to a menu row crosses a surface that is neither. That
/// crossing has to read as "nothing picked yet" without snapping to the nearest item
/// and without ending the slide.
final class MessageActionsSlideTests: XCTestCase {
    // iPhone 17-class portrait, as `MessageActionsLayoutTests` uses.
    private let width: CGFloat = 393
    private let height: CGFloat = 852
    private let safeTop: CGFloat = 59
    private let safeBottom: CGFloat = 34
    private let menuWidth: CGFloat = 250
    private let items = MessageMenu.items // Copy · Reply · Forward · Reply in DM · Delete

    /// A bubble with room for the capsule above and the menu below: the `.menuBelow`
    /// placement, which is the one that puts the bubble between the two surfaces.
    private func surfaces(bubble: CGRect = CGRect(x: 16, y: 400, width: 361, height: 60),
                          mine: Bool = false,
                          keyboardInset: CGFloat = 0) -> (MessageActionsSurfaces, MessageActionsLayout) {
        let layout = MessageActionsLayout.compute(.init(
            bubble: bubble,
            capsuleHeight: ReactionCapsule.height,
            menuHeight: MessageActionsMenuRows.compute(items: items, width: menuWidth).height,
            height: height, safeTop: safeTop, safeBottom: safeBottom, keyboardInset: keyboardInset))
        return (.compute(layout: layout, width: width, menuWidth: menuWidth, items: items, mine: mine), layout)
    }

    /// Every sample a finger moving in a straight line would hand us, half a point apart.
    private func slide(from a: CGPoint, to b: CGPoint) -> [CGPoint] {
        let steps = max(1, Int((hypot(b.x - a.x, b.y - a.y) / 0.5).rounded()))
        return (0 ... steps).map { i in
            let t = CGFloat(i) / CGFloat(steps)
            return CGPoint(x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t)
        }
    }

    private func centre(ofRow i: Int, in s: MessageActionsSurfaces) -> CGPoint {
        let row = s.rows.rows[i]
        return CGPoint(x: s.menu.minX + row.labelRect.midX, y: s.menu.minY + row.labelRect.midY)
    }

    private func centre(ofCell i: Int, in s: MessageActionsSurfaces) -> CGPoint {
        let cell = ReactionCapsule.cellFrames[i]
        return CGPoint(x: s.capsule.minX + cell.midX, y: s.capsule.minY + cell.midY)
    }

    /// Where the finger already is when the menu opens: on the bubble it pressed.
    private func longPressOrigin(_ s: MessageActionsSurfaces) -> CGPoint {
        CGPoint(x: s.bubble.midX, y: s.bubble.midY)
    }

    // MARK: - The deliverable: drag from the long press to an item and lift

    /// The bug, stated as a test: slide from where the long press landed down to Reply
    /// and lift — Reply is chosen.
    func testASlideFromTheLongPressOriginToAMenuRowPicksThatRow() {
        let (s, _) = surfaces()
        XCTAssertEqual(items[1], .reply, "row 1 is Reply (Copy · Reply · …)")
        let samples = slide(from: longPressOrigin(s), to: centre(ofRow: 1, in: s))
        XCTAssertEqual(s.pick(at: samples.first!), .nothing, "the slide starts on the bubble, picking nothing")
        XCTAssertEqual(s.pick(at: samples.last!), .menu(.reply), "lifting on Reply picks Reply")
    }

    /// Every row is reachable the same way, so this is not one lucky row.
    func testEveryMenuRowIsReachableFromTheLongPressOrigin() {
        let (s, _) = surfaces()
        for (i, item) in items.enumerated() {
            let samples = slide(from: longPressOrigin(s), to: centre(ofRow: i, in: s))
            XCTAssertEqual(s.pick(at: samples.last!), .menu(item), "sliding to \(item) picks \(item)")
        }
    }

    /// And every quick reaction, which is the other half of the ask.
    func testEveryQuickReactionIsReachableFromTheLongPressOrigin() {
        let (s, _) = surfaces()
        for (i, emoji) in ReactionCapsule.palette.enumerated() {
            let samples = slide(from: longPressOrigin(s), to: centre(ofCell: i, in: s))
            XCTAssertEqual(s.pick(at: samples.last!), .reaction(emoji), "sliding to \(emoji) picks \(emoji)")
        }
        XCTAssertEqual(s.pick(at: centre(ofCell: ReactionCapsule.palette.count, in: s)), .morePicker,
                       "the last cell raises the emoji keyboard, as a tap on it does")
    }

    // MARK: - The case in the middle: crossing the bubble

    /// One slide from a quick reaction down to a menu row. It crosses the held bubble,
    /// which is neither surface: the crossing picks nothing, and it does not stop the
    /// slide from landing on the row it lifts on.
    func testASlideCrossingTheBubbleStillPicksTheRowItLiftsOn() {
        let (s, layout) = surfaces()
        XCTAssertEqual(layout.placement, .menuBelow)
        let fire = ReactionCapsule.palette.count - 2
        let samples = slide(from: centre(ofCell: fire, in: s), to: centre(ofRow: 1, in: s))

        XCTAssertEqual(s.pick(at: samples.first!), .reaction(ReactionCapsule.palette[fire]))
        XCTAssertEqual(s.pick(at: samples.last!), .menu(.reply), "it lifts on Reply, so Reply wins")

        let overBubble = samples.filter { s.bubble.contains($0) }
        XCTAssertFalse(overBubble.isEmpty, "this slide really does cross the bubble")
        for p in overBubble {
            XCTAssertEqual(s.pick(at: p), .nothing, "over the bubble at \(p) nothing is picked, and nothing is guessed")
        }
    }

    /// The bubble is between the two surfaces, so the crossing above is the ordinary
    /// case rather than a contrived one — and the surfaces never overlap each other.
    func testTheBubbleSitsBetweenTheTwoSurfaces() {
        let (s, _) = surfaces()
        XCTAssertFalse(s.capsule.intersects(s.menu), "the capsule and the menu are two rects, not one")
        XCTAssertLessThan(s.capsule.maxY, s.bubble.minY)
        XCTAssertLessThan(s.bubble.maxY, s.menu.minY)
        XCTAssertNotEqual(s.capsule.width, s.menu.width, "and they are different widths")
    }

    // MARK: - Lifting on nothing picks nothing

    /// Lifting on empty space picks nothing. Worth being explicit about: a nearest-item
    /// guess here would be worse than the bug it fixes.
    func testALiftOnEmptySpacePicksNothing() {
        let (s, _) = surfaces()
        let empty: [(String, CGPoint)] = [
            ("the bubble itself", longPressOrigin(s)),
            ("the gap above the bubble", CGPoint(x: s.bubble.midX, y: s.bubble.minY - 4)),
            ("the gap below the bubble", CGPoint(x: s.bubble.midX, y: s.bubble.maxY + 4)),
            ("beside the menu", CGPoint(x: s.menu.maxX + 20, y: s.menu.midY)),
            ("above the capsule", CGPoint(x: s.capsule.midX, y: s.capsule.minY - 1)),
            ("below the menu", CGPoint(x: s.menu.midX, y: s.menu.maxY + 1)),
            ("the backdrop, top left", CGPoint(x: 1, y: 1)),
            ("the backdrop, bottom right", CGPoint(x: width - 1, y: height - 1)),
        ]
        for (what, p) in empty {
            XCTAssertEqual(s.pick(at: p), .nothing, "lifting on \(what) picks nothing")
        }
    }

    /// Half a point outside a surface is outside it. No item reaches past its own rect.
    func testNothingIsGuessedJustOutsideASurface() {
        let (s, _) = surfaces()
        for (what, r) in [("capsule", s.capsule), ("menu", s.menu)] {
            for (edge, p) in [("left", CGPoint(x: r.minX - 0.5, y: r.midY)),
                              ("right", CGPoint(x: r.maxX + 0.5, y: r.midY)),
                              ("top", CGPoint(x: r.midX, y: r.minY - 0.5)),
                              ("bottom", CGPoint(x: r.midX, y: r.maxY + 0.5))] {
                XCTAssertEqual(s.pick(at: p), .nothing, "0.5 pt off the \(what)'s \(edge) edge picks nothing")
            }
        }
    }

    // MARK: - Inside a surface there is no dead stripe

    /// The counterpart rule: inside either surface the answer is never "nothing", so a
    /// slide along the menu or across the capsule cannot flicker through a dead zone.
    /// Walked every half point down and across, as `MessageActionsHitAreaTests` walks taps.
    func testInsideASurfaceSomethingIsAlwaysUnderTheFinger() {
        let (s, _) = surfaces()
        for r in [s.capsule, s.menu] {
            var y = r.minY + 0.25
            while y < r.maxY {
                XCTAssertNotEqual(s.pick(at: CGPoint(x: r.midX, y: y)), .nothing, "y=\(y) is inside a surface")
                y += 0.5
            }
            var x = r.minX + 0.25
            while x < r.maxX {
                XCTAssertNotEqual(s.pick(at: CGPoint(x: x, y: r.midY)), .nothing, "x=\(x) is inside a surface")
                x += 0.5
            }
        }
    }

    /// The capsule is a single row of cells, so its 4 pt padding band belongs to the
    /// column it is in rather than being a stripe that drops the pick mid-slide.
    func testTheCapsulesPaddingBandBelongsToItsColumn() {
        let (s, _) = surfaces()
        for (i, emoji) in ReactionCapsule.palette.enumerated() {
            let x = s.capsule.minX + ReactionCapsule.cellFrames[i].midX
            for y in [s.capsule.minY + 0.5, s.capsule.midY, s.capsule.maxY - 0.5] {
                XCTAssertEqual(s.pick(at: CGPoint(x: x, y: y)), .reaction(emoji),
                               "the whole column of \(emoji) picks \(emoji)")
            }
        }
    }

    // MARK: - The other placements and the other side

    /// `.menuAbove` puts the menu between the capsule and the bubble, so the two
    /// surfaces are adjacent instead. The rule does not change: the gap picks nothing
    /// and both surfaces still resolve.
    func testTheFlippedPlacementObeysTheSameRule() {
        // A bubble low enough that the menu has to flip above it.
        let (s, layout) = surfaces(bubble: CGRect(x: 16, y: 640, width: 361, height: 60))
        XCTAssertEqual(layout.placement, .menuAbove, "this fixture is the flipped case")
        XCTAssertFalse(s.capsule.intersects(s.menu))
        XCTAssertEqual(s.pick(at: centre(ofRow: 0, in: s)), .menu(items[0]))
        XCTAssertEqual(s.pick(at: centre(ofCell: 0, in: s)), .reaction(ReactionCapsule.palette[0]))
        XCTAssertEqual(s.pick(at: CGPoint(x: s.menu.midX, y: s.capsule.maxY + 4)), .nothing,
                       "the gap between the capsule and the menu picks nothing")
    }

    /// My own messages align both groups to the trailing edge. They move together, and
    /// the leading side is then backdrop.
    func testMyOwnMessagePutsBothSurfacesOnTheTrailingSide() {
        let (mine, _) = surfaces(mine: true)
        XCTAssertEqual(mine.capsule.maxX, width - 16)
        XCTAssertEqual(mine.menu.maxX, width - 16)
        XCTAssertEqual(mine.pick(at: centre(ofRow: 1, in: mine)), .menu(.reply))
        XCTAssertEqual(mine.pick(at: CGPoint(x: 8, y: mine.menu.midY)), .nothing,
                       "the leading side is backdrop for my own message")
    }

    /// With a keyboard still up the group is fitted into what is left of the screen; a
    /// slide still picks the row it lifts on at the group's new position.
    func testTheGroupStaysPickableWithTheKeyboardUp() {
        let (s, layout) = surfaces(keyboardInset: 336)
        XCTAssertLessThanOrEqual(s.menu.maxY, layout.visibleBottom + 0.001, "the menu is above the keyboard")
        let last = items.count - 1
        let samples = slide(from: longPressOrigin(s), to: centre(ofRow: last, in: s))
        XCTAssertEqual(s.pick(at: samples.last!), .menu(items[last]))
    }
}
