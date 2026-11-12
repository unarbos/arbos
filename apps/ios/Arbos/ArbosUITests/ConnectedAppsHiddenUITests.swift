import XCTest

/// Settings must not list Connected apps until the feature ships
/// ([feedback connected-apps-hidden-until-implemented-2026-11-10]).
/// Same rule on the Mac (`SettingsPage::LISTED` omits Connectors).
///
/// Fixture only (`ARBOS_FIXTURE=1`). Lands on the Settings sheet and checks the
/// row title is absent — no stub page, no search hit, no list entry.
final class ConnectedAppsHiddenUITests: XCTestCase {
    private let appear: TimeInterval = 90

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launchSettings() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = [
            "ARBOS_FIXTURE": "1",
            "ARBOS_NO_STORE": "1",
            "ARBOS_SHOW": "settings",
        ]
        app.launch()
        return app
    }

    /// Settings is up (Credits is still listed) and Connected apps is not.
    func testSettingsDoesNotListConnectedApps() {
        let app = launchSettings()
        let credits = app.staticTexts["Credits"]
        XCTAssertTrue(credits.waitForExistence(timeout: appear), "Settings lists Credits")
        XCTAssertFalse(app.staticTexts["Connected apps"].exists,
                       "Connected apps stays off Settings until the feature ships")
        // A second pass via any-descendant match, so a renamed accessibility
        // label cannot sneak the row back past the staticTexts query alone.
        let connected = app.descendants(matching: .any).matching(
            NSPredicate(format: "label == %@", "Connected apps")
        ).firstMatch
        XCTAssertFalse(connected.exists,
                       "no Connected apps control is reachable from Settings")
    }
}
