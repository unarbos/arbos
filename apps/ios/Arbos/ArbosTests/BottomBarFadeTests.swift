import XCTest
@testable import Arbos

/// The fade the bottom bars float on ([feedback iphone-composer-see-through-2026-10-28], Jacob
/// 2026-10-28 on TestFlight 202610281715985037: "we should actually see the text behind the
/// search bar"). Grok: rows scroll under the glass pills and stay readable through and between
/// them; a soft fade dims the content toward the bottom edge; no opaque band, no hard edge.
/// Geometry from `BottomBarFade.chat / .home`, the same the layer draws from.
///
/// Since [feedback iphone-bottom-fade-newest-bubble-2026-11-01] the ramp ends at the *controls'*
/// top edge rather than the bar's. The chat bar also carries the status row and a send error's
/// note, so a ramp measured from the bar's top edge climbed with the bar and washed the status
/// row and the newest bubble at rest.
final class BottomBarFadeTests: XCTestCase {
    /// Under the canvas: the bar + the home indicator (iPhone 17 / Air 34, SE 0), or + the keyboard.
    let belowBar: [CGFloat] = [34, 0, 336]

    /// A chat bar as the screen measures it (`ChatBarHeight`): its top pad, what rides over the
    /// composer, the composer, and the composer's bottom pad — 18 pt with the keyboard up, 2 down.
    private func chatBar(statusRow: Bool, errorNote: CGFloat, keyboard: Bool) -> CGFloat {
        let over = (statusRow ? Theme.presenceAvatar + Theme.presenceBottomGap - Theme.presenceLift : 0)
            + (errorNote > 0 ? errorNote + 10 : 0)
        return Theme.chatBarTopPad + over + Theme.composerHeight
            + (keyboard ? Theme.composerBottomKeyboard : Theme.composerBottom)
    }

    private func check(_ spec: BottomBarFade.Spec, bar: CGFloat, below: CGFloat, _ name: String) {
        let where_ = "\(name) bar \(bar) + \(below)"
        XCTAssertEqual(spec.height, spec.fade + bar + below, where_)
        // Clear from the layer's top down to the ramp, so a row resting there is untouched.
        XCTAssertEqual(spec.alpha(at: 0), 0, where_)
        XCTAssertEqual(spec.alpha(at: spec.controlsTop - spec.ramp), 0, where_)
        // At the controls' top edge the content is only dimmed.
        XCTAssertEqual(spec.alpha(at: spec.controlsTop), BottomBarFade.edgeWash, accuracy: 0.001, where_)
        // Readable over every control: from the controls' top edge to their bottom edge the
        // wash stays under `readableMax`.
        var y = spec.controlsTop
        while y <= spec.controlsTop + spec.controls {
            XCTAssertLessThanOrEqual(spec.alpha(at: y), BottomBarFade.readableMax, "\(where_) @\(y)")
            y += 2
        }
        XCTAssertEqual(spec.alpha(at: spec.controlsTop + spec.controls), BottomBarFade.controlsWash, accuracy: 0.001, where_)
        // Never opaque anywhere — no band, no hard edge — and darkest at the screen's edge.
        for k in spec.knots { XCTAssertLessThan(k.alpha, 1, where_) }
        XCTAssertEqual(spec.alpha(at: spec.height), BottomBarFade.bottomWash, accuracy: 0.001, where_)
        // Monotone: never brighter further down.
        for (a, b) in zip(spec.knots, spec.knots.dropFirst()) {
            XCTAssertGreaterThanOrEqual(b.alpha, a.alpha, where_); XCTAssertGreaterThanOrEqual(b.y, a.y, where_)
        }
        XCTAssertEqual(spec.stops.count, spec.knots.count)
        XCTAssertEqual(spec.stops.last?.location, 1)
    }

    func testChatBarFloats_rowsReadBehindThePlusTheFieldAndThePill() {
        for keyboard in [false, true] {
            for statusRow in [false, true] {
                for note in [CGFloat(0), 44] {
                    let bar = chatBar(statusRow: statusRow, errorNote: note, keyboard: keyboard)
                    for below in belowBar {
                        let above = BottomBarFade.aboveComposer(bar: bar, keyboard: keyboard)
                        let spec = BottomBarFade.chat(bottomInset: bar + below, aboveComposer: above)
                        XCTAssertEqual(spec.controls, Theme.composerHeight)
                        // The ramp ends on the composer, whatever else the bar carries.
                        XCTAssertEqual(spec.controlsTop + spec.controls,
                                       spec.height - below - (keyboard ? Theme.composerBottomKeyboard : Theme.composerBottom),
                                       accuracy: 0.001, "composer bottom edge")
                        check(spec, bar: bar, below: below, "chat status=\(statusRow) note=\(note) kb=\(keyboard)")
                    }
                }
            }
        }
    }

