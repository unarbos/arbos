import XCTest
@testable import Arbos

/// The composer's pills and Home's search capsule on Glass are real, clear glass with nothing
/// of ours on them ([feedback iphone-composer-glass-2026-10-28], Jacob 2026-10-28: "Grok chat is
/// actual transparent glass. Ours isn't. Fix that."). Matte stays solid.
final class PillGlassTests: XCTestCase {
    func testGlassPillsAreClearUntintedSystemGlass() {
        let g = ChatHeaderGlass.pillGlass(surface: .glass, liquidGlassAvailable: true)
        XCTAssertEqual(g.material, .liquidGlass)
        XCTAssertTrue(g.clear, "Liquid Glass .clear, the transparent variant")
        XCTAssertFalse(g.tinted, "no field tint over the glass")
        let f = ChatHeaderGlass.pillGlass(surface: .glass, liquidGlassAvailable: false)
        XCTAssertEqual(f.material, .frosted)
        XCTAssertTrue(f.clear, "plain frost, no tint, before iOS 26")
        XCTAssertFalse(f.tinted)
    }

    func testMatteStaysSolid() {
        let m = ChatHeaderGlass.pillGlass(surface: .matte, liquidGlassAvailable: true)
        XCTAssertEqual(m.material, .solid)
        XCTAssertFalse(m.clear)
    }

    func testFadeUnderThePillsLetsTheTextRead() {
        // The only layer under the pills is the page-colour fade; it stays light where the
        // controls are and never opaque (`BottomBarFadeTests` pins the ramp's shape).
        XCTAssertLessThanOrEqual(BottomBarFade.controlsWash, 0.4)
        XCTAssertLessThanOrEqual(BottomBarFade.edgeWash, 0.25)
        XCTAssertLessThan(BottomBarFade.bottomWash, 1)
    }
}
