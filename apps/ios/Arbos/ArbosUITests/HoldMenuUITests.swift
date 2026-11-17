import XCTest

/// The hold menu on a bubble (tester reports, 2026-11-13 to 2026-11-17): a tap anywhere off
/// the capsule and the menu closes it — on the lifted message too, whatever it holds, and
/// nothing under the blur takes that tap; a message taller than the screen scrolls in
/// place with the menu still there; a bubble that continues a run is lifted with its
/// sender's name; a hold on a button in the row — the reply quote, a sender's name — opens
/// the menu and does not press the button.
/// Runs the fixture app (`ARBOS_FIXTURE=1`, Debug builds): no relay, deterministic rows.
final class HoldMenuUITests: XCTestCase {
    /// The shared Mac runs the gate and releases at the same time; waits are generous.
    private let appear: TimeInterval = 150
    private let settle: TimeInterval = 20

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launch(_ environment: [String: String]) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1", "ARBOS_NO_STORE": "1"].merging(environment) { $1 }
        app.launch()
        let bubbles = app.descendants(matching: .any).matching(NSPredicate(format: "identifier BEGINSWITH 'bubble:'"))
        XCTAssertTrue(bubbles.firstMatch.waitForExistence(timeout: appear), "the transcript is up")
        return app
    }

    private func group() -> XCUIApplication { launch(["ARBOS_OPEN_PEOPLE": "fx-group-launch"]) }

    private func menu(_ app: XCUIApplication) -> XCUIElement { app.descendants(matching: .any)["message-actions-menu"].firstMatch }
    private func lifted(_ app: XCUIApplication) -> XCUIElement { app.scrollViews["message-actions-bubble"].firstMatch }

    /// Scrolls back until `element` can be pressed clear of the header and the composer.
    private func reveal(_ element: XCUIElement, in app: XCUIApplication) {
        let window = app.windows.firstMatch.frame
        for _ in 0 ..< 12 {
            if element.exists, element.isHittable, element.frame.minY > 130, element.frame.maxY < window.maxY - 130 { return }
            let from = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.4))
            from.press(forDuration: 0.05, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.6)))
        }
        XCTFail("\(element) never came on screen")
    }

    private func hold(_ element: XCUIElement, in app: XCUIApplication) {
        element.press(forDuration: 0.8)
        XCTAssertTrue(menu(app).waitForExistence(timeout: settle), "the hold opened the menu")
    }

    private func assertClosed(_ app: XCUIApplication, _ what: String, file: StaticString = #filePath, line: UInt = #line) {
        let gone = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: menu(app))
        XCTAssertEqual(XCTWaiter().wait(for: [gone], timeout: settle), .completed, what, file: file, line: line)
        // A link that took the tap would have sent the app to the background.
        XCTAssertEqual(app.state, .runningForeground, "the tap opened nothing", file: file, line: line)
    }

    private func shot(_ name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = "hold-menu-\(name)"
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    /// A tap on the link of a lifted message closes the menu and does not follow the link.
    func testATapOnALiftedLinkClosesTheMenu() {
        let app = group()
        let bubble = app.descendants(matching: .any)["bubble:g3"]
        reveal(bubble, in: app)
        hold(bubble, in: app)
        shot("lifted-link")
        let text = lifted(app).staticTexts.matching(NSPredicate(format: "label CONTAINS 'arbos.life'")).firstMatch
        XCTAssertTrue(text.exists, "the lifted message shows its link")
        // "And the signup form: https://arbos.life/i/launch": the link ends the text, on a
        // line of its own when the text wraps.
        let wrapped = text.frame.height > 30
        text.coordinate(withNormalizedOffset: wrapped ? CGVector(dx: 0.3, dy: 0.75) : CGVector(dx: 0.8, dy: 0.5)).tap()
        assertClosed(app, "a tap on the lifted link closes the menu")
        XCTAssertTrue(bubble.waitForExistence(timeout: settle) && bubble.isHittable, "the chat is back, nothing over it")
    }

    /// A message that is a file: a tap on the lifted card closes the menu and opens no file.
    func testATapOnALiftedFileClosesTheMenu() {
        let app = group()
        let bubble = app.descendants(matching: .any)["bubble:g2"]
        reveal(bubble, in: app)
        hold(bubble, in: app)
        shot("lifted-file")
        lifted(app).tap()
        assertClosed(app, "a tap on the lifted file closes the menu")
        XCTAssertTrue(bubble.waitForExistence(timeout: settle) && bubble.isHittable, "the chat is back, nothing over it")
    }

    /// With the emoji keyboard up (the capsule's add-emoji cell) a tap on the lifted message
    /// still closes the overlay, and the keyboard goes with it.
    func testATapWhileTheEmojiKeyboardIsUpClosesTheOverlay() {
        let app = group()
        let bubble = app.descendants(matching: .any)["bubble:g3"]
        reveal(bubble, in: app)
        hold(bubble, in: app)
        app.buttons["More emoji"].firstMatch.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: settle), "the emoji keyboard came up")
        shot("picker")
        lifted(app).tap()
        let capsule = app.descendants(matching: .any)["message-actions-capsule"].firstMatch
        let gone = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: capsule)
        XCTAssertEqual(XCTWaiter().wait(for: [gone], timeout: settle), .completed, "the overlay closed")
        XCTAssertEqual(app.state, .runningForeground, "the tap opened nothing")
    }

    /// A hold that lands on a sender's name opens the menu; the lift does not open the profile.
    func testAHoldOnASendersNameOpensTheMenuAndNotTheProfile() {
        let app = group()
        let name = app.descendants(matching: .any)["bubble:g3"].buttons["bubble-name"].firstMatch
        reveal(name, in: app)
        hold(name, in: app)
        lifted(app).tap()
        assertClosed(app, "a tap on the lifted message closes the menu")
        // The profile page is the one place with this button.
        let profile = app.buttons["Block user"].firstMatch
        XCTAssertFalse(profile.waitForExistence(timeout: 3), "the hold did not open the profile")
        name.tap()
        XCTAssertTrue(profile.waitForExistence(timeout: settle), "a tap on the name still opens it")
    }

    /// Mira's second line continues her run and has no name in the transcript; lifted, it does.
    func testALiftedBubbleCarriesItsSendersName() {
        let app = group()
        let bubble = app.descendants(matching: .any)["bubble:g1m"]
        reveal(bubble, in: app)
        XCTAssertFalse(bubble.buttons["bubble-name"].firstMatch.exists, "no name on a bubble that continues a run")
        hold(bubble, in: app)
        let name = lifted(app).buttons["bubble-name"].firstMatch
        XCTAssertTrue(name.waitForExistence(timeout: settle), "the lifted bubble is named")
        // The menu opens where it stays: no second pass moves it once the lifted bubble is measured.
        let opened = menu(app).frame
        Thread.sleep(forTimeInterval: 1)
        XCTAssertEqual(menu(app).frame, opened, "the menu did not move after it opened")
        shot("named")
        XCTAssertLessThanOrEqual(app.descendants(matching: .any)["message-actions-capsule"].firstMatch.frame.maxY, name.frame.minY,
                                 "the capsule sits above the name")
    }

    /// A hold that lands on the reply quote opens the menu and leaves the chat where it is; a
    /// tap on the quote still jumps to the original.
    func testAHoldOnTheReplyQuoteOpensTheMenuAndATapStillJumps() {
        let app = group()
        let quote = app.buttons["reply-quote"].firstMatch
        reveal(quote, in: app)
        let back = app.buttons["Back to reply"]
        hold(quote, in: app)
        lifted(app).tap()
        assertClosed(app, "a tap on the lifted reply closes the menu")
        XCTAssertFalse(back.exists, "the hold did not jump to the original")
        XCTAssertTrue(quote.isHittable, "the reply is where it was")
        quote.tap()
        XCTAssertTrue(back.waitForExistence(timeout: settle), "a tap on the quote jumps to the original")
    }

    /// A reply taller than the screen, full of links: it scrolls inside the hold view, the
    /// menu stays whole under it, and a tap beside the menu — over the part of the message
    /// that runs on under it — closes the menu instead of following a link.
    func testALongMessageScrollsAndATapBesideTheMenuCloses() {
        let app = launch(["ARBOS_OPEN_PROJECT": "fx-comms", "ARBOS_OPEN_CHAT": "fx-comms-main", "ARBOS_FIXTURE_LONG_REPLY": "tall"])
        let window = app.windows.firstMatch.frame
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.4, dy: 0.5)).press(forDuration: 0.8)
        XCTAssertTrue(menu(app).waitForExistence(timeout: settle), "the hold opened the menu")
        let view = lifted(app)
        XCTAssertLessThanOrEqual(view.frame.maxY, menu(app).frame.minY, "the message ends above the menu")
        XCTAssertLessThanOrEqual(menu(app).frame.maxY, window.maxY, "the whole menu is on screen")
        XCTAssertGreaterThanOrEqual(app.descendants(matching: .any)["message-actions-capsule"].firstMatch.frame.minY, 44,
                                    "the capsule is on screen, above the message")
        shot("long-1")
        let text = view.staticTexts.firstMatch
        let before = text.frame.minY
        view.swipeUp()
        XCTAssertTrue(menu(app).exists, "a drag scrolls the message, it does not close the menu")
        XCTAssertLessThan(text.frame.minY, before, "the message scrolled")
        shot("long-2-scrolled")
        app.coordinate(withNormalizedOffset: .zero)
            .withOffset(CGVector(dx: window.maxX - 30, dy: menu(app).frame.midY)).tap()
        assertClosed(app, "a tap beside the menu closes it")

        // The rows still take their own taps: Copy closes the menu too, as an action.
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.4, dy: 0.5)).press(forDuration: 0.8)
        XCTAssertTrue(menu(app).waitForExistence(timeout: settle), "the hold opened the menu again")
        app.buttons["Copy"].firstMatch.tap()
        assertClosed(app, "Copy ran and closed the menu")
    }
}
