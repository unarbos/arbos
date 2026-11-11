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
//    channel list) is also "back" (since 7d2d3797, 2026-10-16), and it is interactive in the
//    same way the edge pop is: the screen starts moving on the first rightward millimetre and
//    follows the finger 1:1 until it is let go (`InteractiveBackPop`). Never from the composer
//    bar (a drag there selects text or slides the mic) and never from the edge zone, which is
//    the system's. `FullScreenBackHost`, a `DirectionalSwipeRecognizer` that fails at once on
//    anything but a clear rightward pan.
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
// #2530 and #2533 then widened where the swipe may start and what it pops, and left the rule
// itself alone: outside the edge zone the drag still only ever resolved on touch-up (Jacob,
// 2026-11-03: "i swipe and then it doesn't register until i let go … should register on first
// touch and then as i slide my finger across the chat slides out with my finger"). The release
// now only decides whether to finish or to put the screen back — the movement is the drag's.
// The drag is measured in the WINDOW and not in the recognizer's own view, because the pop
// moves that view out from under the measurement on its very first frame; see
// `DirectionalSwipeRecognizer.startInWindow`, which is the whole of that bug.
//
// More than once a bug here has been invisible in a screenshot, and for the same reason each
// time: `UIView.animate` writes the end state to the model layer immediately and animates
// only the presentation. So geometry that is being animated reads as already finished — to a
// gesture measuring itself against it, and to a person looking at the screen. When this
// screen misbehaves, suspect the model layer before the pixels.
//
// And then measure it rather than reason about it. Every wrong answer this file has had came
// from a confident account of what UIKit does, and every right one came from a number:
// `BackSwipeTrace.probe` prints the model transform, the presentation transform, the
// animation count and the layer speed side by side for exactly that reason. An assertion here
// that can only report pass or fail will cost an hour of gate time and teach nothing.

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

    /// Must `other` wait for a back gesture to decide? Only a scroll view's own pan, and only
    /// while that scroll view is still gliding, so "back" works on a chat that is moving
    /// (2026-11-09: "this should be able to swipe out while moving").
    ///
    /// A finger on a decelerating scroll view catches it, and UIKit then begins the scroll's
    /// pan on the first move (translation 0, ~30 ms after touch-down) instead of after its usual
    /// ~10 pt. That is before either back recognizer has moved `lockDistance`, so the pan
    /// prevented them both, and the system's edge pop too, until the list stood still.
    /// Measured with `ARBOS_SWIPE_LOG=1`: `scroll pan began t=(0,0) decel=true` 32 ms in, and
    /// no back recognizer ever reached its decision.
    ///
    /// Waiting costs a vertical drag at most `lockDistance` (a back gesture fails there), and
    /// only on a list that was already moving. At rest nothing waits, as before.
    static func waitsForBack(_ other: UIGestureRecognizer) -> Bool {
        guard let scroll = other.view as? UIScrollView, other === scroll.panGestureRecognizer else { return false }
        // A markdown table gliding sideways keeps its own pan: a second flick scrolls it on,
        // it does not leave the chat.
        if scroll.contentSize.width > scroll.bounds.width + 1 { return false }
        return scroll.isDecelerating
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

    /// A hold just opened the message menu. The row's long-press runs alongside the
    /// picture's tap, and a tap has no time limit, so the finger that held also "taps"
    /// when it lifts — and the media viewer covered the menu it had just opened. That one
    /// tap is spent here; closing the menu clears it, so a hold that ended without a tap
    /// never eats the next real one.
    static var heldIntoMenu = false

    /// True (once) when this tap is the end of the hold that opened the menu.
    static func tapEndsHold() -> Bool {
        defer { heldIntoMenu = false }
        return heldIntoMenu
    }

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

/// A claimed back drag, as data: what the screen does at each touch sample. Unit-tested in
/// `BackDragTests`, played by `BackSwipePlayer`.
///
/// Every sample while the finger is down answers `.track`, so the screen moves then; only the
/// release answers `.finish` or `.cancel`. That order is the fix (Jacob, 2026-11-03: "i swipe
/// and then it doesn't register until i let go … as i slide my finger across the chat slides
/// out with my finger") — before it every sample but the last answered nothing at all, and a
/// swipe was a threshold read once, at touch-up.
///
/// **What letting go does is not changed here.** The release still asks `fullScreenPops`, the
/// same question with the same numbers it has always had, so this moves only *when* the screen
/// moves and never *whether* the swipe commits. Where it commits is Jacob's to set — about
/// three quarters of the way across, with a velocity escape, both as constants he can feel on
/// his own phone (2026-11-01: "telegram registers back nearly at the full screen let's go with
/// that") — and it is a separate build. A threshold invented here would be a second change in
/// a one-fix build, and that build would have to undo it.
struct BackDrag: Equatable {
    /// The travel of a complete pop: the screen's width, so the page follows the finger 1:1.
    var width: CGFloat

    enum Step: Equatable {
        /// Nothing to draw: the drag has not been claimed yet.
        case idle
        /// The screen sits `progress` (0…1) of the way out.
        case track(progress: CGFloat)
        /// Let go past the line (`BackSwipe.fullScreenPops`): run the pop out.
        case finish
        /// Let go short of it, or the touch was taken away: put the screen back.
        case cancel
    }

    func step(for state: UIGestureRecognizer.State, translation: CGSize, velocity: CGSize) -> Step {
        switch state {
        case .began, .changed:
            return .track(progress: Self.progress(dx: translation.width, width: width))
        case .ended:
            return BackSwipe.fullScreenPops(translation: translation, velocity: velocity) ? .finish : .cancel
        case .cancelled, .failed:
            return .cancel
        case .possible:
            return .idle
        @unknown default:
            return .idle
        }
    }

    /// How far out the screen is: the finger's own travel, clamped to the screen.
    static func progress(dx: CGFloat, width: CGFloat) -> CGFloat {
        guard width > 0 else { return 0 }
        return min(1, max(0, dx / width))
    }
}

/// A pan that decides its direction on the first points and otherwise fails at once. Not a
/// `UIPanGestureRecognizer`: that one begins at its own ~10 pt in any direction, before the
/// direction is known. Exclusive with the scroll view's pan (no `require(toFail:)` either way):
/// a vertical pan scrolls the instant the scroll view says so; this one has failed by then.
/// The one exception is a scroll view that is still gliding (`BackSwipe.waitsForBack`).
final class DirectionalSwipeRecognizer: UIGestureRecognizer {
    /// Decide once the finger has moved `minimumDistance` (`start` in the recognizer's view):
    /// `.claimed` begins, `.declined` fails, `.undecided` asks again on the next move.
    var decide: (_ start: CGPoint, _ translation: CGSize) -> SwipeClaim = { _, _ in .declined }
    /// Movement (|dx| + |dy|) before `decide` is first asked.
    var minimumDistance: CGFloat = BackSwipe.lockDistance
    /// Fail on touch-down when the finger landed somewhere this gesture must leave alone.
    var shouldStart: (_ touch: UITouch, _ start: CGPoint) -> Bool = { _, _ in true }

    /// Where the finger landed, in the recognizer's own view. Only the two callbacks above
    /// read it, and both want it there: the composer bar and the edge zone are measured in
    /// the screen's space.
    private(set) var start = CGPoint.zero
    /// How far the finger has moved, **in the window**, which is the only frame in this view
    /// hierarchy that a running transition cannot move. See `startInWindow`.
    private(set) var translation = CGSize.zero
    /// Points per second over the last two samples, in the window for the same reason.
    private(set) var velocity = CGSize.zero
    /// Where the finger landed, in the window.
    ///
    /// Measuring the drag in the recognizer's own view is what broke the interactive pop, and
    /// it is worth spelling out because the bug is invisible in the recognizer: the moment the
    /// pop begins, `BackPopAnimator` moves the leaving screen to its finished position — by
    /// transforming the box it puts the screen in — because that is what `UIView.animate` does:
    /// the model value jumps to the end state and only the presentation layer is animated. The
    /// recognizer sits on that screen, and an ancestor moving carries it just as its own
    /// transform did.
    /// So from the second touch sample onwards `location(in: view)` was read in a frame that
    /// had already slid a whole screen width to the right, the translation collapsed to about
    /// minus one screen, and a rightward drag read as a leftward one: the screen moved on the
    /// first sample, snapped back, and the release cancelled. The window never moves.
    private var startInWindow = CGPoint.zero
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
        startInWindow = t.location(in: nil)
        lastPoint = startInWindow
        lastTime = t.timestamp
        translation = .zero
        velocity = .zero
        if !shouldStart(t, start) { state = .failed }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent) {
        guard let t = tracked, touches.contains(t) else { return }
        let p = t.location(in: nil)
        let dt = t.timestamp - lastTime
        if dt > 0 {
            velocity = CGSize(width: (p.x - lastPoint.x) / dt, height: (p.y - lastPoint.y) / dt)
        }
        lastPoint = p
        lastTime = t.timestamp
        translation = CGSize(width: p.x - startInWindow.x, height: p.y - startInWindow.y)
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
        startInWindow = .zero
        translation = .zero
        velocity = .zero
    }
}

