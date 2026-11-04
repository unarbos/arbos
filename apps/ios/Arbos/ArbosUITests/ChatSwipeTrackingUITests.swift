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
/// moved while the finger was down, then `finish` or `cancel` for the release — or `taken`,
/// if the touch was pulled out from under the drag rather than let go.
///
/// The two cancelling cases drag **short of the line that commits** (`comesBack`), so the chat
/// is still there to be read afterwards. That is the whole point: a drag that goes back proves
/// nothing about when it started moving, while a drag that comes back has to have moved first.
/// The committing case reads the same trace after the chat has gone, since the readout is on
/// the window rather than on the screen that leaves.
/// On the rule this replaces — one `fullScreenPops` reading taken at touch-up — these drags
/// produced no movement at any point, so every case here is red on `main`.
///
/// Separate from `BackSwipeUITests` and `AvatarBackSwipeUITests`: both are being edited in
/// other lanes (#2386).
///
/// **Hittability after the transition is deliberately not asserted here, and re-adding it will
/// go red for a reason this fix does not own.** It is not the frames: those were a real defect
/// of this animator (AF-834, the last case below), and they are fixed. It is the toast layer —
/// an empty toast container publishes a full-screen accessibility element over every row, so
/// XCUITest computes no hit point for anything underneath it while UIKit delivers touches
/// normally. Filed against `ErrorNote.swift`; `main` works around it in two places by tapping
/// by coordinate, as the last two cases here do. See
/// `internal/2637-back-swipe-tracks-the-finger.md`.
final class ChatSwipeTrackingUITests: XCTestCase {
    /// The shared Mac runs the gate and releases at the same time; waits are generous.
    private let appear: TimeInterval = 90
    private let settle: TimeInterval = 20

    private let chatId = "fx-group-launch"

    /// A drag long enough to see the chat move and short enough that letting go puts it back.
    ///
    /// 70 pt is the ceiling, and the reason is a trap rather than a preference
    /// (`internal/ios-profile-push-media-rig/gesture-velocity-traps.md`). These cases end with
    /// `thenHoldForDuration`, which stops the finger before it lifts, so the release carries no
    /// velocity at all. `BackSwipe.fullScreenPops` then reads `|vx| >= |vy|` as `0 >= 0` — true —
    /// and goes back on distance alone, for any horizontal drag over 70 pt. Under 70 pt it
    /// cannot, so the chat is still on screen to be read, which is the whole method here.
    ///
    /// 60 pt is still 15 % of a 393 pt screen: about fifteen entries in the trace, far more than
    /// the two the assertion needs.
    private static let comesBack: CGFloat = 60

    /// How a drag ends. Anything the trace holds after one of these is the transition
    /// reporting on itself, not part of the drag.
    private static let verdicts: Set<String> = ["finish", "cancel", "taken"]

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

    /// What the chat did, in order. Entries are `start`, `track:<0…1>`, and one of `finish`,
    /// `cancel` or `taken`.
    private func trace(_ app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line) -> [String] {
        let readout = app.descendants(matching: .any)["back-swipe-trace"].firstMatch
        XCTAssertTrue(readout.waitForExistence(timeout: settle),
                      "the rig readout is on screen (ARBOS_SWIPE_TRACE=1)", file: file, line: line)
        return ((readout.value as? String) ?? "").split(separator: " ").map(String.init)
    }

    /// Where a `distance` pt drag leaves the chat if it follows the finger 1:1 — the pop
    /// travels one screen, so the fraction is the drag over the screen's width.
    private func oneToOne(_ app: XCUIApplication, _ distance: CGFloat) -> Double {
        Double(distance / app.windows.firstMatch.frame.width)
    }

    /// How far out the chat was at each sample taken while the finger was down.
    private func travel(_ entries: [String]) -> [Double] {
        entries.compactMap { $0.hasPrefix("track:") ? Double(String($0.dropFirst("track:".count))) : nil }
    }

    /// Every case asks the same four things of a trace: the chat moved while the finger was
    /// down, it kept moving as the finger did, it ended up **where the finger was** rather
    /// than merely somewhere, and the release is the last word and not the first one.
    ///
    /// `reaching` is the one that earns its place. "The chat moved" is satisfied by a single
    /// frame of movement, and a single frame is exactly what the first version of this fix
    /// produced: it tracked one sample, then measured the finger inside the view the
    /// transition had just moved, read the drag as leftward and sat at zero for the rest of
    /// the touch. The trace still said the chat had moved. Asserting where it came to rest is
    /// what tells 1:1 tracking from one frame of it.
    private func assertTrackedUnderTheFinger(_ entries: [String], endingWith ending: String,
                                             reaching: Double? = nil,
                                             file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(entries.first, "start", "the drag was read: \(entries)", file: file, line: line)
        // The drag is the part up to the verdict. What follows it is the transition's own
        // report of how it ended, which these cases do not read.
        let verdict = entries.firstIndex { Self.verdicts.contains($0) }
        XCTAssertEqual(verdict.map { entries[$0] }, ending,
                       "the release is the last thing the drag says: \(entries)", file: file, line: line)

        let drag = verdict.map { Array(entries[..<$0]) } ?? entries
        let moves = travel(drag)
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
        if let reaching {
            // 0.05 of a 393 pt screen is 20 pt: wide enough for the last sample before the
            // hold and the trace's own 1 % rounding, far tighter than any collapse.
            XCTAssertEqual(moves.last ?? 0, reaching, accuracy: 0.05,
                           "the chat came to rest under the finger and not short of it: \(entries)",
                           file: file, line: line)
        }
        let between = drag.dropFirst().filter { !$0.hasPrefix("track:") }
        XCTAssertEqual(Array(between), [],
                       "nothing but movement happened while the finger was down: \(entries)",
                       file: file, line: line)
    }

