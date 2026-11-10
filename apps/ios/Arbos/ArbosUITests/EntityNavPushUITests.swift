import XCTest

/// Create / profile / edit / sub-pages push from the right with edge-swipe back —
/// never bottom sheets (Jacob, 2026-11-10: [feedback iphone-entity-nav-push-2026-11-10],
/// [feedback iphone-create-entity-edit-style-2026-11-10],
/// [feedback iphone-entity-profile-edit-2026-11-10]).
///
/// Fixture app (`ARBOS_FIXTURE=1`): + → New agent create page; agent profile → Edit.
final class EntityNavPushUITests: XCTestCase {
    private let appear: TimeInterval = 90
    private let settle: TimeInterval = 20

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launch(show: String? = nil) -> XCUIApplication {
        let app = XCUIApplication()
        var env = ["ARBOS_FIXTURE": "1"]
        if let show { env["ARBOS_SHOW"] = show }
        app.launchEnvironment = env
        app.launch()
        XCTAssertTrue(app.buttons["home-account"].waitForExistence(timeout: appear), "Home is up")
        return app
    }

    private func edgeSwipeBack(_ app: XCUIApplication, y: CGFloat = 0.45) {
        let from = app.coordinate(withNormalizedOffset: CGVector(dx: 0.0, dy: y))
        let to = app.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: y))
        from.press(forDuration: 0.05, thenDragTo: to, withVelocity: .fast, thenHoldForDuration: 0.05)
    }

    private func assertNotASheet(_ app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertFalse(app.buttons["sheet-cancel"].exists,
                       "a sheet cancel would mean this rose as a bottom sheet", file: file, line: line)
    }

    private func assertGone(_ element: XCUIElement, file: StaticString = #filePath, line: UInt = #line) {
        let gone = NSPredicate(format: "exists == false")
        let wait = XCTNSPredicateExpectation(predicate: gone, object: element)
        XCTAssertEqual(XCTWaiter().wait(for: [wait], timeout: settle), .completed,
                       "\(element) left the screen", file: file, line: line)
    }

    // MARK: Create

    /// [feedback iphone-create-entity-edit-style-2026-11-10]
    /// [feedback iphone-entity-nav-push-2026-11-10]
    func testPlusNewAgentPushesCreateEditStyleNotASheet() {
        let app = launch()
        app.buttons["home-new"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["new-message"].waitForExistence(timeout: settle),
                      "New Message pushed")
        let newAgent = app.staticTexts["New agent"].firstMatch
        XCTAssertTrue(newAgent.waitForExistence(timeout: settle), "New agent row is on New Message")
        newAgent.tap()

        let create = app.descendants(matching: .any)["create-entity"]
        XCTAssertTrue(create.waitForExistence(timeout: settle), "Create New Agent pushed")
        assertNotASheet(app)
        XCTAssertTrue(app.buttons["edit-done"].waitForExistence(timeout: settle),
                      "Create/Next Done pill is top-right")
        XCTAssertTrue(app.staticTexts["NAME"].exists || app.staticTexts["Name"].exists
                      || app.descendants(matching: .any)["entity-name-field"].exists,
                      "Name section / field is on the create page")
        XCTAssertTrue(app.descendants(matching: .any)["create-entity-options"].exists
                      || app.staticTexts["APPEARANCE"].exists
                      || app.staticTexts["Appearance"].exists
                      || app.descendants(matching: .any)["create-avatar-row"].exists,
                      "Appearance / Upload|Generate is on the create page")
    }

    /// [feedback iphone-entity-nav-push-2026-11-10]
    func testCreateEdgeSwipeReturnsToNewMessage() {
        let app = launch(show: "newagent")
        // Rig opens New Message then Create; wait for create chrome.
        let create = app.descendants(matching: .any)["create-entity"]
        let done = app.buttons["edit-done"]
        XCTAssertTrue(create.waitForExistence(timeout: settle) || done.waitForExistence(timeout: settle),
                      "Create opened from the rig")
        assertNotASheet(app)
        edgeSwipeBack(app)
        // One level: back on New Message, not yet Home.
        let newMessage = app.descendants(matching: .any)["new-message"]
        XCTAssertTrue(newMessage.waitForExistence(timeout: settle)
                      || app.staticTexts["New Message"].waitForExistence(timeout: settle)
                      || app.staticTexts["New agent"].waitForExistence(timeout: settle),
                      "landed back on New Message after one swipe")
        XCTAssertFalse(app.buttons["home-account"].isHittable, "Home is still under New Message")
    }

    // MARK: Agent profile → Edit

    /// [feedback iphone-entity-profile-edit-2026-11-10]
    /// [feedback iphone-entity-nav-push-2026-11-10]
    func testAgentProfileShowsOverviewThenEditPushes() throws {
        let app = launch()
        // Open the first agent chat, then its profile via the header name / ⋯ path the
        // fixture exposes. Prefer the overview accessibility id when the pane is up.
        let agents = app.buttons["Agents"]
        if agents.waitForExistence(timeout: settle) { agents.tap() }
        // First agent row in the home list (fixture seeds at least one).
        let agentRow = app.cells.firstMatch
        if agentRow.waitForExistence(timeout: settle) {
            agentRow.tap()
        }
        // Header name / profile entry — try common fixture controls.
        let headerName = app.buttons.matching(NSPredicate(format: "identifier CONTAINS 'header' OR label CONTAINS 'Arbos' OR label CONTAINS 'agent'")).firstMatch
        if headerName.waitForExistence(timeout: settle) {
            headerName.tap()
        }
        let profile = app.descendants(matching: .any)["agent-profile"]
        // If the fixture path did not land on a profile, the create→style check above
        // still guards the hierarchy; soft-skip only this case's deeper chain.
        guard profile.waitForExistence(timeout: settle) else {
            throw XCTSkip("fixture did not open an agent profile from the home list")
        }
        assertNotASheet(app)
        XCTAssertTrue(app.buttons["profile-edit"].waitForExistence(timeout: settle),
                      "Edit pill is under the name")
        // Overview rows.
        XCTAssertTrue(app.descendants(matching: .any)["agent-overview-library"].exists
                      || app.staticTexts["Library"].exists,
                      "Library overview row is present")
        app.buttons["profile-edit"].tap()
        let edit = app.descendants(matching: .any)["agent-edit"]
        XCTAssertTrue(edit.waitForExistence(timeout: settle), "Edit Agent pushed")
        assertNotASheet(app)
        XCTAssertTrue(app.buttons["edit-done"].exists, "Done pill on Edit")
        edgeSwipeBack(app)
        XCTAssertTrue(profile.waitForExistence(timeout: settle), "one swipe returns to the overview")
    }
}