// MARK: - The interactive pop (Jacob, 2026-11-03)

extension UIView {
    /// The navigation stack this view sits in, through the responder chain. A representable's
    /// view usually has no view controller of its own, and SwiftUI's `background` hosts hang
    /// off the screen's content rather than off its controller.
    func enclosingNavigationStack() -> UINavigationController? {
        var responder: UIResponder? = self
        while let current = responder {
            if let nav = current as? UINavigationController { return nav }
            if let vc = current as? UIViewController, let nav = vc.navigationController { return nav }
            responder = current.next
        }
        return nil
    }
}

/// UIKit's own pop, scrubbed by the finger instead of played at a fixed speed: the screen
/// slides out, the page underneath comes in from a third of the way back behind a dimming
/// layer, and the whole thing runs forwards or backwards as the drag moves (Telegram's
/// half-swipe, which is also what the system's edge pop does).
///
/// This is UIKit's standard interactive-transition machinery — a `UIPercentDrivenInteractiveTransition`
/// handed to the navigation stack with an animator — and not the edge pop itself, which is a
/// `UIScreenEdgePanGestureRecognizer` and cannot be asked to read a drag that started in the
/// middle of the screen.
///
/// The stack's existing delegate is kept and everything but the two transition questions is
/// forwarded to it, because that delegate is SwiftUI's: `NavigationStack` learns that a screen
/// was popped through it, and replacing it outright would leave `path` holding a screen that
/// is no longer on the stack.
final class InteractiveBackPop: NSObject, UINavigationControllerDelegate {
    /// One driver per stack, so two chat screens on the same stack cannot chain their
    /// delegates into a loop. Weak keys: the entry goes when the stack does.
    private static let drivers = NSMapTable<UINavigationController, InteractiveBackPop>.weakToStrongObjects()

