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

    /// A filtered list is pinned to the composer, not floating above a block of empty space.
    ///
    /// The re-key that `testAKeywordWithHitsDrawsThem` protects costs the filtered list its
    /// bottom anchor on iOS 27: the rebuilt stack has no measured child heights, iOS 27 never
    /// re-measures it, and the scroll view rests on the estimates. Measured on the rig on a
    /// 420-row thread, the newest hit came to rest 161 pt clear of the composer with that much
    /// black under it, and the match above it was pushed entirely behind the search bar: two
    /// matches on screen where the same build without the re-key shows three.
    ///
    /// The slack is 24–25 pt in every healthy cell (both runtimes, both with and without the
    /// re-key) and 161 pt in the broken one, so 60 pt separates them with room either side.
    ///
    /// Needs the deep fixture: the shipped "Launch week" thread is ~10 rows, which is shorter
    /// than the viewport, so the short-thread `minHeight` frame pins it regardless and this
    /// case cannot express the failure. Before the fix it fails on **iOS 27.0 only** — the
    /// same binary passes on iOS 26.5, which re-measures by itself.
    func testAFilteredListStaysPinnedToTheComposer() {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1", "ARBOS_OPEN_PEOPLE": "fx-group-launch",
                                 "ARBOS_FIXTURE_LONG_ROWS": "420"]
        app.launch()
        let search = app.descendants(matching: .any)["chat-search-button"].firstMatch
        XCTAssertTrue(search.waitForExistence(timeout: appear), "the chat header's search disc is on screen")
        let field = app.textFields["chat-search-field"]
        for _ in 0..<3 where !field.exists {
            sleep(1)
            search.tap()
            _ = field.waitForExistence(timeout: 6)
        }
        XCTAssertTrue(field.exists, "the Search-in-chat bar opened")
        field.tap()
        field.typeText("embargo")
        // The newest hit: the shipped rows sit after the synthetic history, so the fixture's
        // own "draft the embargo note" message is the last row of the filtered list.
        let newestHit = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label CONTAINS %@", "embargo note"))
            .firstMatch
        XCTAssertTrue(newestHit.waitForExistence(timeout: settle), "the filtered transcript holds its hits")
        // The chat bar's own top edge (`ChatBarHeightProbe`, DEBUG): where the rows must end.
        let composer = app.descendants(matching: .any)["chat-bar-height"].firstMatch
        XCTAssertTrue(composer.waitForExistence(timeout: settle), "the chat bar probe is up")
        let hitFrame = restingFrame(newestHit)
        shot(app, "10-filtered-list-pinned-to-the-composer")
        let slack = composer.frame.minY - hitFrame.maxY
        print("[pin] hit \(hitFrame) composer \(composer.frame) screen \(app.frame) slack \(slack)")
        XCTAssertTrue(newestHit.isHittable, "…and the newest hit is on screen")
        XCTAssertLessThan(slack, 60,
                          "the filtered list ends at the composer (slack \(slack) pt; healthy is 24–25, the iOS 27 re-key regression was 161)")
    }

    /// The hit's frame once the transcript has stopped moving, for the pinned-list measurement.
    ///
    /// The stack is still in motion when the hit first exists: every keystroke re-keys it, so
    /// `pinFilteredStackToEnd` runs its last rung 0.25 s after the final one, and the lazy stack
    /// goes on swapping estimated child heights for measured ones after that. The case used to
    /// read `isHittable` on the first snapshot after existence and only then sleep 2 s before
    /// measuring the slack — the assertion that is actually its subject. That ordering is what
    /// reddened it about one run in ten on the 26.2 gate: on a loaded hosted runner the first
    /// snapshot arrives mid-settle with the hit below the fold, and the precondition fails before
    /// the measurement is ever taken. Gate runs 41308036692 attempt 1 (`a9a87ad7d`) and
    /// 41346954704 (`758001b67`) both failed there and neither reached the slack line; the pass on
    /// the byte-identical tree took its snapshot 1.6 s after typing, the two failures took theirs
    /// at 2.4 s or later on runners that were slower throughout.
    ///
    /// Waiting for rest softens nothing. The regression this case guards leaves the hit perfectly
    /// hittable — 161 pt clear of the composer is on screen — so hittability never detected it,
    /// and the strict `slack < 60` is unchanged and is still taken at the deadline if the stack
    /// never comes to rest. The budget is the same `settle` the one-shot read already had.
    private func restingFrame(_ element: XCUIElement,
                              file: StaticString = #filePath, line: UInt = #line) -> CGRect {
        let deadline = Date().addingTimeInterval(settle)
        var last = CGRect.null
        var still = 0
        while Date() < deadline, still < 2 {
            usleep(300_000)
            let now = element.exists ? element.frame : CGRect.null
            // Sub-pixel jitter is rest; a row swapping an estimate for a measurement moves points.
            still = (abs(now.minY - last.minY) < 0.5 && abs(now.maxY - last.maxY) < 0.5) ? still + 1 : 0
            last = now
        }
        if still < 2 { print("[pin] never came to rest inside \(settle)s; measuring \(last)") }
        XCTAssertFalse(last.isNull, "the newest hit still has a frame", file: file, line: line)
        return last
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
