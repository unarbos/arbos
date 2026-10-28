import XCTest
import notify

/// "Sometimes after taking a screenshot the chat empties entirely" (Jacob, 16:40 UTC
/// 2026-10-28, [feedback iphone-chat-empties-after-screenshot-2026-10-28]): header, presence
/// row and composer stay, the transcript shows no bubble. A system screenshot opens the
/// feedback sheet over the chat (`RootView`), and the phone usually goes inactive and back
/// while the thumbnail is handled. This drives that path on the fixture app, several times
/// per shape (the bug is intermittent), and after every return asks the one question the
/// user asks: is at least one bubble drawn inside the transcript?
///
/// The simulator never posts `userDidTakeScreenshotNotification` on `simctl io screenshot`;
/// the Debug build's `RigTriggers` turns a Darwin notification into it (the runner lives in
/// the same simulator, so `notify_post` reaches the app). Shots go to `ARBOS_SHOT_DIR` when set
/// (`TEST_RUNNER_ARBOS_SHOT_DIR=…` on the xcodebuild line).
final class ScreenshotTranscriptUITests: XCTestCase {
    private let appear: TimeInterval = 120
    private let settle: TimeInterval = 30
    private static let screenshotTrigger = "com.unarbos.arbos.rig.screenshot"
    /// Arbonites-shaped group: thousands of rows, pictures, cards, mentions.
    private static let group = "fx-group-launch"
    private static let dm = "fx-dm-unconst"

    private var rounds: Int { Int(ProcessInfo.processInfo.environment["ARBOS_ROUNDS"] ?? "") ?? 3 }
    private var rows: String { ProcessInfo.processInfo.environment["ARBOS_SCROLL_ROWS"] ?? "2200" }

    override func setUpWithError() throws {
        continueAfterFailure = true
    }

    // MARK: Shapes

    /// At the tail of the big group, keyboard down: screenshot → sheet → Cancel.
    func testGroupAtTailScreenshotKeepsBubbles() {
        let app = launchGroup()
        for round in 1...rounds {
            cycle(app, chatId: Self.group, shape: "tail", round: round, before: {}, between: {})
        }
    }

    /// The composer focused (keyboard up, as after a send): the sheet takes the keyboard
    /// down under the chat, the composer inset shrinks while the sheet covers the transcript.
    func testGroupWithKeyboardScreenshotKeepsBubbles() {
        let app = launchGroup()
        for round in 1...rounds {
            cycle(app, chatId: Self.group, shape: "keyboard", round: round, before: { self.raiseKeyboard(app) }, between: {})
        }
    }

    /// Reading history (the reader scrolled up, the bottom pin off): the bubbles under the
    /// finger must still be there afterwards, and the reader must not be yanked elsewhere.
    func testGroupInHistoryScreenshotKeepsBubbles() {
        let app = launchGroup()
        for round in 1...rounds {
            cycle(app, chatId: Self.group, shape: "history", round: round, before: { self.flickIntoHistory(app, flicks: 10) }, between: {})
        }
    }

    /// The phone's usual sequence: screenshot, sheet up, the app goes to the background and
    /// comes back (the thumbnail tapped, Photos, the share sheet), then the sheet is dismissed.
    func testGroupScreenshotThenBackgroundForegroundKeepsBubbles() {
        let app = launchGroup()
        for round in 1...rounds {
            cycle(app, chatId: Self.group, shape: "background", round: round, before: {}, between: { self.backgroundAndForeground(app) })
        }
    }

    /// Same with the keyboard up first.
    func testGroupKeyboardScreenshotThenBackgroundForegroundKeepsBubbles() {
        let app = launchGroup()
        for round in 1...rounds {
            cycle(app, chatId: Self.group, shape: "keyboard-background", round: round,
                  before: { self.raiseKeyboard(app) }, between: { self.backgroundAndForeground(app) })
        }
    }

    /// Arbonites is live: while the sheet covers the chat someone starts and stops typing, so the
    /// presence row over the composer comes and goes — the bottom inset changes under the sheet.
    func testGroupScreenshotWithTypingUnderTheSheetKeepsBubbles() {
        let app = launch(["ARBOS_OPEN_PEOPLE": Self.group, "ARBOS_FIXTURE_LONG_ROWS": rows, "ARBOS_FIXTURE_LONG_RICH": "1", "ARBOS_FIXTURE_TYPING": "0"])
        for round in 1...rounds {
            cycle(app, chatId: Self.group, shape: "typing-under-sheet", round: round, before: {}, between: { self.toggleTypingUnderTheSheet() })
        }
    }