    /// Whether `view` is on a stack with something to pop to. The release-only fallback is
    /// for a screen that is not — never for a second recognizer that reached a stack another
    /// one is already driving, which would pop two levels for one swipe.
    static func available(from view: UIView) -> Bool {
        (view.enclosingNavigationStack()?.viewControllers.count ?? 0) > 1
    }

    /// Begin an interactive pop of the stack `view` is in, or nil when there is nothing to
    /// pop to, the stack is already animating, or the view is not in a stack at all.
    static func start(from view: UIView) -> InteractiveBackPop? {
        guard let nav = view.enclosingNavigationStack(), nav.viewControllers.count > 1,
              nav.transitionCoordinator == nil else { return nil }
        let driver = drivers.object(forKey: nav) ?? {
            let made = InteractiveBackPop(nav: nav)
            drivers.setObject(made, forKey: nav)
            return made
        }()
        return driver.begin() ? driver : nil
    }

    private weak var nav: UINavigationController?
    /// SwiftUI's delegate, which keeps `NavigationStack.path` in step with the stack.
    private weak var host: UINavigationControllerDelegate?
    /// The running transition.
    ///
    /// **This is held until UIKit says the transition is over, and not until the finger comes
    /// up.** Letting go of it at `finish()` or `cancel()` is the obvious thing to write and it
    /// breaks the screen: UIKit does not own the interaction controller, the delegate does, so
    /// releasing it mid-flight deallocates it while the remainder of the pop is still playing.
    /// The animation then never reaches its completion, `completeTransition(_:)` is never
    /// called, and the navigation controller never re-enables user interaction on its view —
    /// which it turned off when the transition began.
    ///
    /// The damage is invisible in a screenshot, for the same reason the first bug in this file
    /// was: `UIView.animate` writes the end state to the model layer immediately, so every view
    /// is sitting at exactly the right place and the screen looks finished. It is simply dead
    /// to touch.
    private var interaction: UIPercentDrivenInteractiveTransition?
    /// Whether the transition UIKit is asking about is ours. Separate from `interaction`
    /// because that now outlives the drag, and a later pop UIKit runs by itself — a back
    /// button, a programmatic one — must still get SwiftUI's animation.
    private var driving = false

    /// How far the screen travels on a complete pop.
    var width: CGFloat { nav?.view.bounds.width ?? 0 }

    private init(nav: UINavigationController) {
        self.nav = nav
    }

