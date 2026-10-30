import XCTest
@testable import Arbos

/// The long-press overlay (capsule + bubble + menu) is laid out inside the visible
/// area — never under the keyboard — and flips the menu above the bubble when there
/// is no room below (feedback iphone-bubble-menu-keyboard-2026-10-25, Jacob's
/// 4:19 screenshot: Copy / Reply shown, the rest of the menu under the keyboard).
final class MessageActionsLayoutTests: XCTestCase {
    // iPhone 17-class portrait screen; the composer sits in the bottom safe area.
    private let height: CGFloat = 852
    private let safeTop: CGFloat = 59
    private let safeBottom: CGFloat = 34
    private let keyboard: CGFloat = 336
    private let menuHeight: CGFloat = 5 * 44 + 8 // Copy · Reply · Forward · Reply in DM · Delete
    private var capsule: CGFloat { ReactionCapsule.height }

    private func input(bubble: CGRect, keyboardInset: CGFloat = 0, menuHeight: CGFloat? = nil) -> MessageActionsLayout.Input {
        .init(bubble: bubble, capsuleHeight: capsule, menuHeight: menuHeight ?? self.menuHeight,
              height: height, safeTop: safeTop, safeBottom: safeBottom, keyboardInset: keyboardInset)
    }

    private func assertInsideVisibleArea(_ layout: MessageActionsLayout, _ input: MessageActionsLayout.Input,
                                         file: StaticString = #filePath, line: UInt = #line) {
        let top = input.safeTop + input.margin
        let bottom = input.height - max(input.safeBottom, input.keyboardInset) - input.margin
        XCTAssertEqual(layout.visibleTop, top, file: file, line: line)
        XCTAssertEqual(layout.visibleBottom, bottom, file: file, line: line)
        XCTAssertGreaterThanOrEqual(layout.capsuleY, top, "capsule above the visible top", file: file, line: line)
        XCTAssertGreaterThanOrEqual(layout.bubbleY, top, "bubble above the visible top", file: file, line: line)
        XCTAssertGreaterThanOrEqual(layout.menuY, top, "menu above the visible top", file: file, line: line)
        XCTAssertLessThanOrEqual(layout.capsuleY + input.capsuleHeight, bottom, "capsule under the keyboard / home bar", file: file, line: line)
        XCTAssertLessThanOrEqual(layout.bubbleY + layout.bubbleHeight, bottom, "bubble under the keyboard / home bar", file: file, line: line)
        XCTAssertLessThanOrEqual(layout.menuY + input.menuHeight, bottom, "menu under the keyboard / home bar", file: file, line: line)
        // The three pieces never overlap, whichever order they are in.
        var spans = [(layout.capsuleY, layout.capsuleY + input.capsuleHeight),
                     (layout.bubbleY, layout.bubbleY + layout.bubbleHeight),
                     (layout.menuY, layout.menuY + input.menuHeight)]
        spans.sort { $0.0 < $1.0 }
        XCTAssertLessThanOrEqual(spans[0].1 + input.gap, spans[1].0 + 0.001, file: file, line: line)
        XCTAssertLessThanOrEqual(spans[1].1 + input.gap, spans[2].0 + 0.001, file: file, line: line)
    }

    /// Jacob's screenshot: a long agent bubble just above the composer with the keyboard
    /// up. The old code clamped against the home bar only, so the menu ran under the
    /// keyboard. With the keyboard inset every piece — the menu first of all — sits above
    /// it; the 300 pt bubble is the one thing cut to make that true.
    func testKeyboardUpKeepsEveryPieceAboveTheKeyboard() {
        let bubble = CGRect(x: 16, y: 150, width: 361, height: 300) // ends at 450; keyboard top at 852-336 = 516
        let i = input(bubble: bubble, keyboardInset: keyboard)
        let l = MessageActionsLayout.compute(i)
        assertInsideVisibleArea(l, i)
        XCTAssertEqual(l.visibleBottom, 852 - 336 - 8)
        XCTAssertEqual(l.menuY + menuHeight, l.visibleBottom, "the menu ends at the keyboard, not under it")
        XCTAssertEqual(l.capsuleY, l.visibleTop)
        XCTAssertLessThan(l.bubbleHeight, 300)
    }

    /// A shorter bubble with the keyboard up: no room for the menu below it, room above →
    /// the menu flips above the bubble and the bubble stays where the finger is.
    func testKeyboardUpFlipsTheMenuAboveWhenThatFits() {
        let bubble = CGRect(x: 16, y: 360, width: 361, height: 120) // ends at 480; visible bottom 508
        let i = input(bubble: bubble, keyboardInset: keyboard)
        let l = MessageActionsLayout.compute(i)
        assertInsideVisibleArea(l, i)
        XCTAssertEqual(l.placement, .menuAbove)
        XCTAssertEqual(l.bubbleY, 360)
        XCTAssertEqual(l.bubbleShift(from: i), 0)
        XCTAssertEqual(l.menuY + i.menuHeight + i.gap, l.bubbleY)
        XCTAssertEqual(l.capsuleY + i.capsuleHeight + i.gap, l.menuY)
    }

    /// Same press with the keyboard already down (what the fix does first): room below, so the
    /// preferred order holds — capsule above, menu below, nothing moved.
    func testKeyboardDownKeepsMenuBelowTheBubble() {
        let bubble = CGRect(x: 16, y: 150, width: 361, height: 300)
        let i = input(bubble: bubble)
        let l = MessageActionsLayout.compute(i)
        assertInsideVisibleArea(l, i)
        XCTAssertEqual(l.placement, .menuBelow)
        XCTAssertEqual(l.bubbleY, 150)
        XCTAssertEqual(l.capsuleY, 150 - 8 - capsule)
        XCTAssertEqual(l.menuY, 450 + 8)
    }

