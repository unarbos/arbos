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

    /// Group shaped like Arbonites: ≥ 2 000 rows, pictures, charts, code cards, mentions, reactions.
    func testGroupPans() {
        let app = launch(["ARBOS_OPEN_PEOPLE": "fx-group-launch", "ARBOS_FIXTURE_LONG_ROWS": rows, "ARBOS_FIXTURE_LONG_RICH": "1"])
        drive(app, chatId: "fx-group-launch")
    }

    /// The small chat: the fixture DM as it ships (a handful of rows).
    func testDMPans() {
        let app = launch(["ARBOS_OPEN_PEOPLE": "fx-dm-unconst"])
        drive(app, chatId: "fx-dm-unconst")
    }

    /// Deep into history with the finger: 90 fast flicks with the finger moving DOWN (older
    /// rows come in from the top; past the 360-row window cap the window slides and re-pins),
    /// then 24 with the finger moving up, back toward the newest. The touch log says whether
    /// any pan was dropped on the way and the frame log what a slide costs. (Finger up at the
    /// bottom of a chat is a bounce at the end, not history — the first version of this test
    /// had it backwards.)
    func testGroupDeepHistoryPans() {
        let app = launch(["ARBOS_OPEN_PEOPLE": "fx-group-launch", "ARBOS_FIXTURE_LONG_ROWS": rows, "ARBOS_FIXTURE_LONG_RICH": "1"])
        XCTAssertTrue(app.buttons["chat-back:fx-group-launch"].waitForExistence(timeout: appear), "the chat opened")
        sleep(4)
        let window = app.windows.firstMatch.frame
        let low = CGPoint(x: window.midX, y: window.maxY - 240)
        let high = CGPoint(x: window.midX, y: 180)
        phase(app, "deep-into-history") {
            for _ in 0..<90 { pan(app, from: high, by: CGVector(dx: 0, dy: 300), velocity: .fast); usleep(600_000) }
        }
        sleep(2)
        phase(app, "deep-back-toward-newest") {
            for _ in 0..<24 { pan(app, from: low, by: CGVector(dx: 0, dy: -300), velocity: .fast); usleep(600_000) }
        }
        sleep(3)
    }

    // MARK: The script

    private func drive(_ app: XCUIApplication, chatId: String) {
        XCTAssertTrue(app.buttons["chat-back:\(chatId)"].waitForExistence(timeout: appear), "the chat opened")
        let bubbles = app.descendants(matching: .any).matching(NSPredicate(format: "identifier BEGINSWITH 'bubble:'"))
        XCTAssertTrue(bubbles.firstMatch.waitForExistence(timeout: settle), "the transcript shows bubbles")
        sleep(4)
        let window = app.windows.firstMatch.frame
        let midX = window.midX
        // Pans start inside the transcript, clear of the header (top ~140 pt) and the composer (bottom ~200 pt).
        let low = CGPoint(x: midX, y: window.maxY - 240)
        let high = CGPoint(x: midX, y: 180)

        // 1. Flicks: fast, 8 with the finger moving up (toward the newest — at the bottom a
        //    bounce at the end) then 8 moving down (older rows come in). The finger moves 300 pt.
        phase(app, "flick-finger-up") {
            for _ in 0..<8 { pan(app, from: low, by: CGVector(dx: 0, dy: -300), velocity: .fast); usleep(900_000) }
        }
        phase(app, "flick-finger-down") {
            for _ in 0..<8 { pan(app, from: high, by: CGVector(dx: 0, dy: 300), velocity: .fast); usleep(900_000) }
        }
        // 2. Slow drags with the finger held: 6 × 250 pt up, 6 down.
        phase(app, "drag-up") {
            for _ in 0..<6 { pan(app, from: low, by: CGVector(dx: 0, dy: -250), velocity: .slow); usleep(700_000) }
        }
        phase(app, "drag-down") {
            for _ in 0..<6 { pan(app, from: high, by: CGVector(dx: 0, dy: 250), velocity: .slow); usleep(700_000) }
        }
        // 3. Short pans with a slight sideways wobble, as a thumb makes them: 12 × 70 pt. These are
        //    the ones a competing horizontal recognizer can swallow.
        phase(app, "short-up") {
            for i in 0..<12 {
                let wobble = CGFloat(i % 3) * 6 - 6
                pan(app, from: CGPoint(x: midX + CGFloat(i % 4) * 30 - 45, y: low.y - CGFloat(i % 5) * 40),
                    by: CGVector(dx: wobble, dy: -70), velocity: .default)
                usleep(600_000)
            }
        }
        // 4. Pans starting on a bubble (not the gutter): the per-bubble recognizers see these first.
        phase(app, "bubble-up") {
            for _ in 0..<8 {
                guard let bubble = bubbles.allElementsBoundByIndex.first(where: { $0.frame.minY > 160 && $0.frame.maxY < window.maxY - 260 && $0.frame.height > 20 }) else { break }
                let start = bubble.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
                start.press(forDuration: 0.02, thenDragTo: start.withOffset(CGVector(dx: 4, dy: -220)), withVelocity: .default, thenHoldForDuration: 0.02)
                usleep(800_000)
            }
        }
        sleep(3)
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

    // MARK: System scroll metrics (hitch time ratio) over a fixed swipe set

    /// XCTest's scroll signpost metrics (UIKit's own scroll-deceleration / dragging intervals):
    /// hitch time ratio in ms/s, frame rate and duration, five iterations of the same swipes.
    func testGroupScrollMetrics() throws {
        let app = launch(["ARBOS_OPEN_PEOPLE": "fx-group-launch", "ARBOS_FIXTURE_LONG_ROWS": rows, "ARBOS_FIXTURE_LONG_RICH": "1"])
        XCTAssertTrue(app.buttons["chat-back:fx-group-launch"].waitForExistence(timeout: appear), "the chat opened")
        sleep(4)
        let window = app.windows.firstMatch.frame
        let low = CGPoint(x: window.midX, y: window.maxY - 240)
        let high = CGPoint(x: window.midX, y: 180)
        let options = XCTMeasureOptions()
        options.iterationCount = 5
        measure(metrics: [XCTOSSignpostMetric.scrollDecelerationMetric, XCTOSSignpostMetric.scrollDraggingMetric], options: options) {
            for _ in 0..<3 { pan(app, from: low, by: CGVector(dx: 0, dy: -300), velocity: .fast); usleep(900_000) }
            for _ in 0..<3 { pan(app, from: high, by: CGVector(dx: 0, dy: 300), velocity: .fast); usleep(900_000) }
        }
    }
}
