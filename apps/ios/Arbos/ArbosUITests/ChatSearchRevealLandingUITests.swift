import XCTest

/// Why a tapped search result sometimes rests short of the message it was given (#2404).
///
/// This is the landing symptom, and it is a different bug from the profile page a touch over
/// the avatar used to push over the chat. It is stated separately here because it survives
/// the frontmost check: the chat is in front, the bar has gone, nothing covers the message —
/// the transcript has simply come to rest somewhere else.
///
/// **What the cases vary is whether the tapped result is inside the row window, and whether
/// the reveal is the one from before the fix.** A hit among the newest is in the window
/// already and needs nothing. A hit the reader scrolled back to is not, and reaching it means
/// moving the window and then scrolling into rows the `LazyVStack` has never laid out.
/// Everything else is identical: same keyword, same tap point in the bubble's bottom padding
/// clear of every button, same settle.
///
/// Three earlier explanations were measured and ruled out, which is why none is a case here.
/// A near-top window grow firing inside the reveal: across four launches the trace recorded
/// no grow at all. The animation on the scroll: unanimated, it landed in the same wrong place
/// twice out of two. The shape of the window on its own: sliding it onto the target instead
/// of stretching it back to the tail moved the landing from 18–35 rows *newer* than the
/// message to 13 rows *older* than it, twice out of two — the same fault with its sign
/// flipped, which is what showed the estimate itself to be the mechanism.
///
/// The first run of this file also varied the wrong thing. It tapped two named rows four
/// apart in the list and both landed, with byte-identical offset trajectories from the same
/// starting offset of 8676, because both were reachable without scrolling the list at all and
/// both were already in the window.
///
/// Order matters and `continueAfterFailure` is off so it holds: a coordinate read says nothing
/// until it is established what is frontmost, because XCUITest reports a parent view's
/// elements, with plausible frames, from behind a pushed navigation destination.
final class ChatSearchRevealLandingUITests: XCTestCase {
    private let appear: TimeInterval = 150
    private let settle: TimeInterval = 20
    private static let longRows = 420
    private static let keyword = "embargo"

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func openSearch(preFix: Bool) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = [
            "ARBOS_FIXTURE": "1",
            "ARBOS_OPEN_PEOPLE": "fx-group-launch",
            "ARBOS_FIXTURE_LONG_ROWS": "\(Self.longRows)",
            // The app prints what moves the transcript; the run's stdout is the evidence.
            "ARBOS_TRACE_REVEAL": "1",
        ]
        if preFix { app.launchEnvironment["ARBOS_REVEAL_PRE_FIX"] = "1" }
        app.launch()
        let search = app.descendants(matching: .any)["chat-search-button"].firstMatch
        // Poll rather than one long wait, and give up the moment the app is gone. A trap in
        // the app under test looks identical to a slow Mac for the whole timeout, and four
        // cases each sat out 150 s of a contended heavy slot before saying so once.
        let deadline = Date().addingTimeInterval(appear)
        while !search.exists, Date() < deadline {
            XCTAssertEqual(app.state, .runningForeground,
                           "the app is still running (state \(app.state.rawValue)) — it died "
                           + "before the chat opened, so nothing below this is about the chat")
            usleep(500_000)
        }
        XCTAssertTrue(search.exists, "the chat header's search disc is on screen")
        // The arm, read back out of the app rather than assumed from what was set. A launch
        // variable that does not arrive turns the symptom arm into a second control arm, and
        // the run then reports the fix landing against no baseline at all.
        XCTAssertEqual(search.value as? String, preFix ? "reveal-pre-fix" : "reveal-fixed",
                       "the app is running the \(preFix ? "pre-fix" : "fixed") reveal")
        let field = app.textFields["chat-search-field"]
        for _ in 0..<4 where !field.exists {
            sleep(1)
            search.tap()
            _ = field.waitForExistence(timeout: 8)
        }
        XCTAssertTrue(field.exists, "the Search-in-chat bar opened")
        field.tap()
        field.typeText(Self.keyword)
        sleep(2)
        return app
    }

    private func shot(_ app: XCUIApplication, _ name: String) {
        let a = XCTAttachment(screenshot: app.screenshot())
        a.name = name
        a.lifetime = .keepAlways
        add(a)
    }

    private func row(_ app: XCUIApplication, _ id: String) -> XCUIElement {
        app.descendants(matching: .any)["bubble:\(id)"].firstMatch
    }

    private func inRow(_ app: XCUIApplication, _ id: String, _ child: String) -> XCUIElement {
        row(app, id).descendants(matching: .any)[child].firstMatch
    }

    /// Every realized bubble, as the tree lists them.
    private func bubbles(_ app: XCUIApplication) -> [(id: String, frame: CGRect)] {
        let all = app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier BEGINSWITH %@", "bubble:"))
        var out: [(id: String, frame: CGRect)] = []
        for i in 0..<min(all.count, 60) {
            let e = all.element(boundBy: i)
            guard e.exists else { continue }
            out.append((id: e.identifier, frame: e.frame))
        }
        return out
    }

    private func describe(_ rows: [(id: String, frame: CGRect)]) -> String {
        rows.map { "\($0.id)@y\(Int($0.frame.minY))…\(Int($0.frame.maxY))" }.joined(separator: ", ")
    }

    /// The part of the window a touch actually reaches: below the header and the Search bar,
    /// above the keyboard. Being inside the window is not the same as being reachable.
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

    /// One drag back through the results, toward older hits — the reader scrolling the list.
    /// The press is slow and short so it drags rather than flicks: a flick's deceleration
    /// would leave the list somewhere this cannot name. With a keyword typed a scroll keeps
    /// the bar open (`ChatSearchClose.scrollCloses`), so this moves where the list stands and
    /// changes nothing else about the state under test.
    private func nudge(_ app: XCUIApplication, band: CGRect) {
        let origin = app.coordinate(withNormalizedOffset: CGVector(dx: 0, dy: 0))
        origin.withOffset(CGVector(dx: band.midX, dy: band.minY + 30))
            .press(forDuration: 0.1,
                   thenDragTo: origin.withOffset(CGVector(dx: band.midX, dy: band.minY + 230)))
        usleep(900_000)
    }

    private func tap(_ app: XCUIApplication, at point: CGPoint) {
        app.coordinate(withNormalizedOffset: CGVector(dx: 0, dy: 0))
            .withOffset(CGVector(dx: point.x, dy: point.y))
            .tap()
    }

    /// A still hit whose bottom padding is reachable, and the point to tap in it.
    ///
    /// Only the touch point has to be in the band. A tall bubble whose top is behind the
    /// header is still perfectly tappable near its bottom, and requiring the whole row to be
    /// reachable threw away `long-412` — a 292-point markdown bubble — eight nudges running.
    ///
    /// `Theme.bubbleVPad` is 11, so the bottom few points are the bubble's own padding:
    /// inside the row's hit area, below the last line of text, and clear of the sender's
    /// avatar and of the name, neither of which must take this touch. This branch is off
    /// `main`, where nothing suppresses those buttons, so the clearance is asserted.
    private func aimable(_ app: XCUIApplication, screen: CGRect) -> (id: String, point: CGPoint)? {
        let band = reachable(app, screen: screen)
        for (id, frame) in bubbles(app) {
            let point = CGPoint(x: frame.midX, y: frame.maxY - 5)
            guard band.contains(point), frame.height > 24 else { continue }
            usleep(400_000)
            let e = app.descendants(matching: .any)[id].firstMatch
            guard e.exists, e.frame == frame else { continue }
            let short = String(id.dropFirst("bubble:".count))
            let name = inRow(app, short, "bubble-name")
            if name.exists, name.frame.contains(point) { continue }
            let avatar = inRow(app, short, "sender-avatar")
            if avatar.exists, avatar.frame.contains(point) { continue }
            return (short, point)
        }
        return nil
    }

    /// Open the bar, drag the result list `scrollUp` times toward older hits, then aim at
    /// whichever hit is reachable there. Everything here is harness, not finding: kept apart
    /// from the landing so the symptom arm's expected-failure wrapper cannot swallow a broken
    /// rig — it did exactly that once, reporting a run whose app died at launch as a
    /// reproduction.
    private func aimed(scrollUp: Int, preFix: Bool)
        -> (app: XCUIApplication, screen: CGRect, id: String, point: CGPoint)? {
        let app = openSearch(preFix: preFix)
        let screen = app.windows.firstMatch.frame
        for _ in 0..<scrollUp { nudge(app, band: reachable(app, screen: screen)) }
        for _ in 0..<4 {
            if let hit = aimable(app, screen: screen) { return (app, screen, hit.id, hit.point) }
            nudge(app, band: reachable(app, screen: screen))
        }
        XCTFail("no reachable hit after \(scrollUp) drags up the result list "
                + "(results: \(describe(bubbles(app))))")
        return nil
    }

    /// Tap the aimed point and say where the transcript came to rest. Identical for every
    /// case but how far up the result list it has scrolled and how the reveal moves the
    /// row window onto the target.
    private func landing(_ arm: String,
                         _ aimed: (app: XCUIApplication, screen: CGRect, id: String, point: CGPoint)) {
        let (app, screen, id, point) = aimed
        shot(app, "1-\(arm)-aimed")
        // The test runner's stdout lands in the result bundle's Staging tree, which survives
        // the import that never finishes. It is the only record of the pre-tap state on a
        // green run, and the four cases are only comparable if that state matches.
        print("[rig] \(arm) aimed at \(point); results before the tap: \(describe(bubbles(app)))")

        tap(app, at: point)
        let field = app.textFields["chat-search-field"]
        let deadline = Date().addingTimeInterval(settle)
        while field.exists, Date() < deadline { usleep(150_000) }
        sleep(3)
        shot(app, "2-\(arm)-after-the-tap")

        // Frontmost first: a frame means nothing until the chat is what the reader sees.
        let header = app.descendants(matching: .any)["chat-search-button"].firstMatch
        let resting = bubbles(app)
        let detail = "tapped \(arm) at \(point); screen \(screen); resting [\(describe(resting))]"
        print("[rig] \(detail)")
        XCTAssertTrue(header.isHittable, "the chat is frontmost — \(detail)")
        XCTAssertFalse(field.exists, "a result tap closes the bar — \(detail)")

        let revealed = row(app, id)
        XCTAssertTrue(revealed.exists, "the tapped message is realized in the transcript — \(detail)")
        XCTAssertTrue(screen.contains(revealed.frame),
                      "the tapped message is wholly on screen at \(revealed.frame) — \(detail)")
        let middleHalf = screen.insetBy(dx: 0, dy: screen.height / 4)
        XCTAssertTrue(middleHalf.contains(CGPoint(x: revealed.frame.midX, y: revealed.frame.midY)),
                      "…and centred, not against an edge — \(detail)")
    }

    /// A whole case, with the landing allowed to fail or not. `expectShort` marks the baseline
    /// arm: its landing is recorded rather than asserted, because the fault is intermittent by
    /// report and a suite should not go red for the run where it does not fire.
    ///
    /// That tolerance has to be paid for. An arm that is excused either way says nothing on
    /// its own, so the baseline prints its own verdict on a line that can be read without
    /// interpreting coordinates: a run whose baseline **landed** has measured the fix against
    /// no baseline at all, which is what happened once already here and read as four passes.
    /// Launching and aiming stay outside the expectation, so a broken rig still goes red.
    private func run(scrollUp: Int, preFix: Bool, expectShort: Bool = false) {
        let arm = "up\(scrollUp)/\(preFix ? "pre-fix" : "fixed")"
        guard let a = aimed(scrollUp: scrollUp, preFix: preFix) else { return }
        guard expectShort else { return landing(arm, a) }
        continueAfterFailure = true
        XCTExpectFailure("the reveal as it was before the fix (#2404)", strict: false) { landing(arm, a) }
        let onTarget = restedOn(a)
        print("[rig] BASELINE \(onTarget ? "LANDED — no baseline this run" : "MISSED — baseline holds"): "
              + "\(arm) target \(a.id)")
    }

    /// Did the transcript come to rest with the tapped message realized and in the middle
    /// half of the screen? The same question `landing` asserts, asked without asserting it.
    private func restedOn(_ aimed: (app: XCUIApplication, screen: CGRect, id: String, point: CGPoint)) -> Bool {
        let revealed = row(aimed.app, aimed.id)
        guard revealed.exists else { return false }
        let middleHalf = aimed.screen.insetBy(dx: 0, dy: aimed.screen.height / 4)
        return aimed.screen.contains(revealed.frame)
            && middleHalf.contains(CGPoint(x: revealed.frame.midX, y: revealed.frame.midY))
    }

    /// The control: the reader taps a result without scrolling the list. The hit is one of
    /// the newest, so it is already inside the row window and the reveal has no window to
    /// move. Measured correct four times.
    func testAResultAlreadyInTheWindowLands() {
        run(scrollUp: 0, preFix: false)
    }

    /// The reader scrolls back through the results before tapping one — the ordinary way of
    /// hunting something that is not the most recent mention of it, and what #2404 describes.
    /// The hit is then outside the row window, and this arm reveals it the way the code used
    /// to: stretch the window back from the tail, then one `scrollTo` and no correction.
    ///
    /// Measured short six times out of six, resting 18 to 35 rows newer than the message
    /// with the message never realized.
    func testAResultOutsideTheWindowIsTheSymptomBeforeTheFix() {
        run(scrollUp: 14, preFix: true, expectShort: true)
    }

    /// The same reveal with the fix: the window slides onto the target rather than stretching
    /// back to it, and the scroll is corrected as the rows gain real heights.
    func testAResultOutsideTheWindowLandsAfterTheFix() {
        run(scrollUp: 14, preFix: false)
    }

    /// The fix again. One correct landing says nothing about a fault that only shows
    /// sometimes, and the symptom arm above is the thing that establishes the rate.
    func testAResultOutsideTheWindowLandsAfterTheFixAgain() {
        run(scrollUp: 14, preFix: false)
    }
}