    /// Same with the keyboard up first and a background → foreground trip as well.
    func testGroupKeyboardScreenshotTypingAndBackgroundKeepsBubbles() {
        let app = launch(["ARBOS_OPEN_PEOPLE": Self.group, "ARBOS_FIXTURE_LONG_ROWS": rows, "ARBOS_FIXTURE_LONG_RICH": "1", "ARBOS_FIXTURE_TYPING": "0"])
        for round in 1...rounds {
            cycle(app, chatId: Self.group, shape: "keyboard-typing-background", round: round,
                  before: { self.raiseKeyboard(app) },
                  between: { self.toggleTypingUnderTheSheet(); self.backgroundAndForeground(app) })
        }
    }

    /// No sheet at all: background and foreground alone (lock / app switcher).
    func testGroupBackgroundForegroundAloneKeepsBubbles() {
        let app = launchGroup()
        XCTAssertTrue(app.buttons["chat-back:\(Self.group)"].waitForExistence(timeout: appear), "the chat opened")
        XCTAssertGreaterThan(waitForBubbles(app), 0, "bubbles before")
        for round in 1...rounds {
            backgroundAndForeground(app)
            let n = waitForBubbles(app)
            shot(app, "bgfg-only-r\(round)-after-\(n)")
            XCTAssertGreaterThan(n, 0, "round \(round): bubbles drawn after background → foreground")
        }
    }

    /// The small DM at its tail.
    func testDMScreenshotKeepsBubbles() {
        let app = launch(["ARBOS_OPEN_PEOPLE": Self.dm])
        for round in 1...rounds {
            cycle(app, chatId: Self.dm, shape: "dm", round: round, before: {}, between: {})
        }
    }

    // MARK: One cycle

    /// before → screenshot trigger → the feedback sheet is up → between → Cancel → bubbles?
    private func cycle(_ app: XCUIApplication, chatId: String, shape: String, round: Int,
                       before: () -> Void, between: () -> Void) {
        XCTAssertTrue(app.buttons["chat-back:\(chatId)"].waitForExistence(timeout: appear), "the chat opened")
        let countBefore = waitForBubbles(app)
        XCTAssertGreaterThan(countBefore, 0, "\(shape) r\(round): bubbles drawn before the screenshot")
        before()
        sleep(1)
        shot(app, "\(shape)-r\(round)-1-before-\(countBefore)")

        notify_post(Self.screenshotTrigger)
        let cancel = app.buttons["Cancel"]
        XCTAssertTrue(cancel.waitForExistence(timeout: settle), "\(shape) r\(round): the feedback sheet opened on the screenshot")
        sleep(1)
        shot(app, "\(shape)-r\(round)-2-sheet")
        between()
        if cancel.exists { cancel.tap() } else {
            XCTFail("\(shape) r\(round): the sheet's Cancel is gone after the lifecycle step")
        }
        XCTAssertTrue(app.buttons["chat-back:\(chatId)"].waitForExistence(timeout: settle), "\(shape) r\(round): back on the chat")
        // What the user sees first — before any scroll or tap that would make a lazy list recover.
        usleep(900_000)
        let countAfter = drawnBubbleCount(app)
        let probeAfter = probe(app)
        shot(app, "\(shape)-r\(round)-3-after-\(countAfter)")
        NSLog("[cempty] \(shape) r\(round): before=\(countBefore) after=\(countAfter) probe=\(probeAfter)")
        if countAfter == 0 {
            // Give layout a moment more, then judge; the first shot shows what the user saw.
            let late = waitForBubbles(app, timeout: 4)
            shot(app, "\(shape)-r\(round)-4-late-\(late)")
            XCTFail("\(shape) r\(round): the transcript came back EMPTY after the screenshot (late count \(late); \(probeAfter) → \(probe(app)))")
        }
        // Leave the keyboard down for the next round.
        if app.keyboards.firstMatch.exists {
            app.buttons["chat-back:\(chatId)"].coordinate(withNormalizedOffset: CGVector(dx: 6, dy: 4)).tap()
        }
        sleep(1)
    }

