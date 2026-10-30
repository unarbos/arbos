import XCTest
@testable import Arbos

/// The list top fade ([feedback iphone-list-top-fade-2026-10-26], Jacob 2026-10-26 16:05 CR):
/// on every iPhone list with a floating bar the fade layer starts at the physical top edge
/// (top inset 0 — it ignores the safe area, the list does not) and is at least status bar +
/// bar tall (+ the tab bar on Home), so scrolled rows never read through behind the status
/// bar or around the tabs. Geometry from `ListTopFade.spec`, the same the view draws from.
final class ListTopFadeTests: XCTestCase {
    /// iPhone 17 / Air status bar (Dynamic Island) and the SE's.
    let statusBars: [CGFloat] = [59, 62, 20]

    func testEveryListFadeStartsAtTheTopEdgeAndCoversStatusBarAndChrome() {
        for screen in ListTopFade.Screen.allCases {
            for status in statusBars {
                let spec = ListTopFade.spec(screen, statusBar: status)
                XCTAssertEqual(spec.topInset, 0, "\(screen): the fade ignores the top safe area")
                let chrome = status + screen.barTop + Controls.diameter + screen.chrome
                XCTAssertGreaterThanOrEqual(spec.height, chrome, "\(screen) @\(status): fade shorter than status bar + bar + chrome")
                XCTAssertGreaterThanOrEqual(spec.solid, status, "\(screen) @\(status): the status bar band must be opaque")
                XCTAssertGreaterThan(spec.height, spec.solid, "\(screen): a gradient band follows the solid part")
            }
        }
    }

    func testHomeFloatsItsBarsOverTheRowsAndFadesOutWhereTheFirstRowStarts() {
        // Jacob 2026-10-28 ([feedback iphone-home-bar-fades-2026-10-28]) reversed the opaque block
        // through the discs and the tabs: the bars float, the rows fade out under them. Opaque
        // through the status bar and a hair past it only; clear exactly where the list's first
        // row sits at rest (the band's end never moved). `HomeBarFadeTests` pins the curve.
        let status: CGFloat = 59
        let spec = ListTopFade.spec(.home, statusBar: status)
        XCTAssertEqual(spec.solid, status + HomeChrome.topSolid)
        XCTAssertLessThan(spec.solid, status + Controls.top, "the discs are not under the opaque part")
        XCTAssertEqual(spec.height, status + HomeChrome.listTop)
        XCTAssertEqual(HomeChrome.listTop, Controls.top + Controls.diameter + HomeChrome.filterBand + HomeChrome.fadeBand)
        XCTAssertEqual(HomeChrome.filterBand, 44)
    }

    func testBarOnlyListsAndPanePagesFadeToWhereTheirContentStarts() {
        let status: CGFloat = 59
        for screen in [ListTopFade.Screen.homeSearch, .hiddenChats, .serverChannels] {
            XCTAssertEqual(ListTopFade.spec(screen, statusBar: status).height, status + HomeChrome.barOnlyListTop, "\(screen)")
        }
        for screen in [ListTopFade.Screen.agentPage, .personPage, .groupPage, .serverPage, .channelPage, .ownProfile, .library] {
            XCTAssertEqual(ListTopFade.spec(screen, statusBar: status).height, status + PaneKit.pageTop, "\(screen)")
        }
        // The chat keeps its #1336 band (20 pt into the header, clear 6 pt under it); extras stack.
        XCTAssertEqual(ListTopFade.spec(.chat, statusBar: status).height, status + Controls.top + Controls.diameter + 6)
        XCTAssertEqual(ListTopFade.spec(.chat, statusBar: status, extra: 56).height, status + Controls.top + Controls.diameter + 6 + 56)
    }
}
