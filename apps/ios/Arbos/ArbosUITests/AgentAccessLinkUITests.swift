import XCTest

/// Settings → Agent access link on the iPhone
/// ([feedback agent-access-link-ios-2026-11-10]; Jacob: give an agent a key from the phone
/// so it can read bug reports sent from iPhone). Guards the Settings row and the
/// create / copy / revoke flow against the Mac's Agent access link page.
///
/// Fixture only (`ARBOS_FIXTURE=1`). The mint URL is a short open-secret tail
/// (`…/fxOpen1`) — never a real bearer (`arbl_…`), and never logged here.
final class AgentAccessLinkUITests: XCTestCase {
    private let appear: TimeInterval = 90

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launch(page: String? = "agent-links") -> XCUIApplication {
        let app = XCUIApplication()
        var env = [
            "ARBOS_FIXTURE": "1",
            "ARBOS_NO_STORE": "1",
            "ARBOS_SHOW": "settings",
        ]
        if let page { env["ARBOS_SETTINGS_PAGE"] = page }
        app.launchEnvironment = env
        app.launch()
        return app
    }

    private func any(_ app: XCUIApplication, _ id: String) -> XCUIElement {
        app.descendants(matching: .any)[id].firstMatch
    }

    /// Tap even when XCTest says the control is not hittable (a Settings row under the
    /// sheet chrome, a Revoke below the fold).
    private func tap(_ el: XCUIElement) {
        if el.isHittable {
            el.tap()
        } else {
            el.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        }
    }

    /// The Settings list carries an Agent access link row that opens the page.
    func testSettingsShowsTheAgentAccessLinkRow() {
        let app = launch(page: nil)
        let row = any(app, "settings-agent-links")
        XCTAssertTrue(row.waitForExistence(timeout: appear),
                      "Settings lists Agent access link")
        // Scroll it into the band under the profile header if the sheet opened tall.
        var spins = 0
        while !row.isHittable, spins < 5 {
            app.swipeUp()
            spins += 1
        }
        tap(row)
        XCTAssertTrue(any(app, "agent-link-create").waitForExistence(timeout: appear),
                      "the row opens the Agent access link page")
        XCTAssertTrue(any(app, "agent-link-scope-read").exists)
        XCTAssertTrue(any(app, "agent-link-row-lnk-fx-claude").exists,
                      "fixture active links are listed")
    }

    /// Create → copy screen → Copy → Done, then Revoke removes a row. No real key values.
    func testCreateCopyAndRevokeALink() {
        let app = launch()
        let create = any(app, "agent-link-create")
        XCTAssertTrue(create.waitForExistence(timeout: appear), "the create control is on screen")
        XCTAssertTrue(any(app, "agent-link-row-lnk-fx-codex").exists)

        tap(create)
        let copy = any(app, "agent-link-copy")
        XCTAssertTrue(copy.waitForExistence(timeout: appear), "the copy screen opens with the link")
        XCTAssertTrue(any(app, "agent-link-share").exists,
                      "Share hands the open URL (the llms.txt path) to the agent")
        // The screen shows a shortened host/tail only — never a bearer prefix.
        XCTAssertFalse(app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'arbl_'")).firstMatch.exists)
        tap(copy)
        let done = any(app, "agent-link-done")
        XCTAssertTrue(done.waitForExistence(timeout: 5))
        XCTAssertTrue(done.isEnabled, "Done waits for Copy, then is live")
        tap(done)

        // Back on the Agent access link page (not Settings root, not Home).
        XCTAssertTrue(create.waitForExistence(timeout: appear), "Done returns to the page")
        XCTAssertFalse(copy.exists, "the copy screen is gone")

        let revoke = any(app, "agent-link-revoke-lnk-fx-codex")
        XCTAssertTrue(revoke.waitForExistence(timeout: appear), "active links are back after Done")
        // The new link sits above; scroll the fixture row into reach if needed.
        var spins = 0
        while !revoke.isHittable, spins < 5 {
            app.swipeUp()
            spins += 1
        }
        tap(revoke)
        let gone = any(app, "agent-link-row-lnk-fx-codex")
        let deadline = Date().addingTimeInterval(10)
        while gone.exists, Date() < deadline { usleep(100_000) }
        XCTAssertFalse(gone.exists, "Revoke drops the row at once")
        XCTAssertTrue(any(app, "agent-link-row-lnk-fx-claude").exists,
                      "the other live links stay")
    }
}
