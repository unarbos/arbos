import XCTest

/// Long-press a bubble and, without lifting, slide onto a quick reaction or a menu row
/// and lift to pick it — one gesture, the way iMessage does (feedback
/// iphone-bubble-menu-slide-to-pick-2026-11-05, Affinity 11:28 CR: "today you cannot").
///
/// This is the half of the fix that geometry cannot prove. `MessageActionsSlideTests`
/// shows the right thing is under the finger; only a real touch shows the samples still
/// *arrive* once the overlay is on screen. UIKit routes a touch to the view it
/// hit-tested when the finger went down, so before the fix every sample after the menu
/// opened belonged to a recogniser that had already finished, and the overlay — added
/// afterwards, as a sibling — was never on that touch's delivery path at all.
///
/// `press(forDuration:thenDragTo:)` is one continuous touch: down, hold past the 0.35 s
/// long press, drag, lift. A tap-based test would pass on the broken build, because
/// tapping the menu has always worked; it is the touch staying down that is the bug.
///
/// Runs the fixture app (`ARBOS_FIXTURE=1`, Debug builds): no relay, deterministic rows.
/// A bubble is `bubble:<messageId>`, the capsule `message-actions-capsule`, the menu
/// `message-actions-menu`, a row `message-menu-copy`, a cell is labelled "React 🔥",
/// a chip "<emoji> reaction, <count>".
final class MessageSlideToPickUITests: XCTestCase {
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

    /// The lowest bubble fully on screen, with room above and below for the group.
    private func lastBubble(_ app: XCUIApplication) -> XCUIElement {
        let bubbles = app.descendants(matching: .any).matching(NSPredicate(format: "identifier BEGINSWITH 'bubble:'"))
        XCTAssertTrue(bubbles.firstMatch.waitForExistence(timeout: settle), "the transcript shows bubbles")
        let window = app.windows.firstMatch.frame
        let visible = bubbles.allElementsBoundByIndex.filter {
            $0.isHittable && $0.frame.maxY < window.maxY - 120 && $0.frame.minY > 120 && $0.frame.height < 200
        }
        XCTAssertFalse(visible.isEmpty, "a bubble is hittable with room around it")
        return visible.last!
    }

