import CoreGraphics
import Foundation

/// Where the reaction capsule, the held bubble and the menu go when a bubble is
/// long-pressed (`MessageActionsOverlay`). Pure geometry, so the keyboard-up and
/// flip cases are unit-tested without a simulator.
///
/// Rules (iMessage / Grok Bot shape, feedback iphone-bubble-menu-keyboard-2026-10-25):
/// 1. Everything stays inside the *visible* area — below the status bar, above the
///    home indicator **and above the keyboard** while one is still on screen.
/// 2. Preferred: capsule above the bubble, menu below it, bubble where the finger is.
/// 3. No room below for the menu → the menu flips above the bubble (capsule above the
///    menu); the bubble does not move.
/// 4. Neither fits where the bubble is → the whole group shifts the least distance
///    that brings it into view (the bubble is "scrolled into view").
/// 5. The group is taller than the visible area → capsule at the top, menu at the
///    bottom, and the bubble is clipped between them.
struct MessageActionsLayout: Equatable {
    enum Placement: Equatable {
        /// Capsule · bubble · menu (top to bottom).
        case menuBelow
        /// Capsule · menu · bubble.
        case menuAbove
    }

    struct Input: Equatable {
        /// The pressed bubble, in the overlay's coordinate space.
        var bubble: CGRect
        var capsuleHeight: CGFloat
        var menuHeight: CGFloat
        /// Space between capsule, bubble and menu.
        var gap: CGFloat = 8
        /// The overlay's height (the whole screen).
        var height: CGFloat
        /// Device safe area: status bar / notch at the top, home indicator at the bottom.
        var safeTop: CGFloat
        var safeBottom: CGFloat
        /// How far the keyboard (system or emoji) reaches up from the overlay's bottom edge;
        /// 0 when it is down.
        var keyboardInset: CGFloat = 0
        /// Breathing room kept from the visible edges.
        var margin: CGFloat = 8
    }

    let placement: Placement
    /// Top edges, in the overlay's coordinate space.
    let capsuleY: CGFloat
    let bubbleY: CGFloat
    let menuY: CGFloat
    /// The bubble's drawn height — its own, unless rule 5 clipped it.
    let bubbleHeight: CGFloat
    /// Bounds everything was fitted into.
    let visibleTop: CGFloat
    let visibleBottom: CGFloat

    /// How far the bubble was moved from where the finger pressed it (rules 4–5).
    func bubbleShift(from input: Input) -> CGFloat { bubbleY - input.bubble.minY }

    static func compute(_ i: Input) -> MessageActionsLayout {
        let top = i.safeTop + i.margin
        let bottom = i.height - max(i.safeBottom, i.keyboardInset) - i.margin
        let bubbleH = i.bubble.height
        let gap = i.gap

        // Rule 2: the bubble stays put, capsule above, menu below.
        let capsuleBelowTop = i.bubble.minY - gap - i.capsuleHeight
        let menuBelowEnd = i.bubble.maxY + gap + i.menuHeight
        if capsuleBelowTop >= top, menuBelowEnd <= bottom {
            return MessageActionsLayout(placement: .menuBelow, capsuleY: capsuleBelowTop, bubbleY: i.bubble.minY,
                                        menuY: i.bubble.maxY + gap, bubbleHeight: bubbleH,
                                        visibleTop: top, visibleBottom: bottom)
        }

        // Rule 3: flip the menu above the bubble (capsule on top of it); the bubble must be fully visible.
        let menuAboveTop = i.bubble.minY - gap - i.menuHeight
        let capsuleAboveTop = menuAboveTop - gap - i.capsuleHeight
        if capsuleAboveTop >= top, i.bubble.maxY <= bottom, i.bubble.minY >= top {
            return MessageActionsLayout(placement: .menuAbove, capsuleY: capsuleAboveTop, bubbleY: i.bubble.minY,
                                        menuY: menuAboveTop, bubbleHeight: bubbleH,
                                        visibleTop: top, visibleBottom: bottom)
        }

        // Rule 4: shift the preferred group the least distance into view.
        let groupH = i.capsuleHeight + gap + bubbleH + gap + i.menuHeight
        let available = bottom - top
        if groupH <= available {
            let wanted = capsuleBelowTop
            let groupTop = min(max(wanted, top), bottom - groupH)
            let bubbleY = groupTop + i.capsuleHeight + gap
            return MessageActionsLayout(placement: .menuBelow, capsuleY: groupTop, bubbleY: bubbleY,
                                        menuY: bubbleY + bubbleH + gap, bubbleHeight: bubbleH,
                                        visibleTop: top, visibleBottom: bottom)
        }

        // Rule 5: too tall even shifted — pin capsule and menu to the edges, clip the bubble.
        let clippedH = max(44, available - i.capsuleHeight - i.menuHeight - 2 * gap)
        let bubbleY = top + i.capsuleHeight + gap
        return MessageActionsLayout(placement: .menuBelow, capsuleY: top, bubbleY: bubbleY,
                                    menuY: bubbleY + clippedH + gap, bubbleHeight: clippedH,
                                    visibleTop: top, visibleBottom: bottom)
    }
}

