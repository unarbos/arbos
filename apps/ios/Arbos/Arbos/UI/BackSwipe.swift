// Horizontal gestures on every chat screen (agent 1:1, DM, group, server channel,
// job tab) and on the server channel list — one owner for all of them (Jacob,
// 2026-10-26, iMessage-style; supersedes #1706's "no swipe-to-reply"):
//
// 1. A swipe that starts in the screen's left edge zone always pops back one level,
//    keyboard open or closed; the keyboard goes down with the pop. Nothing else
//    reads a drag from that zone.
// 2. A horizontal swipe to the right on a message bubble, away from the edge, reveals
//    the reply arrow behind the bubble, taps at the threshold and ends in the composer
//    with the quote (`BubbleSwipe`). Vertical scroll is untouched: the swipe decides
//    once, at 12 pt, whether the pan is horizontal-right or not.
// 3. A swipe to the right anywhere else on the screen (header, empty transcript, the
//    channel list) is also "back" (#925) — never from the composer bar, where a drag
//    selects text or slides the mic, and never while a bubble swipe owns the drag.
//
// Each screen hides the system navigation bar and back button for its own chevron,
// which also turns UIKit's edge-swipe pop off. Under SwiftUI's NavigationStack on iOS 27
// no UIKit recognizer gets the touches back: re-enabling `interactivePopGestureRecognizer`
// with our own delegate, or adding a `UIScreenEdgePanGestureRecognizer` to the screen's
// hosting view, left both enabled and never asked to begin (logging delegates, 2026-10-26).
// So the edge pop here is a SwiftUI drag at the screen's root, decided by where it starts.
// History:
// #1554 gated the back gestures on the composer's focus and `send()` keeps the composer
// focused, so once anything was typed no back swipe worked (#1831).

import SwiftUI
import UIKit

/// The back-gesture rules, as data (unit-tested in `BackSwipeTests`).
enum BackSwipe {
    /// The left band the edge pop owns: a drag that starts inside it is "back" and nothing
    /// else (not a bubble swipe, not text selection), whatever is under the finger.
    static let edgeZoneWidth: CGFloat = 24
    /// How far right an edge drag must travel to pop (and to drop the keyboard early).
    static let edgePopDistance: CGFloat = 40
    /// How far right an edge drag must travel before the keyboard goes down with it.
    static let edgeKeyboardDistance: CGFloat = 12

    static func startsAtEdge(_ start: CGPoint) -> Bool { start.x < edgeZoneWidth }

    /// An edge drag pops when it has moved right by `edgePopDistance` and more sideways
    /// than up or down. No screen state (keyboard, draft, edit, composer bar) blocks it.
    static func edgePops(translation: CGSize) -> Bool {
        translation.width >= edgePopDistance && translation.width > abs(translation.height)
    }

    /// A finished drag counts as "back" when:
    /// - it started in the edge zone and `edgePops`; or
    /// - it is a clear horizontal pan to the right — far enough (> 70 pt), more horizontal
    ///   than vertical (1.25×), and either faster sideways than up/down or long (> 120 pt)
    ///   — that did not start inside the composer bar (`start.y >= composerTop`, where a
    ///   drag selects text or slides the mic) and that no bubble swipe claimed.
    static func isBackSwipe(translation: CGSize, velocity: CGSize, start: CGPoint, composerTop: CGFloat?,
                            bubbleSwipeActive: Bool) -> Bool {
        if startsAtEdge(start) { return edgePops(translation: translation) }
        if bubbleSwipeActive { return false }
        if let composerTop, start.y >= composerTop { return false }
        let dx = translation.width
        let absX = abs(dx), absY = abs(translation.height)
        guard dx > 0, absX > 70, absX > absY * 1.25 else { return false }
        return abs(velocity.width) >= abs(velocity.height) || absX > 120
    }

    /// True while a bubble's reply swipe owns the current drag (set by `RowView`, read by
    /// the full-screen back swipe of the same drag). One drag at a time on a phone, so a
    /// flag, like `NestedHorizontalScroll.claiming`. Cleared a run-loop turn after the
    /// bubble's drag ends so the screen's `onEnded` of the same drag still sees it.
    static var bubbleSwipeActive = false

    /// The keyboard goes down with the pop (and with the full-screen back).
    static func dismissKeyboard() {
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
    }
}

/// Swipe right on a message bubble → reply (Telegram / iMessage). Rules as data, tested
/// in `BackSwipeTests`; `RowView` runs them.
enum BubbleSwipe {
    /// Pan length at which the swipe decides, once, what the drag is.
    static let lockDistance: CGFloat = 12
    /// How far the bubble follows the finger 1:1 before it rubber-bands.
    static let revealWidth: CGFloat = 56
    /// Fraction of `revealWidth` at which the reply arms (haptic; release replies).
    static let triggerFraction: CGFloat = 0.7