    private func begin() -> Bool {
        guard let nav, !driving else { return false }
        if nav.delegate !== self {
            host = nav.delegate
            nav.delegate = self
        }
        let drive = UIPercentDrivenInteractiveTransition()
        drive.completionCurve = .easeOut
        interaction = drive
        driving = true
        nav.popViewController(animated: true)
        if let coordinator = nav.transitionCoordinator {
            _ = coordinator.animate(alongsideTransition: nil) { [weak self] _ in self?.settle() }
        } else {
            // No transition started, so nothing will ever come back to say one ended. Letting
            // `driving` stand here would wedge every later swipe.
            settle()
        }
        #if DEBUG
        SwipeLog.event("interactive pop began depth=\(nav.viewControllers.count)")
        #endif
        return true
    }

    func update(_ progress: CGFloat) {
        interaction?.update(progress)
    }

    func finish() {
        // A drag let go at the far edge leaves almost no transition to play out, and UIKit
        // plays that remainder at full speed as one hitched frame; just under 1 keeps it smooth.
        interaction?.completionSpeed = 0.99
        interaction?.finish()
    }

    func cancel() {
        interaction?.cancel()
    }

    /// UIKit has finished with the transition, whichever way it went: let the interaction
    /// controller go and let the next drag start one.
    private func settle() {
        interaction = nil
        driving = false
        #if DEBUG
        // What the transition left behind. A pop that ends with the screen drawn correctly and
        // dead to touch has happened here twice, and from the outside the two reasons for it —
        // interaction never switched back on, or something left lying over the screen — look
        // exactly alike. This says which, in the one place that can still see both.
        if let nav {
            let live = { (view: UIView?) in view.map { $0.isUserInteractionEnabled ? "1" : "0" } ?? "-" }
            BackSwipeTrace.note("live:nav=\(live(nav.view))"
                + ",win=\(live(nav.view.window))"
                + ",top=\(live(nav.topViewController?.view))"
                + ",stack=\(nav.viewControllers.count)")
            // Named, not counted. "kids=2" was true on every red run and said nothing: a
            // navigation controller holds one content view, so the question has always been
            // what the other one is, and a number cannot answer that.
            let kids = nav.view.subviews.map { child in
                String(describing: type(of: child)) + "@" + String(Int(child.transform.tx))
                    + (child.isHidden ? ":hidden" : "")
            }
            BackSwipeTrace.note("kids:" + kids.joined(separator: ","))
        }
        #endif
    }

    func navigationController(_ navigationController: UINavigationController,
                              animationControllerFor operation: UINavigationController.Operation,
                              from fromVC: UIViewController,
                              to toVC: UIViewController) -> UIViewControllerAnimatedTransitioning? {
        if driving, operation == .pop { return BackPopAnimator() }
        return host?.navigationController?(navigationController, animationControllerFor: operation, from: fromVC, to: toVC)
    }

    func navigationController(_ navigationController: UINavigationController,
                              interactionControllerFor animationController: UIViewControllerAnimatedTransitioning)
        -> UIViewControllerInteractiveTransitioning? {
        if driving, let interaction { return interaction }
        return host?.navigationController?(navigationController, interactionControllerFor: animationController)
    }

    override func responds(to aSelector: Selector!) -> Bool {
        super.responds(to: aSelector) || (host?.responds(to: aSelector) ?? false)
    }

    override func forwardingTarget(for aSelector: Selector!) -> Any? {
        host?.responds(to: aSelector) == true ? host : nil
    }
}

/// The pop UIKit draws for a pushed screen, written out so an interactive transition can
/// scrub it: the screen leaves to the right, the page underneath comes from `parallax` of a
/// screen back, and a dimming layer over it clears as it arrives. Linear on purpose — a
/// percent-driven transition sets the time itself, and any other curve fights the finger.
final class BackPopAnimator: NSObject, UIViewControllerAnimatedTransitioning {
    static let duration: TimeInterval = 0.35
    /// How far behind the outgoing screen the page underneath starts, as a fraction of it.
    static let parallax: CGFloat = 0.3

    func transitionDuration(using transitionContext: UIViewControllerContextTransitioning?) -> TimeInterval {
        Self.duration
    }

