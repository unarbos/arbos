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

    /// The archived test's chat goes back on the chat list whatever happened in the test:
    /// the archive is kept across launches, and a failure half way must not leave the
    /// fixture changed for the tests after it.
    private func unarchiveFixtureDM() {
        let app = XCUIApplication()
        app.terminate()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1"]
        app.launch()
        guard app.buttons["home-account"].waitForExistence(timeout: appear),
              !element(app, "home-row:fx-dm-unconst").waitForExistence(timeout: 5) else { return }
        let footer = app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'Archived chats'")).firstMatch
        if !footer.waitForExistence(timeout: 3) { app.swipeUp() }
        guard footer.waitForExistence(timeout: settle) else { return }
        tap(footer)
        let archived = app.staticTexts["Unconst"].firstMatch
        guard archived.waitForExistence(timeout: settle) else { return }
        archived.swipeLeft()
        let unarchive = app.buttons["Unarchive"].firstMatch
        if unarchive.waitForExistence(timeout: 5) { tap(unarchive) }
        _ = element(app, "home-row:fx-dm-unconst").waitForExistence(timeout: 1)
    }

    /// Settings / New Message chrome often marks rows and glass discs non-hittable
    /// (`hit point {-1,-1}`); coordinate-tap still reaches them (same as AgentAccessLinkUITests).
    private func tap(_ item: XCUIElement) {
        item.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
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
        tap(app.buttons["home-account"])
        let back = app.buttons["Back"].firstMatch
        XCTAssertTrue(back.waitForExistence(timeout: settle), "Settings drew its glass back chevron")
        assertNotASheet(app)
        // The Settings tab (or Credits row) is on the pushed pane.
        let credits = app.descendants(matching: .any)["settings-credits"].firstMatch
        let settingsTab = app.buttons["Settings"].firstMatch
        XCTAssertTrue(credits.waitForExistence(timeout: settle) || settingsTab.exists,
                      "Settings content is on screen")
        // Home's chrome is covered / not the frontmost screen.
        XCTAssertFalse(app.buttons["home-new"].isHittable, "Home's + is behind the pushed Settings")
    }

    /// [feedback iphone-settings-nav-push-2026-11-10]
    func testSettingsEdgeSwipeReturnsToChatList() {
        let app = launch()
        tap(app.buttons["home-account"])
        let back = app.buttons["Back"].firstMatch
        XCTAssertTrue(back.waitForExistence(timeout: settle), "Settings opened")
        edgeSwipeBack(app)
        assertGone(back)
        assertHome(app)
    }

    /// [feedback iphone-settings-nav-push-2026-11-10]
    func testSubSettingPushesAndSwipesBackOneLevel() {
        let app = launch()
        tap(app.buttons["home-account"])
        XCTAssertTrue(app.buttons["Back"].firstMatch.waitForExistence(timeout: settle), "Settings opened")

        // Prefer the stable id; Settings chrome often marks the row non-hittable even when
        // it is on screen — coordinate-tap like AgentAccessLinkUITests. Do not swipeUp,
        // which parked the row under the sticky ProfileTopBar chrome.
        let credits = app.descendants(matching: .any)["settings-credits"].firstMatch
        XCTAssertTrue(credits.waitForExistence(timeout: settle), "the Credits row is on Settings")
        tap(credits)

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
                      app.descendants(matching: .any)["settings-credits"].firstMatch.exists,
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
        tap(plus)
        let page = app.descendants(matching: .any)["new-message"].firstMatch
        XCTAssertTrue(page.waitForExistence(timeout: settle), "New Message pushed in")
        // Prefer the identifier over the buttons query — the glass back is a Button, but
        // descendants are more reliable across SwiftUI accessibility trees.
        let back = app.descendants(matching: .any)["new-message-back"].firstMatch
        XCTAssertTrue(back.waitForExistence(timeout: settle),
                      "New Message drew its glass back chevron")
        assertNotASheet(app)
        // Title is accessibility-hidden so it does not cover the back chevron; "New agent"
        // (first card row) still proves the compose body is on screen.
        XCTAssertTrue(app.staticTexts["New agent"].exists || app.staticTexts["New Message"].exists
                        || app.buttons["Back"].exists,
                      "New Message content is on screen")
        XCTAssertFalse(app.buttons["home-account"].isHittable, "Home's avatar is behind the pushed page")
    }

    /// [feedback iphone-compose-nav-push-2026-11-10]
    func testNewMessageEdgeSwipeReturnsToChatList() {
        let app = launch()
        let plus = app.buttons["home-new"]
        XCTAssertTrue(plus.waitForExistence(timeout: settle), "Home + is on screen")
        tap(plus)
        let back = app.descendants(matching: .any)["new-message-back"].firstMatch
        XCTAssertTrue(back.waitForExistence(timeout: settle), "New Message opened")
        edgeSwipeBack(app)
        assertGone(back)
        assertHome(app)
    }

    // MARK: Back returns to the list it came from

    private func element(_ app: XCUIApplication, _ id: String) -> XCUIElement {
        app.descendants(matching: .any)[id].firstMatch
    }

    /// Still on screen after the transitions have run: the old code opened the chat and
    /// popped it a moment later.
    private func assertStays(_ item: XCUIElement, _ what: String, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertTrue(item.waitForExistence(timeout: settle), what, file: file, line: line)
        sleep(2)
        XCTAssertTrue(item.exists, "\(what), and it stays", file: file, line: line)
    }

    /// Tester report, 2026-11-15: "I select a user it gives me a black screen".
    func testAPersonPickedOnNewMessageShowsTheirChat() {
        let app = launch()
        tap(app.buttons["home-new"])
        XCTAssertTrue(element(app, "new-message").waitForExistence(timeout: settle), "New Message opened")
        let person = app.staticTexts["Alex Rivera"].firstMatch
        if !person.waitForExistence(timeout: 3) { app.swipeUp() }
        XCTAssertTrue(person.waitForExistence(timeout: settle), "the person is listed")
        tap(person)
        assertStays(element(app, "chat-header-pill"), "the chat with the person is shown")
        XCTAssertFalse(app.buttons["home-new"].exists, "not back on Home")
    }

    /// Tester report, 2026-11-17: back from a profile opened in a group's members list
    /// went to the page before the list. Then the profile's Message with a chat that
    /// already exists, which must show that chat.
    func testBackFromAMembersProfileLandsOnMembers() {
        let app = launch()
        tap(element(app, "home-row:fx-group-launch"))
        let pill = element(app, "chat-header-pill")
        XCTAssertTrue(pill.waitForExistence(timeout: settle), "the group chat opened")
        tap(pill)
        let membersRow = element(app, "group-overview-members")
        XCTAssertTrue(membersRow.waitForExistence(timeout: settle), "the group page opened")
        tap(membersRow)
        let mira = app.staticTexts["Mira Kaplan"].firstMatch
        XCTAssertTrue(mira.waitForExistence(timeout: settle), "the members list opened")

        tap(mira)
        let viewProfile = app.buttons["View profile"].firstMatch
        XCTAssertTrue(viewProfile.waitForExistence(timeout: settle), "the member's actions are offered")
        viewProfile.tap()
        let message = app.buttons["Message"].firstMatch
        XCTAssertTrue(message.waitForExistence(timeout: settle), "the member's profile opened")
        edgeSwipeBack(app)
        assertGone(message)
        XCTAssertTrue(mira.waitForExistence(timeout: settle), "back lands on the members list")
        XCTAssertFalse(membersRow.exists, "not on the group page before it")

        tap(app.staticTexts["Unconst"].firstMatch)
        XCTAssertTrue(viewProfile.waitForExistence(timeout: settle), "the member's actions are offered")
        viewProfile.tap()
        XCTAssertTrue(message.waitForExistence(timeout: settle), "the member's profile opened")
        tap(message)
        assertStays(element(app, "chat-back:fx-dm-unconst"), "the existing chat with the member is shown")
    }

    /// Tester report, 2026-11-17: back from a chat opened in the archived list went to
    /// the chat list.
    func testBackFromAnArchivedChatLandsOnArchivedChats() {
        addTeardownBlock { [self] in unarchiveFixtureDM() }
        let app = launch()
        let row = element(app, "home-row:fx-dm-unconst")
        if row.waitForExistence(timeout: 5) {
            row.swipeLeft()
            let archive = app.buttons["Archive"].firstMatch
            XCTAssertTrue(archive.waitForExistence(timeout: settle), "the row offers Archive")
            tap(archive)
        }
        let footer = app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'Archived chats'")).firstMatch
        if !footer.waitForExistence(timeout: 3) { app.swipeUp() }
        XCTAssertTrue(footer.waitForExistence(timeout: settle), "Home lists Archived chats")
        tap(footer)
        let page = element(app, "archived-chats")
        XCTAssertTrue(page.waitForExistence(timeout: settle), "the archived list opened")
        let archived = app.staticTexts["Unconst"].firstMatch
        XCTAssertTrue(archived.waitForExistence(timeout: settle), "the archived chat is listed")
        tap(archived)
        let back = element(app, "chat-back:fx-dm-unconst")
        XCTAssertTrue(back.waitForExistence(timeout: settle), "the archived chat opened")
        tap(back)
        assertGone(back)
        XCTAssertTrue(page.waitForExistence(timeout: settle), "back lands on the archived list")
        XCTAssertFalse(app.buttons["home-new"].exists, "not on the chat list")
    }

    /// A channel opened from a server's pushed page is shown and stays: the open replaces
    /// the path, and a dismiss beside it must not pop the channel.
    func testAChannelOpenedFromAServerPageIsShown() {
        let app = XCUIApplication()
        // The rig pushes the server's page over its channel list, on the Channels page.
        app.launchEnvironment = ["ARBOS_FIXTURE": "1", "ARBOS_FIXTURE_SERVERS": "1",
                                 "ARBOS_OPEN_SERVER": "fx-server-hq:panel", "ARBOS_PANEL_TAB": "Channels"]
        app.launch()
        XCTAssertTrue(element(app, "server-subpage-channels").waitForExistence(timeout: appear), "the server's Channels page opened")
        let general = app.staticTexts["general"].firstMatch
        XCTAssertTrue(general.waitForExistence(timeout: settle), "the channel is listed")
        tap(general)
        assertStays(element(app, "chat-back:fx-server-hq-general"), "the channel is shown")
    }
}
