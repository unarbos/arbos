import XCTest

/// The chat bar's height must not move the transcript scroll view's bottom inset
/// ([feedback empty-transcript-on-open-ios-2026-10-30], with the keyboard blackout and the
/// blank-while-typing reports: #2390).
///
/// **This guards the mechanism, not the symptom.** It proves that a change in the bar's
/// height leaves the scroll view's bottom inset where it was. It does **not** prove that an
/// agent chat draws its cached rows, and it cannot: the blank never reproduced in the
/// simulator, on `main` or on the tester build `7bbfaf425`. The row-drop is a device-only
/// behaviour of `LazyVStack` on an older iOS, so a simulator test that waited for bubbles to
/// vanish would pass against the bug. The symptom proof has to come from Affinity's and
/// Jacob's phones once TestFlight unblocks; that gap stays open.
///
/// What the two builds do differently, which is what makes this assertable at all: before
/// #2390 the bar was a bottom `safeAreaInset`, so every change of its height resized the
/// transcript's inset; after it the bar is an overlay and only the content height changes.
///
/// The baseline is read with the keyboard already up, on purpose. The keyboard is still a
/// safe-area change in both builds, so opening it moves the inset legitimately; only the
/// bar's own growth is under test. The growth is a long wrapping draft rather than a
/// long-press then Reply, because typing has no menu lookup to miss (the actions menu has
/// already been seen to go missing while on screen) and the field grows by several lines.
///
/// Needs the DEBUG inset probe from #2390. Until that lands the test skips loudly rather
/// than passing: a green run here would otherwise mean nothing, and nothing in the iOS gate
/// asserts a nonzero executed-test count until PR #2381 merges.
final class ChatBarInsetUITests: XCTestCase {
    /// The shared Mac runs the gate and releases at the same time; waits are generous.
    private let appear: TimeInterval = 90
    private let settle: TimeInterval = 20

    /// Comms, an agent chat (`Ask Comms` composer, not a people chat) with seeded rows.
    private let project = "fx-comms"
    private let chat = "fx-comms-main"

    /// Set by #2390. Values are plain numbers with one decimal, so sub-pixel jitter does not
    /// read as a change and both parse as `Double`.
    private let insetProbe = "chat-transcript-bottom-inset"
    private let barProbe = "chat-bar-height"

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func launch() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = [
            "ARBOS_FIXTURE": "1",
            "ARBOS_NO_STORE": "1",
            "ARBOS_OPEN_PROJECT": project,
            "ARBOS_OPEN_CHAT": chat,
        ]
        app.launch()
        XCTAssertTrue(app.buttons["chat-back:\(chat)"].waitForExistence(timeout: appear), "the agent chat opened")
        return app
    }

    private func field(_ app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any)["composer-field"]
    }

    private func probe(_ app: XCUIApplication, _ id: String) -> Double? {
        let element = app.descendants(matching: .any)[id]
        guard element.exists, let raw = element.value as? String else { return nil }
        return Double(raw)
    }

    private func read(_ app: XCUIApplication) -> (inset: Double, bar: Double)? {
        guard let inset = probe(app, insetProbe), let bar = probe(app, barProbe) else { return nil }
        return (inset, bar)
    }

    /// Waits until two reads a beat apart agree, then returns that pair. The transcript
    /// springs back to the bottom after a layout change, so a single read can land mid-flight.
    private func settled(_ app: XCUIApplication) -> (inset: Double, bar: Double)? {
        guard var last = read(app) else { return nil }
        for _ in 0..<20 {
            Thread.sleep(forTimeInterval: 0.4)
            guard let now = read(app) else { return nil }
            if abs(now.inset - last.inset) < 0.5, abs(now.bar - last.bar) < 0.5 { return now }
            last = now
        }
        return last
    }

    private func shot(_ name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = "chat-bar-inset-\(name)"
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    func testGrowingTheChatBarLeavesTheTranscriptInsetWhereItWas() throws {
        let app = launch()
        try XCTSkipIf(probe(app, insetProbe) == nil,
                      "needs the DEBUG inset probe '\(insetProbe)' from #2390; skipping rather than passing on nothing")
        try XCTSkipIf(probe(app, barProbe) == nil,
                      "needs the DEBUG bar-height probe '\(barProbe)' from #2390; without it an unchanged inset is vacuous")

        // Keyboard first, and let it settle: the keyboard is a safe-area change in both
        // builds, so it must not be part of what this test measures.
        field(app).tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: settle), "the keyboard is up")
        let before = try XCTUnwrap(settled(app), "the probes read before the bar grows")
        shot("1-one-line-bar")

        // A draft long enough to wrap the field over several lines (it grows to eight).
        app.typeText(String(repeating: "the bar has to get taller than one line ", count: 8))

        // The control: if the bar did not actually grow, an unchanged inset proves nothing.
        var grew = false
        for _ in 0..<Int(settle / 0.4) where !grew {
            Thread.sleep(forTimeInterval: 0.4)
            grew = (probe(app, barProbe) ?? 0) > before.bar + 1
        }
        XCTAssertTrue(grew, "the typed draft grew the chat bar (was \(before.bar) pt)")

        let after = try XCTUnwrap(settled(app), "the probes read after the bar grew")
        shot("2-multi-line-bar")
        XCTAssertGreaterThan(after.bar, before.bar + 1, "premise: the bar is taller than it was")
        XCTAssertEqual(after.inset, before.inset, accuracy: 0.5,
                       "the bar grew by \(after.bar - before.bar) pt and the transcript's bottom inset "
                       + "moved from \(before.inset) to \(after.inset); it must not move at all")
    }
}
