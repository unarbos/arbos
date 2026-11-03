import XCTest

/// The chat follows the finger during a back swipe, and does not wait for it to be lifted
/// (Jacob, 2026-11-03: "Sometimes i swipe and then it doesn't register until i let go. But
/// this is not the experience we want… Should register on first touch and then as i slide my
/// finger across the chat slides out with my finger").
///
/// **Why the app is asked rather than the screen read.** XCUITest's drag call does not return
/// until the touch is lifted, so a test cannot look at the screen while the finger is down —
/// and once it is up, a chat that tracked the whole way and a chat that jumped at touch-up
/// look exactly the same. So the app records what it did, sample by sample, on the
/// `back-swipe-trace` element (`ARBOS_SWIPE_TRACE=1`, Debug only) and the cases read that
/// afterwards: `start`, then `track:<fraction of the screen>` for every further 1% the chat
/// moved while the finger was down, then `finish` or `cancel` for the release.
///
/// Each case drags **short of the halfway line and lets go slowly**, so the swipe cancels and
/// the chat is still there to be read. That is the whole point: a drag that goes back proves
/// nothing about when it started moving, while a drag that comes back has to have moved first.
/// On the rule this replaces — one `fullScreenPops` reading taken at touch-up — these drags
/// produced no movement at any point, so every case here is red on `main`.
///
/// Separate from `BackSwipeUITests` and `AvatarBackSwipeUITests`: both are being edited in
/// other lanes (#2386).
final class ChatSwipeTrackingUITests: XCTestCase {
    /// The shared Mac runs the gate and releases at the same time; waits are generous.
    private let appear: TimeInterval = 90
    private let settle: TimeInterval = 20

