import XCTest

/// The Search-in-chat bar closes ([feedback iphone-search-close-2026-10-28]; Jacob: "The x
/// button doesn't work on the search on iPhone. Also clicking away from the search should
/// close it."). Fixture app (`ARBOS_FIXTURE=1`), the "Launch week" group: ✕ closes; a tap on
/// the transcript closes; a tap on a result closes and the message stays on screen.
final class ChatSearchCloseUITests: XCTestCase {
    private let appear: TimeInterval = 90
    private let settle: TimeInterval = 15
    /// The fixture's newest "Launch week" row (`Fixture.swift`, g9).
    private static let newestRow = "1,284 signups"

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func openLaunchWeekSearch() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1", "ARBOS_OPEN_PEOPLE": "fx-group-launch"]
        app.launch()
        let search = app.descendants(matching: .any)["chat-search-button"].firstMatch
        XCTAssertTrue(search.waitForExistence(timeout: appear), "the chat header's search disc is on screen")
        // Let the push settle (a tap during the slide lands on the home list on a loaded Mac).
        let field = app.textFields["chat-search-field"]
        for _ in 0..<3 where !field.exists {
            sleep(1)
            search.tap()
            _ = field.waitForExistence(timeout: 6)
        }
        XCTAssertTrue(field.exists, "the Search-in-chat bar opened")
        return app
    }

    /// A screenshot kept in the result bundle (the before/after shots of the fix).
    private func shot(_ app: XCUIApplication, _ name: String) {
        let a = XCTAttachment(screenshot: app.screenshot())
        a.name = name
        a.lifetime = .keepAlways
        add(a)
    }

    private func barGone(_ app: XCUIApplication, _ what: String, file: StaticString = #filePath, line: UInt = #line) {
        let field = app.textFields["chat-search-field"]
        let deadline = Date().addingTimeInterval(settle)
        while field.exists, Date() < deadline { usleep(150_000) }
        XCTAssertFalse(field.exists, "\(what): the bar is gone", file: file, line: line)
        // The keyboard slides down after the field has gone (0.02–0.22 s on hosted macos-15).
        let keyboardDeadline = Date().addingTimeInterval(settle)
        while app.keyboards.count > 0, Date() < keyboardDeadline { usleep(100_000) }
        XCTAssertEqual(app.keyboards.count, 0, "\(what): the keyboard is down", file: file, line: line)
    }

    /// The ✕ closes the bar, clears the query and drops the keyboard — tapped on its
    /// glyph and on the edge of its cell (the old 17 pt target missed there).
    func testCloseButtonClosesTheBar() {
        let app = openLaunchWeekSearch()
        let field = app.textFields["chat-search-field"]
        field.tap()
        field.typeText("embargo")
        let close = app.descendants(matching: .any)["chat-search-close"].firstMatch
        XCTAssertTrue(close.waitForExistence(timeout: settle))
        XCTAssertGreaterThanOrEqual(close.frame.width, 44, "the ✕ cell is a real touch target")
        shot(app, "1-bar-open-with-x")
        close.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.1)).tap()
        barGone(app, "✕ at the cell's corner")
        shot(app, "2-after-x-tap-closed")
        // Re-open: the query did not survive the close.
        app.descendants(matching: .any)["chat-search-button"].firstMatch.tap()
        XCTAssertTrue(field.waitForExistence(timeout: settle))
        // An empty SwiftUI TextField reports its prompt as the value.
        let value = field.value as? String ?? ""
        XCTAssertTrue(value.isEmpty || value == "Search in chat", "the query was cleared (value: \(value))")
        close.tap()
        barGone(app, "✕ on its glyph")
    }

    /// A tap on the messages area (not on the bar) closes the bar the same way.
    func testTapOnTheTranscriptClosesTheBar() {
        let app = openLaunchWeekSearch()
        let field = app.textFields["chat-search-field"]
        field.tap()
        shot(app, "3-bar-open")
        // A message bubble in the messages area (not empty chrome): the newest row, Comms's
        // "Slide 4 now reads 1,284 signups…" (fixture g9; the rows above may sit outside the
        // lazy stack or under the keyboard).
        let row = app.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS %@", Self.newestRow)).firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: settle), "a message row is on screen")
        row.tap()
        barGone(app, "tap on the transcript")
        shot(app, "4-after-transcript-tap-closed")
    }

    /// A keyword filters the list to its hits; a tap on a hit closes the bar and the
    /// message is still on screen (revealed in the full transcript).
    func testTapOnAResultClosesAndKeepsTheMessageOnScreen() {
        let app = openLaunchWeekSearch()
        let field = app.textFields["chat-search-field"]
        field.tap()
        field.typeText("embargo")
        let hit = app.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS %@", "embargo note")).firstMatch
        XCTAssertTrue(hit.waitForExistence(timeout: settle), "the hit is listed")
        let press = app.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS %@", "press kit")).firstMatch
        XCTAssertFalse(press.exists, "a row without the keyword is filtered out")
        shot(app, "5-results-for-embargo")
        hit.tap()
        barGone(app, "tap on a result")
        shot(app, "6-after-result-tap-revealed")
        XCTAssertTrue(hit.waitForExistence(timeout: settle), "the tapped message is on screen after the close")
        XCTAssertTrue(hit.isHittable, "…and visible")
    }

    /// A keyword WITH hits draws them. This is the iOS 27 regression
    /// ([feedback iphone-search-blank-ios27-2026-10-31]): the filter ran — the newest
    /// unfiltered row went and no "No matches" line appeared — but the transcript was blank,
    /// no row existed under any label or identifier at 1, 5 or 15 s, and the scroll view
    /// claimed two pages of content at 100 % while holding nothing. The lazy stack had kept
    /// the space it reserved for the rows the filter removed and built none of the ones that
    /// stayed.
    ///
    /// Before the fix this case fails on **iOS 27.0 only**. The same binary passes it on iOS
    /// 26.5, so an older simulator cannot catch it; the gate Mac's simulator is 27.
    func testAKeywordWithHitsDrawsThem() {
        let app = openLaunchWeekSearch()
        let field = app.textFields["chat-search-field"]
        field.tap()
        field.typeText("embargo")
        // Label OR identifier, so a row that is built but unlabelled still counts as drawn.
        let hit = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label CONTAINS %@ OR identifier CONTAINS %@", "embargo note", "embargo note"))
            .firstMatch
        XCTAssertTrue(hit.waitForExistence(timeout: settle), "the filtered transcript holds the hit")
        shot(app, "9-filtered-transcript-draws-the-hit")
        // Existence alone would pass on a row parked below the viewport in reserved space;
        // the symptom is that nothing is on screen, so assert it is.
        XCTAssertTrue(hit.isHittable, "…and the hit is on screen, not parked outside the viewport")
        XCTAssertFalse(app.descendants(matching: .any)["chat-search-no-matches"].exists,
                       "a keyword with hits never says No matches")
    }

    /// A keyword with no hit is never a blank transcript.
    func testNoMatchesIsSaidNotBlank() {
        let app = openLaunchWeekSearch()
        let field = app.textFields["chat-search-field"]
        field.tap()
        field.typeText("zzqqxx")
        XCTAssertTrue(app.descendants(matching: .any)["chat-search-no-matches"].waitForExistence(timeout: settle))
        shot(app, "7-no-matches-line")
        app.descendants(matching: .any)["chat-search-close"].firstMatch.tap()
        barGone(app, "✕ after no matches")
        sleep(1)
        shot(app, "8-after-x-from-no-matches")
        XCTAssertFalse(app.descendants(matching: .any)["chat-search-no-matches"].exists, "the no-matches line went with the bar")
        // The newest row (the close re-pins the tail; older rows may sit outside the lazy stack).
        let newest = app.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS %@", Self.newestRow)).firstMatch
        XCTAssertTrue(newest.waitForExistence(timeout: settle), "the messages are back")
    }
}