/// The menu's rows as rectangles in the menu's own space (feedback
/// iphone-bubble-menu-hit-area-2026-10-26). Every row's **hit rect is its whole
/// frame** — full width, full height, the padding and the space right of the label
/// included — and the rows tile the menu top to bottom with no gap: the hairline
/// divider is drawn inside the bottom of the row above it and the 8 pt band before
/// Delete is the top of the Delete row, so no tap lands on a dead zone.
struct MessageActionsMenuRows: Equatable {
    struct Row: Equatable {
        let item: MessageMenuItem
        /// Full-width rect in menu space; also the hit target.
        let frame: CGRect
        /// Height of the band drawn inside the row's top (destructive group separator).
        let bandAbove: CGFloat
        /// A hairline is drawn along the row's bottom edge (inside `frame`).
        let dividerBelow: Bool

        var hitRect: CGRect { frame }
        /// Where the label row (icon + text) sits inside the frame, under the band.
        var labelRect: CGRect {
            CGRect(x: frame.minX, y: frame.minY + bandAbove, width: frame.width, height: frame.height - bandAbove)
        }
        var dividerRect: CGRect? {
            guard dividerBelow else { return nil }
            return CGRect(x: frame.minX + MessageActionsMenuRows.dividerInset, y: frame.maxY - MessageActionsMenuRows.dividerHeight,
                          width: frame.width - MessageActionsMenuRows.dividerInset, height: MessageActionsMenuRows.dividerHeight)
        }
    }

    static let rowHeight: CGFloat = 44
    static let destructiveBand: CGFloat = 8
    static let dividerHeight: CGFloat = 0.5
    static let dividerInset: CGFloat = 50

    let rows: [Row]
    let width: CGFloat
    var height: CGFloat { rows.last?.frame.maxY ?? 0 }

    static func compute(items: [MessageMenuItem], width: CGFloat) -> MessageActionsMenuRows {
        var rows: [Row] = []
        var y: CGFloat = 0
        for (i, item) in items.enumerated() {
            let band = item.isDestructive && i > 0 ? destructiveBand : 0
            let next = items.indices.contains(i + 1) ? items[i + 1] : nil
            let divider = next.map { !$0.isDestructive } ?? false
            let h = band + rowHeight
            rows.append(Row(item: item, frame: CGRect(x: 0, y: y, width: width, height: h), bandAbove: band, dividerBelow: divider))
            y += h
        }
        return MessageActionsMenuRows(rows: rows, width: width)
    }
}

extension ReactionCapsule {
    /// The capsule's cells (palette + add-emoji) in the capsule's own space; each cell's
    /// hit rect is its whole cell, and the cells tile the capsule's content width.
    static var cellFrames: [CGRect] {
        (0 ..< palette.count + 1).map { i in
            CGRect(x: padding + CGFloat(i) * cell, y: padding, width: cell, height: cell)
        }
    }
}

/// What happens to the composer's keyboard around the bubble menu
/// (feedback iphone-bubble-menu-keyboard-2026-10-25): it goes down when the menu
/// opens (iMessage / Grok Bot), the typed draft stays (`ComposerDraft` is its own
/// object; focus is not text), and it comes back when the menu closes only if it
/// was up before and the chosen action does not take over the screen itself.
enum MessageActionsKeyboard {
    /// Should the composer take focus again after the menu closes with `item`
    /// (`nil` = dismissed by a tap outside or a reaction)?
    static func restoresFocus(wasFocused: Bool, after item: MessageMenuItem?) -> Bool {
        guard wasFocused else { return false }
        guard let item else { return true }
        switch item {
        case .copy, .delete:
            return true
        case .reply, .edit:
            // These focus the composer themselves (reply preview / editing bar).
            return true
        case .forward, .viewThread, .replyInDM:
            // A sheet or another chat takes the screen; a keyboard under it would fight it.
            return false
        }
    }
}