    private let chatId = "fx-group-launch"

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func openGroup() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1", "ARBOS_OPEN_PEOPLE": chatId,
                                 "ARBOS_FIXTURE_LONG_ROWS": "120", "ARBOS_SWIPE_TRACE": "1"]
        app.launch()
        XCTAssertTrue(back(app).waitForExistence(timeout: appear), "the Launch week group opened")
        settleTranscript(app)
        return app
    }

    private func back(_ app: XCUIApplication) -> XCUIElement {
        app.buttons["chat-back:\(chatId)"]
    }

    private var bubbleQuery: NSPredicate { NSPredicate(format: "identifier BEGINSWITH 'bubble:'") }

    /// Wait until the transcript has stopped moving: a chat opens by scrolling to its newest
    /// message, and a frame read mid-scroll says where a row was, not where the finger lands.
    private func settleTranscript(_ app: XCUIApplication, timeout: TimeInterval = 8) {
        let bubbles = app.descendants(matching: .any).matching(bubbleQuery)
        var previous = CGRect.null
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            let current = bubbles.firstMatch.frame
            if !current.isEmpty, current == previous { return }
            previous = current
            Thread.sleep(forTimeInterval: 0.25)
        }
    }

    /// A point on the transcript, clear of the 24 pt edge zone (the system pop's, which is
    /// interactive already and is not what these cases are about), of the header and of the
    /// composer bar, with room for a `distance` pt drag before the screen edge.
    private func startPoint(_ app: XCUIApplication, distance: CGFloat,
                            file: StaticString = #filePath, line: UInt = #line) -> XCUICoordinate {
        let bubbles = app.descendants(matching: .any).matching(bubbleQuery)
        XCTAssertTrue(bubbles.firstMatch.waitForExistence(timeout: settle), "the transcript shows bubbles",
                      file: file, line: line)
        let window = app.windows.firstMatch.frame
        let x = max(60, window.midX - distance)
        XCTAssertGreaterThan(x, 24, "the drag starts outside the edge zone", file: file, line: line)
        XCTAssertLessThan(x + distance, window.maxX - 8,
                          "a \(Int(distance)) pt drag from \(Int(x)) would be clamped at the screen edge and never "
                          + "reach the app", file: file, line: line)
        return app.coordinate(withNormalizedOffset: .zero).withOffset(CGVector(dx: x, dy: window.midY))
    }

    /// What the chat did, in order. Entries are `start`, `track:<0…1>` and `finish` / `cancel`.
    private func trace(_ app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line) -> [String] {
        let readout = app.descendants(matching: .any)["back-swipe-trace"].firstMatch
        XCTAssertTrue(readout.waitForExistence(timeout: settle),
                      "the rig readout is on screen (ARBOS_SWIPE_TRACE=1)", file: file, line: line)
        return ((readout.value as? String) ?? "").split(separator: " ").map(String.init)
    }

    /// How far out the chat was at each sample taken while the finger was down.
    private func travel(_ entries: [String]) -> [Double] {
        entries.compactMap { $0.hasPrefix("track:") ? Double($0.dropFirst("track:".count)) : nil }
    }

    /// Every case asks the same three things of a trace: the chat moved while the finger was
    /// down, it kept moving as the finger did, and the release is the last word and not the
    /// first one.
    private func assertTrackedUnderTheFinger(_ entries: [String], endingWith ending: String,
                                             file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(entries.first, "start", "the drag was read: \(entries)", file: file, line: line)
        XCTAssertEqual(entries.last, ending, "the release is the last entry: \(entries)", file: file, line: line)

        let moves = travel(entries)
        XCTAssertGreaterThanOrEqual(moves.count, 2,
                                    "the chat moved more than once before the touch was lifted: \(entries)",
                                    file: file, line: line)
        XCTAssertGreaterThan(moves.first ?? 0, 0,
                             "the chat was already out from the first sample — this is the "
                             + "difference between tracking the finger and resolving on release: \(entries)",
                             file: file, line: line)
        XCTAssertGreaterThan(moves.last ?? 0, moves.first ?? 0,
                             "the chat kept up with the finger across the drag: \(entries)",
                             file: file, line: line)
        let between = Array(entries.dropFirst().dropLast()).filter { !$0.hasPrefix("track:") }
        XCTAssertEqual(between, [], "nothing but movement happened while the finger was down: \(entries)",
                       file: file, line: line)
    }

    // MARK: The report

    /// A slow drag, the case Jacob called finicky. 120 pt on a 393 pt screen is short of the
    /// halfway line and the release is gentle, so the chat comes back — after having followed
    /// the finger the whole way out.
    func testASlowDragMovesTheChatBeforeTheFingerIsLifted() {
        let app = openGroup()
        let distance: CGFloat = 120
        let start = startPoint(app, distance: distance)

        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: distance, dy: 0)),
                    withVelocity: .slow, thenHoldForDuration: 0.6)

        assertTrackedUnderTheFinger(trace(app), endingWith: "cancel")
        XCTAssertTrue(back(app).exists, "the chat is still here: a drag this short does not go back")
        XCTAssertTrue(back(app).isHittable, "and it is usable again")
    }

    /// The same, started on the diagonal — a thumb reaching across the phone goes down and
    /// out at once. The chat tracks the sideways part from the first sample.
    func testADiagonalDragMovesTheChatBeforeTheFingerIsLifted() {
        let app = openGroup()
        let distance: CGFloat = 120
        let start = startPoint(app, distance: distance)

        // About 30° below the horizontal: a diagonal, and inside the cone the swipe claims on.
        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: distance, dy: distance * 0.55)),
                    withVelocity: .slow, thenHoldForDuration: 0.6)

        assertTrackedUnderTheFinger(trace(app), endingWith: "cancel")
        XCTAssertTrue(back(app).exists, "the chat is still here")
    }

    // MARK: What following the finger must not cost

    /// The pop is now UIKit's: the screen comes off the navigation stack through
    /// `popViewController`, where it used to come off through SwiftUI's `dismiss()`. So the
    /// thing to hold is that `NavigationStack`'s `path` came off with it — a stack that
    /// dropped a screen SwiftUI still believes is pushed leaves a Home that is drawn and
    /// dead. Going back and then opening the same chat again is the shortest way to say so.
    func testGoingBackLeavesAHomeThatStillOpensChats() {
        let app = openGroup()
        let window = app.windows.firstMatch.frame
        let distance = window.width * 0.6
        let start = app.coordinate(withNormalizedOffset: .zero).withOffset(CGVector(dx: 40, dy: window.midY))
        XCTAssertLessThan(40 + distance, window.maxX - 8, "the drag stays on screen")

        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: distance, dy: 0)),
                    withVelocity: .fast, thenHoldForDuration: 0.05)

        let gone = NSPredicate(format: "exists == false")
        let left = XCTNSPredicateExpectation(predicate: gone, object: back(app))
        XCTAssertEqual(XCTWaiter().wait(for: [left], timeout: settle), .completed, "the chat went back")
        XCTAssertTrue(app.buttons["Agents"].waitForExistence(timeout: settle), "Home is underneath")

        // Home opens on the All tab, which lists the group (`HomeFilterTabsUITests`).
        let row = app.descendants(matching: .any)["home-row:\(chatId)"].firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: settle), "the group is in the list again")
        row.tap()
        XCTAssertTrue(back(app).waitForExistence(timeout: settle),
                      "Home pushed the chat back on: the stack and SwiftUI's path agree about what is on it")
    }
}
