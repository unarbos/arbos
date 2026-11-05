import XCTest

/// Scroll rig (iphone-scroll-perf, 2026-10-27): the same scripted pans on the same seeded
/// chat, launch after launch, so two builds (or one build with a gesture left out through
/// `ARBOS_PERF_GESTURES_OFF`) can be compared on the `perf` log the app writes:
/// `PerfLog` frame windows (fps, dropped, worst frame) and `TouchPerf` touch lines
/// (touch-down → pan began / first movement, dropped pans). The harness on the Mac pulls
/// that log after the run (`log show --predicate 'subsystem == "com.unarbos.arbos.ios"
/// AND category == "perf"'`); this file only drives the finger.
///
/// Every `ARBOS_*` variable of the test runner's environment is passed to the app, so one
/// `xcodebuild test-without-building` with `TEST_RUNNER_ARBOS_PERF_GESTURES_OFF=time` is
/// the "time peek off" arm of the A/B (the reply swipe is gone since 2026-10-28). Defaults: `ARBOS_PERF_LOG=1`, `ARBOS_PERF_TOUCH=1`.
final class ScrollPerfUITests: XCTestCase {
    private let appear: TimeInterval = 120
    private let settle: TimeInterval = 30

    override func setUpWithError() throws {
        continueAfterFailure = true
    }

    private func launch(_ env: [String: String]) -> XCUIApplication {
        let app = XCUIApplication()
        var merged = ["ARBOS_FIXTURE": "1", "ARBOS_PERF_LOG": "1", "ARBOS_PERF_TOUCH": "1"]
        for (k, v) in ProcessInfo.processInfo.environment where k.hasPrefix("ARBOS_") { merged[k] = v }
        for (k, v) in env { merged[k] = v }
        app.launchEnvironment = merged
        app.launch()
        return app
    }

    private var rows: String { ProcessInfo.processInfo.environment["ARBOS_SCROLL_ROWS"] ?? "2200" }

    /// Deep into history with the finger: 90 fast flicks with the finger moving DOWN (older
    /// rows come in from the top; past the 360-row window cap the window slides and re-pins),
    /// then 24 with the finger moving up, back toward the newest. The touch log says whether
    /// any pan was dropped on the way and the frame log what a slide costs. (Finger up at the
    /// bottom of a chat is a bounce at the end, not history — the first version of this test
    /// had it backwards.)
    func testGroupDeepHistoryPans() {
        let app = launch(["ARBOS_OPEN_PEOPLE": "fx-group-launch", "ARBOS_FIXTURE_LONG_ROWS": rows, "ARBOS_FIXTURE_LONG_RICH": "1"])
        let back = app.buttons["chat-back:fx-group-launch"]
        XCTAssertTrue(back.waitForExistence(timeout: appear), "the chat opened")
        let bubbles = app.descendants(matching: .any).matching(NSPredicate(format: "identifier BEGINSWITH 'bubble:'"))
        XCTAssertTrue(bubbles.firstMatch.waitForExistence(timeout: settle), "the transcript shows bubbles")
        let window = app.windows.firstMatch.frame
        let low = CGPoint(x: window.midX, y: window.maxY - 240)
        let high = CGPoint(x: window.midX, y: 180)
        // No pause between flicks: each lands when XCUITest reports the app idle, before the last one's
        // deceleration ends.
        phase(app, "deep-into-history") {
            for _ in 0..<90 { pan(app, from: high, by: CGVector(dx: 0, dy: 300), velocity: .fast) }
        }
        phase(app, "deep-back-toward-newest") {
            for _ in 0..<24 { pan(app, from: low, by: CGVector(dx: 0, dy: -300), velocity: .fast) }
        }
        XCTAssertEqual(app.state, .runningForeground, "the app survived the flicks")
        XCTAssertTrue(back.isHittable, "the chat is still open")
        XCTAssertTrue(bubbles.firstMatch.waitForExistence(timeout: settle), "the transcript still shows bubbles")
    }

    private func phase(_ app: XCUIApplication, _ name: String, _ body: () -> Void) {
        // The marker lands in the test log; the app's perf log is correlated by wall clock.
        NSLog("scrollperf phase %@ begin", name)
        body()
        NSLog("scrollperf phase %@ end", name)
    }

    /// The window's frame, read once per test: every `app.windows.firstMatch.frame` is an
    /// accessibility snapshot of the whole screen (≈ 200 ms of the app's main thread on
    /// 80 rich rows), which was the per-pan stall in the first runs.
    private var windowFrame: CGRect?

    private func pan(_ app: XCUIApplication, from: CGPoint, by: CGVector, velocity: XCUIGestureVelocity) {
        let window = app.windows.firstMatch
        let frame = windowFrame ?? window.frame
        windowFrame = frame
        let origin = window.coordinate(withNormalizedOffset: .zero)
        let start = origin.withOffset(CGVector(dx: from.x - frame.minX, dy: from.y - frame.minY))
        start.press(forDuration: 0.02, thenDragTo: start.withOffset(by), withVelocity: velocity, thenHoldForDuration: 0.02)
    }
}
