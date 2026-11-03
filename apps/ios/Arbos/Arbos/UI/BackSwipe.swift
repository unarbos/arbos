// Horizontal gestures on every chat screen (agent 1:1, DM, group, server channel, job tab)
// and on the server channel list — one owner for all of them (Jacob, 2026-10-28:
// "remove the swipe to reply and replace it with performant and high fidelity swipe to go
// back"; supersedes the 2026-10-26 iMessage reply-swipe rule):
//
// 1. A swipe from the screen's left edge is the system's interactive pop: the screen
//    follows the finger, a release short of halfway cancels, UIKit drives the frames. Each
//    chat screen hides the system navigation bar and back button for its own chevron, and
//    SwiftUI turns the pop gesture off with them — `NativePopEnabler` turns it back on and
//    is its delegate (begins whenever the stack has something to pop to; keyboard open or
//    closed — the keyboard goes down as the pop begins). Nothing else reads a drag that
//    starts in the edge zone.
// 2. A swipe to the right anywhere else on the screen (header, transcript, empty space, the
//    channel list) is also "back" (since 7d2d3797, 2026-10-16) — an animated pop, never from
//    the composer bar (a drag there selects text or slides the mic) and never from the edge
//    zone, which is the system's. `FullScreenBackHost`, a `DirectionalSwipeRecognizer` that
//    fails at once on anything but a clear rightward pan.
// 3. A swipe to the left on the transcript peeks the message times (iMessage); the same
//    recognizer class, on the scroll view, declines every rightward or vertical pan.
//
// There is no swipe-to-reply. Reply stays on long-press and in the actions menu.
//
// A drag that starts on something tappable — a sender's avatar above all — is the swipe, not
// the tap: both back recognizers declare themselves simultaneous with the presses and taps
// and exclusive with the pans (`BackSwipe.sharesTouch`, Jacob 2026-11-01). Only a touch that
// does not become a drag opens the profile.
//
// History: #1554 gated the back gestures on the composer's focus (#1831). #1985 made the edge
// pop a SwiftUI drag that called `dismiss()` after 40 pt and added the per-bubble reply drag;
// #2122 turned both into exclusive UIKit recognizers — the root one claimed an edge drag at
// 6 pt and, being exclusive with everything, failed the system's edge pan before it could
// begin: the finger-tracking pop was gone and "back" became a 40 pt threshold with an
// animation that did not follow the finger (Jacob, 2026-10-28: "very buggy and not working").
// The system pop had been on since 731cdec9 (2026-10-16) until #1985 removed its enabler.

import SwiftUI
import UIKit
import UIKit.UIGestureRecognizerSubclass
import os

/// The back-gesture rules, as data (unit-tested in `BackSwipeTests`).
enum BackSwipe {
    /// The left band that belongs to the system's edge pop: nothing else reads a drag that
    /// starts inside it. UIKit's own edge recognizer is narrower; the band is wider so a
    /// finger that lands a little off the edge is not caught by the full-screen rule either.
    static let edgeZoneWidth: CGFloat = 24

    static func startsAtEdge(_ start: CGPoint) -> Bool { start.x < edgeZoneWidth }

    /// Pan length at which a non-edge swipe decides, once, what the drag is.
    static let lockDistance: CGFloat = 12

    /// What a drag that did not start in the edge zone is, at `lockDistance`: a clear pan to
    /// the right (more than 1.15× as wide as tall) is "back"; anything else is left to the
    /// scroll view / the time peek / the presses, for the rest of the touch.
    static func fullScreenLock(translation: CGSize, start: CGPoint, composerTop: CGFloat?) -> SwipeClaim {
        let dx = translation.width, dy = translation.height
        guard abs(dx) + abs(dy) >= lockDistance else { return .undecided }
        if startsAtEdge(start) { return .declined }
        if let composerTop, start.y >= composerTop { return .declined }
        return dx > 0 && abs(dx) > abs(dy) * 1.15 ? .claimed : .declined
    }