    func animateTransition(using context: UIViewControllerContextTransitioning) {
        guard let arriving = context.viewController(forKey: .to),
              let leavingView = context.view(forKey: .from),
              let arrivingView = context.view(forKey: .to) else {
            context.completeTransition(false)
            return
        }
        let container = context.containerView
        let width = container.bounds.width

        arrivingView.frame = context.finalFrame(for: arriving)
        container.insertSubview(arrivingView, at: 0)
        arrivingView.transform = CGAffineTransform(translationX: -width * Self.parallax, y: 0)

        let dim = UIView(frame: container.bounds)
        dim.backgroundColor = .black
        dim.alpha = 0.12
        dim.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        container.insertSubview(dim, aboveSubview: arrivingView)

        // What moves is a container put up for the transition, not the leaving screen itself.
        //
        // Moving the screen directly is what the first version of this animator did, and it cost
        // the open chat its accessibility (AF-834): every bubble came back from a cancelled drag
        // reported one screen width to the right of where it was drawn, so VoiceOver read the
        // chat as off-screen and XCUITest could not compute a hit point on it. `UIView.animate`
        // writes the end state to the model layer immediately and animates the presentation
        // layer away from it, and SwiftUI publishes accessibility frames from the model layer —
        // so for the length of the transition, and after a cancel for as long as nothing
        // republishes, the frames say where the screen was going rather than where it is.
        //
        // Moving a parent instead fixes it because SwiftUI does not read an ancestor's transform
        // at all: the frames stay at the resting position throughout, which is where the screen
        // ends up on a cancel and is the only answer that is ever right for a screen that is
        // about to be gone. Five arms off one build say it is this and nothing else — moving by
        // `center` instead of `transform` reproduces the defect just as the transform does, and
        // so do both halves of the debug trace that was accidentally masking it. The three ways
        // of moving the screen were also photographed mid-drag and are byte-identical, so this
        // is invisible to the eye. `internal/af834-voiceover-bubble-frames.md`.
        let box = UIView(frame: container.bounds)
        box.backgroundColor = .clear
        container.addSubview(box)
        leavingView.frame = box.bounds
        box.addSubview(leavingView)

        // The shadow stays on the screen the eye sees, not on the box carrying it.
        let layer = leavingView.layer
        let shadow = (layer.shadowColor, layer.shadowOpacity, layer.shadowRadius, layer.shadowOffset)
        layer.shadowColor = UIColor.black.cgColor
        layer.shadowOpacity = 0.18
        layer.shadowRadius = 8
        layer.shadowOffset = CGSize(width: -3, height: 0)

        UIView.animate(withDuration: transitionDuration(using: context), delay: 0, options: .curveLinear) {
            box.transform = CGAffineTransform(translationX: width, y: 0)
            arrivingView.transform = .identity
            dim.alpha = 0
        } completion: { _ in
            #if DEBUG
            // Before this block touches anything, so the state the transition left behind is
            // on the record whether or not the cleanup below turns out to be right.
            BackSwipeTrace.probe("left", leavingView)
            #endif
            dim.removeFromSuperview()
            arrivingView.transform = .identity
            (layer.shadowColor, layer.shadowOpacity, layer.shadowRadius, layer.shadowOffset) = shadow
            // The box comes down and the screen goes back to the container, before the lines
            // below decide which of the two screens this animator leaves behind.
            leavingView.transform = .identity
            container.addSubview(leavingView)
            leavingView.frame = container.bounds
            box.removeFromSuperview()
            let cancelled = context.transitionWasCancelled
            // Whichever view this animator leaves behind, it takes out of the container
            // itself. UIKit does not promise to tidy up after a transition it did not run,
            // and a leftover full-screen view here is invisible — its content is gone — and
            // solid to every touch.
            if cancelled {
                arrivingView.removeFromSuperview()
            } else {
                leavingView.removeFromSuperview()
            }
            context.completeTransition(!cancelled)
            // What used to be here was a defensive `leavingView.transform = .identity`, kept
            // because this screen had been through three attempted fixes that all rewrote it.
            // Nothing sets that transform any more — the box above carries the movement — so
            // the teardown resets it once and there is nothing left for this line to undo.
            #if DEBUG
            BackSwipeTrace.probe("rest", leavingView)
            BackSwipeTrace.note(cancelled ? "done:back" : "done:popped")
            // And once more after UIKit, SwiftUI and the navigation controller have all had
            // their turn, because the question is what the screen settles at, not what this
            // block leaves. The hit test names whatever is on top at the middle of the
            // window, which is where the chat list's rows are.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak leavingView] in
                BackSwipeTrace.probe("settled", leavingView)
                if let window = BackSwipeTrace.window {
                    BackSwipeTrace.hitTest("mid", CGPoint(x: window.bounds.midX,
                                                          y: window.bounds.midY))
                    // From the window and not from the leaving view, because which view is
                    // carrying the offset is the open question and starting the walk at a
                    // guess would beg it.
                    BackSwipeTrace.displaced("off", from: window)
                }
            }
            #endif
        }
    }
}

