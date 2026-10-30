import XCTest

/// Hold a bubble → the reaction capsule → its add-emoji cell → the emoji keyboard →
/// type 🔥: the overlay closes and ONE 🔥 chip sits under the bubble
/// ([feedback iphone-emoji-picker-reaction-2026-10-28]: the pick closed the overlay and
/// added nothing, because the keyboard's insert reached the hidden field twice and the
/// second report toggled the reaction off again). Runs the fixture app (`ARBOS_FIXTURE=1`,
/// Debug builds): no relay, deterministic rows.
///
/// A message bubble is `bubble:<messageId>`, the capsule `message-actions-capsule`, its
/// add-emoji cell is labelled "More emoji", a chip "<emoji> reaction, <count>". Each step
/// is attached as a screenshot (`reaction-pick-<step>`, kept in the result bundle:
/// `xcresulttool export attachments`) — the rig's before/after shots.
final class ReactionPickerUITests: XCTestCase {
    /// The shared Mac runs the gate and releases at the same time; waits are generous.
    private let appear: TimeInterval = 90
    private let settle: TimeInterval = 20

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launch() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1", "ARBOS_OPEN_PROJECT": "fx-comms"]
        app.launch()
        XCTAssertTrue(app.buttons["chat-back:fx-comms-main"].waitForExistence(timeout: appear), "the Comms chat is open")
        return app
    }

    private var bubbleQuery: NSPredicate { NSPredicate(format: "identifier BEGINSWITH 'bubble:'") }

    /// The lowest bubble fully on screen (the newest message of the open chat).
    private func lastBubble(_ app: XCUIApplication) -> XCUIElement {
        let bubbles = app.descendants(matching: .any).matching(bubbleQuery)
        XCTAssertTrue(bubbles.firstMatch.waitForExistence(timeout: settle), "the transcript shows bubbles")
        let window = app.windows.firstMatch.frame
        let visible = bubbles.allElementsBoundByIndex.filter { $0.isHittable && $0.frame.maxY < window.maxY - 120 && $0.frame.minY > 120 }
        XCTAssertFalse(visible.isEmpty, "a bubble is hittable")
        return visible.last!
    }

    private func chip(_ app: XCUIApplication, _ emoji: String) -> XCUIElement {
        app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH %@", "\(emoji) reaction,")).firstMatch
    }

    private func assertGone(_ element: XCUIElement, _ what: String, file: StaticString = #filePath, line: UInt = #line) {
        let gone = NSPredicate(format: "exists == false")
        let wait = XCTNSPredicateExpectation(predicate: gone, object: element)
        XCTAssertEqual(XCTWaiter().wait(for: [wait], timeout: settle), .completed, what, file: file, line: line)
    }

    private func shot(_ name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = "reaction-pick-\(name)"
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    func testEmojiFromTheKeyboardBecomesOneReaction() {
        let app = launch()
        let bubble = lastBubble(app)
        XCTAssertFalse(chip(app, "🔥").exists, "no 🔥 chip before the pick")
        shot("1-chat")

        bubble.press(forDuration: 0.8)
        let capsule = app.descendants(matching: .any)["message-actions-capsule"]
        XCTAssertTrue(capsule.waitForExistence(timeout: settle), "the reaction capsule opened")
        shot("2-capsule")

        let more = app.buttons["More emoji"]
        XCTAssertTrue(more.waitForExistence(timeout: settle), "the add-emoji cell is in the capsule")
        more.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: settle), "the emoji keyboard came up")
        shot("3-keyboard")

        app.typeText("🔥")

        assertGone(capsule, "the overlay closed on the pick")
        let fire = chip(app, "🔥")
        // The chip appears with the overlay's close; give the transcript a moment, then
        // record the row as it is — with the chip (fixed) or without it (the bug).
        _ = fire.waitForExistence(timeout: 3)
        shot("4-after-pick")
        XCTAssertTrue(fire.exists, "a 🔥 chip sits under the bubble")
        XCTAssertEqual(fire.label, "🔥 reaction, 1", "exactly one 🔥 — the pick toggled once")
    }

    /// A symbol from the keyboard's Symbols page (a text-default character the keyboard
    /// sends with U+FE0F) is a reaction like any other
    /// ([feedback full-picker-reaction-ios-2026-10-29]: the overlay stayed up, no chip).
    func testSymbolEmojiFromTheKeyboardBecomesAReaction() {
        let app = launch()
        let bubble = lastBubble(app)
        XCTAssertFalse(chip(app, "‼️").exists, "no ‼️ chip before the pick")
        shot("symbol-1-chat")

        bubble.press(forDuration: 0.8)
        let capsule = app.descendants(matching: .any)["message-actions-capsule"]
        XCTAssertTrue(capsule.waitForExistence(timeout: settle), "the reaction capsule opened")
        app.buttons["More emoji"].tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: settle), "the emoji keyboard came up")
        sleep(1)
        shot("symbol-2-keyboard")

        app.typeText("‼️")

        let closed = XCTWaiter().wait(for: [XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: capsule)], timeout: 5)
        let bang = chip(app, "‼️")
        _ = bang.waitForExistence(timeout: 3)
        shot("symbol-3-after-pick")
        XCTAssertEqual(closed, .completed, "the overlay closed on the pick")
        XCTAssertTrue(bang.exists, "a ‼️ chip sits under the bubble")
        XCTAssertEqual(bang.label, "‼️ reaction, 1", "exactly one ‼️")
    }

    /// The same pick with a finger on the emoji keyboard's grid instead of `typeText`
    /// (which injects the text without touching the keyboard).
    func testEmojiTappedOnTheEmojiKeyboardBecomesAReaction() {
        let app = launch()
        tapPick(app, on: lastBubble(app), tag: "tap-last")
    }

    /// A message high on the screen: the emoji keyboard shrinks the transcript under it.
    func testEmojiTappedForAMessageHighOnTheScreenBecomesAReaction() {
        let app = launch()
        tapPick(app, on: topBubble(app), tag: "tap-top")
    }

    /// The highest bubble fully on screen, below the header.
    private func topBubble(_ app: XCUIApplication) -> XCUIElement {
        let bubbles = app.descendants(matching: .any).matching(bubbleQuery)
        XCTAssertTrue(bubbles.firstMatch.waitForExistence(timeout: settle), "the transcript shows bubbles")
        let visible = bubbles.allElementsBoundByIndex.filter { $0.isHittable && $0.frame.minY > 140 && $0.frame.height < 200 }
        XCTAssertFalse(visible.isEmpty, "a bubble is hittable")
        return visible.min { $0.frame.minY < $1.frame.minY }!
    }

    private func tapPick(_ app: XCUIApplication, on bubble: XCUIElement, tag: String) {
        let anyChip = app.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS ' reaction, '"))
        let chipsBefore = anyChip.count
        shot("\(tag)-1-chat")

        bubble.press(forDuration: 0.8)
        let capsule = app.descendants(matching: .any)["message-actions-capsule"]
        XCTAssertTrue(capsule.waitForExistence(timeout: settle), "the reaction capsule opened")
        shot("\(tag)-2-capsule")
        app.buttons["More emoji"].tap()
        let keyboard = app.keyboards.firstMatch
        XCTAssertTrue(keyboard.waitForExistence(timeout: settle), "the emoji keyboard came up")
        sleep(2)
        shot("\(tag)-3-keyboard")
        XCTAssertTrue(capsule.exists, "the capsule is still up over the emoji keyboard")

        // The grid's first column, third row: an emoji with no skin tones (a toned one
        // raises the simulator's one-time skin-tone tip instead of typing), not a tab.
        keyboard.coordinate(withNormalizedOffset: CGVector(dx: 0.05, dy: 0.42)).tap()

        assertGone(capsule, "the overlay closed on the pick")
        _ = anyChip.element(boundBy: chipsBefore).waitForExistence(timeout: 3)
        shot("\(tag)-4-after-pick")
        XCTAssertEqual(anyChip.count, chipsBefore + 1, "the tapped emoji is a new chip under the bubble")
    }
}
