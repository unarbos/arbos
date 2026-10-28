import XCTest
@testable import Arbos

/// Home's floating bars ([feedback iphone-home-bar-fades-2026-10-28], Jacob 2026-10-28 18:21 UTC):
/// the discs + tabs at the top and the search capsule at the bottom float over the list like Grok
/// Bot's; rows stay visible and fade out under them instead of being cut. Geometry from
/// `ListTopFade.spec(.home)` and `HomeBottomFade.spec`, the same the layers draw from.
final class HomeBarFadeTests: XCTestCase {
    /// iPhone 17 / Air status bar (Dynamic Island) and the SE's.
    let statusBars: [CGFloat] = [59, 62, 20]
    /// Under the canvas at the bottom: the bar + the home indicator (34 / 0), or + the keyboard.
    let belowBar: [CGFloat] = [34, 0, 336]

    private func alpha(_ knots: [ListTopFade.Spec.Knot], at y: CGFloat) -> Double {
        // The mask's alpha at `y`: linear between the knots around it.
        guard let i = knots.firstIndex(where: { $0.y >= y }) else { return knots.last?.alpha ?? 0 }
        if i == 0 || knots[i].y == y { return knots[i].alpha }
        let a = knots[i - 1], b = knots[i]
        return a.alpha + (b.alpha - a.alpha) * Double((y - a.y) / (b.y - a.y))
    }

    func testTopBarsFloat_rowsReadUnderTheDiscsAndTabsAndAreGoneWhereTheFirstRowRests() {
        for status in statusBars {
            let spec = ListTopFade.spec(.home, statusBar: status)
            XCTAssertEqual(spec.topInset, 0)
            // Opaque through the status bar and `topSolid` past it — never a row behind the status bar.
            XCTAssertEqual(alpha(spec.knots, at: status), 1, "@\(status)")
            XCTAssertEqual(spec.solid, status + HomeChrome.topSolid)
            // Under the disc row the rows read, dimmed: the wash is below 1 and at least 3/4 at its bottom edge.
            let discsBottom = status + HomeChrome.discsBottom
            XCTAssertEqual(alpha(spec.knots, at: discsBottom), HomeChrome.discsWash, accuracy: 0.001)
            XCTAssertLessThan(HomeChrome.discsWash, 1)
            XCTAssertGreaterThanOrEqual(HomeChrome.discsWash, 0.7)
            // Under the tabs: still over half, so the words stay legible.
            let tabsBottom = discsBottom + HomeChrome.filterBand
            XCTAssertEqual(alpha(spec.knots, at: tabsBottom), HomeChrome.tabsWash, accuracy: 0.001)
            XCTAssertGreaterThan(HomeChrome.tabsWash, 0.5)
            XCTAssertLessThan(HomeChrome.tabsWash, HomeChrome.discsWash)
            // Gone where the first row rests, and the layer ends there.
            XCTAssertEqual(alpha(spec.knots, at: status + HomeChrome.listTop), 0)
            XCTAssertEqual(spec.height, status + HomeChrome.listTop)
            // Monotone: never brighter further down.
            for (a, b) in zip(spec.knots, spec.knots.dropFirst()) {
                XCTAssertLessThanOrEqual(b.alpha, a.alpha); XCTAssertGreaterThanOrEqual(b.y, a.y)
            }
            XCTAssertEqual(spec.stops.count, spec.knots.count)
        }
    }

    func testSearchModeKeepsTheSameFloatingDiscsWithoutTabs() {
        let status: CGFloat = 59
        let spec = ListTopFade.spec(.homeSearch, statusBar: status)
        XCTAssertEqual(spec.solid, status + HomeChrome.topSolid)
        XCTAssertEqual(alpha(spec.knots, at: status + HomeChrome.discsBottom), HomeChrome.discsWash, accuracy: 0.001)
        XCTAssertEqual(spec.height, status + HomeChrome.barOnlyListTop)
    }

    func testBottomCapsuleIsTheSharedSeeThroughFade() {
        // The capsule floats on `BottomBarFade.home` — the chat bar's ramp, pinned in
        // `BottomBarFadeTests` (readable over the capsule, dimmed toward the edge, never a band).
        let spec = BottomBarFade.home(bottomInset: HomeChrome.bottomBar + 34)
        XCTAssertEqual(spec.controls, Controls.height)
        XCTAssertLessThanOrEqual(spec.alpha(at: spec.fade + Controls.height), BottomBarFade.readableMax)
        XCTAssertLessThan(spec.knots.last?.alpha ?? 1, 1)
    }

    func testFadeHeightsAreShortOnThePixelAndTheListsClearThem() {
        // Jacob's chat fade set the family's range (24–40); Home's bottom fade is the same one.
        XCTAssertGreaterThanOrEqual(BottomBarFade.fade, 24)
        XCTAssertLessThanOrEqual(BottomBarFade.fade, 40)
        XCTAssertEqual(HomeChrome.listBottom, BottomBarFade.fade)
        XCTAssertEqual(HomeChrome.topSolid * 3, (HomeChrome.topSolid * 3).rounded(), accuracy: 0.001)
        // The lists' trailing clear row is the fade: the last row at rest sits above it. The
        // leading clear row is the whole top layer: the first row at rest sits below it.
        XCTAssertGreaterThanOrEqual(HomeChrome.listBottom, BottomBarFade.fade)
        XCTAssertEqual(HomeChrome.listTop, ListTopFade.spec(.home, statusBar: 0).height)
        XCTAssertEqual(HomeChrome.barOnlyListTop, ListTopFade.spec(.homeSearch, statusBar: 0).height)
    }

    func testMatteAndGlassShareTheGeometryAndDifferOnlyInMaterial() {
        // One geometry; the top layer's wash is the surface's (page colour / 0.94 over material),
        // the bottom fade is the page colour on both and the pills carry the glass.
        XCTAssertEqual(ListTopFade.glassWash, 0.94)
        XCTAssertEqual(ChatHeaderGlass.material(surface: .matte, liquidGlassAvailable: true), .solid)
        XCTAssertNotEqual(ChatHeaderGlass.material(surface: .glass, liquidGlassAvailable: true), .solid)
    }
}
