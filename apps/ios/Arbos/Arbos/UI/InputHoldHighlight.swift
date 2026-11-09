import SwiftUI
import UIKit

/// Grok's touch-and-hold tell: resting a finger on a text box brightens the box under the
/// finger while the system's Paste / AutoFill menu comes up (Jacob, 2026-11-09, shot 5 in
/// `jacob-composer-parity-2026-11-09`). Ours lit nothing at all, and the reason it lit nothing
/// is worth recording, because it had been asked for once already and went quiet:
/// `ControlSurface` has taken a `pressed` flag the whole time and `Theme.controlPill` has
/// plumbed it through, but every call site used the default `false`. The drawing existed; the
/// thing that drives it never landed.
///
/// So the rule lives here, applied by the shared field helpers rather than by each screen, and
/// the lit overlay carries an accessibility identifier — `HoldHighlight.identifier` — which is
/// what `ComposerParityUITests` holds onto. A highlight that stops being driven stops
/// publishing that element, and the test fails; a highlight that is merely *drawable* no longer
/// passes for one that works.
enum HoldHighlight {
    /// Published by the lit overlay, and by nothing else.
    static let identifier = "input-hold-highlight"

    /// How long a finger rests before the box lights. Long enough not to flash under a tap that
    /// is on its way to placing a caret, short enough to arrive with the system's own menu.
    static let delay: TimeInterval = 0.35

    /// How far the finger may drift and still count as a hold. Past this it is a scroll, or a
    /// selection drag, and neither should light anything.
    static let drift: CGFloat = 10

    /// The rule, as a rule: the box lights when the finger has rested long enough without
    /// wandering. `recognizer()` is configured from the same two numbers, so this states what
    /// the recogniser is set to do rather than duplicating a second implementation of it.
    static func lights(held: TimeInterval, drift distance: CGFloat) -> Bool {
        held >= delay && distance <= drift
    }

    /// The watcher.
    ///
    /// It must observe the touch without taking any part of it. The first cut used a SwiftUI
    /// `DragGesture(minimumDistance: 0)` as a `simultaneousGesture`, which looked passive and
    /// was not: it won the touch ahead of the `UITextView` underneath, so tapping the composer
    /// stopped raising the keyboard at all. That took out the keyboard in `BackSwipeUITests`
    /// and `MentionListOverlayUITests` as well as the composer's own test — a useful failure,
    /// because "simultaneous" in SwiftUI does not mean "does not interfere".
    ///
    /// UIKit can say the thing SwiftUI could not: `cancelsTouchesInView = false` leaves every
    /// touch with the view it was going to, the delay and the allowed movement *are* the
    /// arming rule, and the delegate stands aside for every other recogniser — including the
    /// system's own, which is what raises the Paste / AutoFill menu this highlight accompanies.
    static func recognizer(target: Any?, action: Selector) -> UILongPressGestureRecognizer {
        let hold = UILongPressGestureRecognizer(target: target, action: action)
        hold.minimumPressDuration = delay
        hold.allowableMovement = drift
        hold.cancelsTouchesInView = false
        hold.delaysTouchesBegan = false
        hold.delaysTouchesEnded = false
        return hold
    }

    /// The brightening reaches a little past the box's shorter side, so on a one-line pill it
    /// washes the whole height and falls off along the length rather than ending in a visible
    /// disc. Grok's reads as a soft wash under the finger, not as a spotlight.
    static func radius(of size: CGSize) -> CGFloat {
        max(24, min(size.width, size.height) * 1.6)
    }

    /// How strongly the box shifts at the centre of the wash. Drawn in the theme's text
    /// colour, so on dark glass it brightens the way Grok's does and on light glass it
    /// deepens — on a pale surface a white wash would be invisible.
    static let peak: Double = 0.16

    /// How long the box stays lit after the finger lifts.
    ///
    /// Not decoration: the Paste / AutoFill menu this gesture raises only appears *on lift*,
    /// and Jacob's reference has the box lit underneath it. Going dark the instant the finger
    /// leaves would mean the highlight is never once seen together with the menu it belongs
    /// to. A fixed tail is the approximation — the menu's own lifetime is the system's and is
    /// not published to us — so a menu left open longer than this does outlive the light.
    static let tail: TimeInterval = 3
}

