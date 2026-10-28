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
import UIKit.UIGestureRecognizerSubclass

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

    /// An edge drag decides early (6 pt): it claims the touch as soon as the finger has moved
    /// right at least as much as up or down (the same "more sideways than vertical" shape
    /// `edgePops` asks for at the end), fails at once when the motion is leftward or clearly
    /// vertical (twice as much up/down as right), and otherwise waits for the next point. The
    /// scroll view's own pan begins at ~10 pt, so a rightward edge drag is the pop's before the
    /// transcript can take it (Jacob, 2026-10-26: the edge always pops), while a scroll that
    /// happens to start in the edge zone is the transcript's as before.
    static let edgeLockDistance: CGFloat = 6

    static func edgeLock(translation: CGSize) -> SwipeClaim {
        let dx = translation.width, dy = abs(translation.height)
        if dx <= 0 { return .declined }
        if dx >= dy { return .claimed }
        if dy > dx * 2 { return .declined }
        return .undecided
    }

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

/// Which optional chat-screen gestures are installed. All of them in Release; the Debug
/// scroll rig leaves single ones out (`ARBOS_PERF_GESTURES_OFF`) to measure what each one
/// costs the scroll view's pan (iphone-scroll-perf, 2026-10-27).
enum ChatGestures {
    static let replySwipe = on("reply")
    static let timeReveal = on("time")
    static let backSwipe = on("back")
    static let longPress = on("longpress")

    private static func on(_ name: String) -> Bool {
        #if DEBUG
        return !DebugHooks.gesturesOff.contains(name)
        #else
        return true
        #endif
    }
}