/// Plays one `DirectionalSwipeRecognizer`'s rightward drag as an interactive pop. Both back
/// recognizers — the screen's and the transcript's — own one of these, and each call is one
/// touch sample.
///
/// `fallback` is the old release-only behaviour and runs only when there is no stack to pop
/// interactively (a chat put on screen some other way): nothing can follow the finger there,
/// so letting go must still go back.
final class BackSwipePlayer {
    private var pop: InteractiveBackPop?
    private var drag: BackDrag?
    private var onAStack = false

    func play(_ recognizer: DirectionalSwipeRecognizer, in view: UIView, fallback: () -> Void) {
        // The first sample of a drag opens the transition; the width it travels is the
        // stack's, not this view's, so a transcript inset from the screen edge still tracks 1:1.
        if drag == nil, recognizer.state == .began || recognizer.state == .changed {
            // The keyboard goes down as the pop begins, as it does under the edge pop.
            BackSwipe.dismissKeyboard()
            onAStack = InteractiveBackPop.available(from: view)
            pop = InteractiveBackPop.start(from: view)
            let stack = pop?.width ?? 0
            drag = BackDrag(width: stack > 0 ? stack : (view.window?.bounds.width ?? view.bounds.width))
            #if DEBUG
            BackSwipeTrace.start()
            #endif
        }
        guard let drag else { return }
        switch drag.step(for: recognizer.state, translation: recognizer.translation, velocity: recognizer.velocity) {
        case .idle:
            break
        case .track(let progress):
            pop?.update(progress)
            #if DEBUG
            BackSwipeTrace.track(progress)
            #endif
        case .finish:
            #if DEBUG
            BackSwipeTrace.note("finish")
            #endif
            if let pop {
                pop.finish()
            } else if !onAStack {
                fallback()
            }
            reset()
        case .cancel:
            #if DEBUG
            // "taken" rather than "cancel" when the touch was pulled out from under the drag
            // instead of being let go. The two are one answer here and very different faults
            // to read back off a rig.
            BackSwipeTrace.note(recognizer.state == .ended ? "cancel" : "taken")
            #endif
            pop?.cancel()
            reset()
        }
    }

    private func reset() {
        pop = nil
        drag = nil
        onAStack = false
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
        private let backPop = BackSwipePlayer()

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
            case (.back, _):
                // The screen follows the finger from the first sample; `onEvent(.back)` is
                // only reached when there is no stack to pop interactively.
                guard let scroll else { return }
                backPop.play(gr, in: scroll) {
                    #if DEBUG
                    SwipeLog.event("transcript back dx=\(Int(gr.translation.width))")
                    #endif
                    onEvent(.back)
                }
                if gr.state == .ended || gr.state == .cancelled || gr.state == .failed { self.mode = nil }
            case (_, .possible):
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

        /// A gliding scroll waits for this one to decide (`BackSwipe.waitsForBack`).
        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldBeRequiredToFailBy other: UIGestureRecognizer) -> Bool {
            BackSwipe.waitsForBack(other)
        }
    }
}