    private func capsule(_ app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any)["message-actions-capsule"]
    }

    private func chips(_ app: XCUIApplication) -> XCUIElementQuery {
        app.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS ' reaction, '"))
    }

    private func chip(_ app: XCUIApplication, _ emoji: String) -> XCUIElement {
        app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH %@", "\(emoji) reaction,")).firstMatch
    }

    private func point(_ app: XCUIApplication, _ p: CGPoint) -> XCUICoordinate {
        app.coordinate(withNormalizedOffset: .zero).withOffset(CGVector(dx: p.x, dy: p.y))
    }

    private func assertGone(_ element: XCUIElement, _ what: String, file: StaticString = #filePath, line: UInt = #line) {
        let wait = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: element)
        XCTAssertEqual(XCTWaiter().wait(for: [wait], timeout: settle), .completed, what, file: file, line: line)
    }

    private func shot(_ name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = "slide-to-pick-\(name)"
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    /// Where the group lands for this bubble, read from the app rather than recomputed:
    /// the 🔥 cell, the Copy row, and the two groups' own frames.
    private struct Group {
        let fire: CGRect
        let copy: CGRect
        let capsule: CGRect
        let menu: CGRect
    }

    /// Open the overlay with a plain hold to read the real frames, then put it away.
    /// The hold lifts where it started, which picks nothing and leaves the overlay up —
    /// the same rule `testALiftOnEmptySpacePicksNothing` asserts deliberately.
    private func group(_ app: XCUIApplication, _ bubble: XCUIElement) -> Group {
        bubble.press(forDuration: 0.8)
        XCTAssertTrue(capsule(app).waitForExistence(timeout: settle), "the reaction capsule opened")
        let fire = app.buttons["React 🔥"]
        XCTAssertTrue(fire.waitForExistence(timeout: settle), "🔥 is a cell in the capsule")
        let copy = app.descendants(matching: .any)["message-menu-copy"]
        XCTAssertTrue(copy.waitForExistence(timeout: settle), "Copy is a row in the menu")
        let menu = app.descendants(matching: .any)["message-actions-menu"]
        let found = Group(fire: fire.frame, copy: copy.frame, capsule: capsule(app).frame, menu: menu.frame)

        point(app, clear(app, found, bubble.frame)).tap()
        assertGone(capsule(app), "the overlay closed again, ready for the real gesture")
        return found
    }

    /// A point on the backdrop: on no surface, not on the held bubble's band, and clear
    /// of all three by a margin. The group never fills the screen, so one always exists.
    private func clear(_ app: XCUIApplication, _ g: Group, _ bubble: CGRect) -> CGPoint {
        let window = app.windows.firstMatch.frame
        let busy = [g.capsule, g.menu, CGRect(x: window.minX, y: bubble.minY, width: window.width, height: bubble.height)]
        var y = window.minY + 80
        while y < window.maxY - 40 {
            let p = CGPoint(x: window.midX, y: y)
            if !busy.contains(where: { $0.insetBy(dx: -12, dy: -12).contains(p) }) { return p }
            y += 10
        }
        XCTFail("no backdrop clear of the capsule, the bubble and the menu")
        return CGPoint(x: window.midX, y: window.maxY - 40)
    }

    private func centre(_ r: CGRect) -> CGPoint { CGPoint(x: r.midX, y: r.midY) }

    // MARK: - The deliverable

    /// Hold the bubble, slide up onto 🔥 and lift, all in one touch: a 🔥 chip appears.
    /// Before the fix the lift did nothing, because the capsule never saw the touch.
    func testSlidingFromTheLongPressOntoAQuickReactionPicksIt() {
        let app = launch()
        let bubble = lastBubble(app)
        XCTAssertFalse(chip(app, "🔥").exists, "no 🔥 chip before the slide")
        let g = group(app, bubble)
        shot("reaction-1-before")

        bubble.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
            .press(forDuration: 0.8, thenDragTo: point(app, centre(g.fire)))

        assertGone(capsule(app), "the overlay closed on the pick")
        let fire = chip(app, "🔥")
        _ = fire.waitForExistence(timeout: 3)
        shot("reaction-2-after")
        XCTAssertTrue(fire.exists, "a 🔥 chip sits under the bubble")
        XCTAssertEqual(fire.label, "🔥 reaction, 1", "exactly one 🔥 — the slide picked it once")
    }

    /// The same one-touch gesture down onto a menu row. A lift that picks nothing leaves
    /// the overlay up, so the overlay closing is what says an item was taken; no new chip
    /// says it was the row rather than a reaction.
    func testSlidingFromTheLongPressOntoAMenuRowPicksIt() {
        let app = launch()
        let bubble = lastBubble(app)
        let before = chips(app).count
        let g = group(app, bubble)
        shot("menu-1-before")

        bubble.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
            .press(forDuration: 0.8, thenDragTo: point(app, centre(g.copy)))

        assertGone(capsule(app), "the overlay closed, so Copy was picked by the lift")
        shot("menu-2-after")
        XCTAssertEqual(chips(app).count, before, "a menu row was picked, not a reaction")
    }

    /// Lifting where no item is picks nothing: no reaction, no menu action, and the
    /// overlay stays up to be tapped — which is exactly what a long press and a release
    /// already do. Guessing at the nearest item would be worse than the bug.
    func testALiftOnEmptySpacePicksNothing() {
        let app = launch()
        let bubble = lastBubble(app)
        let before = chips(app).count
        let g = group(app, bubble)
        let empty = clear(app, g, bubble.frame)
        shot("empty-1-before")

        bubble.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
            .press(forDuration: 0.8, thenDragTo: point(app, empty))

        XCTAssertTrue(capsule(app).waitForExistence(timeout: settle), "the overlay is still up, nothing was picked")
        shot("empty-2-after")
        XCTAssertEqual(chips(app).count, before, "no reaction was added by a lift on the backdrop")
    }
}
