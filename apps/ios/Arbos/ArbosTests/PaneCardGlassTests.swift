import SwiftUI
import XCTest
@testable import Arbos

/// The pane card on Glass ([feedback right-pane-glass-cards-ios-2026-10-28], Jacob
/// 2026-10-28): a translucent white wash with a hairline rim, the Mac's numbers
/// (`PANE_CARD_GLASS_DARK` 0.08 / `_LIGHT` 0.55, rim 0.10 / 0.06), so both panes read
/// the same; Matte keeps the solid assistant-bubble grey and no rim. The Mac twin is
/// `theme::glass_contrast_tests` in `crates/arbos-mac/src/ui/theme.rs`.
final class PaneCardGlassTests: XCTestCase {
    func testGlassCardIsATranslucentWashWithTheMacsNumbers() {
        XCTAssertEqual(PaneCardGlass.fillAlpha(isDark: true), 0.08)
        XCTAssertEqual(PaneCardGlass.fillAlpha(isDark: false), 0.55)
        XCTAssertEqual(PaneCardGlass.rimAlphaDark, 0.10)
        XCTAssertEqual(PaneCardGlass.rimAlphaLight, 0.06)
        for alpha in [PaneCardGlass.fillAlphaDark, PaneCardGlass.fillAlphaLight] {
            XCTAssertGreaterThan(alpha, 0, "a wash, not nothing")
            XCTAssertLessThan(alpha, 1, "a wash, not a slab")
        }
    }

    func testOnlyGlassMakesTheCardGlass() {
        XCTAssertTrue(PaneCardGlass.isGlass(surface: .glass))
        XCTAssertFalse(PaneCardGlass.isGlass(surface: .matte))
    }

    func testMatteKeepsTheSolidCardAndNoRim() {
        for appearance in [Appearance.dark, Appearance.light] {
            let matte = Theme.resolve(appearance: appearance, surface: .matte, systemDark: true)
            XCTAssertEqual(matte.paneCardFill, matte.paneCard, "\(appearance): Matte unchanged")
            // The number, not just the wiring. `paneCard` was `bubbleAssistant`
            // until the Grok parity pass and is still close to it, so asserting
            // that the two differ would pass for the wrong reason — and would
            // only start working after the bubble had already dragged the card
            // with it. Pin the shade itself.
            XCTAssertEqual(
                matte.paneCard,
                Color(hex: appearance == .dark ? "#202020" : "#F2F2F2")!,
                "\(appearance): the pane card keeps its own shade"
            )
            XCTAssertEqual(matte.paneCardRim, .clear, "\(appearance): no rim on Matte")
            let glass = Theme.resolve(appearance: appearance, surface: .glass, systemDark: true)
            XCTAssertNotEqual(glass.paneCardFill, glass.paneCard, "\(appearance): Glass is the wash")
            XCTAssertNotEqual(glass.paneCardRim, .clear, "\(appearance): the rim shows on Glass")
        }
    }
}
