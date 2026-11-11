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

    private func el(_ app: XCUIApplication, _ id: String) -> XCUIElement {
        // Prefer buttons: our controls are Buttons with accessibility identifiers.
        let button = app.buttons[id]
        if button.waitForExistence(timeout: 2) { return button }
        return app.descendants(matching: .any)[id].firstMatch
    }

    private func tap(_ item: XCUIElement) {
        if item.isHittable {
            item.tap()
        } else {
            item.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        }
    }

    /// The Settings list carries an Agent access link row that opens the page.
    func testSettingsShowsTheAgentAccessLinkRow() {
        let app = launch(page: nil)
        let row = el(app, "settings-agent-links")
        XCTAssertTrue(row.waitForExistence(timeout: appear),
                      "Settings lists Agent access link")
        var spins = 0
        while !row.isHittable, spins < 6 {
            app.swipeUp()
            spins += 1
        }
        tap(row)

        let create = el(app, "agent-link-create")
        XCTAssertTrue(create.waitForExistence(timeout: appear),
                      "the row opens the Agent access link page")
        XCTAssertTrue(el(app, "agent-link-scope-read").exists)
        XCTAssertTrue(el(app, "agent-link-row-lnk-fx-claude").waitForExistence(timeout: 10),
                      "fixture active links are listed")
    }

    /// Create → copy screen → Copy → Done, then Revoke removes a row. No real key values.
    func testCreateCopyAndRevokeALink() {
        let app = launch()
        let create = el(app, "agent-link-create")
        XCTAssertTrue(create.waitForExistence(timeout: appear), "the create control is on screen")
        XCTAssertTrue(el(app, "agent-link-row-lnk-fx-codex").exists)

        tap(create)

        let copy = el(app, "agent-link-copy")
        XCTAssertTrue(copy.waitForExistence(timeout: appear), "the copy screen opens with the link")
        XCTAssertTrue(el(app, "agent-link-share").exists,
                      "Share hands the open URL (the llms.txt path) to the agent")
        // The screen shows a shortened host/tail only — never a bearer prefix.
        XCTAssertFalse(app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'arbl_'")).firstMatch.exists)

        tap(copy)
        let done = el(app, "agent-link-done")
        XCTAssertTrue(done.waitForExistence(timeout: 5))
        let enabledDeadline = Date().addingTimeInterval(5)
        while !done.isEnabled, Date() < enabledDeadline { usleep(100_000) }
        XCTAssertTrue(done.isEnabled, "Done waits for Copy, then is live")
        tap(done)

        XCTAssertTrue(create.waitForExistence(timeout: appear), "Done returns to the page")
        let copyGoneDeadline = Date().addingTimeInterval(10)
        while copy.exists, Date() < copyGoneDeadline { usleep(100_000) }
        XCTAssertFalse(copy.exists, "the copy screen is gone")

        let revoke = el(app, "agent-link-revoke-lnk-fx-codex")
        XCTAssertTrue(revoke.waitForExistence(timeout: appear), "active links are back after Done")
        var spins = 0
        while !revoke.isHittable, spins < 6 {
            app.swipeUp()
            spins += 1
        }
        tap(revoke)

        let gone = el(app, "agent-link-row-lnk-fx-codex")
        let deadline = Date().addingTimeInterval(10)
        while gone.exists, Date() < deadline { usleep(100_000) }
        XCTAssertFalse(gone.exists, "Revoke drops the row at once")
        XCTAssertTrue(el(app, "agent-link-row-lnk-fx-claude").exists,
                      "the other live links stay")
    }
}
