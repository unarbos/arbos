// Swipe right goes back one level on every chat screen (agent 1:1, group, DM,
// server channel, job tab) and on the server channel list.
//
// Each of those screens hides the system navigation bar and back button for its
// own chevron, which also turns UIKit's edge-swipe pop off. This file is the one
// place that turns it back on, and the one place for the full-screen swipe-right →
// back (#916, #925). History: #1554 gated both on the composer's focus so that
// selecting text in the field (a horizontal drag) could not fire "back"; `send()`
// keeps the composer focused, so after typing anything in a chat the keyboard was
// up and no back swipe worked at all. The rules below replace that gate: the edge
// pop is never blocked, and the full-screen swipe ignores drags that start inside
// the composer bar instead of ignoring focus.

import SwiftUI
import UIKit

/// The back-gesture rules, as data (unit-tested in `BackSwipeTests`).
enum BackSwipe {
    /// Minimum pan before the full-screen swipe is considered at all.
    static let minimumDistance: CGFloat = 50

    /// The system edge pop may begin when the stack has a screen to pop to and no push
    /// or pop is animating (starting a pop mid-transition corrupts the stack). Keyboard,
    /// draft, edit or any other screen state never blocks it.
    static func popAllowed(stackDepth: Int, transitioning: Bool) -> Bool {
        stackDepth > 1 && !transitioning
    }

    /// A finished full-screen drag counts as "back" when it is a clear horizontal pan to
    /// the right — far enough (> 70 pt), more horizontal than vertical (1.25×), and either
    /// faster sideways than up/down or long (> 120 pt) — and it did not start inside the
    /// composer bar (`startY >= composerTop`), where a drag selects text or slides the mic.
    static func isBackSwipe(translation: CGSize, velocity: CGSize, startY: CGFloat, composerTop: CGFloat?) -> Bool {
        if let composerTop, startY >= composerTop { return false }
        let dx = translation.width
        let absX = abs(dx), absY = abs(translation.height)
        guard dx > 0, absX > 70, absX > absY * 1.25 else { return false }
        return abs(velocity.width) >= abs(velocity.height) || absX > 120
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

extension BackSwipe {
    /// Named coordinate space of a screen that uses `backSwipeToPop`; the composer bar
    /// measures its top edge in it.
    static let space = "arbosBackSwipeRoot"
}

/// Hides the system bar and back button, keeps the edge pop alive and adds the
/// full-screen swipe-right → `dismiss()`.
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
            .background(InteractivePopGestureEnabler())
            .simultaneousGesture(
                DragGesture(minimumDistance: BackSwipe.minimumDistance, coordinateSpace: .named(BackSwipe.space))
                    .onEnded { value in
                        if BackSwipe.isBackSwipe(translation: value.translation, velocity: value.velocity,
                                                 startY: value.startLocation.y, composerTop: composerTop) {
                            dismiss()
                        }
                    }
            )
    }
}

extension View {
    /// Custom-back screen: swipe right (edge or anywhere outside the composer bar) pops it.
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

/// `.navigationBarBackButtonHidden(true)` turns off the system edge-swipe back
/// gesture; this turns it back on for the enclosing `UINavigationController` and
/// gives it one shared delegate (`PopDelegate`) that answers for every screen.
struct InteractivePopGestureEnabler: UIViewControllerRepresentable {
    func makeUIViewController(context: Context) -> Controller { Controller() }

    func updateUIViewController(_ uiViewController: Controller, context: Context) {
        uiViewController.applyPopEnabled()
    }

    final class Controller: UIViewController {
        override func viewDidAppear(_ animated: Bool) {
            super.viewDidAppear(animated)
            applyPopEnabled()
        }

        override func didMove(toParent parent: UIViewController?) {
            super.didMove(toParent: parent)
            applyPopEnabled()
        }

        func applyPopEnabled() {
            guard let nav = enclosingNavigationController(),
                  let pop = nav.interactivePopGestureRecognizer else { return }
            pop.isEnabled = true
            pop.delegate = PopDelegate.shared
        }

        /// Background representables sometimes have a nil `navigationController`;
        /// walk parents / the responder chain to find the stack that pushed the screen.
        private func enclosingNavigationController() -> UINavigationController? {
            if let nav = navigationController { return nav }
            var parent = self.parent
            while let current = parent {
                if let nav = current as? UINavigationController { return nav }
                if let nav = current.navigationController { return nav }
                parent = current.parent
            }
            var responder: UIResponder? = view
            while let current = responder {
                if let nav = current as? UINavigationController { return nav }
                if let vc = current as? UIViewController, let nav = vc.navigationController { return nav }
                responder = current.next
            }
            return nil
        }
    }

    /// One delegate for every navigation stack: begins per `BackSwipe.popAllowed`, and
    /// makes scroll views and SwiftUI drags wait for the edge pan (UIKit's own delegate
    /// did that too; replacing it must not lose it).
    final class PopDelegate: NSObject, UIGestureRecognizerDelegate {
        static let shared = PopDelegate()

        func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
            guard let nav = navigationController(of: gestureRecognizer) else { return false }
            return BackSwipe.popAllowed(stackDepth: nav.viewControllers.count,
                                        transitioning: nav.transitionCoordinator != nil)
        }

        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer,
                               shouldBeRequiredToFailBy other: UIGestureRecognizer) -> Bool {
            other is UIPanGestureRecognizer && !(other is UIScreenEdgePanGestureRecognizer)
        }

        private func navigationController(of recognizer: UIGestureRecognizer) -> UINavigationController? {
            var responder: UIResponder? = recognizer.view
            while let current = responder {
                if let nav = current as? UINavigationController { return nav }
                responder = current.next
            }
            return nil
        }
    }
}
