import XCTest

/// A tap on a Search-in-chat result must leave the reader in the chat, looking at the message
/// they tapped (#2404). The fixture group padded with `ARBOS_FIXTURE_LONG_ROWS` is what makes
/// this a real test: the transcript is far longer than the lazy window, so most of the rows
/// restored when the filter goes have estimated heights at the moment the reveal scrolls. On
/// the short fixture chat `ChatSearchCloseUITests` covers, the content is shorter than the
/// viewport and the view cannot move at all.
///
/// **Order matters here, and `continueAfterFailure` is off so it holds.** The first version of
/// this test asserted the landing geometry alone and passed — while the app had pushed a
/// member's profile page over the chat. XCUITest reports a parent view's elements, with
/// plausible frames, from behind a pushed navigation destination, so a coordinate read says
/// nothing about what anybody can see. What is frontmost has to be established first; only
/// then does a frame mean anything.
///
/// The first three cases began as one experiment and are now the guard. `SearchResultTap` is a
/// `simultaneousGesture`, by design, so that a link inside a bubble still reaches the system —
/// but the sender's avatar and the name inside the bubble are `Button`s too (`sender-avatar`,
/// `bubble-name`, both calling `toggleProfile`), and they fired alongside the row. Run split,
/// a tap clear of both buttons passed and a tap on the avatar failed, which is what named the
/// cause; the page that opened was an agent's, so nothing about it was person-specific. All
/// three assert the same thing, because staying in the chat is correct wherever inside a
/// result the touch lands (`ChatSearchClose.rowOwnsInnerTap`). The fourth is the other half of
/// that rule: with nothing filtering, the same avatar must still open the page.
final class ChatSearchResultJumpUITests: XCTestCase {
    private let appear: TimeInterval = 150
    private let settle: TimeInterval = 20
    /// Rows the long fixture writes, newest last.
    private static let longRows = 420
    /// The fixture's "embargo" line is `lines[0]`, every eighth synthetic row.
    private static let keyword = "embargo"
    /// Clear space an aim point must have below the search chrome before this will tap there.
    /// The same number, for the same reason, as `ChatSearchRevealLandingUITests.aimBandMargin`,
    /// which measured it; these are two suites with their own copy of `reachable`, not one
    /// shared helper, so the guard has to be written in both.
    private static let aimBandMargin: CGFloat = 24
    /// How far above a result row's bottom edge a touch aimed at the row's own padding goes.
    /// Result rows overlap their neighbours, so an aim too near the edge belongs to the next
    /// row and the app reveals that one instead — correctly, since it owns the touch. Measured
    /// off two gate logs on the same fixture: the deepest overlap was 4 pt on main and 6 pt
    /// once [feedback group-chat-spacing-ios-2026-11-01] tightened `runGap` to 2, so the old
    /// aim of 5 went from 1 pt clear to 1 pt inside. `Theme.bubbleVPad` is 11, which puts the
    /// padding band roughly 4–15 pt above the edge; 9 sits in the middle of it, below the last
    /// line of text and clear of the deepest overlap seen. The guard in `settledResult` is what
    /// keeps this honest if a later pass deepens the overlap again.
    private static let bubbleBottomAim: CGFloat = 9

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launchLongLaunchWeek() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = [
            "ARBOS_FIXTURE": "1",
            "ARBOS_OPEN_PEOPLE": "fx-group-launch",
            "ARBOS_FIXTURE_LONG_ROWS": "\(Self.longRows)",
        ]
        app.launch()
        let search = app.descendants(matching: .any)["chat-search-button"].firstMatch
        XCTAssertTrue(search.waitForExistence(timeout: appear), "the chat header's search disc is on screen")
        return app
    }

    private func openLongLaunchWeekSearch() -> XCUIApplication {
        let app = launchLongLaunchWeek()
        let search = app.descendants(matching: .any)["chat-search-button"].firstMatch
        // Let the push settle (a tap during the slide lands on the home list on a loaded Mac).
        let field = app.textFields["chat-search-field"]
        for _ in 0..<4 where !field.exists {
            sleep(1)
            search.tap()
            _ = field.waitForExistence(timeout: 8)
        }
        XCTAssertTrue(field.exists, "the Search-in-chat bar opened")
        return app
    }

    private func shot(_ app: XCUIApplication, _ name: String) {
        let a = XCTAttachment(screenshot: app.screenshot())
        a.name = name
        a.lifetime = .keepAlways
        add(a)
    }

    /// Every realized transcript bubble, as the tree lists them. While a keyword filters the
    /// list each one is a hit; after the close they are the unfiltered rows around the landing.
    private func bubbles(_ app: XCUIApplication, screen: CGRect) -> [(id: String, frame: CGRect, onScreen: Bool)] {
        let all = app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier BEGINSWITH %@", "bubble:"))
        var out: [(id: String, frame: CGRect, onScreen: Bool)] = []
        for i in 0..<min(all.count, 60) {
            let e = all.element(boundBy: i)
            guard e.exists else { continue }
            let f = e.frame
            out.append((id: e.identifier, frame: f, onScreen: screen.intersects(f) && f.height > 0))
        }
        return out
    }

    private func describe(_ rows: [(id: String, frame: CGRect, onScreen: Bool)]) -> String {
        rows.map { "\($0.id)@y\(Int($0.frame.minY))…\(Int($0.frame.maxY))\($0.onScreen ? "" : " (off)")" }
            .joined(separator: ", ")
    }

    /// Rows that have held the same frame across two reads. The filtered list is still settling
    /// for a moment after the last keystroke, so a frame read even a few hundred milliseconds
    /// early describes where the row *was*, and the touch then lands on a different row than
    /// the test believes it tapped.
    ///
    /// Stillness is all this asks. Whether a row is somewhere a touch can reach is a separate
    /// question with a separate answer (`reachable`), and the two were conflated here at first:
    /// "wholly inside the window" let through a row sitting under the Search bar.
    private func stillRows(_ app: XCUIApplication, screen: CGRect) -> [(id: String, frame: CGRect)] {
        var previous: [String: CGRect] = [:]
        for _ in 0..<12 {
            let now = bubbles(app, screen: screen)
            let held = now
                .filter { $0.frame.height > 0 && previous[$0.id] == $0.frame }
                .map { (id: $0.id, frame: $0.frame) }
            if !held.isEmpty { return held }
            previous = Dictionary(now.map { ($0.id, $0.frame) }, uniquingKeysWith: { first, _ in first })
            usleep(400_000)
        }
        return []
    }

    /// Whether a profile pane is covering the chat. Existence is the test, not hittability:
    /// the pane is what sits in front, so its own elements are the frontmost ones.
    ///
    /// A diagnosis, not the verdict. `profile-*` comes from a person pane's hero pill
    /// (`PanePillButton`), and an **agent**'s page has none of them — its own pills live in a
    /// section far down Details and overwrite the identifier. So this returning false does
    /// not mean the chat is in front, which is exactly how the measured run behaved: the
    /// avatar case failed on `headerHittable` with this marker still false, on an agent's
    /// page. Whether the chat is frontmost is `headerHittable`'s job; this only names the
    /// thing in front when it can.
    private func profilePaneIsUp(_ app: XCUIApplication) -> Bool {
        app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier BEGINSWITH %@", "profile-"))
            .firstMatch.exists
            || app.staticTexts["Block user"].exists
    }

    private func row(_ app: XCUIApplication, _ id: String) -> XCUIElement {
        app.descendants(matching: .any)[id]
    }

    private func inRow(_ app: XCUIApplication, _ id: String, _ child: String) -> XCUIElement {
        row(app, id).descendants(matching: .any)[child].firstMatch
    }

    private func tap(_ app: XCUIApplication, at point: CGPoint) {
        app.coordinate(withNormalizedOffset: CGVector(dx: 0, dy: 0))
            .withOffset(CGVector(dx: point.x, dy: point.y))
            .tap()
    }

    /// Everything the assertions need, read once so they all describe the same moment, with
    /// `detail` carrying the geometry into every failure message.
    private struct TapOutcome {
        let profileUp: Bool
        let headerHittable: Bool
        let barOpen: Bool
        let revealedExists: Bool
        let revealedFrame: CGRect
        let screen: CGRect
        let detail: String
    }

    private func outcome(_ app: XCUIApplication, tapped: String, point: CGPoint,
                         screen: CGRect) -> TapOutcome {
        let revealed = row(app, tapped)
        let exists = revealed.exists
        let after = bubbles(app, screen: screen).filter { $0.onScreen }
        return TapOutcome(
            profileUp: profilePaneIsUp(app),
            headerHittable: app.descendants(matching: .any)["chat-search-button"].firstMatch.isHittable,
            barOpen: app.textFields["chat-search-field"].exists,
            revealedExists: exists,
            revealedFrame: exists ? revealed.frame : .null,
            screen: screen,
            detail: "tapped \(tapped) at \(point); screen \(screen); "
                + "message realized: \(exists)\(exists ? " at \(revealed.frame)" : ""); "
                + "transcript holding [\(describe(after))]")
    }

    /// The claim this branch makes: wherever inside a result the touch landed, the reader is
    /// still in the chat with nothing pushed over it, and the bar has gone.
    private func assertStillInTheChat(_ o: TapOutcome) {
        XCTAssertFalse(o.profileUp,
                       "the tap leaves the reader in the chat, but a profile pane is in front of "
                       + "it — see the screenshot — \(o.detail)")
        XCTAssertTrue(o.headerHittable,
                      "the chat header is reachable, so the chat is frontmost — \(o.detail)")
        XCTAssertFalse(o.barOpen, "a result tap closes the bar — \(o.detail)")
    }

    /// And #2404's own claim, which is a different one: the transcript came to rest on the
    /// message that was tapped. Separate from the above because the two can fail independently —
    /// see `testAResultTapOnTheNameInTheBubbleAlsoStaysInTheChat`.
    private func assertRestedOnTheMessage(_ o: TapOutcome) {
        XCTAssertTrue(o.revealedExists,
                      "the tapped message is realized in the transcript — \(o.detail)")
        XCTAssertTrue(o.screen.contains(o.revealedFrame),
                      "the tapped message is wholly on screen at \(o.revealedFrame) — \(o.detail)")
        // The middle half rules out a landing jammed under the composer or up behind the header
        // without naming either one's height. The keyword's hits are mid-history, so nothing
        // here is clamped against an end of the transcript.
        let middleHalf = o.screen.insetBy(dx: 0, dy: o.screen.height / 4)
        XCTAssertTrue(middleHalf.contains(CGPoint(x: o.revealedFrame.midX, y: o.revealedFrame.midY)),
                      "…and centred, not against an edge: middle \(o.revealedFrame.midY) is "
                      + "outside \(Int(middleHalf.minY))…\(Int(middleHalf.maxY)) — \(o.detail)")
    }

    private func assertLandedInChat(_ o: TapOutcome) {
        assertStillInTheChat(o)
        assertRestedOnTheMessage(o)
    }

    /// The part of the window a touch actually reaches: below the header and the Search bar,
    /// above the keyboard. Being inside the window is not the same as being reachable, and the
    /// difference is not theoretical — the first row this picked for the in-bubble name case
    /// had the name at y 33, behind the bar, and the tap went to the bar.
    private func reachable(_ app: XCUIApplication, screen: CGRect) -> CGRect {
        var top = screen.minY
        for chrome in [app.descendants(matching: .any)["chat-search-button"].firstMatch,
                       app.textFields["chat-search-field"]] where chrome.exists {
            top = max(top, chrome.frame.maxY)
        }
        var bottom = screen.maxY
        if app.keyboards.count > 0 { bottom = min(bottom, app.keyboards.firstMatch.frame.minY) }
        return CGRect(x: screen.minX, y: top, width: screen.width, height: max(0, bottom - top))
    }

    /// Drag the result list down a little, so a row whose name is tucked under the Search bar
    /// comes into reach. With a keyword typed a scroll keeps the bar open
    /// (`ChatSearchClose.scrollCloses`), so this changes which results are reachable and
    /// nothing else about the state under test.
    private func nudgeResultsDown(_ app: XCUIApplication, band: CGRect) {
        let origin = app.coordinate(withNormalizedOffset: CGVector(dx: 0, dy: 0))
        origin.withOffset(CGVector(dx: band.midX, dy: band.minY + 30))
            .press(forDuration: 0.1,
                   thenDragTo: origin.withOffset(CGVector(dx: band.midX, dy: band.minY + 230)))
        sleep(2)
    }

    /// Type the keyword, then pick the result row to tap: holding still, with `child` realized
    /// inside it, and — the part that matters — with the point `aim` asks for actually
    /// reachable. Each case says where in the row it is aiming, so the pick and the touch
    /// cannot disagree.
    ///
    /// Which hits are named is not up to the test: the fixture's group repeats senders, so a run
    /// can come up with exactly one named result and its name behind the bar. When nothing
    /// qualifies the list is nudged down and asked again rather than failing on the harness.
    private func settledResult(_ app: XCUIApplication, screen: CGRect, showing child: String,
                               aim: (_ bubble: CGRect, _ child: CGRect) -> CGPoint)
        -> (id: String, frame: CGRect, point: CGPoint)? {
        let field = app.textFields["chat-search-field"]
        field.tap()
        field.typeText(Self.keyword)
        sleep(2)
        var band = reachable(app, screen: screen)
        var ambiguous: [String] = []
        for attempt in 0..<3 {
            band = reachable(app, screen: screen)
            let rows = stillRows(app, screen: screen)
            for candidate in rows {
                let inner = inRow(app, candidate.id, child)
                guard inner.exists else { continue }
                let point = aim(candidate.frame, inner.frame)
                guard band.contains(point) else { continue }
                // Inside the band is not enough: a point can sit within it and still be close
                // enough to its top edge that the chrome takes the touch, because
                // `CGRect.contains` admits the boundary exactly. `ChatSearchRevealLandingUITests`
                // measured the number on the same fixture — its one traced mis-tap had 7 pt of
                // clearance where its nine good taps had 15 at worst — and 24 sits above the
                // worst good tap. Hardening, not a fix for anything currently failing: the two
                // landing failures on iOS 26.2 aim 97 and 115 pt below the band top.
                guard point.y - band.minY >= Self.aimBandMargin else { continue }
                // The point has to belong to this row and to no other. Result rows overlap
                // their neighbours by a few points, so an aim near an edge can sit inside the
                // next row's frame; the app then reveals the row that owns the touch while the
                // assertions read the row this picked, and the failure reads as a landing bug
                // when the landing was right. Rejecting the candidate removes that whole class,
                // rather than the one aim that happened to cross the line
                // ([feedback group-chat-spacing-ios-2026-11-01] took the overlap from 4 pt to 6
                // and broke a 5 pt aim). `ChatSearchRevealLandingUITests` guards its own picks
                // the same way.
                if let other = rows.first(where: { $0.id != candidate.id && $0.frame.contains(point) }) {
                    ambiguous.append("\(candidate.id) at \(point) is also inside \(other.id) \(other.frame)")
                    continue
                }
                return (id: candidate.id, frame: candidate.frame, point: point)
            }
            if attempt < 2 { nudgeResultsDown(app, band: band) }
        }
        // Both exits fail. A run where every candidate is ambiguous says something specific —
        // the rows have closed up far enough that no result can be tapped unambiguously — and
        // it must never be mistaken for a pass or for the harness simply not finding a row.
        let why: String
        if ambiguous.isEmpty {
            why = "no result row is still, showing \(child), with somewhere inside it to tap "
                + "that a touch can reach, after two nudges"
        } else {
            why = "no unambiguous result row available: every candidate showing \(child) has its "
                + "aim point inside a neighbouring row too, so a tap would reveal a row this test "
                + "did not name — the result rows have closed up, check the transcript's gap "
                + "metrics. Rejected [\(ambiguous.joined(separator: "; "))]"
        }
        XCTFail("\(why) (reachable \(band); results: \(describe(bubbles(app, screen: screen))))")
        return nil
    }

    /// Tap, wait for the bar to go, then wait past the reveal's animation so everything after
    /// this reads where the transcript came to rest rather than where it was passing through.
    private func tapAndSettle(_ app: XCUIApplication, at point: CGPoint) {
        tap(app, at: point)
        let field = app.textFields["chat-search-field"]
        let deadline = Date().addingTimeInterval(settle)
        while field.exists, Date() < deadline { usleep(150_000) }
        sleep(3)
    }

    /// What a reader does: type a keyword and tap a result at once, with the keyboard still up.
    /// The touch lands in the bubble's bottom padding — inside the row, clear of the sender's
    /// avatar and of the name, and clear of the text, so no `Button` inside the bubble is under
    /// it. The bar goes, the reader stays in the chat, and the transcript comes to rest on the
    /// message that was tapped. This is the case that passed before the fix: nothing under the
    /// touch but the row, so nothing fired alongside the reveal.
    func testAResultTapClearOfTheAvatarAndNameRevealsTheMessageInTheChat() {
        let app = openLongLaunchWeekSearch()
        let screen = app.windows.firstMatch.frame
        // The bottom of the row is the bubble's own padding (`Self.bubbleBottomAim`): inside
        // the row's hit area, below the last line of text, and far enough off the edge that
        // the point belongs to this row and not to the one under it. The name column's midX
        // keeps it inside the bubble horizontally, away from the avatar lane.
        guard let chosen = settledResult(app, screen: screen, showing: "bubble-name",
                                         aim: { bubble, name in
                                             CGPoint(x: name.midX, y: bubble.maxY - Self.bubbleBottomAim)
                                         }) else { return }
        XCTAssertEqual(app.keyboards.count, 1, "the keyboard is up, as it is for a reader who just typed")
        let point = chosen.point
        let name = inRow(app, chosen.id, "bubble-name").frame
        let avatar = inRow(app, chosen.id, "sender-avatar")
        XCTAssertFalse(name.contains(point), "the touch is clear of the name button at \(name)")
        if avatar.exists {
            XCTAssertFalse(avatar.frame.contains(point),
                           "the touch is clear of the avatar button at \(avatar.frame)")
        }
        shot(app, "1-results-keyboard-up")

        tapAndSettle(app, at: point)
        shot(app, "2-after-a-tap-clear-of-both-buttons")

        assertLandedInChat(outcome(app, tapped: chosen.id, point: point, screen: screen))
    }

    /// The same tap, on the sender's avatar inside the result row — the case that failed, and
    /// the one the fix is for. Staying in the chat is correct here too: while a keyword filters
    /// the list the row owns the tap, and a reader aiming at a result has not asked to leave
    /// the conversation. What it used to do was reveal the message *and* push the sender's page
    /// over it, because the avatar is a `Button` firing alongside the row's `simultaneousGesture`.
    func testAResultTapOnTheSenderAvatarAlsoStaysInTheChat() {
        let app = openLongLaunchWeekSearch()
        let screen = app.windows.firstMatch.frame
        guard let chosen = settledResult(app, screen: screen, showing: "sender-avatar",
                                         aim: { _, avatar in
                                             CGPoint(x: avatar.midX, y: avatar.midY)
                                         }) else { return }
        let point = chosen.point
        shot(app, "1-results-keyboard-up")

        tapAndSettle(app, at: point)
        shot(app, "2-after-a-tap-on-the-sender-avatar")

        assertLandedInChat(outcome(app, tapped: chosen.id, point: point, screen: screen))
    }

    /// And on the name inside the bubble, the other `Button` on the same profile target. It is
    /// its own case because it is the one the tester reached by accident on a short message,
    /// where the name takes up most of the bubble.
    ///
    /// **This case asserts the profile claim only, deliberately.** Reaching an in-bubble name
    /// that a touch can get to takes a scroll of the result list, after which the row that comes
    /// up is `long-408` — and a reveal of `long-408` comes to rest about thirty rows short of
    /// it, with the message never realized, both times it has been measured. The chat is
    /// frontmost while that happens, so it is not the bug this branch fixes: it is #2404's
    /// landing, and it is its own task. This case must not pretend to carry it, and must not go
    /// red for it either.
    ///
    /// **Do not read this as "only `long-408` lands short".** That was true of every run up to
    /// and including iOS 27.0 on the merged tree, where `long-412` — what the two cases above
    /// pick — landed correctly. It is false on iOS 26.2 at `0b77fdbf7`, where `long-412` is the
    /// row that fails, in both of those cases, about thirty rows into history with the message
    /// never realized. The cause of that is not established and is not this branch's.
    /// Recorded in `internal/ios-search-result-reveal-landing.md`.
    func testAResultTapOnTheNameInTheBubbleAlsoStaysInTheChat() {
        let app = openLongLaunchWeekSearch()
        let screen = app.windows.firstMatch.frame
        guard let chosen = settledResult(app, screen: screen, showing: "bubble-name",
                                         aim: { _, name in
                                             CGPoint(x: name.midX, y: name.midY)
                                         }) else { return }
        let point = chosen.point
        shot(app, "1-results-keyboard-up")

        tapAndSettle(app, at: point)
        shot(app, "2-after-a-tap-on-the-name-in-the-bubble")

        assertStillInTheChat(outcome(app, tapped: chosen.id, point: point, screen: screen))
    }

    /// The suppression is scoped to the filter, and this says so from the outside: with no
    /// keyword, the same avatar in the same chat still opens the sender's page. A reader who is
    /// only reading must not notice this fix at all.
    func testWithNoFilterTheSameAvatarStillOpensTheSendersPage() {
        let app = launchLongLaunchWeek()
        let screen = app.windows.firstMatch.frame
        let band = reachable(app, screen: screen)
        let resting = bubbles(app, screen: screen)
        var picked: (id: String, point: CGPoint)?
        for candidate in stillRows(app, screen: screen) where picked == nil {
            let avatar = inRow(app, candidate.id, "sender-avatar")
            guard avatar.exists else { continue }
            let point = CGPoint(x: avatar.frame.midX, y: avatar.frame.midY)
            if band.contains(point) { picked = (id: candidate.id, point: point) }
        }
        guard let chosen = picked else {
            return XCTFail("no bubble is wholly on screen, still, with a reachable sender avatar "
                           + "(reachable \(band); rows: \(describe(resting)))")
        }
        shot(app, "1-chat-at-rest-nothing-filtering")

        tap(app, at: chosen.point)
        sleep(3)
        shot(app, "2-after-a-tap-on-the-avatar-with-nothing-filtering")

        let header = app.descendants(matching: .any)["chat-search-button"].firstMatch
        XCTAssertFalse(header.isHittable,
                       "with nothing filtering, a tap on the avatar opens the sender's page over "
                       + "the chat as it always has — profile pane marker seen: "
                       + "\(profilePaneIsUp(app)); tapped \(chosen.id)'s avatar at \(chosen.point)")
    }
}
