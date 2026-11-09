import SwiftUI

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
/// what `InputHoldHighlightUITests` holds onto. A highlight that stops being driven stops
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

    /// The box lights when the finger has rested long enough without wandering.
    static func lights(held: TimeInterval, drift distance: CGFloat) -> Bool {
        held >= delay && distance <= drift
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

/// Watches for a resting finger without taking the touch away from the field underneath — the
/// Paste / AutoFill menu is the system's and has to come up exactly as it did. The gesture is
/// simultaneous and observes only; it never calls an action.
private struct InputHoldHighlightModifier<S: InsettableShape>: ViewModifier {
    let shape: S
    @State private var at: CGPoint?
    @State private var start: CGPoint?
    @State private var began: Date?
    @State private var lit = false
    /// Identifies the pending fade so a fresh hold's timer cannot be put out by the last
    /// hold's.
    @State private var fade: UUID?

    func body(content: Content) -> some View {
        content
            .overlay {
                GeometryReader { geo in
                    ZStack {
                        if lit, let at {
                            HoldHighlightWash(shape: shape, at: at, size: geo.size)
                                .transition(.opacity)
                        }
                    }
                }
                .allowsHitTesting(false)
            }
            .animation(.easeOut(duration: 0.18), value: lit)
            .simultaneousGesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { value in
                        fade = nil
                        at = value.location
                        let from = start ?? value.location
                        start = from
                        let since = began ?? Date()
                        began = since
                        let drift = hypot(value.location.x - from.x, value.location.y - from.y)
                        if drift > HoldHighlight.drift {
                            lit = false
                            // A finger that has wandered is doing something else, and must not
                            // light the box later by coming to rest somewhere new.
                            began = .distantFuture
                            return
                        }
                        lit = HoldHighlight.lights(held: Date().timeIntervalSince(since), drift: drift)
                    }
                    .onEnded { _ in
                        start = nil
                        began = nil
                        guard lit else { at = nil; return }
                        // Lit boxes keep their light for the menu that is about to appear.
                        let token = UUID()
                        fade = token
                        DispatchQueue.main.asyncAfter(deadline: .now() + HoldHighlight.tail) {
                            guard fade == token else { return }
                            fade = nil
                            lit = false
                            at = nil
                        }
                    }
            )
            // A resting finger sends no further events, so the hold has to be noticed on a clock
            // rather than on the next change. Cheap: it only runs while a finger is down.
            .modifier(HoldTick(active: began != nil && !lit) {
                guard let since = began, let from = start, let now = at else { return }
                let drift = hypot(now.x - from.x, now.y - from.y)
                lit = HoldHighlight.lights(held: Date().timeIntervalSince(since), drift: drift)
            })
    }
}

/// A timer that exists only while a finger is down.
private struct HoldTick: ViewModifier {
    let active: Bool
    let tick: () -> Void

    func body(content: Content) -> some View {
        if active {
            content.onReceive(Timer.publish(every: 0.05, on: .main, in: .common).autoconnect()) { _ in tick() }
        } else {
            content
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