    enum Lock: Equatable {
        /// Under `lockDistance`: nothing decided yet.
        case undecided
        /// A horizontal pan to the right, outside the edge zone: this drag replies.
        case reply
        /// Anything else (vertical scroll, swipe left, edge zone): this drag is left alone until it ends.
        case declined
    }

    /// Decide what a drag on a bubble is. `startX` is the drag's start in the screen's
    /// coordinate space: inside `BackSwipe.edgeZoneWidth` the edge pop owns the drag.
    static func lock(translation: CGSize, startX: CGFloat) -> Lock {
        let dx = translation.width, dy = translation.height
        guard abs(dx) + abs(dy) >= lockDistance else { return .undecided }
        if startX < BackSwipe.edgeZoneWidth { return .declined }
        return dx > 0 && abs(dx) > abs(dy) * 1.15 ? .reply : .declined
    }

    /// Bubble travel for a rightward pan: 1:1 up to `revealWidth`, then a fifth of the
    /// excess, at most 24 pt more (the time reveal's curve, mirrored).
    static func reveal(for dx: CGFloat) -> CGFloat {
        let raw = max(0, dx)
        return raw <= revealWidth ? raw : revealWidth + min(24, (raw - revealWidth) * 0.2)
    }

    /// Armed: releasing here replies.
    static func triggers(reveal: CGFloat) -> Bool { reveal >= revealWidth * triggerFraction }
}

/// Top edge of the composer bar in the chat root's coordinate space (`BackSwipe.space`),
/// reported by the bar so the full-screen swipe can leave text selection alone.
struct ComposerTopKey: PreferenceKey {
    static let defaultValue: CGFloat? = nil
    static func reduce(value: inout CGFloat?, nextValue: () -> CGFloat?) {
        if let next = nextValue() { value = next }
    }
}

extension BackSwipe {
    /// Named coordinate space of a screen that uses `backSwipeToPop`; the composer bar
    /// measures its top edge in it.
    static let space = "arbosBackSwipeRoot"
}

/// Hides the system bar and back button and owns the back gestures: the edge pop and the
/// full-screen swipe-right, both → `dismiss()` (SwiftUI pops, so the stack's path stays
/// the truth). One simultaneous drag at the screen's root; which rule applies is decided
/// by where the drag started (`BackSwipe.isBackSwipe`).
private struct BackSwipeModifier: ViewModifier {
    @Environment(\.dismiss) private var dismiss
    /// Where the composer bar starts (nil: the whole screen counts).
    @State private var composerTop: CGFloat?
    /// The keyboard already went down for the current edge drag.
    @State private var edgeKeyboardDropped = false

    func body(content: Content) -> some View {
        content
            .coordinateSpace(name: BackSwipe.space)
            .onPreferenceChange(ComposerTopKey.self) { composerTop = $0 }
            .toolbar(.hidden, for: .navigationBar)
            .navigationBarBackButtonHidden(true)
            .simultaneousGesture(
                DragGesture(minimumDistance: BackSwipe.edgeKeyboardDistance, coordinateSpace: .named(BackSwipe.space))
                    .onChanged { value in
                        // The keyboard leaves with the edge pop, not after it.
                        guard !edgeKeyboardDropped, BackSwipe.startsAtEdge(value.startLocation),
                              value.translation.width >= BackSwipe.edgeKeyboardDistance else { return }
                        edgeKeyboardDropped = true
                        BackSwipe.dismissKeyboard()
                    }
                    .onEnded { value in
                        edgeKeyboardDropped = false
                        if BackSwipe.isBackSwipe(translation: value.translation, velocity: value.velocity,
                                                 start: value.startLocation, composerTop: composerTop,
                                                 bubbleSwipeActive: BackSwipe.bubbleSwipeActive) {
                            BackSwipe.dismissKeyboard()
                            dismiss()
                        }
                    }
            )
    }
}

extension View {
    /// Custom-back screen: swipe right (edge, or anywhere outside the composer bar and
    /// the bubbles) pops it.
    func backSwipeToPop() -> some View { modifier(BackSwipeModifier()) }

    /// Mark the composer bar of a `backSwipeToPop` screen so a drag that starts in it
    /// (text selection, the mic slide) is never read as "back".
    func backSwipeComposerBar() -> some View {
        background(
            GeometryReader { geo in
                Color.clear.preference(key: ComposerTopKey.self,
                                       value: geo.frame(in: .named(BackSwipe.space)).minY)
            }
        )
    }
}
