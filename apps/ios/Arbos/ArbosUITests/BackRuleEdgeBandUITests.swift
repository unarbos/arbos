import XCTest

/// What a 50 pt rightward drag does in the chat and at the row where one was once observed to
/// pop, swept across its start x.
///
/// **This file replaces an earlier sweep whose conclusion was retracted.** That sweep ran the
/// same 50 pt drag across fourteen start points from x = 8 to x = 120 and nothing popped, ever
/// — gate 41772738320, head `fe02425d1`. The decisive point was **x = 8**: `startsAtEdge(8)` is
/// true, so `shouldStart` returns `false` in both of the app's hosts by construction, leaving
/// UIKit's `interactivePopGestureRecognizer` as the only candidate for that touch. The chat
/// stayed. That trial measures *completion*, not reach — whether UIKit's interactive pop
/// completes for 50 pt at 250 pt/s with a half-second hold killing the release velocity — and
/// completion is a deterministic threshold rather than a race, so one trial settles it. UIKit
/// will not complete this pop from 8 pt off the edge, so it will not at 40 either.
///
/// Three claims written in this repo rest on the opposite and none of them is measured:
///
/// - `BackSwipe.edgeZoneWidth`'s doc comment ("UIKit's own edge recognizer is narrower; the
///   band is wider") and the identical `// The edge zone is the system pop's from touch-down.`
///   above both `shouldStart` closures. **Unverified** — not contradicted either.
/// - `SwipeDirectionsUITests.stableFrame`'s rationale, that a start meant for x = 40 landing
///   inside the 24 pt zone is "where a 50 pt drag is a pop". **Contradicted**, by x = 8 under a
///   byte-identical gesture. `stableFrame` is still worth having — three frame reads are three
///   chances to disagree — but that is not why.
///
/// **So the mechanism of the original pop is unexplained, and this file does not assume one.**
///
/// **Why the earlier sweep could not have found it.** It pinned press duration, distance,
/// velocity and hold to the observed failure and then changed the chat *and* the y. It ran in
/// `fx-comms`, chosen because the signed-in user's rows render trailing-aligned so the leading
/// gutter is bare transcript — which is exactly what removes the two interactive things sitting
/// at x = 40 in the chat that popped: the sender avatar spanning 12–48 pt, and `g3`'s autolinked
/// URL. A control is not a reproduction.
///
/// **What this file does instead.** The observed failure's conditions, varying only x:
/// `fx-group-launch`, the `g3` row, that row's own `midY`, and the same drag —
/// `press(forDuration: 0.05, … dx: 50, withVelocity: .slow, thenHoldForDuration: 0.5)`. The
/// avatar lane is a **condition under test** rather than something to avoid, so the guard that
/// used to fail on it is now a label: each trial records whether its start point landed on a
/// sender avatar, and the table carries it.
///
/// **Candidates, none preferred.** The avatar is a SwiftUI `Button`, a `Button` shares the touch
/// with the back recognisers since #2530, and it is therefore the most plausible mechanism and
/// the one most likely to produce early confidence. It is on this list and no higher than the
/// rest: the original may have been a **cover** rather than a pop, since the test that observed
/// it read only `chat-back`'s absence; the delivered drag may have exceeded 50 pt; or it may be
/// intermittent below the rate this file can see.
///
/// **It never fails on a result.** iOS gates cut a public TestFlight build and land one at a
/// time, so a diagnostic that reds the chain delays releases. This file asserts only the things
/// that would make its own numbers meaningless — the chat opened, the row resolved, the frame
/// settled, the drag stayed on the row — and prints everything else. A pop is a finding to read
/// in the log, not a reason to block a merge.
final class BackRuleEdgeBandUITests: XCTestCase {
    /// The shared Mac runs the gate and releases at the same time; waits are generous.
    private let appear: TimeInterval = 90

    /// The chat the pop was observed in, and the row it was observed on.
    private static let group = "fx-group-launch"
    private static let row = "g3"

    /// The observed drag, unchanged in every respect.
    private static let distance: CGFloat = 50

    /// Start points, in screen points from the leading edge.
    ///
    /// 40 is the observed failure's own start and carries most of the trials. 16, 28 and 48 are
    /// inside the avatar lane (x ∈ [12, 48]) — 28 is also the lone datum from the recorded hand
    /// sweep that this investigation spent a long time arguing about. 80 is
    /// `SwipeDirectionsUITests.appOwnedStartX`, off the avatar, the control.
    private static let observed: CGFloat = 40
    private static let starts: [CGFloat] = [16, 28, observed, 48, 80]

    /// Trials at the observed start point, and at each of the others.
    ///
    /// Weighted rather than even, because the question is reproduction at one known point and
    /// not the shape of a boundary. Ten at x = 40 is worth stating as a bound: the original
    /// popped on two of three gates, and if a rate near that were real, ten trials would miss it
    /// with probability about 0.0015 %. Zero pops in ten would bound the rate below 25.9 % —
    /// which is the honest reading, rather than "it does not happen".
    private static let observedTrials = 10
    private static let otherTrials = 3

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func chatBack(_ app: XCUIApplication) -> XCUIElement {
        app.buttons["chat-back:\(Self.group)"]
    }