    // MARK: Helpers

    private func launch(_ env: [String: String]) -> XCUIApplication {
        let app = XCUIApplication()
        var merged = ["ARBOS_FIXTURE": "1", "ARBOS_FIXTURE_TYPING": "1"]
        for (k, v) in ProcessInfo.processInfo.environment where k.hasPrefix("ARBOS_") { merged[k] = v }
        for (k, v) in env { merged[k] = v }
        app.launchEnvironment = merged
        app.launch()
        return app
    }

    private func launchGroup() -> XCUIApplication {
        launch(["ARBOS_OPEN_PEOPLE": Self.group, "ARBOS_FIXTURE_LONG_ROWS": rows, "ARBOS_FIXTURE_LONG_RICH": "1"])
    }

    private func bubbleQuery(_ app: XCUIApplication) -> XCUIElementQuery {
        app.descendants(matching: .any).matching(NSPredicate(format: "identifier BEGINSWITH 'bubble:' OR identifier BEGINSWITH 'row:'"))
    }

    /// Bubbles with a real frame inside the transcript band (under the header, over the composer).
    private func drawnBubbleCount(_ app: XCUIApplication) -> Int {
        let window = app.windows.firstMatch.frame
        let band = CGRect(x: 0, y: 100, width: window.width, height: max(0, window.height - 100 - 160))
        var n = 0
        for el in bubbleQuery(app).allElementsBoundByIndex {
            let f = el.frame
            if f.width > 1, f.height > 1, f.intersects(band) { n += 1 }
        }
        return n
    }

    @discardableResult
    private func waitForBubbles(_ app: XCUIApplication, timeout: TimeInterval? = nil) -> Int {
        let deadline = Date().addingTimeInterval(timeout ?? settle)
        var n = drawnBubbleCount(app)
        while n == 0, Date() < deadline {
            usleep(300_000)
            n = drawnBubbleCount(app)
        }
        return n
    }

    private func raiseKeyboard(_ app: XCUIApplication) {
        let field = app.descendants(matching: .any)["composer-field"]
        XCTAssertTrue(field.waitForExistence(timeout: settle), "the composer field is there")
        field.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: settle), "the keyboard came up")
        sleep(1)
    }

    /// Finger moving down = older rows come in from the top (the reader leaves the tail).
    private func flickIntoHistory(_ app: XCUIApplication, flicks: Int) {
        let window = app.windows.firstMatch.frame
        let high = CGPoint(x: window.midX, y: 200)
        for _ in 0..<flicks {
            let start = app.coordinate(withNormalizedOffset: .zero).withOffset(CGVector(dx: high.x, dy: high.y))
            let end = start.withOffset(CGVector(dx: 0, dy: 300))
            start.press(forDuration: 0.02, thenDragTo: end, withVelocity: .fast, thenHoldForDuration: 0.0)
            usleep(500_000)
        }
        sleep(2)
    }

    /// Someone types for a moment, then stops (the fixture's presence row shows and hides).
    private func toggleTypingUnderTheSheet() {
        notify_post("com.unarbos.arbos.rig.typing-on")
        sleep(2)
        notify_post("com.unarbos.arbos.rig.typing-off")
        sleep(1)
        notify_post("com.unarbos.arbos.rig.typing-on")
        sleep(1)
    }

    /// The Debug probe's line (rows / shown / realised / pinned / window / epoch), for failure messages.
    private func probe(_ app: XCUIApplication) -> String {
        let el = app.descendants(matching: .any)["transcript-probe"]
        return el.exists ? (el.value as? String ?? "?") : "no probe"
    }

    private func backgroundAndForeground(_ app: XCUIApplication) {
        XCUIDevice.shared.press(.home)
        sleep(2)
        app.activate()
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: settle), "the app is back in the foreground")
        sleep(1)
    }

    private func shot(_ app: XCUIApplication, _ name: String) {
        guard let dir = ProcessInfo.processInfo.environment["ARBOS_SHOT_DIR"], !dir.isEmpty else { return }
        let png = XCUIScreen.main.screenshot().pngRepresentation
        let url = URL(fileURLWithPath: dir).appendingPathComponent("\(name).png")
        try? png.write(to: url)
    }
}