    /// A finished full-screen drag pops when it is far enough (> 70 pt), more horizontal
    /// than vertical (1.25×), and either faster sideways than up/down or long (> 120 pt).
    static func fullScreenPops(translation: CGSize, velocity: CGSize) -> Bool {
        let dx = translation.width
        let absX = abs(dx), absY = abs(translation.height)
        guard dx > 0, absX > 70, absX > absY * 1.25 else { return false }
        return abs(velocity.width) >= abs(velocity.height) || absX > 120
    }

    /// The keyboard goes down with the pop.
    static func dismissKeyboard() {
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
    }

    /// Named coordinate space of a screen that uses `backSwipeToPop`; the composer bar
    /// measures its top edge in it.
    static let space = "arbosBackSwipeRoot"

    /// May a back gesture run alongside `other`? Asked by the delegate of both
    /// `DirectionalSwipeRecognizer`s, and the reason a drag that starts on a sender's
    /// avatar is read as "back" at all (Jacob, 2026-11-01: "the slide to go back doesn't
    /// register when you are over top a user pfp which is common but this should still
    /// register as the slide and only a direct click should go to the user profile screen").
    ///
    /// Every avatar beside a bubble is a SwiftUI `Button` (`sender-avatar` → `toggleProfile`),
    /// and a `Button` tracks its press with a recognizer that enters `.began` on touch-down —
    /// that is how the pressed state draws. UIKit then lets that already-begun recognizer
    /// prevent every later one that does not share the touch with it, so the swipe was
    /// prevented at `lockDistance`, before it had decided anything: the finger was on a
    /// picture, so the screen did not move.
    ///
    /// The delegate's simultaneity call is UIKit's own answer to this and it is one-sided by
    /// design — returning true is *guaranteed* to allow simultaneous recognition, whatever
    /// the other delegate says, while returning false only declines. So "back" says yes to
    /// the presses and taps (the avatar's and the name's `Button`, the bubble's long-press)
    /// and keeps saying no to every pan: the scroll view's, the time peek's and the system
    /// pop's arbitration is exactly what it was.
    ///
    /// Nothing is made to wait. `require(toFail:)` would be the other way to give the swipe
    /// priority, and it is the wrong one: the avatar's tap would then be held until the
    /// swipe had failed, and a profile that opens after a pause is its own regression. A
    /// touch that does not move still opens the page on the frame it always did; a touch
    /// that becomes a swipe leaves the avatar, and a button that the finger leaves cancels
    /// itself, as every button does.
    ///
    /// The two back recognizers are excluded from each other by name, not by being pans —
    /// `DirectionalSwipeRecognizer` is a plain `UIGestureRecognizer`. One swipe right inside
    /// the transcript is claimed by both the transcript's recognizer and the screen's, and
    /// they have always been exclusive, so the first to begin pops and the other is
    /// prevented. Letting them share would pop two levels at once.
    static func sharesTouch(with other: UIGestureRecognizer) -> Bool {
        !(other is UIPanGestureRecognizer) && !(other is DirectionalSwipeRecognizer)
    }
}

