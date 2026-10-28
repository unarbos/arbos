import XCTest
@testable import Arbos

/// The fade the bottom bars float on ([feedback iphone-composer-see-through-2026-10-28], Jacob
/// 2026-10-28 on TestFlight 202610281715985037: "we should actually see the text behind the
/// search bar"). Grok: rows scroll under the glass pills and stay readable through and between
/// them; a soft fade dims the content toward the bottom edge; no opaque band, no hard edge.
/// Geometry from `BottomBarFade.chat / .home`, the same the layer draws from.
final class BottomBarFadeTests: XCTestCase {
    /// Under the canvas: the bar + the home indicator (iPhone 17 / Air 34, SE 0), or + the keyboard.
    let belowBar: [CGFloat] = [34, 0, 336]
    /// The chat bar: composer alone, keyboard down / up, and with the status row over it.
    var chatBars: [CGFloat] {
        let composer = Theme.chatBarTopPad + Theme.composerHeight
        return [composer + Theme.composerBottom, composer + Theme.composerBottomKeyboard,
                composer + Theme.presenceAvatar + Theme.presenceBottomGap + Theme.composerBottom]
    }

    private func check(_ spec: BottomBarFade.Spec, bar: CGFloat, below: CGFloat, _ name: String) {
        let where_ = "\(name) bar \(bar) + \(below)"
        XCTAssertEqual(spec.height, spec.fade + bar + below, where_)
        // Clear where the fade starts, so the row that rests there is untouched.
        XCTAssertEqual(spec.alpha(at: 0), 0, where_)
        // At the bar's top edge the content is only dimmed.
        XCTAssertEqual(spec.alpha(at: spec.fade), BottomBarFade.edgeWash, accuracy: 0.001, where_)
        // Readable over every control: from the bar's top edge to the controls' bottom edge the
        // wash stays under `readableMax`.
        var y = spec.fade
        while y <= spec.fade + spec.controls {
            XCTAssertLessThanOrEqual(spec.alpha(at: y), BottomBarFade.readableMax, "\(where_) @\(y)")
            y += 2
        }
        XCTAssertEqual(spec.alpha(at: spec.fade + spec.controls), BottomBarFade.controlsWash, accuracy: 0.001, where_)
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
        for bar in chatBars {
            for below in belowBar {
                let spec = BottomBarFade.chat(bottomInset: bar + below)
                XCTAssertEqual(spec.controls, Theme.chatBarTopPad + Theme.composerHeight)
                check(spec, bar: bar, below: below, "chat")
            }
        }
    }

    func testHomeCapsuleFloats_rowsReadBehindIt() {
        for below in belowBar {
            let spec = BottomBarFade.home(bottomInset: HomeChrome.bottomBar + below)
            XCTAssertEqual(spec.controls, Controls.height)
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
