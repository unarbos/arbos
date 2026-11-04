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
/// ## What the coarse pass found, and what this pass is for (AF-859)
///
/// The coarse pass reproduced it: the chat left the front on 19 of 19 trials at x = 16, 28, 40
/// and 48, and on 0 of 3 at x = 80. Every one of those 19 also showed a `profile-` page, so it is
/// a **cover and not a pop** — the sender's profile opening and being read as the chat leaving.
///
/// Reading the source killed three candidates at no cost. `sender-avatar-slot` is `Color.clear`,
/// 1 pt tall and `accessibilityHidden`, with no action, and it only renders when
/// `row.showsAvatar` is false, which this row is not (`Fixture.swift:243` gives `g3` a different
/// sender from the message before it, so `Transcript.swift:818` puts a disc on its run). A hit
/// region wider than the disc is out: `PersonAvatar` is `frame(width: size, height: size)` under
/// a `.plain` `Button` with no `contentShape`. And a row-level tap is out: the only row-level
/// `toggleProfile` is inside an `OpenURLAction` (`ChatView.swift:1197`), needing a link or
/// mention-chip URL.
///
/// What the read turned up instead is a second door nobody had listed. Two views call the same
/// `openProfile(personTarget)`: the avatar `Button` in the column, and **`bubble-name`** inside
/// the bubble (`Transcript.swift:1701`). And every drag that opened a page *ended* on the bubble
/// — 16→66, 28→78, 40→90, 48→98 — while the control that opened nothing both started and ended
/// there. So "starts in the column" and "ends on the bubble" fit the coarse table equally well,
/// and nothing measured so far can separate them:
///
/// - **The avatar button** requires touch-down inside the disc, which `avatar=false` on 22 of 22
///   contradicts.
/// - **The name button** requires a `Button` to fire from a touch that began outside it, which
///   SwiftUI does not do.
///
/// Each survivor is contradicted by something, so one of the *readings* is wrong rather than one
/// of the mechanisms — and the suspect is nominated: `avatar=false` is a single
/// `frame.contains` against a disc whose vertical position was inferred from
/// `HStack(alignment: .bottom)` and never measured.
///
/// So this pass measures instead of inferring. It prints the frames, maps every touch to the row
/// it lands in, sweeps x in 4 pt steps across the column boundary, and adds the two controls that
/// can each reach one survivor: a drag from the measured centre of the disc, and a drag that ends
/// inside the measured `bubble-name`. It also carries a **positive control for the avatar
/// detector itself**, because `avatar=false` on 22 of 22 is otherwise indistinguishable from a
/// detector that cannot fire, and that reading is what contradicts one of the two survivors.
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

    /// Where the bubble body begins, from `Theme.memberAvatar`'s own comment: the disc spans
    /// 12–48 pt from the screen and the body starts at 52. Used only to keep the name control
    /// clear of the avatar column, never as a measurement.
    private static let bubbleBodyX: CGFloat = 52

    /// Wall clock to millisecond resolution, so a line here can be placed against anything else
    /// timestamped on the same machine — the device log included, if a run ever captures it.
    private static let stamp: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "HH:mm:ss.SSS"
        return f
    }()

    /// Start points, in screen points from the leading edge: 8 to 60 in 4 pt steps.
    ///
    /// The coarse pass has already run (16, 28, 40 and 48 took the chat off the front on 19 of 19
    /// trials; 80 did not on 3 of 3), so the open question is no longer reproduction but where
    /// the boundary sits. It brackets the measured avatar column, 12–48 pt, and the bubble body's
    /// leading edge at 52 — `Theme.memberAvatar`'s own documented geometry.
    private static let sweep: [CGFloat] = Array(stride(from: CGFloat(8), through: 60, by: 4))

    /// One trial per sweep point, and three per control.
    ///
    /// Even rather than weighted, and thin on purpose: the coarse pass was 19/19 and 0/3, so the
    /// effect is deterministic at this resolution and a boundary needs points rather than
    /// repeats. The controls carry three each because they are the trials the verdict rests on.
    /// A single `stayed` still only bounds a rate, and the boundary in the table is read as the
    /// step between a fired point and a neighbouring one that did not, never as proof of zero.
    private static let sweepTrials = 1
    private static let controlTrials = 3

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

    /// What a trial did, as far as this test can see.
    ///
    /// `popped` is read from `chat-back:<id>` being absent, and that is weaker than it sounds: the
    /// button is a navigation-bar item, so a push swaps it out along with the rest of the bar.
    /// Absence therefore covers both "the chat popped" and "a page was pushed whose cover marker
    /// this test failed to recognise". Only `covered`, which carries a positive marker for the
    /// page on top, is read from evidence of its own — so prefer it when both could apply, and
    /// treat `popped` as implicating the back rule only when no cover marker is present.
    private enum Outcome: String {
        /// No `chat-back` and no cover named itself. See the caveat above before reading this as
        /// a pop.
        case popped
        /// The chat is still there and still in front: nobody acted.
        case stayed
        /// Something named itself over the chat. Never a pop.
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

    /// Whether a person's page is on screen.
    ///
    /// A cover has to **name itself** rather than be inferred from the chat not being frontmost.
    /// The sender avatar is a `Button` wired to `toggleProfile`, so the cover this run is most
    /// likely to see is that page, and `profile-` prefixed identifiers are how
    /// `ChatSearchResultJumpUITests` already recognises one.
    private func profilePageShowing(_ app: XCUIApplication) -> Bool {
        app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier BEGINSWITH %@", "profile-"))
            .firstMatch.exists
    }

    // MARK: Geometry
    //
    // The sweep's x-axis was read against a disc whose vertical position was inferred from
    // `HStack(alignment: .bottom)` and never measured, and `Transcript.swift:818` puts the disc on
    // the *last* avatar-bearing row of a run's first message, which need not be this row. So the
    // frames are measured and printed, and each touch is mapped to the row it actually lands in.

    private struct RowBox {
        let id: String
        let frame: CGRect
    }

    private struct AvatarBox {
        let row: String
        let frame: CGRect
    }

    private struct Geometry {
        let bubble: CGRect
        let rows: [RowBox]
        let avatars: [AvatarBox]
        /// `bubble-name`, the other door to the same `openProfile(personTarget)`.
        let name: CGRect?
        /// The disc on the row under test, when the run put one there.
        let ownAvatar: CGRect?
    }

    private func box(_ r: CGRect) -> String {
        "(x \(Int(r.minX))–\(Int(r.maxX)), y \(Int(r.minY))–\(Int(r.maxY)))"
    }

    private func rowBoxes(_ app: XCUIApplication) -> [RowBox] {
        app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier BEGINSWITH %@", "bubble:"))
            .allElementsBoundByIndex
            .map { RowBox(id: $0.identifier, frame: $0.frame) }
            .filter { !$0.frame.isEmpty }
    }

    /// The row a point lands in: the smallest container holding it, since a row's elements nest.
    private func rowContaining(_ point: CGPoint, _ rows: [RowBox]) -> String {
        rows.filter { $0.frame.contains(point) }
            .min { $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height }?
            .id ?? "none"
    }

    private func measure(_ app: XCUIApplication, bubble: CGRect) -> Geometry {
        let rows = rowBoxes(app)
        let avatars = app.descendants(matching: .any).matching(identifier: "sender-avatar")
            .allElementsBoundByIndex
            .map(\.frame)
            .filter { !$0.isEmpty }
            .map { AvatarBox(row: rowContaining(CGPoint(x: $0.midX, y: $0.midY), rows), frame: $0) }
        let name = app.descendants(matching: .any).matching(identifier: "bubble-name")
            .allElementsBoundByIndex
            .map(\.frame)
            .first { !$0.isEmpty && bubble.intersects($0) }
        return Geometry(bubble: bubble, rows: rows, avatars: avatars, name: name,
                        ownAvatar: avatars.first { $0.row == "bubble:\(Self.row)" }?.frame)
    }

    private func printGeometry(_ g: Geometry, _ tag: String) {
        print("short-right-swipe geometry [\(tag)]: bubble:\(Self.row)=\(box(g.bubble)) "
              + "bubble-name=\(g.name.map { box($0) } ?? "none") "
              + "own-row-disc=\(g.ownAvatar.map { box($0) } ?? "none") discs=\(g.avatars.count)")
        for a in g.avatars {
            print("short-right-swipe geometry [\(tag)]: disc \(box(a.frame)) sits in row \(a.row)")
        }
    }

    /// The disc nearest `y`, for the detector control when the row under test carries none.
    private func nearestDisc(_ g: Geometry, to y: CGFloat) -> CGRect? {
        g.ownAvatar ?? g.avatars.map(\.frame).min { abs($0.midY - y) < abs($1.midY - y) }
    }

    // MARK: Aiming

    /// Where a trial puts its finger down. The three aims exist to separate the two surviving
    /// explanations for the profile page, which the earlier table could not tell apart because
    /// every one of its drags both started in the avatar column and ended on the bubble.
    private enum Aim {
        /// A start x at the row's vertical middle: the observed failure's own aim, and the sweep.
        case atRowMiddle(CGFloat)
        /// The measured centre of the disc — the only aim that puts touch-down inside the avatar
        /// `Button`, which is what the avatar explanation requires.
        case atDiscCentre
        /// Start on the bubble body so the drag *ends* inside `bubble-name`, which is what the
        /// name-button explanation requires. Nothing has tested it: `bubble-name` sits at the
        /// bubble's top and every earlier drag ran at the row's middle, so they missed it
        /// vertically however far right they went.
        case endingInTheName
    }

    private struct Trial {
        /// `nil` when the trial could not be aimed. Never scored as a result — a trial that did
        /// not happen is not a trial that found nothing.
        let outcome: Outcome?
        let onAvatar: Bool
    }

    private func trial(_ aim: Aim, _ label: String) -> Trial {
        let app = launchGroup()
        let bubble = revealRow(app, Self.row)
        let frame = stableFrame(of: bubble)
        let g = measure(app, bubble: frame)
        let point: CGPoint
        switch aim {
        case .atRowMiddle(let x):
            point = CGPoint(x: x, y: frame.midY)
        case .atDiscCentre:
            guard let disc = nearestDisc(g, to: frame.midY) else {
                printGeometry(g, label)
                print("short-right-swipe repro: [\(label)] no disc on screen to aim at — not a trial")
                return Trial(outcome: nil, onAvatar: false)
            }
            point = CGPoint(x: disc.midX, y: disc.midY)
        case .endingInTheName:
            guard let name = g.name else {
                printGeometry(g, label)
                print("short-right-swipe repro: [\(label)] no bubble-name in the row — not a trial")
                return Trial(outcome: nil, onAvatar: false)
            }
            // Clamped to the bubble body: a control that also started in the avatar column would
            // reproduce the ambiguity it exists to remove.
            point = CGPoint(x: max(Self.bubbleBodyX, name.midX - Self.distance), y: name.midY)
        }
        let onAvatar = onSenderAvatar(app, point)
        let rowAtPoint = rowContaining(point, g.rows)
        let endPoint = CGPoint(x: point.x + Self.distance, y: point.y)
        let endsInName = g.name.map { $0.contains(endPoint) } ?? false
        XCTAssertLessThan(point.x + Self.distance, frame.maxX,
                          "the whole \(Int(Self.distance)) pt drag stays on the row, so the drag "
                          + "measured is the one asked for and not a clamped one")
        if case .endingInTheName = aim {
            // Both conditions are what make this a control rather than another ambiguous trial:
            // it has to begin clear of the avatar column and actually finish inside the name.
            XCTAssertGreaterThanOrEqual(point.x, Self.bubbleBodyX,
                                        "the name control starts on the bubble body, not in the "
                                        + "avatar column it exists to rule out")
            XCTAssertTrue(endsInName,
                          "the name control's drag finishes inside bubble-name \(box(g.name ?? .zero)); "
                          + "a drag that misses it tests nothing and must not be read as the name "
                          + "button standing down")
        }
        let start = screenPoint(app, x: point.x, y: point.y)
        // Absolute timestamps either side of the drag. The runner's own `t = …s` lines are
        // relative to the test, and a reading taken 1 s after the press cannot be placed against
        // anything the app did without a shared clock.
        let pressedAt = Date()
        start.press(forDuration: 0.05,
                    thenDragTo: start.withOffset(CGVector(dx: Self.distance, dy: 0)),
                    withVelocity: .slow, thenHoldForDuration: 0.5)
        let releasedAt = Date()
        sleep(1)
        let backThere = chatBack(app).exists
        let frontmost = backThere && chatIsFrontmost(app)
        let profile = profilePageShowing(app)
        // The cover marker is asked first and on its own. A push swaps the navigation bar, so it
        // takes `chat-back` with it and satisfies the pop test too — ordering the pop test first
        // scored 19 covers as pops and re-opened a retracted theory.
        let outcome: Outcome = profile ? .covered
            : (!backThere ? .popped : (frontmost ? .stayed : .covered))
        let readAt = Date()
        // Every discriminator printed, so a cover, a pop and a profile page are read off the
        // line rather than worked out from it.
        print("short-right-swipe repro: [\(label)] x=\(Int(point.x)) y=\(Int(point.y)) "
              + "end-x=\(Int(endPoint.x)) row-at-point=\(rowAtPoint) "
              + "avatar=\(onAvatar) ends-in-name=\(endsInName) "
              + "chat-back=\(backThere) frontmost=\(frontmost) "
              + "profile=\(profile) -> \(outcome.rawValue) "
              + "| pressed=\(Self.stamp.string(from: pressedAt)) "
              + "released=\(Self.stamp.string(from: releasedAt)) "
              + "read=\(Self.stamp.string(from: readAt)) "
              + "drag=\(String(format: "%.3f", releasedAt.timeIntervalSince(pressedAt)))s")
        return Trial(outcome: outcome, onAvatar: onAvatar)
    }

    /// A positive control for the avatar detector, which is load-bearing and was never proven.
    ///
    /// `avatar=false` on 22 of 22 trials is indistinguishable from a detector that cannot fire,
    /// and it is the one reading that contradicts the avatar-button explanation — so it is exactly
    /// the shape this project files as "no instrument distinguishes *not happening* from *not
    /// observed*". The control removes the ambiguity: aim a plain tap at the measured centre of a
    /// disc, and the detector must say `true` while the tap opens the page.
    ///
    /// This one asserts, unlike the sweep. A detector that cannot fire does not weaken the
    /// sweep's x-axis, it voids it, and the file's rule is to assert the things that would make
    /// its own numbers meaningless.
    func testTheAvatarDetectorFiresAtTheMeasuredCentreOfADisc() {
        let app = launchGroup()
        let bubble = revealRow(app, Self.row)
        let g = measure(app, bubble: stableFrame(of: bubble))
        printGeometry(g, "detector-control")
        guard let disc = nearestDisc(g, to: g.bubble.midY) else {
            return XCTFail("a sender disc is on screen to aim the detector control at")
        }
        let centre = CGPoint(x: disc.midX, y: disc.midY)
        let onAvatar = onSenderAvatar(app, centre)
        print("short-right-swipe detector control: disc=\(box(disc)) "
              + "centre=(\(Int(centre.x)), \(Int(centre.y))) "
              + "row-at-point=\(rowContaining(centre, g.rows)) "
              + "on-row-under-test=\(g.ownAvatar != nil) avatar=\(onAvatar)")
        XCTAssertTrue(onAvatar,
                      "the detector fires at the centre of the disc it has just measured. If this "
                      + "fails, `avatar=false` in the sweep is a reading about the detector and "
                      + "not about the geometry, and the sweep's x-axis means nothing")

        screenPoint(app, x: centre.x, y: centre.y).tap()
        let page = app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier BEGINSWITH %@", "profile-")).firstMatch
        let appeared = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == true"),
                                                 object: page)
        XCTAssertEqual(XCTWaiter().wait(for: [appeared], timeout: 20), .completed,
                       "a plain tap at the disc's centre opens the sender's page, so the tap path "
                       + "and the cover marker both work and a silent profile is not the reason a "
                       + "trial reads as stayed")
    }

    /// Sweep the start point in the chat and on the row where the pop was observed, and print
    /// what each trial did.
    ///
    /// Read the table with two asymmetries in mind. A run of `stayed` bounds a rate and never
    /// establishes absence — except at a start point inside `edgeZoneWidth`, where the app stands
    /// aside by construction and the trial becomes a test of UIKit's completion threshold instead
    /// of a sample of a race. And a `popped` is the **weakest** reading here, not the soundest: it
    /// is the absence of a navigation-bar button, which a push also produces, so it means "no
    /// cover named itself" rather than "the chat popped". It was the sound reading only while the
    /// pop sites' `absX > 70` guard was thought to leave no other way for the chat to leave the
    /// front; a page pushed over it is exactly that other way, and it is what this run found.
    ///
    /// The fixture is part of the measurement rather than a backdrop: the mechanism lives in the
    /// sender-avatar column, which only a group draws, so the same sweep in a DM returns `stayed`
    /// at every start point and says nothing at all.
    func testWhatAFiftyPointRightSwipeDoesWhereOneWasObservedToPop() {
        // The geometry first, so the x-axis below is read against measured frames rather than
        // against a column position inferred from a layout alignment.
        let opening = launchGroup()
        let openingRow = revealRow(opening, Self.row)
        let g0 = measure(opening, bubble: stableFrame(of: openingRow))
        printGeometry(g0, "sweep")
        // `bubble-name`'s box against the drag end points. If it contains none of them, the
        // name-button explanation is refuted by arithmetic before a finger moves.
        let ends = Self.sweep.map { CGPoint(x: $0 + Self.distance, y: g0.bubble.midY) }
        let reachable = g0.name.map { nameBox in ends.filter { nameBox.contains($0) } } ?? []
        print("short-right-swipe geometry [sweep]: of \(ends.count) sweep end points at the row's "
              + "middle, \(reachable.count) land inside bubble-name — "
              + (g0.name == nil ? "no name box measured"
                 : (reachable.isEmpty
                    ? "so the name button cannot be what any row-middle drag reached"
                    : "so the name button is reachable from the row's middle")))

        var rows: [(x: CGFloat, fired: Int, covers: Int, avatar: Int, trials: Int)] = []
        for x in Self.sweep {
            var fired = 0, covers = 0, avatar = 0, ran = 0
            for _ in 1...Self.sweepTrials {
                let result = trial(.atRowMiddle(x), "sweep")
                guard let outcome = result.outcome else { continue }
                ran += 1
                if outcome != .stayed { fired += 1 }
                if outcome == .covered { covers += 1 }
                if result.onAvatar { avatar += 1 }
            }
            rows.append((x, fired, covers, avatar, ran))
            print("short-right-swipe repro: x=\(Int(x)) fired \(fired)/\(ran) "
                  + "covered \(covers)/\(ran) on-avatar \(avatar)/\(ran)")
        }

        let table = rows.map {
            "\(Int($0.x)):fired\($0.fired)/\($0.trials),cov\($0.covers),av\($0.avatar)"
        }.joined(separator: " ")
        print("short-right-swipe repro table: \(table)")
        let fired = rows.filter { $0.fired > 0 }.map { Int($0.x) }
        print("short-right-swipe repro: the drag took the chat off the front at x = \(fired), "
              + "and did not at x = \(rows.filter { $0.fired == 0 }.map { Int($0.x) })")

        // MARK: The two controls that separate the explanations

        let discControl = (1...Self.controlTrials).map { _ in trial(.atDiscCentre, "disc-centre") }
        let nameControl = (1...Self.controlTrials).map { _ in trial(.endingInTheName, "ends-in-name") }
        let discRan = discControl.compactMap(\.outcome)
        let nameRan = nameControl.compactMap(\.outcome)
        XCTAssertFalse(discRan.isEmpty, "the disc-centre control was aimed at least once; it is "
                       + "the control that can reach the avatar-button explanation at all")
        XCTAssertFalse(nameRan.isEmpty, "the ends-in-name control was aimed at least once; without "
                       + "it the name-button explanation stays untested rather than refuted")
        let discFired = discRan.filter { $0 != .stayed }.count
        let nameFired = nameRan.filter { $0 != .stayed }.count
        let discOnAvatar = discControl.filter(\.onAvatar).count
        print("short-right-swipe controls: disc-centre fired \(discFired)/\(discRan.count) "
              + "(detector said on-avatar \(discOnAvatar)/\(discControl.count)); "
              + "ends-in-name fired \(nameFired)/\(nameRan.count)")

        // A verdict, not a ranking. Each explanation has a control that can reach it, so each
        // one's own control decides it.
        switch (discFired > 0, nameFired > 0) {
        case (true, false):
            print("short-right-swipe verdict: the AVATAR BUTTON is the mechanism. Touch-down on "
                  + "the measured disc opens the page and a drag ending in bubble-name does not, "
                  + "so the x band in the table is the avatar column and the row-middle trials "
                  + "were landing on the button after all — read the printed disc frame against "
                  + "`avatar=false` to see where the earlier geometry reading went wrong.")
        case (false, true):
            print("short-right-swipe verdict: the NAME BUTTON is the mechanism. A drag ending in "
                  + "bubble-name opens the page and touch-down on the disc does not, so the x band "
                  + "in the table is where the drags ENDED and not where they began — the avatar "
                  + "column was a coincidence of a 50 pt drag starting left of the bubble.")
        case (true, true):
            print("short-right-swipe verdict: BOTH doors open the page, which is consistent with "
                  + "them sharing one openProfile(personTarget). The table alone cannot apportion "
                  + "the row-middle trials between them; the fix has to answer for both.")
        case (false, false):
            print("short-right-swipe verdict: NEITHER control fired while the sweep did, so the "
                  + "mechanism is a third thing and both surviving explanations are refuted. Read "
                  + "the row-at-point column: the sweep's touches are landing somewhere neither "
                  + "control aimed at.")
        }
    }
}
