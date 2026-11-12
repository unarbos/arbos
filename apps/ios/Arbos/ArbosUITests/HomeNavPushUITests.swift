import XCTest

/// Home's account circle and "+" push Settings and New Message from the right, with the
/// stack's interactive edge swipe back — not bottom sheets (Jacob, 2026-11-10:
/// [feedback iphone-settings-nav-push-2026-11-10],
/// [feedback iphone-compose-nav-push-2026-11-10]).
///
/// Runs the fixture app (`ARBOS_FIXTURE=1`, Debug builds): no relay, deterministic screens.
final class HomeNavPushUITests: XCTestCase {
    private let appear: TimeInterval = 90
    private let settle: TimeInterval = 20

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launch() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1"]
        app.launch()
        XCTAssertTrue(app.buttons["home-account"].waitForExistence(timeout: appear), "Home is up")
        return app
    }

    /// One finger from the very left edge to past the middle — the system's interactive pop.
    private func edgeSwipeBack(_ app: XCUIApplication, y: CGFloat = 0.45) {
        let from = app.coordinate(withNormalizedOffset: CGVector(dx: 0.0, dy: y))
        let to = app.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: y))
        from.press(forDuration: 0.05, thenDragTo: to, withVelocity: .fast, thenHoldForDuration: 0.05)
    }

    private func assertHome(_ app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertTrue(app.buttons["home-account"].waitForExistence(timeout: settle),
                      "Home's account circle is on screen", file: file, line: line)
        XCTAssertTrue(app.buttons["home-new"].exists, "Home's + is on screen", file: file, line: line)
        // Filter tabs are gone ([feedback chat-list-no-filter-ios-2026-11-10]).
        XCTAssertFalse(app.buttons["Agents"].exists, "the list filter is gone", file: file, line: line)
    }

    private func assertGone(_ element: XCUIElement, file: StaticString = #filePath, line: UInt = #line) {
        let gone = NSPredicate(format: "exists == false")
        let wait = XCTNSPredicateExpectation(predicate: gone, object: element)
        XCTAssertEqual(XCTWaiter().wait(for: [wait], timeout: settle), .completed,
                       "\(element) left the screen", file: file, line: line)
    }

    /// A pushed page is not a sheet: the sheet cancel control is absent.
    private func assertNotASheet(_ app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertFalse(app.buttons["sheet-cancel"].exists,
                       "a sheet cancel would mean this rose as a bottom sheet", file: file, line: line)
    }

    // MARK: Settings

    /// [feedback iphone-settings-nav-push-2026-11-10]
    func testAvatarPushesSettingsNotASheet() {
        let app = launch()
        app.buttons["home-account"].tap()
        let back = app.buttons["Back"].firstMatch
        XCTAssertTrue(back.waitForExistence(timeout: settle), "Settings drew its glass back chevron")
        assertNotASheet(app)
        // The Settings tab (or Credits row) is on the pushed pane.
        let credits = app.descendants(matching: .any)["Credits"].firstMatch
        let settingsTab = app.buttons["Settings"].firstMatch
        XCTAssertTrue(credits.waitForExistence(timeout: settle) || settingsTab.exists,
                      "Settings content is on screen")
        // Home's chrome is covered / not the frontmost screen.
        XCTAssertFalse(app.buttons["home-new"].isHittable, "Home's + is behind the pushed Settings")
    }

    /// [feedback iphone-settings-nav-push-2026-11-10]
    func testSettingsEdgeSwipeReturnsToChatList() {
        let app = launch()
        app.buttons["home-account"].tap()
        let back = app.buttons["Back"].firstMatch
        XCTAssertTrue(back.waitForExistence(timeout: settle), "Settings opened")
        edgeSwipeBack(app)
        assertGone(back)
        assertHome(app)
    }

    /// [feedback iphone-settings-nav-push-2026-11-10]
    func testSubSettingPushesAndSwipesBackOneLevel() {
        let app = launch()
        app.buttons["home-account"].tap()
        XCTAssertTrue(app.buttons["Back"].firstMatch.waitForExistence(timeout: settle), "Settings opened")

        // Settings chrome often marks pane rows non-hittable even when they are on
        // screen (same as AgentAccessLinkUITests): coordinate-tap, do not swipeUp.
        let credits = app.descendants(matching: .any).matching(
            NSPredicate(format: "label == %@ OR identifier == %@", "Credits", "Credits")
        ).firstMatch
        XCTAssertTrue(credits.waitForExistence(timeout: settle), "the Credits row is on Settings")
        credits.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()

        // Credits page title is on screen; Settings root chrome is under it.
        let creditsTitle = app.staticTexts["Credits"].firstMatch
        XCTAssertTrue(creditsTitle.waitForExistence(timeout: settle), "Credits pushed in from the right")
        assertNotASheet(app)

        edgeSwipeBack(app)
        // One level: back on Settings, not yet Home.
        XCTAssertTrue(app.buttons["Back"].firstMatch.waitForExistence(timeout: settle),
                      "Settings is still under Credits after one swipe")
        let settingsTab = app.buttons["Settings"].firstMatch
        let profileTab = app.buttons["Profile"].firstMatch
        XCTAssertTrue(settingsTab.exists || profileTab.exists ||
                      app.descendants(matching: .any)["Credits"].firstMatch.exists,
                      "landed back on Settings, not Home")
        XCTAssertFalse(app.buttons["home-account"].isHittable, "Home is still under Settings")

        edgeSwipeBack(app)
        assertHome(app)
    }

    // MARK: Compose / +

    /// [feedback iphone-compose-nav-push-2026-11-10]
    func testPlusPushesNewMessageNotASheet() {
        let app = launch()
        let plus = app.buttons["home-new"]
        XCTAssertTrue(plus.waitForExistence(timeout: settle), "Home + is on screen")
        plus.tap()
        let page = app.descendants(matching: .any)["new-message"]
        XCTAssertTrue(page.waitForExistence(timeout: settle), "New Message pushed in")
        // Prefer the identifier over the buttons query — the glass back is a Button, but
        // descendants are more reliable across SwiftUI accessibility trees.
        let back = app.descendants(matching: .any)["new-message-back"]
        XCTAssertTrue(back.waitForExistence(timeout: settle),
                      "New Message drew its glass back chevron")
        assertNotASheet(app)
        XCTAssertTrue(app.staticTexts["New Message"].exists || app.staticTexts["New agent"].exists,
                      "New Message content is on screen")
        XCTAssertFalse(app.buttons["home-account"].isHittable, "Home's avatar is behind the pushed page")
    }

    /// [feedback iphone-compose-nav-push-2026-11-10]
    func testNewMessageEdgeSwipeReturnsToChatList() {
        let app = launch()
        let plus = app.buttons["home-new"]
        XCTAssertTrue(plus.waitForExistence(timeout: settle), "Home + is on screen")
        plus.tap()
        let back = app.descendants(matching: .any)["new-message-back"]
        XCTAssertTrue(back.waitForExistence(timeout: settle), "New Message opened")
        edgeSwipeBack(app)
        assertGone(back)
        assertHome(app)
    }
}