    /// The point of the fix: at rest the newest bubble and the status row over it draw at full
    /// colour, because both sit above the ramp (Jacob, 2026-11-01).
    func testNewestRowAndStatusRowAtRestAreOutsideTheRamp() {
        for keyboard in [false, true] {
            for statusRow in [false, true] {
                let bar = chatBar(statusRow: statusRow, errorNote: 0, keyboard: keyboard)
                let above = BottomBarFade.aboveComposer(bar: bar, keyboard: keyboard)
                let spec = BottomBarFade.chat(bottomInset: bar + 34, aboveComposer: above)
                let where_ = "status=\(statusRow) kb=\(keyboard)"
                // The last row's bottom edge at rest: `Theme.transcriptTail` over the bar's top
                // edge, which is `spec.fade` under the layer's top.
                let newestRow = spec.fade - Theme.transcriptTail
                XCTAssertLessThan(newestRow, spec.controlsTop - spec.ramp, where_)
                XCTAssertEqual(spec.alpha(at: newestRow), 0, where_)
                // And the clearance is the bar's pad plus whatever it carries, less the ramp.
                XCTAssertEqual(spec.controlsTop - spec.ramp - newestRow,
                               Theme.transcriptTail + above - BottomBarFade.composerRamp,
                               accuracy: 0.001, where_)
                guard statusRow else { continue }
                // The status row's bottom edge is its own gap over the composer.
                XCTAssertEqual(spec.alpha(at: spec.controlsTop - Theme.presenceBottomGap), 0, where_)
            }
        }
    }

    func testHomeCapsuleFloats_rowsReadBehindIt() {
        for below in belowBar {
            let spec = BottomBarFade.home(bottomInset: HomeChrome.bottomBar + below)
            XCTAssertEqual(spec.controls, Controls.height)
            // Home's capsule *is* the bar's first view, so the ramp is the clear lead, as before.
            XCTAssertEqual(spec.aboveControls, 0)
            XCTAssertEqual(spec.ramp, spec.fade)
            XCTAssertEqual(spec.controlsTop, spec.fade)
            check(spec, bar: HomeChrome.bottomBar, below: below, "home")
        }
        XCTAssertEqual(HomeChrome.bottomBar, Controls.height + HomeChrome.bottomPad)
    }

    func testRampIsGroksSoftDimNotABand() {
        XCTAssertLessThan(BottomBarFade.edgeWash, BottomBarFade.controlsWash)
        XCTAssertLessThan(BottomBarFade.controlsWash, BottomBarFade.bottomWash)
        XCTAssertLessThanOrEqual(BottomBarFade.controlsWash, BottomBarFade.readableMax)
        XCTAssertLessThan(BottomBarFade.bottomWash, 1, "the screen's edge is dimmed, never a band")
        XCTAssertGreaterThan(BottomBarFade.bottomWash, BottomBarFade.readableMax, "the fade does darken toward the edge")
    }

    func testFadeIsShortOnThePixelAndTheListsClearIt() {
        XCTAssertGreaterThanOrEqual(BottomBarFade.fade, 24)
        XCTAssertLessThanOrEqual(BottomBarFade.fade, 40)
        XCTAssertEqual(BottomBarFade.fade * 3, (BottomBarFade.fade * 3).rounded(), accuracy: 0.001)
        // The transcript's tail and Home's trailing clear row are the fade: the last row at rest is clear of it.
        XCTAssertGreaterThanOrEqual(Theme.transcriptTail, BottomBarFade.fade)
        XCTAssertGreaterThanOrEqual(HomeChrome.listBottom, BottomBarFade.fade)
        // The chat's ramp cuts off just over the composer, inside the status row's bottom gap.
        XCTAssertLessThan(BottomBarFade.composerRamp, Theme.presenceBottomGap)
        XCTAssertLessThanOrEqual(BottomBarFade.composerRamp, BottomBarFade.fade)
        XCTAssertEqual(BottomBarFade.composerRamp * 3, (BottomBarFade.composerRamp * 3).rounded(), accuracy: 0.001)
        // The status row's Grok gap holds (`presenceLift` follows the tail).
        XCTAssertEqual(Theme.transcriptTail + Theme.chatBarTopPad - Theme.presenceLift, Theme.presenceTopGap)
    }

    func testPillsAreGlassOnGlassAndSolidOnMatte() {
        // The pills themselves carry the surface: Liquid Glass / frost on Glass, the field colour on
        // Matte (`ControlSurface`); the fade layer is the page colour on both.
        XCTAssertEqual(ChatHeaderGlass.material(surface: .glass, liquidGlassAvailable: true), .liquidGlass)
        XCTAssertEqual(ChatHeaderGlass.material(surface: .glass, liquidGlassAvailable: false), .frosted)
        XCTAssertEqual(ChatHeaderGlass.material(surface: .matte, liquidGlassAvailable: true), .solid)
    }
}