    /// Structure test: the layout driven across a keyboard inset sweep (0 → full keyboard,
    /// the emoji keyboard the add-emoji control raises included) never leaves the visible area.
    func testEveryKeyboardInsetKeepsTheGroupVisible() {
        let bubble = CGRect(x: 16, y: 560, width: 361, height: 120)
        for inset in stride(from: CGFloat(0), through: keyboard, by: 16) {
            let i = input(bubble: bubble, keyboardInset: inset)
            let l = MessageActionsLayout.compute(i)
            assertInsideVisibleArea(l, i)
            XCTAssertEqual(l.bubbleHeight, 120, "a 120 pt bubble is never clipped at inset \(inset)")
        }
    }

    /// Flip: a short bubble near the bottom, keyboard down — no room for the menu below,
    /// plenty above → menu above the bubble, capsule above the menu, bubble unmoved.
    func testNoRoomBelowFlipsTheMenuAboveTheBubble() {
        let bubble = CGRect(x: 16, y: 700, width: 361, height: 60) // ends at 760; visible bottom 810
        let i = input(bubble: bubble)
        let l = MessageActionsLayout.compute(i)
        assertInsideVisibleArea(l, i)
        XCTAssertEqual(l.placement, .menuAbove)
        XCTAssertEqual(l.bubbleY, 700)
        XCTAssertEqual(l.menuY, 700 - 8 - menuHeight)
        XCTAssertEqual(l.capsuleY, l.menuY - 8 - capsule)
    }

    /// A bubble at the very top: the capsule has no room above, the menu fits below, so the
    /// group shifts down just enough (no flip — flipping needs even more room above).
    func testNoRoomAboveShiftsTheGroupDown() {
        let bubble = CGRect(x: 16, y: 70, width: 361, height: 60)
        let i = input(bubble: bubble)
        let l = MessageActionsLayout.compute(i)
        assertInsideVisibleArea(l, i)
        XCTAssertEqual(l.placement, .menuBelow)
        XCTAssertEqual(l.capsuleY, safeTop + 8)
        XCTAssertEqual(l.bubbleY, safeTop + 8 + capsule + 8)
        XCTAssertGreaterThan(l.bubbleShift(from: i), 0)
    }

    /// A bubble half under the keyboard: neither order fits where it is, so the whole group
    /// is brought into view above the keyboard (the bubble "scrolls" up) with the preferred order.
    func testBubbleUnderTheKeyboardIsBroughtIntoView() {
        let bubble = CGRect(x: 16, y: 450, width: 361, height: 120) // ends at 570; keyboard top 516
        let i = input(bubble: bubble, keyboardInset: keyboard)
        let l = MessageActionsLayout.compute(i)
        assertInsideVisibleArea(l, i)
        XCTAssertEqual(l.placement, .menuBelow)
        XCTAssertLessThan(l.bubbleShift(from: i), 0, "the bubble moved up")
        XCTAssertEqual(l.menuY + menuHeight, l.visibleBottom, "the group ends at the keyboard")
        XCTAssertEqual(l.bubbleHeight, 120)
    }

    /// A bubble taller than the visible area: capsule at the top, menu at the bottom, the
    /// bubble clipped between them — the menu is always reachable.
    func testTallBubbleIsClippedBetweenCapsuleAndMenu() {
        let bubble = CGRect(x: 16, y: 100, width: 361, height: 900)
        let i = input(bubble: bubble, keyboardInset: keyboard)
        let l = MessageActionsLayout.compute(i)
        assertInsideVisibleArea(l, i)
        XCTAssertEqual(l.capsuleY, l.visibleTop)
        XCTAssertEqual(l.menuY + menuHeight, l.visibleBottom)
        XCTAssertLessThan(l.bubbleHeight, 900)
        XCTAssertGreaterThanOrEqual(l.bubbleHeight, 44)
    }

    /// The menu grows with View thread and Edit; the fit still holds on the keyboard-up case.
    func testLongestMenuStillFitsWithTheKeyboardUp() {
        let longest = CGFloat(MessageMenu.items(forMessage: true, canEdit: true, canDelete: true, hasReplies: true).count) * 44 + 8
        let bubble = CGRect(x: 16, y: 380, width: 361, height: 80)
        let i = input(bubble: bubble, keyboardInset: keyboard, menuHeight: longest)
        let l = MessageActionsLayout.compute(i)
        assertInsideVisibleArea(l, i)
    }
}

/// The keyboard goes down when the bubble menu opens and comes back only when it was up
/// before and the chosen action keeps the chat on screen.
final class MessageActionsKeyboardTests: XCTestCase {
    func testKeyboardNeverComesBackIfItWasDown() {
        for item in MessageMenuItem.allCases {
            XCTAssertFalse(MessageActionsKeyboard.restoresFocus(wasFocused: false, after: item), "\(item)")
        }
        XCTAssertFalse(MessageActionsKeyboard.restoresFocus(wasFocused: false, after: nil))
    }

    func testDismissOrReactionGivesTheKeyboardBack() {
        XCTAssertTrue(MessageActionsKeyboard.restoresFocus(wasFocused: true, after: nil))
    }

    func testActionsThatKeepTheChatRestoreTheKeyboard() {
        for item in [MessageMenuItem.copy, .delete, .reply, .edit, .removePreview] {
            XCTAssertTrue(MessageActionsKeyboard.restoresFocus(wasFocused: true, after: item), "\(item)")
        }
    }

    func testActionsThatOpenAnotherSurfaceLeaveTheKeyboardDown() {
        for item in [MessageMenuItem.forward, .viewThread, .replyInDM] {
            XCTAssertFalse(MessageActionsKeyboard.restoresFocus(wasFocused: true, after: item), "\(item)")
        }
    }
}