/// The wash itself, clipped to the box's own shape so it cannot square off a capsule's ends.
private struct HoldHighlightWash<S: InsettableShape>: View {
    @Environment(\.theme) private var theme
    let shape: S
    let at: CGPoint
    let size: CGSize

    var body: some View {
        RadialGradient(
            colors: [theme.text.opacity(HoldHighlight.peak), theme.text.opacity(0)],
            center: UnitPoint(x: size.width > 0 ? at.x / size.width : 0.5,
                              y: size.height > 0 ? at.y / size.height : 0.5),
            startRadius: 0,
            endRadius: HoldHighlight.radius(of: size)
        )
        .clipShape(shape)
        .allowsHitTesting(false)
        // A gradient is not an element on its own, and an identifier on a non-element is not
        // queryable — which is how a highlight could go missing without a test noticing.
        .accessibilityElement()
        .accessibilityIdentifier(HoldHighlight.identifier)
    }
}

/// Sits behind the box at the box's own size, and watches the window for a resting finger.
///
/// It takes no touches of its own: `hitTest` returns nothing, so it is invisible to the touch
/// system, and the recogniser lives on the window rather than on the box, where it can see a
/// hold anywhere and report only the ones that land inside this box's bounds.
private final class HoldProbeView: UIView {
    /// The point inside this view, or `nil` once the finger has gone.
    var report: ((CGPoint?) -> Void)?
    private var hold: UILongPressGestureRecognizer?

    override func hitTest(_ point: CGPoint, with event: UIEvent?) -> UIView? { nil }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        guard let window else {
            if let hold { hold.view?.removeGestureRecognizer(hold) }
            hold = nil
            return
        }
        guard hold == nil else { return }
        let watcher = HoldHighlight.recognizer(target: self, action: #selector(held))
        watcher.delegate = self
        window.addGestureRecognizer(watcher)
        hold = watcher
    }

    @objc private func held(_ sender: UILongPressGestureRecognizer) {
        switch sender.state {
        case .began, .changed:
            let point = sender.location(in: self)
            // A hold on some other part of the screen is not this box's business.
            report?(bounds.contains(point) ? point : nil)
        default:
            report?(nil)
        }
    }
}

extension HoldProbeView: UIGestureRecognizerDelegate {
    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer,
                           shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
        true
    }
}

private struct HoldProbe: UIViewRepresentable {
    let report: (CGPoint?) -> Void

    func makeUIView(context: Context) -> HoldProbeView {
        let view = HoldProbeView()
        view.backgroundColor = .clear
        view.report = report
        return view
    }

    func updateUIView(_ view: HoldProbeView, context: Context) {
        view.report = report
    }
}

private struct InputHoldHighlightModifier<S: InsettableShape>: ViewModifier {
    let shape: S
    @State private var at: CGPoint?
    @State private var lit = false
    /// Identifies the pending fade so a fresh hold cannot be put out by the last hold's timer.
    @State private var fade: UUID?

    func body(content: Content) -> some View {
        content
            .background(HoldProbe { point in
                if let point {
                    fade = nil
                    at = point
                    lit = true
                } else if lit {
                    startFade()
                }
            })
            .overlay {
                GeometryReader { geo in
                    ZStack {
                        if lit, let at {
                            HoldHighlightWash(shape: shape, at: at, size: geo.size)
                                .transition(.opacity)
                        }
                    }
                    .allowsHitTesting(false)
                }
                .allowsHitTesting(false)
            }
            .animation(.easeOut(duration: 0.18), value: lit)
    }

    /// Lit boxes keep their light for the menu that is about to appear.
    private func startFade() {
        let token = UUID()
        fade = token
        DispatchQueue.main.asyncAfter(deadline: .now() + HoldHighlight.tail) {
            guard fade == token else { return }
            fade = nil
            lit = false
            at = nil
        }
    }
}

extension View {
    /// Light this box under a resting finger. Applied by the shared field helpers, so a new
    /// text box of the family gets it without anyone remembering to ask.
    func inputHoldHighlight<S: InsettableShape>(_ shape: S) -> some View {
        modifier(InputHoldHighlightModifier(shape: shape))
    }
}
