import XCTest

/// Home's chat list has no All / Agents / People / Servers selector
/// ([feedback chat-list-no-filter-ios-2026-11-10]). Fixture app (`ARBOS_FIXTURE=1`,
/// Debug builds): agents, a people group and (when asked) servers all sit in
/// one linear list, and the old segment buttons are gone. The Mac twin ask is
/// guarded by `HomeChatListNoFilterTests`.
final class HomeChatListNoFilterUITests: XCTestCase {
    /// The shared Mac runs the gate and releases at the same time; waits are generous.
    private let appear: TimeInterval = 90

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launch(servers: Bool = false) -> XCUIApplication {
        let app = XCUIApplication()
        var env = ["ARBOS_FIXTURE": "1"]
        if servers { env["ARBOS_FIXTURE_SERVERS"] = "1" }
        app.launchEnvironment = env
        app.launch()
        XCTAssertTrue(app.buttons["home-account"].waitForExistence(timeout: appear),
                      "Home's account circle is on screen")
        return app
    }

    private func labelled(_ app: XCUIApplication, _ text: String) -> XCUIElement {
        app.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS %@", text)).firstMatch
    }

    /// The four segment labels and the old filter container stay off Home.
    func testTheFilterTogglesAreAbsent() {
        let app = launch()
        for label in ["All", "Agents", "People", "Servers"] {
            XCTAssertFalse(app.buttons[label].exists,
                           "Home no longer shows the \(label) filter tab")
        }
        XCTAssertFalse(app.descendants(matching: .any)["home-list-filter"].exists,
                       "the home-list-filter container is gone")
        // A second pass via any-descendant match, so a renamed accessibility
        // label cannot sneak a segment button past the buttons query alone.
        for label in ["All", "Agents", "People", "Servers"] {
            let tab = app.descendants(matching: .any).matching(
                NSPredicate(format: "label == %@", label)
            ).firstMatch
            XCTAssertFalse(tab.exists, "no \(label) control is reachable from Home")
        }
    }

    /// Agents and people chats sit together in the one list (the fixture's
    /// Expense Manager agent and Launch week group).
    func testTheListShowsAgentsAndPeopleTogether() {
        let app = launch()
        XCTAssertTrue(labelled(app, "Expense Manager").waitForExistence(timeout: appear),
                      "an agent row is in the list")
        XCTAssertTrue(labelled(app, "Launch week").exists,
                      "a people group is in the same list")
    }

    /// With servers seeded, a server row joins the same linear list — no
    /// Servers tab to flip to.
    func testTheListShowsServersWithTheOtherChats() {
        let app = launch(servers: true)
        XCTAssertTrue(labelled(app, "Expense Manager").waitForExistence(timeout: appear),
                      "an agent row is in the list")
        XCTAssertTrue(labelled(app, "Launch week").exists,
                      "a people group is in the same list")
        // The fixture's first server is "Arbos HQ" (`Fixture.swift`).
        XCTAssertTrue(labelled(app, "Arbos HQ").waitForExistence(timeout: appear),
                      "a server row sits with the other chats")
    }
}