    /// Whether the chat is still the screen in front.
    ///
    /// A pop takes the chat off the stack. A pushed page — the sender's profile, say — covers it
    /// while leaving its elements queryable with plausible frames
    /// (`AvatarBackSwipeUITests`, `ChatSearchResultJumpUITests`), so the honest read of "in
    /// front" is whether the chat's own header control can be touched. Keeping these apart is
    /// the whole reason `covered` exists: a cover read as a pop is what sent this investigation
    /// down the wrong path.
    private func chatIsFrontmost(_ app: XCUIApplication) -> Bool {
        app.descendants(matching: .any)["chat-search-button"].firstMatch.isHittable
    }

    private func launchGroup() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1", "ARBOS_OPEN_PEOPLE": Self.group]
        app.launch()
        XCTAssertTrue(chatBack(app).waitForExistence(timeout: appear), "the Launch week group opened")
        // Without this control there is no way to tell a cover from a pop, and every reading
        // would silently become `covered`.
        XCTAssertTrue(app.descendants(matching: .any)["chat-search-button"].firstMatch.exists,
                      "the chat's header control is there to read frontmost-ness from")
        return app
    }

    // MARK: Resolving the row
    //
    // Deliberately self-contained rather than shared with `SwipeDirectionsUITests`: that file is
    // the swipe-direction PR's and editing it here would put a merge conflict in the one place
    // this investigation has already identified as a silent-drop hazard. This file is a
    // diagnostic and is meant to be deleted once the question closes.

    /// The element published as `bubble:<id>`, resolved to exactly one.
    ///
    /// A row that draws several accessibility elements publishes the identifier on each, and the
    /// plain subscript raises "Multiple matching elements found" as soon as anything reads a
    /// frame. Ask for the row's container, and let the count stop changing first.
    private func bubbleElement(_ app: XCUIApplication, _ id: String) -> XCUIElement {
        let containers = app.otherElements.matching(identifier: "bubble:\(id)")
        var count = containers.count
        for _ in 0..<20 {
            Thread.sleep(forTimeInterval: 0.1)
            let now = containers.count
            if now == count { break }
            count = now
        }
        if count == 1 { return containers.firstMatch }
        if count > 1 {
            let all = containers.allElementsBoundByIndex
            return all.max { $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height }
                ?? containers.firstMatch
        }
        let matches = app.descendants(matching: .any).matching(identifier: "bubble:\(id)")
        let all = matches.allElementsBoundByIndex
        guard all.count > 1 else { return matches.firstMatch }
        return all.max { $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height }
            ?? matches.firstMatch
    }

    /// Wait until a row has stopped moving, and fail rather than fall through.
    ///
    /// A swipe delivered into a still-decelerating scroll view belongs to the scroll view, so
    /// the drag under test never happens — and a settle that gives up quietly records that as a
    /// result. Loudly, not longer.
    private func settleRow(_ element: XCUIElement, timeout: TimeInterval = 4,
                           file: StaticString = #filePath, line: UInt = #line) {
        var previous = CGRect.null
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            let current = element.frame
            if !current.isEmpty, current == previous { return }
            previous = current
            Thread.sleep(forTimeInterval: 0.1)
        }
        XCTFail("\(element.identifier) was still moving after \(Int(timeout))s", file: file, line: line)
    }

    /// Scroll `bubble:<id>` clear of the header and the composer and return it, dragging by the
    /// offset that would centre it so the sign of the step points the drag the right way.
    private func revealRow(_ app: XCUIApplication, _ id: String,
                           file: StaticString = #filePath, line: UInt = #line) -> XCUIElement {
        let window = app.windows.firstMatch.frame
        var previous = CGRect.null
        for _ in 0..<12 {
            let bubble = bubbleElement(app, id)
            let frame = bubble.frame
            if bubble.exists, frame.width > 1, frame.height > 1, bubble.isHittable {
                let clear = frame.midY > window.height * 0.3 && frame.maxY < window.maxY - 160
                let stuck = frame == previous && frame.minY > window.minY
                    && frame.maxY < window.maxY - 110
                if clear || stuck {
                    settleRow(bubble, file: file, line: line)
                    return bubble
                }
                previous = frame
            }
            let reach = window.height * 0.4
            let step = max(-reach, min(reach, window.midY - frame.midY))
            let mid = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.45))
            mid.press(forDuration: 0.05, thenDragTo: mid.withOffset(CGVector(dx: 0, dy: step)),
                      withVelocity: .slow, thenHoldForDuration: 0.1)
            usleep(400_000)
        }
        XCTFail("bubble:\(id) never came into reach", file: file, line: line)
        return bubbleElement(app, id)
    }

    /// The row's frame, confirmed by two agreeing reads, so the arithmetic and the finger use
    /// the same geometry.
    private func stableFrame(of bubble: XCUIElement, timeout: TimeInterval = 4,
                             file: StaticString = #filePath, line: UInt = #line) -> CGRect {
        XCTAssertTrue(bubble.exists, "the row is still on screen to swipe", file: file, line: line)
        var previous = CGRect.null
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            let current = bubble.frame
            if !current.isEmpty, current == previous { return current }
            previous = current
            Thread.sleep(forTimeInterval: 0.1)
        }
        XCTFail("\(bubble.identifier) frame never settled for the drag", file: file, line: line)
        return previous
    }

    /// A point in screen space, pinned to the frame this test measured — deliberately not
    /// `XCUIElement.coordinate`, which re-resolves the element when the event is synthesized.
    private func screenPoint(_ app: XCUIApplication, x: CGFloat, y: CGFloat) -> XCUICoordinate {
        app.coordinate(withNormalizedOffset: .zero).withOffset(CGVector(dx: x, dy: y))
    }

    // MARK: The measurement

    private enum Outcome: String {
        /// The chat is gone from the stack.
        case popped
        /// The chat is still there and still in front: nobody acted.
        case stayed
        /// The chat is still on the stack but something is over it. Never a pop.
        case covered
    }

    /// Whether `point` is inside a sender avatar — recorded, not refused.
    ///
    /// This was a guard in the retracted version, and failing on it is what excluded the avatar
    /// from the measurement that most needed it.
    private func onSenderAvatar(_ app: XCUIApplication, _ point: CGPoint) -> Bool {
        let avatars = app.descendants(matching: .any).matching(identifier: "sender-avatar")
        return avatars.allElementsBoundByIndex.map(\.frame).contains { $0.contains(point) }
    }

    private struct Trial {
        let outcome: Outcome
        let onAvatar: Bool
    }

    private func trial(startX: CGFloat) -> Trial {
        let app = launchGroup()
        let bubble = revealRow(app, Self.row)
        let frame = stableFrame(of: bubble)
        let y = frame.midY
        let point = CGPoint(x: startX, y: y)
        let onAvatar = onSenderAvatar(app, point)
        XCTAssertLessThan(startX + Self.distance, frame.maxX,
                          "the whole \(Int(Self.distance)) pt drag stays on the row, so the drag "
                          + "measured is the one asked for and not a clamped one")
        let start = screenPoint(app, x: startX, y: y)
        start.press(forDuration: 0.05,
                    thenDragTo: start.withOffset(CGVector(dx: Self.distance, dy: 0)),
                    withVelocity: .slow, thenHoldForDuration: 0.5)
        sleep(1)
        let outcome: Outcome
        if !chatBack(app).exists {
            outcome = .popped
        } else {
            outcome = chatIsFrontmost(app) ? .stayed : .covered
        }
        print("short-right-swipe repro: x=\(Int(startX)) y=\(Int(y)) row=\(Self.row) "
              + "avatar=\(onAvatar) -> \(outcome.rawValue)")
        return Trial(outcome: outcome, onAvatar: onAvatar)
    }

    /// Sweep the start point in the chat and on the row where the pop was observed, and print
    /// what each trial did.
    ///
    /// Read the table with the asymmetry in mind. A `popped` is sound: no app path can pop
    /// 50 pt, since both pop sites are gated on `fullScreenPops` and its `absX > 70` guard.
    /// A run of `stayed` bounds a rate and never establishes absence — except at a start point
    /// inside `edgeZoneWidth`, where the app stands aside by construction and the trial becomes
    /// a test of UIKit's completion threshold instead of a sample of a race.
    func testWhatAFiftyPointRightSwipeDoesWhereOneWasObservedToPop() {
        var rows: [(x: CGFloat, pops: Int, covers: Int, avatar: Int, trials: Int)] = []
        for x in Self.starts {
            let count = x == Self.observed ? Self.observedTrials : Self.otherTrials
            var pops = 0, covers = 0, avatar = 0
            for _ in 1...count {
                let result = trial(startX: x)
                if result.outcome == .popped { pops += 1 }
                if result.outcome == .covered { covers += 1 }
                if result.onAvatar { avatar += 1 }
            }
            rows.append((x, pops, covers, avatar, count))
            print("short-right-swipe repro: x=\(Int(x)) popped \(pops)/\(count) "
                  + "covered \(covers)/\(count) on-avatar \(avatar)/\(count)")
        }

        let table = rows.map {
            "\(Int($0.x)):pop\($0.pops)/\($0.trials),cov\($0.covers),av\($0.avatar)"
        }.joined(separator: " ")
        print("short-right-swipe repro table: \(table)")

        let reproduced = rows.filter { $0.pops > 0 || $0.covers > 0 }.map { Int($0.x) }
        if reproduced.isEmpty {
            print("short-right-swipe repro: not reproduced at any start point. Every trial "
                  + "stayed, so each point's rate is bounded and none is shown to be zero.")
        } else {
            print("short-right-swipe repro: REPRODUCED at x = \(reproduced) — a 50 pt drag took "
                  + "the chat off the front, which BackSwipe.fullScreenPops cannot do. Read the "
                  + "per-trial lines above for popped versus covered, which are different bugs.")
        }
    }
}