/// Which optional chat-screen gestures are installed. All of them in Release; the Debug
/// scroll rig leaves single ones out (`ARBOS_PERF_GESTURES_OFF`) to measure what each one
/// costs the scroll view's pan (iphone-scroll-perf, 2026-10-27).
enum ChatGestures {
    static let timeReveal = on("time")
    /// The full-screen swipe-right (the system edge pop is never off).
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

/// Top edge of the composer bar in the chat root's coordinate space (`BackSwipe.space`),
/// reported by the bar so the full-screen swipe can leave text selection alone.
struct ComposerTopKey: PreferenceKey {
    static let defaultValue: CGFloat? = nil
    static func reduce(value: inout CGFloat?, nextValue: () -> CGFloat?) {
        if let next = nextValue() { value = next }
    }
}

// MARK: - UIKit recognizers (iphone-scroll-perf, 2026-10-27)
//
// The horizontal gestures are UIKit recognizers that decide direction on the first points
// and fail at once otherwise (a SwiftUI `DragGesture` cannot fail: once it reaches its
// minimum distance it runs for the whole touch whatever the direction). Both are exclusive
// with the scroll view's pan — the first to begin wins, nothing waits on anything.

/// What a horizontal drag turned out to be, decided once at `BackSwipe.lockDistance`.
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
    var minimumDistance: CGFloat = BackSwipe.lockDistance
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

/// What the transcript's swipe recognizer tells the chat screen.
enum TranscriptSwipeEvent {
    /// The time peek follows the finger: `dx` is the translation (negative = left).
    case timeChanged(dx: CGFloat)
    case timeEnded
    /// A clear swipe right on the transcript, away from the edge, ended far enough: back.
    case back
}

/// One recognizer on the transcript's `UIScrollView` for the two horizontal swipes inside
/// it — swipe left anywhere on the transcript peeks the message times, swipe right (away
/// from the edge zone, which is the system pop's) is "back" — instead of SwiftUI drags.
/// Sits as the `.background` of the ScrollView's content. It is on the scroll view itself,
/// beside the scroll pan, so the two decide on equal footing; a recognizer on an ancestor
/// of the scroll view does not get a horizontal pan that starts inside it (rig, 2026-10-28).
/// Declines every vertical pan (the scroll's).
struct TranscriptSwipeHost: UIViewRepresentable {
    /// Install the time peek / the swipe-right back (the Debug scroll rig can leave one out).
    var timePeek = true
    var backSwipe = true
    let onEvent: (TranscriptSwipeEvent) -> Void
    /// The scroll view's own pan began (a finger scroll started).
    let onScrollPanBegan: () -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(timePeek: timePeek, backSwipe: backSwipe, onEvent: onEvent, onScrollPanBegan: onScrollPanBegan)
    }

    func makeUIView(context: Context) -> HostView {
        let v = HostView()
        v.isUserInteractionEnabled = false
        v.backgroundColor = .clear
        v.coordinator = context.coordinator
        return v
    }

    func updateUIView(_ uiView: HostView, context: Context) {
        context.coordinator.onEvent = onEvent
        context.coordinator.onScrollPanBegan = onScrollPanBegan
    }

    final class HostView: UIView {
        weak var coordinator: Coordinator?

        override func didMoveToWindow() {
            super.didMoveToWindow()
            guard window != nil else { return }
            var node: UIView? = superview
            while let current = node {
                if let scroll = current as? UIScrollView {
                    coordinator?.attach(to: scroll)
                    return
                }
                node = current.superview
            }
        }
    }

    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        let timePeek: Bool
        let backSwipe: Bool
        var onEvent: (TranscriptSwipeEvent) -> Void
        var onScrollPanBegan: () -> Void
        private weak var scroll: UIScrollView?
        private var recognizer: DirectionalSwipeRecognizer?
        private enum Mode { case time, back }
        private var mode: Mode?

        init(timePeek: Bool, backSwipe: Bool, onEvent: @escaping (TranscriptSwipeEvent) -> Void, onScrollPanBegan: @escaping () -> Void) {
            self.timePeek = timePeek
            self.backSwipe = backSwipe
            self.onEvent = onEvent
            self.onScrollPanBegan = onScrollPanBegan
        }

