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
/// The probes come from #2390, now on main: invisible accessibility elements reading
/// SwiftUI's `ScrollGeometry.contentInsets.bottom` through `onScrollGeometryChange` (so the
/// safe area is included — keyboard down reads about 34, not 0) and the bar view's own
/// measured height. Both emit one-decimal strings, and `-1.0` until the first geometry
/// sample. A reading near zero means the probe is not measuring the safe area; treat the
/// run as void rather than as a pass.
final class ChatBarInsetUITests: XCTestCase {
    /// The shared Mac runs the gate and releases at the same time; waits are generous.
    private let appear: TimeInterval = 90
    private let settle: TimeInterval = 20

    /// Comms, an agent chat (`Ask Comms` composer, not a people chat) with seeded rows.
    private let project = "fx-comms"
    private let chat = "fx-comms-main"

    private let insetId = "transcript-bottom-inset"
    private let insetLabel = "Transcript bottom inset"
    private let barId = "chat-bar-height"
    private let barLabel = "Chat bar height"

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

    /// Identifier first, then accessibility label. The two are independent keys, so renaming
    /// one of them is a loud failure naming both rather than a test that skips for ever —
    /// a permanent skip is the same defect as a vacuous pass, only quieter.
    private func element(_ app: XCUIApplication, id: String, label: String) -> XCUIElement? {
        let byId = app.descendants(matching: .any)[id]
        if byId.exists { return byId }
        let byLabel = app.descendants(matching: .any).matching(NSPredicate(format: "label == %@", label)).firstMatch
        return byLabel.exists ? byLabel : nil
    }

    /// Why a probe can go missing, now that it is never because #2390 has not landed. The
    /// iOS 18 floor is real: the inset probe is built on `onScrollGeometryChange`.
    private func probeMissing(_ id: String, _ label: String) -> String {
        "no probe found by identifier '\(id)' or label '\(label)'. #2390's probes are on main, "
        + "so this is a renamed key, a dropped pbxproj registration, or a simulator older "
        + "than iOS 18 — not a reason to skip."
    }

    private func insetElement(_ app: XCUIApplication) -> XCUIElement? {
        element(app, id: insetId, label: insetLabel)
    }

    private func barElement(_ app: XCUIApplication) -> XCUIElement? {
        element(app, id: barId, label: barLabel)
    }

    /// nil when the probe is absent, unparseable, or still reporting its `-1.0` sentinel —
    /// a not-yet-sampled reading must never become a baseline.
    private func value(_ probe: XCUIElement?) -> Double? {
        guard let probe, probe.exists, let raw = probe.value as? String,
              let number = Double(raw), number >= 0 else { return nil }
        return number
    }

    private func read(_ app: XCUIApplication) -> (inset: Double, bar: Double)? {
        guard let inset = value(insetElement(app)), let bar = value(barElement(app)) else { return nil }
        return (inset, bar)
    }

    /// Waits for a first sampled reading, then until two reads a beat apart agree. The
    /// transcript springs back to the bottom after a layout change, so one read can land
    /// mid-flight, and both probes start at `-1.0`.
    private func settled(_ app: XCUIApplication) -> (inset: Double, bar: Double)? {
        var last: (inset: Double, bar: Double)?
        for _ in 0..<Int(settle / 0.4) {
            Thread.sleep(forTimeInterval: 0.4)
            guard let now = read(app) else { continue }
            if let last, abs(now.inset - last.inset) < 0.5, abs(now.bar - last.bar) < 0.5 { return now }
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

        // #2390 is on main, so both probes are in every Debug build this runs against and a
        // probe it cannot find is never a reason to skip.
        XCTAssertNotNil(insetElement(app), probeMissing(insetId, insetLabel))
        XCTAssertNotNil(barElement(app), probeMissing(barId, barLabel))

        // Keyboard first, and let it settle: the keyboard is a safe-area change in both
        // builds, so it must not be part of what this test measures.
        field(app).tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: settle), "the keyboard is up")
        let before = try XCTUnwrap(settled(app), "both probes reported a sampled reading before the bar grows")
        shot("1-one-line-bar")

        // A draft long enough to wrap the field over several lines (it grows to eight).
        app.typeText(String(repeating: "the bar has to get taller than one line ", count: 8))

        // The control: if the bar did not actually grow, an unchanged inset proves nothing.
        var grew = false
        for _ in 0..<Int(settle / 0.4) where !grew {
            Thread.sleep(forTimeInterval: 0.4)
            grew = (value(barElement(app)) ?? 0) > before.bar + 1
        }
        XCTAssertTrue(grew, "the typed draft grew the chat bar (was \(before.bar) pt)")

        let after = try XCTUnwrap(settled(app), "both probes reported a sampled reading after the bar grew")
        shot("2-multi-line-bar")
        XCTAssertGreaterThan(after.bar, before.bar + 1, "premise: the bar is taller than it was")
        XCTAssertEqual(after.inset, before.inset, accuracy: 0.5,
                       "the bar grew by \(after.bar - before.bar) pt and the transcript's bottom inset "
                       + "moved from \(before.inset) to \(after.inset); it must not move at all")
    }
}
