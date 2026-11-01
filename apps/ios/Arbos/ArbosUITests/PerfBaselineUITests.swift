import XCTest

/// Perf baseline (ios-perf-baseline, 2026-11-01): the scenarios every responsiveness fix is
/// measured on before and after, the same way each time. Skipped unless the test runner has
/// `ARBOS_PERF_BASELINE=1`, so the gate's Debug run pays only the skip.
///
/// Measure a Release build with the fixture compiled in — Debug SwiftUI numbers mean nothing:
///
///   xcodebuild build-for-testing -configuration Release SWIFT_ACTIVE_COMPILATION_CONDITIONS=ARBOS_FIXTURE …
///   TEST_RUNNER_ARBOS_PERF_BASELINE=1 xcodebuild test-without-building -only-testing:ArbosUITests/PerfBaselineUITests …
///
/// The numbers are the app's `PerfSignpost` intervals, read here through `XCTOSSignpostMetric`
/// and line by line in the app's `perf-frames` log (each interval's duration, every late frame,
/// a once-a-second memory line). `perfphase … begin/end` lines in the runner's log bracket the
/// scroll phases so the late frames can be summed per phase.
final class PerfBaselineUITests: XCTestCase {
    private let appear: TimeInterval = 120
    private static let subsystem = "com.unarbos.arbos.ios"
    private static let category = "PointsOfInterest"
    private let group = "fx-group-launch"
    private let dm = "fx-dm-unconst"

    override func setUpWithError() throws {
        try XCTSkipUnless(env("ARBOS_PERF_BASELINE") == "1", "the perf baseline runs only with ARBOS_PERF_BASELINE=1")
        continueAfterFailure = false
    }

    private func env(_ key: String) -> String? { ProcessInfo.processInfo.environment[key] }
    private var iterations: Int { Int(env("ARBOS_PERF_ITERATIONS") ?? "") ?? 8 }
    private var longRows: String { env("ARBOS_SCROLL_ROWS") ?? "2200" }

    /// An Arbonites-shaped account: 60 more chats on the list and a 2 200-row rich group.
    private func environment(longGroup: Bool) -> [String: String] {
        var e = ["ARBOS_FIXTURE": "1", "ARBOS_PERF_FRAMES": "1", "ARBOS_FIXTURE_HOME_ROWS": "60"]
        if longGroup {
            e["ARBOS_FIXTURE_LONG_ROWS"] = longRows
            e["ARBOS_FIXTURE_LONG_RICH"] = "1"
        }
        return e
    }

