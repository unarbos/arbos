import XCTest

/// Typing `@` in a group opens the mention list over the chat and moves nothing
/// ([feedback mention-list-overlay-ios-2026-10-29]; Scout's Mac report, the same push on
/// iPhone): the list was a row of the composer's stack, so the bar's safe-area inset grew by
/// the list's height and every bubble above slid up while it was open. Runs the fixture app
/// (`ARBOS_FIXTURE=1`, Debug builds) in the fixture group "Launch week".
///
/// Each step is attached as a screenshot (`mention-overlay-<step>`, kept in the result
/// bundle) — the rig's before/after shots.
final class MentionListOverlayUITests: XCTestCase {
    /// The shared Mac runs the gate and releases at the same time; waits are generous.
    private let appear: TimeInterval = 90
    private let settle: TimeInterval = 20

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launch() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1", "ARBOS_OPEN_PEOPLE": "fx-group-launch"]
        app.launch()
        XCTAssertTrue(field(app).waitForExistence(timeout: appear), "the Launch week composer is up")
        return app
    }

    private func field(_ app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any)["composer-field"]
    }

    private func hereRow(_ app: XCUIApplication) -> XCUIElement {
        app.buttons.matching(NSPredicate(format: "label BEGINSWITH '@here'")).firstMatch
    }

    /// The bubbles on screen, by identifier.
    private func bubbles(_ app: XCUIApplication) -> [String: CGRect] {
        let query = app.descendants(matching: .any).matching(NSPredicate(format: "identifier BEGINSWITH 'bubble:'"))
        var out: [String: CGRect] = [:]
        for b in query.allElementsBoundByIndex where b.exists && !b.frame.isEmpty {
            out[b.identifier] = b.frame
        }
        return out
    }

    private struct Layout {
        let field: CGRect
        let bubbles: [String: CGRect]
    }

    /// Waits until two reads a beat apart agree (the keyboard and the stick-to-bottom
    /// spring have come to rest), then returns that layout.
    private func layout(_ app: XCUIApplication) -> Layout {
        var last = Layout(field: field(app).frame, bubbles: bubbles(app))
        for _ in 0..<20 {
            Thread.sleep(forTimeInterval: 0.4)
            let now = Layout(field: field(app).frame, bubbles: bubbles(app))
            if now.field == last.field, now.bubbles == last.bubbles { return now }
            last = now
        }
        return last
    }

    private func assertSame(_ a: Layout, _ b: Layout, _ what: String, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(a.field.minY, b.field.minY, accuracy: 0.5, "\(what): the composer field stays put", file: file, line: line)
        let shared = Set(a.bubbles.keys).intersection(b.bubbles.keys)
        XCTAssertFalse(shared.isEmpty, "\(what): bubbles on screen in both reads", file: file, line: line)
        for id in shared.sorted() {
            XCTAssertEqual(a.bubbles[id]!.minY, b.bubbles[id]!.minY, accuracy: 0.5,
                           "\(what): \(id) stays put (\(a.bubbles[id]!.minY) → \(b.bubbles[id]!.minY))", file: file, line: line)
        }
    }

    private func shot(_ name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = "mention-overlay-\(name)"
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    func testTheMentionListFloatsOverTheChatAndMovesNothing() {
        let app = launch()
        field(app).tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: settle), "the keyboard is up")
        let closed = layout(app)
        shot("1-closed")

        app.typeText("@")
        XCTAssertTrue(hereRow(app).waitForExistence(timeout: settle), "typing @ opens the mention list")
        let open = layout(app)
        shot("2-open")
        assertSame(closed, open, "list open")
        // The list sits above the field, over the transcript.
        XCTAssertLessThan(hereRow(app).frame.maxY, open.field.minY, "the list is above the field")

        app.typeText(XCUIKeyboardKey.delete.rawValue)
        let gone = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: hereRow(app))
        XCTAssertEqual(XCTWaiter().wait(for: [gone], timeout: settle), .completed, "⌫ closes the list")
        shot("3-closed-again")
        assertSame(closed, layout(app), "list closed again")
    }

    /// A row of the floating list still takes the tap: the chip lands in the field.
    func testARowOfTheFloatingListStillInserts() {
        let app = launch()
        field(app).tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: settle), "the keyboard is up")
        app.typeText("@")
        let here = hereRow(app)
        XCTAssertTrue(here.waitForExistence(timeout: settle), "typing @ opens the mention list")
        here.tap()
        let gone = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: here)
        XCTAssertEqual(XCTWaiter().wait(for: [gone], timeout: settle), .completed, "the pick closes the list")
        let value = (field(app).value as? String) ?? ""
        XCTAssertFalse(value.isEmpty || value == "@", "the bare @ became the @here chip (\(value))")
        shot("4-picked")
    }
}
