import XCTest
@testable import Arbos

/// The band the chat composer sits on ([feedback iphone-composer-bottom-fade-2026-10-28],
/// Jacob 2026-10-28 15:56 UTC): Grok's rows dissolve into a solid band over the composer;
/// ours read through behind and between the controls. Geometry from `ComposerBottomBand.spec`,
/// the same the views draw from, so a change to the fade, the tail or the wash fails here.
final class ComposerBottomBandTests: XCTestCase {
    /// iPhone 17 / Air home indicator, the SE's none, and the keyboard up (the keyboard is the
    /// bar's safe area then, so the band stops on it: 0).
    let safeBottoms: [CGFloat] = [34, 0]
    /// The bar with the composer alone, keyboard down / up, and with the status row over it.
    var bars: [CGFloat] {
        let composer = Theme.chatBarTopPad + Theme.composerHeight
        return [composer + Theme.composerBottom, composer + Theme.composerBottomKeyboard,
                composer + Theme.presenceAvatar + Theme.presenceBottomGap + Theme.composerBottom]
    }

    func testFadeIsShortAndTheLastRowAtRestClearsIt() {
        // Jacob: a short gradient, ≈ 24–40 pt, from clear into the band.
        XCTAssertGreaterThanOrEqual(ComposerBottomBand.fade, 24)
        XCTAssertLessThanOrEqual(ComposerBottomBand.fade, 40)
        XCTAssertEqual(ComposerBottomBand.fade, Theme.composerFade)
        // The transcript ends `transcriptTail` above the bar's top; the fade reaches that far
        // up and no further, so the last row at rest is fully above it.
        XCTAssertGreaterThanOrEqual(Theme.transcriptTail, ComposerBottomBand.fade,
                                    "the last row at rest would sit under the fade")
        // On the pixel at 3x.
        XCTAssertEqual(ComposerBottomBand.fade * 3, (ComposerBottomBand.fade * 3).rounded(), accuracy: 0.001)
    }

    func testBandCoversTheBarAndTheHomeIndicatorAndGluesToTheKeyboard() {
        for bar in bars {
            for safe in safeBottoms {
                let spec = ComposerBottomBand.spec(bar: bar, safeBottom: safe)
                XCTAssertEqual(spec.bottomInset, 0, "bar \(bar) @\(safe): the band reaches the bottom edge")
                XCTAssertEqual(spec.band, bar + safe, "bar \(bar) @\(safe): opaque through the bar and the inset")
                XCTAssertGreaterThanOrEqual(spec.band, bar, "the band is never shorter than the bar")
                XCTAssertEqual(spec.fade, ComposerBottomBand.fade)
            }
            // Keyboard up: the home indicator is under the keyboard, the band ends on the keyboard.
            XCTAssertEqual(ComposerBottomBand.spec(bar: bar, safeBottom: 0).band, bar)
        }
    }

    func testBandIsOpaqueToTextOnBothSurfaces() {
        // Matte: the page colour itself. Glass: the list top fade's wash over the material —
        // 0.94 hides a row (0.78 let one read at half strength, #1988).
        XCTAssertEqual(ComposerBottomBand.wash(glass: false), 1)
        XCTAssertEqual(ComposerBottomBand.wash(glass: true), ListTopFade.glassWash)
        XCTAssertGreaterThanOrEqual(ComposerBottomBand.wash(glass: true), 0.94)
    }

    func testStatusRowStillLandsGroksDistanceFromTheLastBubble() {
        // The status row rises `presenceLift` into the tail; moving the tail with the fade
        // must keep the bubble → picture gap at Grok's 42 px.
        XCTAssertEqual(Theme.transcriptTail + Theme.chatBarTopPad - Theme.presenceLift, Theme.presenceTopGap)
        XCTAssertEqual(Theme.presenceTopGap, 14)
    }
}