    // MARK: The report

    /// A slow drag, the case Jacob called finicky. It comes back — after having followed the
    /// finger the whole way out.
    func testASlowDragMovesTheChatBeforeTheFingerIsLifted() {
        let app = openGroup()
        let distance = Self.comesBack
        let start = startPoint(app, distance: distance)

        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: distance, dy: 0)),
                    withVelocity: .slow, thenHoldForDuration: 0.6)

        assertTrackedUnderTheFinger(trace(app), endingWith: "cancel", reaching: oneToOne(app, distance))
        XCTAssertTrue(back(app).exists, "the chat is still here: a drag this short does not go back")
    }

    /// The same, started on the diagonal — a thumb reaching across the phone goes down and
    /// out at once. The chat tracks the sideways part from the first sample.
    func testADiagonalDragMovesTheChatBeforeTheFingerIsLifted() {
        let app = openGroup()
        let distance = Self.comesBack
        let start = startPoint(app, distance: distance)

        // About 30° below the horizontal: a diagonal, and inside the cone the swipe claims on.
        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: distance, dy: distance * 0.55)),
                    withVelocity: .slow, thenHoldForDuration: 0.6)

        // The sideways part only: the chat does not follow the finger down the screen.
        assertTrackedUnderTheFinger(trace(app), endingWith: "cancel", reaching: oneToOne(app, distance))
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

        // The drag that commits tracks the finger as well, and this is the only case that can
        // say so: the two above stop short of the line that pops, so neither of them exercises
        // a transition that runs to the end. The readout lives on the window rather than on the
        // chat, so it is still here to be read after the chat has gone.
        assertTrackedUnderTheFinger(trace(app), endingWith: "finish", reaching: oneToOne(app, distance))

        // Home opens on the All tab, which lists the group (`HomeFilterTabsUITests`).
        let row = app.descendants(matching: .any)["home-row:\(chatId)"].firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: settle), "the group is in the list again")
        // By coordinate, as `TranscriptFirstFrameUITests` already does, because the empty toast
        // container stays in the accessibility tree spanning the whole content area, so XCUITest
        // computes no hit point for any row while UIKit delivers the touch normally (filed
        // against `ErrorNote.swift`; measured with no pop at all, so it is not this fix's). The
        // old `row.tap()` here never synthesised an event, so it asserted nothing — the open
        // below is what says the touch landed.
        row.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        XCTAssertTrue(back(app).waitForExistence(timeout: settle),
                      "Home pushed the chat back on: the stack and SwiftUI's path agree about what is on it")
    }

    /// After a drag that comes back, the transcript's accessibility frames are where its views
    /// are (AF-834). Before the fix every bubble came back reported one screen width to the
    /// right of where it was drawn, so VoiceOver read the open chat as off-screen.
    ///
    /// **This case must not read the trace, which is why it cannot also assert tracking.**
    /// Reading the readout heals the defect asserted here — its `accessibilityValue` getter
    /// walks the window, and that walk is enough to make SwiftUI republish. One build measured
    /// both ways read 414 with `ARBOS_SWIPE_TRACE` unset and 12 with it set, and four earlier
    /// measurements taken through the trace all reported "fixed" when nothing was. So this drag
    /// is driven blind, and the two cases above own tracking — which they can, because the
    /// healing moves the frames and not what the trace records.
    func testTheTranscriptIsWhereItIsDrawnAfterTheChatComesBack() {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1", "ARBOS_OPEN_PEOPLE": chatId,
                                 "ARBOS_FIXTURE_LONG_ROWS": "120"]
        app.launch()
        XCTAssertTrue(back(app).waitForExistence(timeout: appear), "the Launch week group opened")
        settleTranscript(app)

        let distance = Self.comesBack
        let start = startPoint(app, distance: distance)
        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: distance, dy: 0)),
                    withVelocity: .slow, thenHoldForDuration: 0.6)
        XCTAssertTrue(back(app).exists, "the chat is still here: a drag this short does not go back")

        // One read after a fixed wait, rather than polling until it passes. The defect is a
        // stale publication, so a poll would turn "the frames were wrong for two seconds and
        // VoiceOver read the chat as gone" into a pass.
        Thread.sleep(forTimeInterval: 1.5)

        let window = app.windows.firstMatch.frame
        let frames = app.descendants(matching: .any).matching(bubbleQuery).allElementsBoundByIndex.map(\.frame)
        XCTAssertFalse(frames.isEmpty, "there are bubbles to read")
        let worst = frames.map(\.minX).max() ?? 0
        XCTAssertLessThan(worst, window.midX,
                          "every bubble is reported inside the window it is drawn in — the "
                          + "furthest out is at \(Int(worst)) of a \(Int(window.width)) pt "
                          + "screen: \(frames.map { Int($0.minX) })")
    }
}