/// Hides the system bar and back button and owns the back gestures: the system's
/// interactive edge pop (`NativePopEnabler`) and the swipe-right outside the transcript —
/// header, empty space, the channel list (`FullScreenBackHost`, which drives an
/// `InteractiveBackPop` and falls back to `dismiss()` only off a navigation stack). Inside
/// the transcript the swipe-right is the transcript recognizer's (`TranscriptSwipeHost`,
/// `.back`), and it drives the same pop.
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

        /// A gliding scroll waits for the edge pop too (`BackSwipe.waitsForBack`).
        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldBeRequiredToFailBy other: UIGestureRecognizer) -> Bool {
            BackSwipe.waitsForBack(other)
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
        private let backPop = BackSwipePlayer()

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
            guard let root else { return }
            // The screen follows the finger from the first sample. `onBack()` — SwiftUI's
            // `dismiss()`, which animates at its own speed — is left for the one case the
            // interactive pop cannot serve: a screen that is not on a navigation stack.
            backPop.play(gr, in: root) {
                #if DEBUG
                SwipeLog.event("full-screen back dx=\(Int(gr.translation.width))")
                #endif
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

        /// A gliding scroll waits for this one to decide (`BackSwipe.waitsForBack`).
        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldBeRequiredToFailBy other: UIGestureRecognizer) -> Bool {
            BackSwipe.waitsForBack(other)
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
/// Rig (`ARBOS_SWIPE_TRACE=1`): what a back drag did, in order, read off the
/// `back-swipe-trace` element — `track:<progress>` as the screen moves, then `finish` or
/// `cancel` for a release, or `taken` for a touch pulled out from under the drag.
///
/// It exists because the two things this fix is between look identical once the finger is
/// up: a screen that followed the finger the whole way, and a screen that jumped at
/// touch-up. Only the order of the entries tells them apart, and a UI test cannot read the
/// screen mid-drag — XCUITest's drag call does not return until the touch is lifted.
///
/// Off unless the variable is set, so no other run pays for it, and Debug-only.
enum BackSwipeTrace {
    static let on = ProcessInfo.processInfo.environment["ARBOS_SWIPE_TRACE"] == "1"
    /// The label the rig reads, held on the WINDOW rather than on a screen. It used to belong
    /// to whichever screen was on top, which failed at the one question worth asking: gate run
    /// 41747877523 asked what was covering Home after the chat popped, and the readout had
    /// popped with the chat. A witness that leaves with the suspect is no witness.
    private static var readout: UILabel?
    private(set) static var entries: [String] = []

    /// Whatever window the app is actually showing. Key first, because an alert or a rig
    /// overlay can add another one.
    static var window: UIWindow? {
        let windows = UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .flatMap(\.windows)
        return windows.first(where: \.isKeyWindow) ?? windows.first
    }

    /// How much the screen must move before the drag is worth another entry. A drag at
    /// `XCUIGestureVelocity.slow` is under the finger for ten seconds or more and is sampled
    /// at the display's rate, so one entry per sample would be thousands of them: a readout
    /// too long to hand back, rewritten thousands of times while the rig is mid-drag. One
    /// per 1% of the screen (about 4 pt) is still far finer than the thing being proved.
    private static let step: CGFloat = 0.01
    /// A finger that wavers can cross the same 1% over and over; this is the hard stop, and
    /// it is on the movement only, so the release always gets the last word.
    private static let maximumTracked = 150
    private static var lastTracked: CGFloat?

    static var text: String { entries.joined(separator: " ") }

    static func start() {
        guard on else { return }
        entries = []
        lastTracked = nil
        note("start")
    }

    static func track(_ progress: CGFloat) {
        guard on, entries.count < maximumTracked else { return }
        if let lastTracked, abs(progress - lastTracked) < step { return }
        lastTracked = progress
        note("track:" + String(format: "%.3f", progress))
    }

    static func note(_ what: String) {
        guard on else { return }
        entries.append(what)
        if let label = onWindow() { write(to: label) }
    }

    /// The readout, put back on the current window if it is not on one. Re-parented rather
    /// than rebuilt, so the trace survives every screen change in a run.
    private static func onWindow() -> UILabel? {
        guard let window else { return readout }
        let label = readout ?? BackSwipeTraceLabel()
        label.isAccessibilityElement = true
        label.accessibilityIdentifier = "back-swipe-trace"
        label.accessibilityLabel = "back swipe trace"
        label.textColor = .clear
        label.backgroundColor = .clear
        label.isUserInteractionEnabled = false
        // A 24 pt corner, so it cannot sit over anything a test reaches for.
        label.frame = CGRect(x: 0, y: 0, width: 24, height: 24)
        if label.superview !== window { window.addSubview(label) }
        readout = label
        return label
    }

    /// What a view is doing, as numbers. Five reasoned answers about this screen have each
    /// cost an hour of gate time and all five were wrong, so this reports rather than judges.
    ///
    /// `tx` is the model transform and `ptx` the presentation one. When they disagree the
    /// model write landed and something is still drawing over it, and `anim` and `spd` say
    /// what: a percent-driven transition pauses the layer it scrubs, and a layer left at
    /// speed 0 with an animation still attached presents its end state for ever.
    static func probe(_ what: String, _ view: UIView?) {
        guard on else { return }
        guard let view else { return note("\(what):gone") }
        let model = Int(view.transform.tx)
        var presented = "-"
        if let shown = view.layer.presentation()?.transform.m41 { presented = String(Int(shown)) }
        let animations = view.layer.animationKeys()?.count ?? 0
        let speed = String(format: "%.1f", view.layer.speed)
        let parented = view.superview == nil ? 0 : 1
        note("\(what):tx=\(model),ptx=\(presented),anim=\(animations),spd=\(speed),sup=\(parented)")
    }

    /// A survey written into the trace as the transition ends, kept alongside the one the
    /// readout takes when it is read. Two samples of the same instrument at known times beat
    /// one sample and an inference about the other.
    static func displaced(_ what: String, from root: UIView?) {
        guard on else { return }
        note("\(what):" + survey(from: root))
    }

    /// How many views a survey will look at before it gives up. Named and printed, never
    /// implied.
    private static let surveyCap = 6000

    /// What the window holds, and **how much was looked at to find it**.
    ///
    /// The second half is not decoration. Three probes in this file have now reported a
    /// conclusion where a count belonged — `kids=2`, a one-trial sweep, and a walk that could
    /// not tell "found nothing" from "stopped early" — and each cost a round. So every survey
    /// prints what it examined: how many views, out of what cap, whether it hit that cap, and
    /// how deep it got. A probe that cannot distinguish an empty result from a truncated one
    /// is not an instrument, however carefully it is read.
    ///
    /// It also reports the first scroll view's own box and transform, so "the container is at
    /// the origin while its contents are a screen width right" is one measurement rather than
    /// two taken twenty seconds apart.
    static func survey(from root: UIView?) -> String {
        guard let root else { return "gone" }
        let edge = root.bounds.width / 2
        var found: [String] = []
        var scroll = "-"
        var stack = [(view: root, depth: 0)]
        var visited = 0
        var deepest = 0
        // The walk continues past the eighth offender on purpose: stopping there would make
        // the count meaningless, and the count is the point.
        while let item = stack.popLast(), visited < surveyCap {
            let view = item.view
            visited += 1
            deepest = max(deepest, item.depth)
            stack.append(contentsOf: view.subviews.map { (view: $0, depth: item.depth + 1) })
            let box = view.convert(view.bounds, to: nil)
            if scroll == "-", view is UIScrollView {
                scroll = String(describing: type(of: view))
                    + "@\(Int(box.minX)),\(Int(box.minY)),\(Int(box.width)),\(Int(box.height))"
                    + ":tx\(Int(view.transform.tx))"
            }
            let shifted = view.transform.tx != 0
            guard shifted || box.minX >= edge, found.count < 8 else { continue }
            found.append(String(describing: type(of: view))
                + "/" + (view.accessibilityIdentifier ?? "-")
                + "@\(Int(box.minX))"
                + (shifted ? ":tx\(Int(view.transform.tx))" : ":frame"))
        }
        return "seen=\(visited)/\(surveyCap)\(visited >= surveyCap ? ":HITCAP" : ":complete")"
            + ",deep=\(deepest),scroll=\(scroll)"
            + ",off=\(found.isEmpty ? "none" : found.joined(separator: "|"))"
    }

    private static var lastReading: (at: TimeInterval, text: String)?

    /// A survey taken now, briefly cached. XCUITest reads this element many times in one pass
    /// — `debugDescription` alone walks the whole tree — and a full survey per read would cost
    /// more than the answer is worth.
    static func reading() -> String {
        // Accessibility does not promise to ask on the main thread, and a view hierarchy is
        // not safe to walk anywhere else. Saying so beats risking the run: an answer of
        // `offthread` is a result about the instrument, not a silent wrong number.
        guard Thread.isMainThread else { return lastReading?.text ?? "offthread" }
        let now = Date().timeIntervalSinceReferenceDate
        if let last = lastReading, now - last.at < 0.25 { return last.text }
        let text = survey(from: window)
        lastReading = (now, text)
        return text
    }

    /// Who takes a touch at a point, named rather than guessed at. "Not hittable" is a
    /// verdict; this is the reason behind it.
    static func hitTest(_ what: String, _ point: CGPoint) {
        guard on, let window else { return }
        var kind = "nobody"
        var named = "-"
        if let hit = window.hitTest(point, with: nil) {
            kind = String(describing: type(of: hit))
            named = hit.accessibilityIdentifier ?? "-"
        }
        note("\(what):\(kind)/\(named)")
    }

    /// The history. The live reading is on the label itself, computed when it is read.
    static func write(to label: UILabel) {
        label.text = text
    }
}

/// The readout, which surveys the hierarchy **when it is read** rather than when the
/// transition ends.
///
/// This is the whole point of the head it arrives in. Every previous reading of this bug
/// compared an app measurement taken half a second after the transition against an XCUITest
/// measurement taken twenty seconds later, and that gap is what left every result with two
/// explanations. Asking the app at the instant the test asks removes the gap: if the survey
/// is clean in the same breath that XCUITest reports the chat a screen width away, then no
/// later displacement can be hiding in between, and the accessibility tree is the suspect by
/// elimination.
private final class BackSwipeTraceLabel: UILabel {
    override var accessibilityValue: String? {
        get { BackSwipeTrace.text + " now:" + BackSwipeTrace.reading() }
        set { super.accessibilityValue = newValue }
    }
}

#endif

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