extension View {
    /// `simultaneousGesture` that is not installed at all when `enabled` is false (a masked
    /// gesture still sits in the recognizer graph; the rig needs it gone). `enabled` is a
    /// launch-time constant, so the view tree never changes shape under SwiftUI.
    @ViewBuilder
    func simultaneousGesture<G: Gesture>(_ gesture: G, including mask: GestureMask = .all, enabled: Bool) -> some View {
        if enabled {
            simultaneousGesture(gesture, including: mask)
        } else {
            self
        }
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

// MARK: - UIKit recognizers (iphone-scroll-perf, 2026-10-27)
//
// The three horizontal gestures used to be SwiftUI `DragGesture`s — one per bubble (reply), one
// on the ScrollView (time peek) and one at the screen root (back), all `simultaneousGesture` so
// the scroll kept working. A SwiftUI drag cannot fail: once it reaches `minimumDistance` it runs
// for the whole touch whatever the direction, so every vertical pan on a bubble kept three
// drags alive beside the scroll and each move event ran their closures. They are now two
// UIKit recognizers that decide direction on the first points and fail at once otherwise:
// `TranscriptSwipeHost` (reply right on a row, time peek left; attached to the transcript's
// UIScrollView, the row found by location) and `BackSwipeHost` (edge pop and full-screen back;
// attached to the screen's root view). Both are exclusive with the scroll view's pan — the
// first to begin wins and the other fails, nothing waits on anything — and the rules stay
// the data above (`BubbleSwipe.lock`, `BackSwipe.isBackSwipe`), pinned by `BackSwipeTests`.

/// What a horizontal drag turned out to be, decided once at `BubbleSwipe.lockDistance`.
enum SwipeClaim: Equatable {
    case undecided
    case claimed
    case declined
}

/// A pan that decides its direction on the first points and otherwise fails at once. Not a
/// `UIPanGestureRecognizer`: that one begins at its own ~10 pt in any direction, before the
/// direction is known. Exclusive with the scroll view's pan (no `require(toFail:)` either way):
/// a vertical pan scrolls the instant the scroll view says so; this one has failed by then.
final class DirectionalSwipeRecognizer: UIGestureRecognizer {
    /// Decide once the finger has moved `minimumDistance` (`start` in the recognizer's view):
    /// `.claimed` begins, `.declined` fails, `.undecided` asks again on the next move.
    var decide: (_ start: CGPoint, _ translation: CGSize) -> SwipeClaim = { _, _ in .declined }
    /// Movement (|dx| + |dy|) before `decide` is first asked.
    var minimumDistance: CGFloat = BubbleSwipe.lockDistance
    /// Fail on touch-down when the finger landed somewhere this gesture must leave alone.
    var shouldStart: (_ touch: UITouch, _ start: CGPoint) -> Bool = { _, _ in true }

    private(set) var start = CGPoint.zero
    private(set) var translation = CGSize.zero
    /// Points per second over the last two samples.
    private(set) var velocity = CGSize.zero
    private var lastPoint = CGPoint.zero
    private var lastTime: TimeInterval = 0
    private var tracked: UITouch?

    override init(target: Any?, action: Selector?) {
        super.init(target: target, action: action)
        delaysTouchesBegan = false
        delaysTouchesEnded = false
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent) {
        // One finger only; a second one is not ours (and must not touch a drag in progress).
        guard tracked == nil else { return }
        guard touches.count == 1, let t = touches.first, let view else {
            state = .failed
            return
        }
        tracked = t
        start = t.location(in: view)
        lastPoint = start
        lastTime = t.timestamp
        translation = .zero
        velocity = .zero
        if !shouldStart(t, start) { state = .failed }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent) {
        guard let t = tracked, touches.contains(t), let view else { return }
        let p = t.location(in: view)
        let dt = t.timestamp - lastTime
        if dt > 0 {
            velocity = CGSize(width: (p.x - lastPoint.x) / dt, height: (p.y - lastPoint.y) / dt)
        }
        lastPoint = p
        lastTime = t.timestamp
        translation = CGSize(width: p.x - start.x, height: p.y - start.y)
        switch state {
        case .possible:
            guard abs(translation.width) + abs(translation.height) >= minimumDistance else { return }
            switch decide(start, translation) {
            case .undecided: break
            case .claimed: state = .began
            case .declined: state = .failed
            }
        case .began, .changed:
            state = .changed
        case .ended, .cancelled, .failed:
            break
        @unknown default:
            break
        }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent) {
        guard let t = tracked, touches.contains(t) else { return }
        tracked = nil
        state = state == .began || state == .changed ? .ended : .failed
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent) {
        guard let t = tracked, touches.contains(t) else { return }
        tracked = nil
        state = state == .began || state == .changed ? .cancelled : .failed
    }

    override func reset() {
        super.reset()
        tracked = nil
        translation = .zero
        velocity = .zero
    }
}

/// The transcript rows' frames in the transcript's coordinate space (`TranscriptSpace.name`),
/// written by the rows on layout (`onGeometryChange`), read by the swipe recognizer at touch
/// time to find the row under the finger. A plain class, never `@Published`: a write must not
/// invalidate any view. Frames do not move with the scroll offset (the space scrolls with the
/// content), so nothing here runs per frame while scrolling.
final class RowFrameIndex {
    private var frames: [String: CGRect] = [:]
    private var replyable: Set<String> = []

    func set(_ id: String, frame: CGRect, canReply: Bool) {
        frames[id] = frame
        if canReply { replyable.insert(id) } else { replyable.remove(id) }
    }

    func remove(_ id: String) {
        frames[id] = nil
        replyable.remove(id)
    }

    /// The row whose frame holds `point` (transcript space), if it can be replied to.
    func replyableRow(at point: CGPoint) -> String? {
        guard let hit = frames.first(where: { $0.value.contains(point) }) else { return nil }
        return replyable.contains(hit.key) ? hit.key : nil
    }

    /// Every row the lazy list holds right now, with its frame (the drawn-check after a return).
    func realized() -> [TranscriptReanchor.Realized] {
        frames.map { TranscriptReanchor.Realized(id: $0.key, frame: $0.value) }
    }
}

enum TranscriptSpace {
    /// Named coordinate space of the transcript's content (the ScrollView's VStack).
    static let name = "arbosTranscript"
}

/// The reply swipe in progress on the chat screen: which row follows the finger, how far
/// (`BubbleSwipe.reveal`), and whether releasing now replies (`BubbleSwipe.triggers`).
struct ReplySwipe: Equatable {
    var rowId: String?
    var reveal: CGFloat = 0
    var armed = false
}

/// What the transcript's swipe recognizer tells the chat screen.
enum TranscriptSwipeEvent {
    /// A reply swipe on `row` follows the finger: `dx` is the translation to the right.
    case replyChanged(row: String, dx: CGFloat)
    /// The reply swipe ended; `fire` when it was armed (`BubbleSwipe.triggers`).
    case replyEnded(row: String, fire: Bool)
    /// The time peek follows the finger: `dx` is the translation (negative = left).
    case timeChanged(dx: CGFloat)
    case timeEnded
}

/// One recognizer on the transcript's `UIScrollView` for the two horizontal swipes inside it —
/// swipe right on a message row → reply, swipe left anywhere → time peek — instead of one
/// SwiftUI drag per bubble plus one on the ScrollView. Sits as the `.background` of the
/// ScrollView's content so its bounds are the transcript space the rows report their frames in.
struct TranscriptSwipeHost: UIViewRepresentable {
    let frames: RowFrameIndex
    let onEvent: (TranscriptSwipeEvent) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(frames: frames, onEvent: onEvent) }

    func makeUIView(context: Context) -> HostView {
        let v = HostView()
        v.isUserInteractionEnabled = false
        v.backgroundColor = .clear
        v.coordinator = context.coordinator
        return v
    }

    func updateUIView(_ uiView: HostView, context: Context) {
        context.coordinator.onEvent = onEvent
    }

    final class HostView: UIView {
        weak var coordinator: Coordinator?

        override func didMoveToWindow() {
            super.didMoveToWindow()
            guard window != nil else { return }
            var node: UIView? = superview
            while let current = node {
                if let scroll = current as? UIScrollView {
                    coordinator?.attach(to: scroll, host: self)
                    return
                }
                node = current.superview
            }
        }

        /// Is `windowPoint` over a row that can be replied to? Asked by the screen's back
        /// recognizer so a right swipe on a bubble is the reply's, never "back".
        func hitsReplyableRow(windowPoint: CGPoint) -> Bool {
            let p = convert(windowPoint, from: nil)
            return coordinator?.frames.replyableRow(at: p) != nil
        }
    }

    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        let frames: RowFrameIndex
        var onEvent: (TranscriptSwipeEvent) -> Void
        private weak var scroll: UIScrollView?
        private weak var host: HostView?
        private var recognizer: DirectionalSwipeRecognizer?
        private enum Mode { case reply(row: String), time }
        private var mode: Mode?

        init(frames: RowFrameIndex, onEvent: @escaping (TranscriptSwipeEvent) -> Void) {
            self.frames = frames
            self.onEvent = onEvent
        }

        func attach(to scroll: UIScrollView, host: HostView) {
            guard self.scroll !== scroll else { return }
            if let old = recognizer { old.view?.removeGestureRecognizer(old) }
            self.scroll = scroll
            self.host = host
            let gr = DirectionalSwipeRecognizer(target: self, action: #selector(handle(_:)))
            gr.delegate = self
            gr.shouldStart = { [weak scroll] touch, _ in
                // A markdown table scrolling sideways is its own scroll view's pan.
                var node: UIView? = touch.view
                while let current = node, current !== scroll {
                    if let s = current as? UIScrollView, s.contentSize.width > s.bounds.width + 1 { return false }
                    node = current.superview
                }
                return true
            }
            gr.decide = { [weak self, weak scroll, weak host] start, translation in
                guard let self, let scroll, let host else { return .declined }
                let inWindow = scroll.convert(start, to: nil)
                if BackSwipe.startsAtEdge(inWindow) { return .declined }
                let dx = translation.width, dy = translation.height
                if dx > 0 {
                    // Same rule as `BubbleSwipe.lock`, with the row found by location.
                    guard ChatGestures.replySwipe, BubbleSwipe.lock(translation: translation, startX: inWindow.x) == .reply,
                          let row = self.frames.replyableRow(at: host.convert(start, from: scroll)) else { return .declined }
                    self.mode = .reply(row: row)
                    return .claimed
                }
                guard ChatGestures.timeReveal, dx < 0, abs(dx) > abs(dy) * 1.15 else { return .declined }
                self.mode = .time
                return .claimed
            }
            scroll.addGestureRecognizer(gr)
            recognizer = gr
        }

        @objc private func handle(_ gr: DirectionalSwipeRecognizer) {
            guard let mode else { return }
            switch gr.state {
            case .began, .changed:
                if case .reply = mode { BackSwipe.bubbleSwipeActive = true }
                switch mode {
                case .reply(let row): onEvent(.replyChanged(row: row, dx: gr.translation.width))
                case .time: onEvent(.timeChanged(dx: gr.translation.width))
                }
            case .ended, .cancelled, .failed:
                switch mode {
                case .reply(let row):
                    let fire = gr.state == .ended && BubbleSwipe.triggers(reveal: BubbleSwipe.reveal(for: gr.translation.width))
                    onEvent(.replyEnded(row: row, fire: fire))
                    // The screen's back recognizer may read the flag for this same touch.
                    DispatchQueue.main.async { BackSwipe.bubbleSwipeActive = false }
                case .time:
                    onEvent(.timeEnded)
                }
                self.mode = nil
            case .possible:
                break
            @unknown default:
                break
            }
        }

        /// Exclusive with every other recognizer (the scroll pan above all); SwiftUI's own
        /// recognizer still asks for itself and keeps taps / long-press alive until we begin.
        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
            false
        }
    }
}