        func attach(to scroll: UIScrollView) {
            guard self.scroll !== scroll else { return }
            self.scroll?.panGestureRecognizer.removeTarget(self, action: #selector(scrollPan(_:)))
            if let old = recognizer { old.view?.removeGestureRecognizer(old) }
            self.scroll = scroll
            scroll.panGestureRecognizer.addTarget(self, action: #selector(scrollPan(_:)))
            guard timePeek || backSwipe else { return }
            let gr = DirectionalSwipeRecognizer(target: self, action: #selector(handle(_:)))
            gr.delegate = self
            gr.shouldStart = { [weak scroll] touch, start in
                guard let scroll else { return false }
                // The edge zone is the system pop's from touch-down.
                if BackSwipe.startsAtEdge(scroll.convert(start, to: nil)) { return false }
                // A markdown table scrolling sideways is its own scroll view's pan.
                var node: UIView? = touch.view
                while let current = node, current !== scroll {
                    if let s = current as? UIScrollView, s.contentSize.width > s.bounds.width + 1 { return false }
                    node = current.superview
                }
                return true
            }
            gr.decide = { [weak self] _, translation in
                guard let self else { return .declined }
                let dx = translation.width, dy = translation.height
                guard abs(dx) > abs(dy) * 1.15 else { return .declined }
                if dx < 0 {
                    guard self.timePeek else { return .declined }
                    self.mode = .time
                } else {
                    guard self.backSwipe else { return .declined }
                    self.mode = .back
                }
                return .claimed
            }
            scroll.addGestureRecognizer(gr)
            recognizer = gr
        }

        @objc private func scrollPan(_ gr: UIPanGestureRecognizer) {
            if gr.state == .began { onScrollPanBegan() }
        }

        @objc private func handle(_ gr: DirectionalSwipeRecognizer) {
            guard let mode else { return }
            switch (mode, gr.state) {
            case (.time, .began), (.time, .changed):
                onEvent(.timeChanged(dx: gr.translation.width))
            case (.time, .ended), (.time, .cancelled), (.time, .failed):
                onEvent(.timeEnded)
                self.mode = nil
            case (.back, .ended):
                if BackSwipe.fullScreenPops(translation: gr.translation, velocity: gr.velocity) {
                    #if DEBUG
                    SwipeLog.event("transcript back dx=\(Int(gr.translation.width))")
                    #endif
                    onEvent(.back)
                }
                self.mode = nil
            case (.back, .cancelled), (.back, .failed):
                self.mode = nil
            case (.back, .began), (.back, .changed), (_, .possible):
                break
            @unknown default:
                break
            }
        }

        /// Exclusive with every pan (the scroll's above all), shared with the presses and
        /// taps inside the transcript — a sender's avatar is a `Button`, and its press
        /// recognizer would otherwise prevent this one (`BackSwipe.sharesTouch`).
        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
            BackSwipe.sharesTouch(with: other)
        }
    }
}

/// Hides the system bar and back button and owns the back gestures: the system's
/// interactive edge pop (`NativePopEnabler`) and the swipe-right outside the transcript —
/// header, empty space, the channel list (`FullScreenBackHost` → `dismiss()`; SwiftUI pops,
/// so the stack's path stays the truth). Inside the transcript the swipe-right is the
/// transcript recognizer's (`TranscriptSwipeHost`, `.back`).
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
                NativePopEnabler()
                if ChatGestures.backSwipe {
                    FullScreenBackHost(composerTop: composerTop, onBack: { dismiss() })
                }
            }
    }
}

/// `.navigationBarBackButtonHidden(true)` / a hidden navigation bar turn the system's
/// interactive pop gesture off; this turns it back on for the enclosing navigation
/// controller and is its delegate: it begins whenever the stack has something to pop to,
/// keyboard open or closed, and the keyboard goes down as it begins. SwiftUI may switch
/// the recognizer off again on its own appearance passes; the enabler re-applies on every
/// appearance and update, and the recognizer's `isEnabled` is watched so a late flip is
/// undone (one write back, never a loop: the flip is SwiftUI's, the re-enable ours).
private struct NativePopEnabler: UIViewControllerRepresentable {
    func makeUIViewController(context: Context) -> Controller { Controller() }

    func updateUIViewController(_ uiViewController: Controller, context: Context) {
        uiViewController.apply()
    }

    final class Controller: UIViewController, UIGestureRecognizerDelegate {
        private weak var pop: UIGestureRecognizer?
        private var enabledWatch: NSKeyValueObservation?
        private var onScreen = false

        override func viewWillAppear(_ animated: Bool) {
            super.viewWillAppear(animated)
            onScreen = true
            apply()
        }

        override func viewDidAppear(_ animated: Bool) {
            super.viewDidAppear(animated)
            onScreen = true
            apply()
        }

        override func viewWillDisappear(_ animated: Bool) {
            super.viewWillDisappear(animated)
            onScreen = false
            // Leave the navigation controller able to pop after this screen is gone.
            pop?.isEnabled = true
        }

