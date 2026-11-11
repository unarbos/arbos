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

    private func tap(_ item: XCUIElement) {
        // Always coordinate-tap: Settings sheet chrome often marks rows non-hittable
        // even when they are on screen, and app.swipeUp to "fix" that can dismiss the sheet.
        item.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
    }

    /// The Settings list carries an Agent access link row; tapping it opens the page.
    func testSettingsShowsTheAgentAccessLinkRow() {
        let app = launch(page: nil)
        let row = any(app, "settings-agent-links")
        XCTAssertTrue(row.waitForExistence(timeout: appear),
                      "Settings lists Agent access link")
        tap(row)

        // Scope is on the page even before the list finishes loading.
        XCTAssertTrue(any(app, "agent-link-scope-read").waitForExistence(timeout: appear),
                      "the row opens the Agent access link page")
        XCTAssertTrue(any(app, "agent-link-create").waitForExistence(timeout: appear),
                      "Create is on the page once links are mintable")
        XCTAssertTrue(any(app, "agent-link-row-lnk-fx-claude").waitForExistence(timeout: appear),
                      "fixture active links are listed")
    }

    /// Revoke a fixture link, then Create → Copy → Done and see the new link. No real keys.
    func testCreateCopyAndRevokeALink() {
        let app = launch()
        XCTAssertTrue(any(app, "agent-link-create").waitForExistence(timeout: appear),
                      "the create control is on screen")
        XCTAssertTrue(any(app, "agent-link-row-lnk-fx-codex").waitForExistence(timeout: appear))

        // Revoke first — before the copy cover — so we are not asserting through a dismiss.
        let revoke = any(app, "agent-link-revoke-lnk-fx-codex")
        XCTAssertTrue(revoke.waitForExistence(timeout: appear))
        tap(revoke)
        let codex = any(app, "agent-link-row-lnk-fx-codex")
        let revokedDeadline = Date().addingTimeInterval(10)
        while codex.exists, Date() < revokedDeadline { usleep(100_000) }
        XCTAssertFalse(codex.exists, "Revoke drops the row at once")
        XCTAssertTrue(any(app, "agent-link-row-lnk-fx-claude").exists,
                      "the other live links stay")

        tap(any(app, "agent-link-create"))

        let copy = any(app, "agent-link-copy")
        XCTAssertTrue(copy.waitForExistence(timeout: appear), "the copy screen opens with the link")
        XCTAssertTrue(any(app, "agent-link-share").exists,
                      "Share hands the open URL (the llms.txt path) to the agent")
        XCTAssertFalse(app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'arbl_'")).firstMatch.exists)

        tap(copy)
        let done = any(app, "agent-link-done")
        XCTAssertTrue(done.waitForExistence(timeout: 5))
        let enabledDeadline = Date().addingTimeInterval(5)
        while !done.isEnabled, Date() < enabledDeadline { usleep(100_000) }
        XCTAssertTrue(done.isEnabled, "Done waits for Copy, then is live")
        tap(done)

        XCTAssertTrue(any(app, "agent-link-create").waitForExistence(timeout: appear),
                      "Done returns to the page")
        let copyGoneDeadline = Date().addingTimeInterval(10)
        while copy.exists, Date() < copyGoneDeadline { usleep(100_000) }
        XCTAssertFalse(copy.exists, "the copy screen is gone")
        // The mint added a live row; Claude from the fixture is still there.
        XCTAssertTrue(any(app, "agent-link-row-lnk-fx-claude").waitForExistence(timeout: appear),
                      "the page still lists live links after Create")
        XCTAssertGreaterThan(
            app.descendants(matching: .any).matching(NSPredicate(format: "identifier BEGINSWITH %@", "agent-link-revoke-")).count,
            0,
            "at least one Revoke control remains after Create"
        )
    }
}