    private func launch(longGroup: Bool = true) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = environment(longGroup: longGroup)
        app.launch()
        XCTAssertTrue(row(app, dm).waitForExistence(timeout: appear), "the chat list is up")
        sleep(2)
        return app
    }

    private func signpost(_ name: String) -> XCTOSSignpostMetric {
        XCTOSSignpostMetric(subsystem: Self.subsystem, category: Self.category, name: name)
    }

    private func options(manual: Bool) -> XCTMeasureOptions {
        let o = XCTMeasureOptions()
        o.iterationCount = iterations
        if manual { o.invocationOptions = [.manuallyStop] }
        return o
    }

    private func row(_ app: XCUIApplication, _ chatId: String) -> XCUIElement { app.buttons["home-row:\(chatId)"] }
    private func back(_ app: XCUIApplication, _ chatId: String) -> XCUIElement { app.buttons["chat-back:\(chatId)"] }

    private func open(_ app: XCUIApplication, _ chatId: String) {
        row(app, chatId).tap()
        XCTAssertTrue(back(app, chatId).waitForExistence(timeout: appear), "\(chatId) opened")
    }

    private func leave(_ app: XCUIApplication, _ chatId: String) {
        back(app, chatId).tap()
        XCTAssertTrue(row(app, chatId).waitForExistence(timeout: appear), "back on the chat list")
    }

    private func phase(_ name: String, _ body: () -> Void) {
        NSLog("perfphase %@ begin", name)
        body()
        NSLog("perfphase %@ end", name)
    }

    // MARK: Launch

    /// Process start → first responsive frame, by XCTest. The app's own `first-content`
    /// line (process start → the populated chat list) is in its `launch` log per launch.
    func testLaunchToChatList() {
        let app = XCUIApplication()
        app.launchEnvironment = environment(longGroup: false)
        measure(metrics: [XCTApplicationLaunchMetric(waitUntilResponsive: true)], options: options(manual: false)) {
            app.launch()
        }
    }

    // MARK: Opening and switching chats

    func testOpenLongGroup() { measureOpen(group) }

    func testOpenSmallDM() { measureOpen(dm) }

    private func measureOpen(_ chatId: String) {
        let app = launch()
        measure(metrics: [signpost("open-chat"), signpost("open-chat-settled"), XCTClockMetric()], options: options(manual: true)) {
            open(app, chatId)
            sleep(1)
            stopMeasuring()
            leave(app, chatId)
            sleep(1)
        }
    }

    /// In the DM → back → the group, as one measured step; the group was opened before, so
    /// this is a warm switch.
    func testSwitchChats() {
        let app = launch()
        open(app, group)
        leave(app, group)
        open(app, dm)
        sleep(1)
        measure(metrics: [signpost("back-settled"), signpost("open-chat"), signpost("open-chat-settled"), XCTClockMetric()],
                options: options(manual: true)) {
            back(app, dm).tap()
            open(app, group)
            sleep(1)
            stopMeasuring()
            leave(app, group)
            open(app, dm)
            sleep(1)
        }
    }

    // MARK: Scrolling

    func testScrollChatList() {
        let app = launch(longGroup: false)
        let frame = app.windows.firstMatch.frame
        let low = CGPoint(x: frame.midX, y: frame.maxY - 220)
        let high = CGPoint(x: frame.midX, y: 220)
        measure(metrics: [XCTOSSignpostMetric.scrollDecelerationMetric, XCTOSSignpostMetric.scrollDraggingMetric], options: options(manual: false)) {
            phase("list-flick") {
                for _ in 0..<4 { flick(app, frame: frame, from: low, dy: -420); usleep(700_000) }
                for _ in 0..<4 { flick(app, frame: frame, from: high, dy: 420); usleep(700_000) }
            }
        }
    }

    /// The long group: flicks into history (older rows come in from the top, the row window
    /// slides) and back toward the newest.
    func testScrollLongChat() {
        let app = launch()
        open(app, group)
        sleep(3)
        phase("chat-idle") { sleep(5) }
        let frame = app.windows.firstMatch.frame
        let low = CGPoint(x: frame.midX, y: frame.maxY - 240)
        let high = CGPoint(x: frame.midX, y: 180)
        measure(metrics: [XCTOSSignpostMetric.scrollDecelerationMetric, XCTOSSignpostMetric.scrollDraggingMetric], options: options(manual: false)) {
            phase("chat-history-flick") {
                for _ in 0..<6 { flick(app, frame: frame, from: high, dy: 300); usleep(600_000) }
            }
            phase("chat-newest-flick") {
                for _ in 0..<6 { flick(app, frame: frame, from: low, dy: -300); usleep(600_000) }
            }
        }
        phase("chat-slow-drag") {
            for dy in [260.0, -260.0, 260.0, -260.0] { drag(app, frame: frame, from: CGPoint(x: frame.midX, y: frame.midY - dy / 2), by: CGVector(dx: 0, dy: dy), velocity: .slow) }
        }
        phase("chat-time-peek") {
            for _ in 0..<4 { drag(app, frame: frame, from: CGPoint(x: frame.maxX - 40, y: frame.midY), by: CGVector(dx: -140, dy: 0), velocity: .default) }
        }
    }

    /// An agent answer streams into the open group, one word every 40 ms, while the
    /// transcript sits on its newest row; then the same stream goes on with the chat closed.
    /// The probe's per-second line counts home, chat and row bodies against tokens.
    func testStreamingAnswer() {
        let app = XCUIApplication()
        var e = environment(longGroup: true)
        e["ARBOS_FIXTURE_STREAM"] = group
        app.launchEnvironment = e
        app.launch()
        XCTAssertTrue(row(app, dm).waitForExistence(timeout: appear), "the chat list is up")
        open(app, group)
        sleep(8)
        phase("stream-pinned") { sleep(15) }
        leave(app, group)
        phase("stream-on-list") { sleep(10) }
    }

    /// One coordinate read per test: every `frame` query is an accessibility snapshot of the
    /// whole screen, which costs the app's main thread far more than the gesture.
    private func flick(_ app: XCUIApplication, frame: CGRect, from: CGPoint, dy: CGFloat) {
        let origin = app.windows.firstMatch.coordinate(withNormalizedOffset: .zero)
        let start = origin.withOffset(CGVector(dx: from.x - frame.minX, dy: from.y - frame.minY))
        start.press(forDuration: 0.02, thenDragTo: start.withOffset(CGVector(dx: 0, dy: dy)), withVelocity: .fast, thenHoldForDuration: 0.02)
    }

    /// A held drag that ends with the finger still down, so nothing decelerates.
    private func drag(_ app: XCUIApplication, frame: CGRect, from: CGPoint, by: CGVector, velocity: XCUIGestureVelocity) {
        let origin = app.windows.firstMatch.coordinate(withNormalizedOffset: .zero)
        let start = origin.withOffset(CGVector(dx: from.x - frame.minX, dy: from.y - frame.minY))
        start.press(forDuration: 0.05, thenDragTo: start.withOffset(by), withVelocity: velocity, thenHoldForDuration: 0.3)
        usleep(400_000)
    }

    // MARK: Composer

    /// Tap the field → keyboard up; then a 43-key line and Send → my bubble.
    func testComposerTypeAndSend() {
        let app = launch()
        measure(metrics: [signpost("keyboard-up"), signpost("keystroke"), signpost("send"), XCTClockMetric()], options: options(manual: true)) {
            open(app, dm)
            let field = app.textViews["composer-field"]
            XCTAssertTrue(field.waitForExistence(timeout: appear), "the composer is up")
            field.tap()
            XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 10), "the keyboard came up")
            field.typeText("The quick brown fox jumps over the lazy dog")
            app.buttons["composer-action"].tap()
            sleep(1)
            stopMeasuring()
            leave(app, dm)
            sleep(1)
        }
    }

    // MARK: Taps

    func testSettingsSheet() {
        let app = launch()
        measure(metrics: [signpost("settings-sheet"), XCTClockMetric()], options: options(manual: true)) {
            app.buttons["home-account"].tap()
            XCTAssertTrue(app.staticTexts["sheet-title"].waitForExistence(timeout: 10), "Settings opened")
            sleep(1)
            stopMeasuring()
            dismissSheet(app)
            XCTAssertTrue(row(app, dm).waitForExistence(timeout: appear), "back on the chat list")
            sleep(1)
        }
    }

    private func dismissSheet(_ app: XCUIApplication) {
        let cancel = app.buttons["sheet-cancel"]
        if cancel.exists {
            cancel.tap()
            return
        }
        let top = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.12))
        top.press(forDuration: 0.05, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.95)))
    }

    /// Long press on the group's newest bubble → its reactions and menu.
    func testMessageMenu() {
        let app = launch()
        open(app, group)
        sleep(2)
        let bubble = app.descendants(matching: .any)["bubble:g9"]
        XCTAssertTrue(bubble.waitForExistence(timeout: appear), "the newest bubble is on screen")
        measure(metrics: [signpost("message-menu"), XCTClockMetric()], options: options(manual: true)) {
            bubble.press(forDuration: 0.6)
            sleep(1)
            stopMeasuring()
            app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.08)).tap()
            sleep(1)
        }
    }
}
