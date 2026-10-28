import XCTest

/// Home's All | Agents | People | Servers tabs take a tap anywhere on the tab — the
/// capsule, the band over and under it, the gap to the neighbour — not only on the
/// label's glyphs ([feedback iphone-filter-tab-taps-2026-10-28]; Jacob, TestFlight
/// 202610281148575584: "can't click All"). Runs the fixture app (`ARBOS_FIXTURE=1`,
/// Debug builds): no relay, deterministic rows.
///
/// The filter is read back from the list, not from the tabs: an agent row ("Expense
/// Manager") is listed under All and Agents, the "Launch week" group under All and People,
/// and Servers shows its empty copy (`home-filter-empty`; the fixture lists no servers
/// unless asked). The tapped tab must also carry the selected trait.
final class HomeFilterTabsUITests: XCTestCase {
    /// The shared Mac runs the gate and releases at the same time; waits are generous.
    private let appear: TimeInterval = 90
    private let settle: TimeInterval = 20

    enum Filter: String, CaseIterable {
        case all = "All", agents = "Agents", people = "People", servers = "Servers"
    }

    /// Where on a tab the taps land: the four edges and the four corners, just inside the
    /// tab's frame — the places a thumb lands when it misses the word. Kept a little
    /// inside 0/1 so XCTest's element frame (which can be a hair larger than the hit
    /// shape) still lands in the padded band.
    static let edgeOffsets: [CGVector] = [
        CGVector(dx: 0.08, dy: 0.12), CGVector(dx: 0.92, dy: 0.12),
        CGVector(dx: 0.08, dy: 0.88), CGVector(dx: 0.92, dy: 0.88),
        CGVector(dx: 0.5, dy: 0.10), CGVector(dx: 0.5, dy: 0.90),
        CGVector(dx: 0.08, dy: 0.5), CGVector(dx: 0.92, dy: 0.5),
    ]

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launch() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1"]
        app.launch()
        XCTAssertTrue(app.buttons[Filter.agents.rawValue].waitForExistence(timeout: appear), "Home's list filter is on screen")
        for f in Filter.allCases {
            XCTAssertTrue(app.buttons[f.rawValue].exists, "the \(f.rawValue) tab is on screen")
        }
        return app
    }

    private func tab(_ app: XCUIApplication, _ f: Filter) -> XCUIElement { app.buttons[f.rawValue] }

    private func labelled(_ app: XCUIApplication, _ text: String) -> XCUIElement {
        app.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS %@", text)).firstMatch
    }

    /// The filter the list shows right now, or nil while it is between two states.
    private func shownFilter(_ app: XCUIApplication) -> Filter? {
        let agents = labelled(app, "Expense Manager").exists
        let people = labelled(app, "Launch week").exists
        let noServers = app.descendants(matching: .any)["home-filter-empty"].exists
        switch (agents, people, noServers) {
        case (true, true, false): return .all
        case (true, false, false): return .agents
        case (false, true, false): return .people
        case (false, false, true): return .servers
        default: return nil
        }
    }

    /// Wait until the list shows `f` (the 0.2 s snappy animation plus a slow Mac).
    private func assertShown(_ app: XCUIApplication, _ f: Filter, _ what: String,
                             file: StaticString = #filePath, line: UInt = #line) {
        let deadline = Date().addingTimeInterval(settle)
        var seen = shownFilter(app)
        while seen != f, Date() < deadline {
            usleep(150_000)
            seen = shownFilter(app)
        }
        XCTAssertEqual(seen, f, "\(what): the list shows \(f.rawValue)", file: file, line: line)
        XCTAssertTrue(tab(app, f).isSelected, "\(what): the \(f.rawValue) tab is selected", file: file, line: line)
    }

    /// A tab other than `f`, as far from it as possible, so the tap under test is a real change.
    private func other(than f: Filter) -> Filter {
        switch f {
        case .all: return .servers
        case .agents: return .servers
        case .people: return .all
        case .servers: return .all
        }
    }

    // MARK: Every tab, its whole area

    func testEveryTabTakesATapOnItsEdgesAndCorners() {
        let app = launch()
        assertShown(app, .all, "at launch")
        for f in Filter.allCases {
            for (i, offset) in Self.edgeOffsets.enumerated() {
                let reset = other(than: f)
                tab(app, reset).tap()
                assertShown(app, reset, "reset before \(f.rawValue) tap \(i)")
                tab(app, f).coordinate(withNormalizedOffset: offset).tap()
                assertShown(app, f, "\(f.rawValue) tapped at (\(offset.dx), \(offset.dy))")
            }
        }
    }

    /// The word itself still works (the one place the old tabs took a tap).
    func testEveryTabTakesATapOnItsLabel() {
        let app = launch()
        for f in [Filter.agents, .people, .servers, .all] {
            tab(app, f).tap()
            assertShown(app, f, "\(f.rawValue) tapped on its label")
        }
    }

    // MARK: Size

    /// Apple's 44 pt: every tab is at least 44 pt tall and wide, and the row tiles — no dead
    /// strip between two tabs.
    func testTabsAreAtLeast44PointsAndTile() {
        let app = launch()
        let frames = Filter.allCases.map { tab(app, $0).frame }
        for (f, frame) in zip(Filter.allCases, frames) {
            XCTAssertGreaterThanOrEqual(frame.height, 44, "\(f.rawValue) is 44 pt tall (\(frame.height))")
            XCTAssertGreaterThanOrEqual(frame.width, 44, "\(f.rawValue) is 44 pt wide (\(frame.width))")
        }
        for i in 1..<frames.count {
            XCTAssertEqual(frames[i].minX, frames[i - 1].maxX, accuracy: 1,
                           "\(Filter.allCases[i].rawValue) starts where \(Filter.allCases[i - 1].rawValue) ends")
        }
    }
}