/// Hides the system bar and back button and owns the back gestures: the edge pop and the
/// full-screen swipe-right, both → `dismiss()` (SwiftUI pops, so the stack's path stays
/// the truth). One UIKit recognizer on the screen's root view that fails at once on anything
/// but a rightward start (`BackSwipeHost`); which rule applies is decided by where the drag
/// started (`BackSwipe.isBackSwipe`).
private struct BackSwipeModifier: ViewModifier {
    @Environment(\.dismiss) private var dismiss
    /// Where the composer bar starts (nil: the whole screen counts).
    @State private var composerTop: CGFloat?

    func body(content: Content) -> some View {
        content
            .coordinateSpace(name: BackSwipe.space)
            .onPreferenceChange(ComposerTopKey.self) { composerTop = $0 }
            .toolbar(.hidden, for: .navigationBar)
            .navigationBarBackButtonHidden(true)
            .background {
                if ChatGestures.backSwipe {
                    BackSwipeHost(composerTop: composerTop, onBack: { dismiss() })
                }
            }
    }
}

/// The screen-level back recognizer. The host view is the `.background` of the screen's root
/// content, so its bounds are `BackSwipe.space`; the recognizer itself goes on the nearest
/// ancestor that is a view controller's view (touches on the content never reach a sibling).
private struct BackSwipeHost: UIViewRepresentable {
    let composerTop: CGFloat?
    let onBack: () -> Void

