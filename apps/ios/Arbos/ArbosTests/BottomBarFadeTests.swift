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
                        let spec = BottomBarFade.chat(bottomInset: bar + below, aboveComposer: above,
                                                      composerTop: nil, layerTop: 0)
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
                let spec = BottomBarFade.chat(bottomInset: bar + 34, aboveComposer: above,
                                              composerTop: nil, layerTop: 0)
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

    /// A device whose bottom inset over-reports by `wrong` pt. The layer's height is that inset,
    /// and its bottom edge is the screen's, so its top edge sits `wrong` pt higher than the
    /// arithmetic assumes while the composer stays where it is. What the layer measures is the
    /// distance between the two, taken with its own top edge as the origin.
    private func chatOnADeviceReporting(wrong: CGFloat, bar: CGFloat, below: CGFloat, keyboard: Bool)
        -> (designed: BottomBarFade.Spec, fixed: BottomBarFade.Spec) {
        let above = BottomBarFade.aboveComposer(bar: bar, keyboard: keyboard)
        let inset = bar + below + wrong
        let designed = BottomBarFade.chat(bottomInset: inset, aboveComposer: above,
                                          composerTop: nil, layerTop: 0)
        let fixed = BottomBarFade.chat(bottomInset: inset, aboveComposer: above,
                                       composerTop: designed.controlsTop + wrong, layerTop: 0)
        return (designed, fixed)
    }

    /// Jacob's iPhone Air reports its bottom inset 76 pt too large, so the layer's top edge sits
    /// that far above where the arithmetic puts it and the ramp reached the newest bubble at
    /// rest — the complaint itself. Neither the iPhone 17 nor the iPhone Air *simulator*
    /// reproduces it (`internal/bottom-fade-sits-76pt-high-on-the-device-only.md`), so this is
    /// the only place that device's behaviour is pinned at all.
    func testAWrongInsetIsCorrectedFromTheMeasuredComposer() {
        for keyboard in [false, true] {
            for statusRow in [false, true] {
                let bar = chatBar(statusRow: statusRow, errorNote: 0, keyboard: keyboard)
                let below: CGFloat = keyboard ? 336 : 34
                let wrong: CGFloat = 76
                let (designed, fixed) = chatOnADeviceReporting(wrong: wrong, bar: bar, below: below,
                                                               keyboard: keyboard)
                let where_ = "status=\(statusRow) kb=\(keyboard)"
                // The ramp moves down by exactly the error, back on to the composer's top edge.
                XCTAssertEqual(fixed.controlsTop, designed.controlsTop + wrong, accuracy: 0.001, where_)
                // Which is the point. The newest row's bottom edge at rest is `transcriptTail`
                // over the bar's top edge, and on this device that is `wrong` pt further down
                // the layer than the designed geometry believes.
                let newestRow = designed.fade - Theme.transcriptTail + wrong
                XCTAssertGreaterThan(designed.alpha(at: newestRow), 0, "\(where_) — the bug")
                XCTAssertEqual(fixed.alpha(at: newestRow), 0, "\(where_) — the fix")
                // And it is still the same ramp: clear lead, dim over the composer, readable.
                check(fixed, bar: bar, below: below + wrong, "air \(where_)")
            }
        }
    }

    /// The no-regression proof, and the reason the simulator before/after pairs for this half of
    /// the fix show nothing: on a device that reports its bottom inset correctly the measurement
    /// agrees with the arithmetic to the pixel, for every shape the chat bar takes. The
    /// correction is dead code there, so what a simulator shows is the first half of the fix,
    /// untouched.
    func testACorrectDeviceMeasuresExactlyTheDesignedGeometry() {
        for keyboard in [false, true] {
            for statusRow in [false, true] {
                for note in [CGFloat(0), 44] {
                    let bar = chatBar(statusRow: statusRow, errorNote: note, keyboard: keyboard)
                    let under = Theme.composerHeight
                        + (keyboard ? Theme.composerBottomKeyboard : Theme.composerBottom)
                    let above = BottomBarFade.aboveComposer(bar: bar, keyboard: keyboard)
                    for below in belowBar {
                        // The layer's top edge is `fade + inset` over the screen's edge and the
                        // composer's top edge is `below + under` over it, so a device whose
                        // inset is right measures the one from the other as `fade + bar - under`.
                        let designed = BottomBarFade.chat(bottomInset: bar + below, aboveComposer: above,
                                                          composerTop: nil, layerTop: 0)
                        let measured = BottomBarFade.chat(bottomInset: bar + below, aboveComposer: above,
                                                          composerTop: BottomBarFade.fade + bar - under,
                                                          layerTop: 0)
                        XCTAssertEqual(measured, designed,
                                       "status=\(statusRow) note=\(note) kb=\(keyboard) below=\(below)")
                    }
                }
            }
        }
    }

    /// The fallback, which matters because the hardware this corrects for is hardware we cannot
    /// test. A measurement wrong the other way would drag the ramp up over the rows at rest —
    /// the bug — or drop it under the composer so rows went beneath it undimmed, which is worse
    /// than the bug. Neither is obeyed: the designed geometry is the floor, and the layer's own
    /// bottom edge is the ceiling.
    func testAnImplausibleMeasurementFallsBackToTheDesignedGeometry() {
        for keyboard in [false, true] {
            let bar = chatBar(statusRow: true, errorNote: 0, keyboard: keyboard)
            let below: CGFloat = keyboard ? 336 : 34
            let above = BottomBarFade.aboveComposer(bar: bar, keyboard: keyboard)
            func spec(_ composerTop: CGFloat?, _ layerTop: CGFloat? = 0) -> BottomBarFade.Spec {
                BottomBarFade.chat(bottomInset: bar + below, aboveComposer: above,
                                   composerTop: composerTop, layerTop: layerTop)
            }
            let designed = spec(nil)
            let where_ = "kb=\(keyboard)"
            // The measurement agrees, which is what every simulator we can run reports: the
            // geometry is the one that shipped in the first half of this fix, to the pixel.
            XCTAssertEqual(spec(designed.controlsTop), designed, where_)
            // Sub-pixel noise does not re-make the gradient either.
            XCTAssertEqual(spec(designed.controlsTop + 0.4), designed, where_)
            // Wrong upward, and not measured yet on the first layout pass: refused.
            XCTAssertEqual(spec(designed.controlsTop - 40), designed, where_)
            XCTAssertEqual(spec(0), designed, where_)
            XCTAssertEqual(spec(nil), designed, where_)
            // The layer has not landed yet, so there is nothing to measure against: refused.
            XCTAssertEqual(spec(designed.controlsTop + 76, nil), designed, where_)
            // Not a number, from either side: refused.
            XCTAssertEqual(spec(.nan), designed, where_)
            XCTAssertEqual(spec(designed.controlsTop + 76, .nan), designed, where_)
            // Wrong downward: clamped, so the composer's band still ends inside the layer.
            let absurd = spec(designed.controlsTop + 10_000)
            XCTAssertGreaterThan(absurd.controlsTop, designed.controlsTop, where_)
            XCTAssertLessThanOrEqual(absurd.controlsTop + absurd.controls, absurd.height, where_)
            for k in absurd.knots { XCTAssertLessThan(k.alpha, 1, where_) }
            for (a, b) in zip(absurd.knots, absurd.knots.dropFirst()) {
                XCTAssertGreaterThan(b.y, a.y, where_)
                XCTAssertGreaterThanOrEqual(b.alpha, a.alpha, where_)
            }
            // Even clamped to its ceiling it cannot wash the rows at rest, which is the one
            // property that must hold whatever a device reports.
            XCTAssertEqual(absurd.alpha(at: absurd.fade - Theme.transcriptTail), 0, where_)
            XCTAssertEqual(absurd.alpha(at: absurd.height), BottomBarFade.bottomWash, accuracy: 0.001, where_)
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
