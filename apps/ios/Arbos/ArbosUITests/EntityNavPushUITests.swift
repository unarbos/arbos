import XCTest

/// Create / profile / edit / sub-pages push from the right with edge-swipe back —
/// never bottom sheets (Jacob, 2026-11-10: [feedback iphone-entity-nav-push-2026-11-10],
/// [feedback iphone-create-entity-edit-style-2026-11-10],
/// [feedback iphone-entity-profile-edit-2026-11-10]).
///
/// Fixture app (`ARBOS_FIXTURE=1`): open create / profile via `ARBOS_SHOW` so the
/// path does not depend on scrolling the home list.
final class EntityNavPushUITests: XCTestCase {
    private let appear: TimeInterval = 90
    private let settle: TimeInterval = 30

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launch(_ env: [String: String] = [:]) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1"].merging(env) { _, new in new }
        app.launch()
        return app
    }

    private func any(_ app: XCUIApplication, _ id: String) -> XCUIElement {
        app.descendants(matching: .any)[id].firstMatch
    }

    /// Coordinate tap: Settings / profile chrome often reports `{-1,-1}` hit points
    /// for plain `.tap()` even when the control is on screen (same pattern as
    /// AgentAccessLinkUITests).
    private func tap(_ item: XCUIElement) {
        item.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
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

    // MARK: Create

    /// [feedback iphone-create-entity-edit-style-2026-11-10]
    /// [feedback iphone-entity-nav-push-2026-11-10]
    func testPlusNewAgentPushesCreateEditStyleNotASheet() {
        // Prefill the name so the Create pill is enabled (disabled pills are easy to miss
        // in the buttons query on some runners).
        let app = launch([
            "ARBOS_SHOW": "newagent",
            "ARBOS_NEW_AGENT_NAME": "Arbos",
        ])
        let create = any(app, "create-entity")
        let done = any(app, "edit-done")
        XCTAssertTrue(create.waitForExistence(timeout: appear) || done.waitForExistence(timeout: appear),
                      "Create New Agent opened from the rig")
        assertNotASheet(app)
        XCTAssertTrue(done.waitForExistence(timeout: settle)
                      || app.buttons["Create"].firstMatch.waitForExistence(timeout: settle),
                      "Create/Next Done pill is top-right")
        XCTAssertTrue(any(app, "entity-name-field").exists
                      || app.staticTexts["NAME"].exists
                      || app.staticTexts["Name"].exists,
                      "Name section / field is on the create page")
        XCTAssertTrue(any(app, "create-entity-options").exists
                      || any(app, "create-avatar-row").exists
                      || app.staticTexts["APPEARANCE"].exists
                      || app.staticTexts["Appearance"].exists,
                      "Appearance / Upload|Generate is on the create page")
    }

    /// [feedback iphone-entity-nav-push-2026-11-10]
    func testCreateEdgeSwipeReturnsToNewMessage() {
        let app = launch([
            "ARBOS_SHOW": "newagent",
            "ARBOS_NEW_AGENT_NAME": "Arbos",
        ])
        let create = any(app, "create-entity")
        let done = any(app, "edit-done")
        XCTAssertTrue(create.waitForExistence(timeout: appear) || done.waitForExistence(timeout: appear),
                      "Create opened from the rig")
        assertNotASheet(app)
        edgeSwipeBack(app)
        let newMessage = any(app, "new-message")
        XCTAssertTrue(newMessage.waitForExistence(timeout: settle)
                      || app.staticTexts["New Message"].waitForExistence(timeout: settle)
                      || app.staticTexts["New agent"].waitForExistence(timeout: settle),
                      "landed back on New Message after one swipe")
        XCTAssertFalse(app.buttons["home-account"].isHittable, "Home is still under New Message")
    }

    // MARK: Agent profile → Edit

    /// [feedback iphone-entity-profile-edit-2026-11-10]
    /// [feedback iphone-entity-nav-push-2026-11-10]
    func testAgentProfileShowsOverviewThenEditPushes() {
        let app = launch([
            "ARBOS_OPEN_PROJECT": "fx-comms",
            "ARBOS_OPEN_CHAT": "fx-comms-main",
            "ARBOS_SHOW": "profile",
        ])
        let profile = any(app, "agent-profile")
        XCTAssertTrue(profile.waitForExistence(timeout: appear), "agent profile opened from the rig")
        assertNotASheet(app)
        let editPill = any(app, "profile-edit")
        XCTAssertTrue(editPill.waitForExistence(timeout: settle), "Edit pill is under the name")
        XCTAssertTrue(any(app, "agent-overview-library").exists
                      || app.staticTexts["Library"].exists,
                      "Library overview row is present")
        tap(editPill)
        let edit = any(app, "agent-edit")
        XCTAssertTrue(edit.waitForExistence(timeout: settle), "Edit Agent pushed")
        assertNotASheet(app)
        XCTAssertTrue(any(app, "edit-done").waitForExistence(timeout: settle)
                      || app.buttons["Done"].firstMatch.waitForExistence(timeout: settle),
                      "Done pill on Edit")
        edgeSwipeBack(app)
        XCTAssertTrue(profile.waitForExistence(timeout: settle), "one swipe returns to the overview")
    }
}