        func apply() {
            guard let nav = enclosingNavigationController(), let gr = nav.interactivePopGestureRecognizer else { return }
            if pop !== gr {
                pop?.removeTarget(self, action: #selector(popChanged(_:)))
                pop = gr
                gr.addTarget(self, action: #selector(popChanged(_:)))
                enabledWatch = gr.observe(\.isEnabled, options: [.new]) { [weak self] gr, change in
                    guard let self, self.onScreen, change.newValue == false else { return }
                    DispatchQueue.main.async { if self.onScreen, !gr.isEnabled { gr.isEnabled = true } }
                }
            }
            gr.isEnabled = true
            gr.delegate = self
        }

        @objc private func popChanged(_ gr: UIGestureRecognizer) {
            if gr.state == .began {
                BackSwipe.dismissKeyboard()
            }
            #if DEBUG
            SwipeLog.event("system pop state=\(gr.state.rawValue)")
            #endif
        }

        func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
            let depth = enclosingNavigationController()?.viewControllers.count ?? 0
            #if DEBUG
            SwipeLog.event("system pop shouldBegin depth=\(depth)")
            #endif
            return depth > 1
        }

        /// Background representables sometimes have a nil `navigationController`; walk the
        /// parents and the responder chain to find the stack that pushed this screen.
        private func enclosingNavigationController() -> UINavigationController? {
            if let nav = navigationController { return nav }
            var parent = self.parent
            while let current = parent {
                if let nav = current as? UINavigationController { return nav }
                if let nav = current.navigationController { return nav }
                parent = current.parent
            }
            var responder: UIResponder? = viewIfLoaded
            while let current = responder {
                if let nav = current as? UINavigationController { return nav }
                if let vc = current as? UIViewController, let nav = vc.navigationController { return nav }
                responder = current.next
            }
            return nil
        }
    }
}

/// The full-screen swipe-right → back. The host view is the `.background` of the screen's
/// root content, so its bounds are `BackSwipe.space`; the recognizer itself goes on the
/// nearest ancestor that is a view controller's view (touches on the content never reach a
/// sibling). It fails at once on an edge-zone start (the system pop's), on a composer-bar
/// start, and on anything but a clear rightward pan, so it never competes with the pop,
/// the scroll or the time peek.
private struct FullScreenBackHost: UIViewRepresentable {
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
        private var recognizer: DirectionalSwipeRecognizer?

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
            gr.shouldStart = { [weak root, weak host] _, start in
                guard let root, let host else { return false }
                // The edge zone is the system pop's from touch-down.
                return !BackSwipe.startsAtEdge(host.convert(start, from: root))
            }
            gr.decide = { [weak self, weak root, weak host] start, translation in
                guard let self, let root, let host else { return .declined }
                return BackSwipe.fullScreenLock(translation: translation, start: host.convert(start, from: root),
                                                composerTop: self.composerTop)
            }
            root.addGestureRecognizer(gr)
            recognizer = gr
        }

        @objc private func handle(_ gr: DirectionalSwipeRecognizer) {
            guard gr.state == .ended else { return }
            if BackSwipe.fullScreenPops(translation: gr.translation, velocity: gr.velocity) {
                #if DEBUG
                SwipeLog.event("full-screen back dx=\(Int(gr.translation.width))")
                #endif
                BackSwipe.dismissKeyboard()
                onBack()
            }
        }

        /// Exclusive with every pan: the scroll pan, the time peek and the system pop
        /// (which never shares a touch with this one anyway — edge starts fail here on
        /// touch-down). Shared with the presses and taps on the screen, so the header
        /// pill's avatar and the `Button`s drawn over the transcript cannot prevent it
        /// (`BackSwipe.sharesTouch`).
        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
            BackSwipe.sharesTouch(with: other)
        }
    }
}

extension View {
    /// Custom-back screen: the system's interactive edge pop, and a swipe right anywhere
    /// outside the composer bar pops it.
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

#if DEBUG
/// Rig log for the back gestures (`ARBOS_SWIPE_LOG=1`, category `swipe`): the system pop's
/// delegate calls and state changes, and the full-screen back's fires.
enum SwipeLog {
    private static let log = Logger(subsystem: "com.unarbos.arbos.ios", category: "swipe")
    private static let on = ProcessInfo.processInfo.environment["ARBOS_SWIPE_LOG"] == "1"

    static func event(_ what: String) {
        guard on else { return }
        log.notice("\(what, privacy: .public)")
    }
}
#endif