    func makeCoordinator() -> Coordinator { Coordinator(onBack: onBack) }

    func makeUIView(context: Context) -> HostView {
        let v = HostView()
        v.isUserInteractionEnabled = false
        v.backgroundColor = .clear
        v.coordinator = context.coordinator
        return v
    }

    func updateUIView(_ uiView: HostView, context: Context) {
        context.coordinator.composerTop = composerTop
        context.coordinator.onBack = onBack
    }

    final class HostView: UIView {
        weak var coordinator: Coordinator?

        override func didMoveToWindow() {
            super.didMoveToWindow()
            guard window != nil else { return }
            // The screen's root: the first ancestor whose next responder is a view controller.
            var node: UIView? = superview
            while let current = node {
                if current.next is UIViewController {
                    coordinator?.attach(to: current, host: self)
                    return
                }
                node = current.superview
            }
        }
    }

    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        var composerTop: CGFloat?
        var onBack: () -> Void
        private weak var root: UIView?
        private weak var host: HostView?
        private weak var transcript: TranscriptSwipeHost.HostView?
        private var recognizer: DirectionalSwipeRecognizer?
        private var edgeKeyboardDropped = false

        init(onBack: @escaping () -> Void) {
            self.onBack = onBack
        }

        func attach(to root: UIView, host: HostView) {
            guard self.root !== root else { return }
            if let old = recognizer { old.view?.removeGestureRecognizer(old) }
            self.root = root
            self.host = host
            let gr = DirectionalSwipeRecognizer(target: self, action: #selector(handle(_:)))
            gr.delegate = self
            gr.minimumDistance = BackSwipe.edgeLockDistance
            gr.decide = { [weak self, weak root, weak host] start, translation in
                guard let self, let root, let host else { return .declined }
                let inSpace = host.convert(start, from: root)
                let dx = translation.width, dy = translation.height
                if BackSwipe.startsAtEdge(inSpace) {
                    return BackSwipe.edgeLock(translation: translation)
                }
                // Off the edge the rules are the bubble's 12 pt.
                guard abs(dx) + abs(dy) >= BubbleSwipe.lockDistance else { return .undecided }
                if let top = self.composerTop, inSpace.y >= top { return .declined }
                guard dx > 0, abs(dx) > abs(dy) * 1.15 else { return .declined }
                // A right swipe on a message bubble is the reply's.
                if self.transcriptHost(under: root)?.hitsReplyableRow(windowPoint: root.convert(start, to: nil)) == true {
                    return .declined
                }
                return .claimed
            }
            root.addGestureRecognizer(gr)
            recognizer = gr
        }

        private func transcriptHost(under root: UIView) -> TranscriptSwipeHost.HostView? {
            if let transcript, transcript.window != nil { return transcript }
            transcript = Self.find(TranscriptSwipeHost.HostView.self, in: root)
            return transcript
        }

        private static func find<T: UIView>(_ type: T.Type, in view: UIView) -> T? {
            for sub in view.subviews {
                if let hit = sub as? T { return hit }
                if let hit = find(type, in: sub) { return hit }
            }
            return nil
        }

        @objc private func handle(_ gr: DirectionalSwipeRecognizer) {
            guard let root, let host else { return }
            let start = host.convert(gr.start, from: root)
            switch gr.state {
            case .began, .changed:
                // The keyboard leaves with the edge pop, not after it.
                if !edgeKeyboardDropped, BackSwipe.startsAtEdge(start), gr.translation.width >= BackSwipe.edgeKeyboardDistance {
                    edgeKeyboardDropped = true
                    BackSwipe.dismissKeyboard()
                }
            case .ended:
                edgeKeyboardDropped = false
                if BackSwipe.isBackSwipe(translation: gr.translation, velocity: gr.velocity, start: start,
                                         composerTop: composerTop, bubbleSwipeActive: BackSwipe.bubbleSwipeActive) {
                    BackSwipe.dismissKeyboard()
                    onBack()
                }
            case .cancelled, .failed:
                edgeKeyboardDropped = false
            case .possible:
                break
            @unknown default:
                break
            }
        }

        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
            false
        }
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
