import XCTest

/// A short horizontal drag that starts on a sender's avatar must not open their profile page
/// (AF-859).
///
/// `AvatarBackSwipeUITests` holds the long drag and the plain tap: 180 pt from an avatar goes
/// back, a tap opens the page, a 200 pt vertical drag scrolls and opens nothing. The gap between
/// those was the short drag, and it was the reported defect — a drag that is too short to pop and
/// too long to be a tap took the reader to someone's profile instead of doing nothing.
///
/// Measured on gate [41812227199](https://github.com/unarbos/arbos/actions/runs/41812227199),
/// on the tree this file guards the fix for. A 50 pt rightward drag at the row's vertical middle
/// opened the page from **11 of 11** start positions between x = 8 and x = 48, and from **0 of 3**
/// at x = 52, 56 and 60 — the avatar column against the bubble body, to the point
/// (`Theme.memberAvatar` documents the disc at 12–48 and the body's leading edge at 52, and
/// `Theme.avatarColumnInset` is −4, which is why 8 fires). Not one of the 20 trials popped the
/// chat, so the back rule was never implicated; the doors were.
///
/// Two more trials 22 pt apart isolated the second door: at x = 58, on the bubble body, the same
/// drag did nothing at the row's middle and opened the page level with `bubble-name`. Both doors
/// ran one `openProfile`, so the defect was activation surviving a pan rather than either door,
/// and all four points are kept below as the regression case.
///
/// **The row is resolved from the live layout, never pinned.** Gate
/// [41828659420](https://github.com/unarbos/arbos/actions/runs/41828659420) read `bubble:g3` at
/// y −421 to −337 — 679 pt above the top of the window — where
/// [41812227199](https://github.com/unarbos/arbos/actions/runs/41812227199) read y 258 to 342, on
/// the same device, the same iOS, the same Xcode and a byte-identical fixture. Where a transcript
/// settles its opening scroll is a property of the tree, so no row id is a safe constant. Every
/// aim here is derived from a measured frame, which means an off-screen frame yields off-screen
/// aims, and not one of the aim checks could catch it: each asked whether the aim was inside the
/// frame it was computed from, which is true by construction. The window is the frame that has to
/// be asserted, and it now is, for every box and every aim. What the fix is about is the avatar
/// column's x band, which is a property of the column rather than of any one row.
///
/// **This file fails on a result, and that is a change of policy from the diagnostic it replaces.**
/// The sweep that produced the numbers above printed its outcomes and asserted almost nothing, so
/// that a finding could never red a chain where an `apps/ios/**` merge cuts a TestFlight build.
/// A guard on a fix has the opposite job: if a short drag opens a profile again, or if a tap stops
/// opening one, this has to say so.
final class AvatarColumnShortDragUITests: XCTestCase {
    /// The shared Mac runs the gate and releases at the same time; waits are generous.
    private let appear: TimeInterval = 90
    private let settle: TimeInterval = 8

    /// The chat the defect was reported in, and the row it was measured on — the latter a
    /// preference the resolver honours when that row is on screen, not a requirement.
    private static let group = "fx-group-launch"
    private static let preferredRow = "g3"

    /// The reported drag, unchanged in every respect — including the velocity, since what makes
    /// this drag interesting is that it is neither a tap nor long enough to pop.
    private static let distance: CGFloat = 50

    /// Where the bubble body begins (`Theme.memberAvatar`). Used to keep the name trials clear of
    /// the avatar column, never as a measurement: every frame below is read off the screen.
    private static let bubbleBodyX: CGFloat = 52

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    // MARK: Reading the screen

    private func chatBack(_ app: XCUIApplication) -> XCUIElement {
        app.buttons["chat-back:\(Self.group)"]
    }

    /// Whether the chat is still the screen in front.
    ///
    /// A pushed page leaves the chat's elements queryable with plausible frames, so "in front" has
    /// to be read from whether the chat's own header control can be touched. Reading a cover as a
    /// pop is what sent the original investigation down the wrong path for a day.
    private func chatIsFrontmost(_ app: XCUIApplication) -> Bool {
        app.descendants(matching: .any)["chat-search-button"].firstMatch.isHittable
    }

    /// Whether a profile page has named itself over the chat.
    ///
    /// A positive marker, not an inference from the chat not being frontmost: `profile-` prefixed
    /// identifiers exist only in `ProfilePages.swift`, and `ChatSearchResultJumpUITests` already
    /// recognises a page this way.
    private func profilePageShowing(_ app: XCUIApplication) -> Bool {
        app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier BEGINSWITH %@", "profile-"))
            .firstMatch.exists
    }

    /// The app's own bounds — the window every measured frame and every aim has to sit inside.
    private func window(_ app: XCUIApplication) -> CGRect {
        let frame = app.frame
        XCTAssertFalse(frame.isEmpty, "the application reports a window to measure frames against")
        return frame
    }

    private func launchGroup() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1", "ARBOS_OPEN_PEOPLE": Self.group]
        app.launch()
        XCTAssertTrue(chatBack(app).waitForExistence(timeout: appear), "the Launch week group opened")
        // Without this the frontmost read silently becomes "not frontmost" for every trial.
        XCTAssertTrue(app.descendants(matching: .any)["chat-search-button"].firstMatch.exists,
                      "the chat's header control is there to read frontmost-ness from")
        XCTAssertFalse(profilePageShowing(app), "no profile page is up before the trial")
        return app
    }

    // MARK: Resolving the row
    //
    // Self-contained rather than shared with `SwipeDirectionsUITests`: that file belongs to the
    // swipe-direction PR, which is not merged, and editing it from here would put a conflict in
    // the file both branches touch.

    /// A row to measure, together with everything the trials aim at.
    private struct Target {
        let id: String
        let bubble: CGRect
        let disc: CGRect
        let name: CGRect?
    }

    /// Wait until the transcript has stopped moving, then hand back every row's frame.
    ///
    /// A chat opens by scrolling to its newest message, and the rows exist before that scroll
    /// ends. A frame read mid-scroll says where a row was, not where the finger will land —
    /// which is how a start meant for the avatar column ends up somewhere else entirely.
    private func settledRows(_ app: XCUIApplication, file: StaticString = #filePath,
                             line: UInt = #line) -> [(id: String, frame: CGRect)] {
        var previous: [(id: String, frame: CGRect)] = []
        let deadline = Date().addingTimeInterval(settle)
        while Date() < deadline {
            let current = rowBoxes(app)
            if !current.isEmpty, current.count == previous.count,
               zip(current, previous).allSatisfy({ $0.0.id == $0.1.id && $0.0.frame == $0.1.frame }) {
                return current
            }
            previous = current
            Thread.sleep(forTimeInterval: 0.1)
        }
        XCTFail("the transcript's row frames never settled in \(Int(settle)) s "
                + "(last reading: \(previous.count) rows)", file: file, line: line)
        return previous
    }

    /// The row the trials use: on screen, with a sender's disc on screen, and — preferred, because
    /// the 22 pt pair needs one — a `bubble-name` on screen as well.
    ///
    /// `g3` wins whenever it qualifies, since it is the row this file's numbers were measured on.
    /// It cannot be required: see the note in this file's header about gate 41828659420, where it
    /// settled 679 pt above the window.
    private func onScreenTarget(_ app: XCUIApplication, in window: CGRect) -> Target? {
        let discs = avatarBoxes(app).filter { window.contains($0) }
        let candidates: [Target] = settledRows(app)
            .filter { window.contains($0.frame) }
            .compactMap { row -> Target? in
                guard let disc = nearestDisc(discs, to: row.frame.midY) else { return nil }
                let name = nameBox(app, in: row.frame)
                    .flatMap { (rect: CGRect) -> CGRect? in window.contains(rect) ? rect : nil }
                return Target(id: row.id, bubble: row.frame, disc: disc, name: name)
            }
        func rank(_ candidate: Target) -> Int {
            (candidate.name == nil ? 2 : 0)
                + (candidate.id == "bubble:\(Self.preferredRow)" ? 0 : 1)
        }
        return candidates.min { rank($0) < rank($1) }
    }

    /// A point in screen space, pinned to the frame this test measured — deliberately not
    /// `XCUIElement.coordinate`, which re-resolves the element when the event is synthesized and
    /// so can put the finger somewhere the surrounding arithmetic never saw.
    private func screenPoint(_ app: XCUIApplication, _ point: CGPoint) -> XCUICoordinate {
        app.coordinate(withNormalizedOffset: .zero).withOffset(CGVector(dx: point.x, dy: point.y))
    }

    // MARK: Geometry

    private func box(_ r: CGRect) -> String {
        "(x \(Int(r.minX))–\(Int(r.maxX)), y \(Int(r.minY))–\(Int(r.maxY)))"
    }

    private func rowBoxes(_ app: XCUIApplication) -> [(id: String, frame: CGRect)] {
        app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier BEGINSWITH %@", "bubble:"))
            .allElementsBoundByIndex
            .map { element -> (id: String, frame: CGRect) in (id: element.identifier, frame: element.frame) }
            .filter { !$0.frame.isEmpty }
    }

    /// The row a point lands in: the smallest container holding it, since a row's elements nest.
    /// Printed with every trial, because "which row was under the finger" is the first thing a
    /// reader of a failed gate wants and the most expensive thing to re-derive from a screenshot.
    private func rowContaining(_ point: CGPoint, _ rows: [(id: String, frame: CGRect)]) -> String {
        rows.filter { $0.frame.contains(point) }
            .min { $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height }?
            .id ?? "none"
    }

    private func avatarBoxes(_ app: XCUIApplication) -> [CGRect] {
        app.descendants(matching: .any).matching(identifier: "sender-avatar")
            .allElementsBoundByIndex
            .map(\.frame)
            .filter { !$0.isEmpty }
    }

    /// `bubble-name` on the row under test — the second door to the same `openProfile`.
    private func nameBox(_ app: XCUIApplication, in bubble: CGRect) -> CGRect? {
        app.descendants(matching: .any).matching(identifier: "bubble-name")
            .allElementsBoundByIndex
            .map(\.frame)
            .first { !$0.isEmpty && bubble.intersects($0) }
    }

    /// The disc nearest `y`. `Row.showsAvatar` puts a disc on one row of a sender's run, which
    /// need not be this row, so the disc is found rather than assumed to be beside `g3`.
    private func nearestDisc(_ discs: [CGRect], to y: CGFloat) -> CGRect? {
        discs.min { abs($0.midY - y) < abs($1.midY - y) }
    }

    // MARK: The regression case

    /// All four measured firing points, and neither the chat nor the profile moves.
    ///
    /// Four drags in one launch rather than four cases: a drag that correctly does nothing leaves
    /// the chat in front, so the next trial starts from the state the last one should have left.
    /// The case fails the moment one of them does something, and prints the point and the row it
    /// landed in either way.
    func testAShortRightDragFromTheAvatarColumnOrTheNameOpensNothing() {
        let app = launchGroup()
        let screen = window(app)
        guard let target = onScreenTarget(app, in: screen) else {
            return XCTFail("no row of the group transcript is on screen with a sender avatar "
                           + "beside it, so there is nothing here to drag from")
        }
        let bubble = target.bubble
        let disc = target.disc
        let name = target.name
        let rows = rowBoxes(app)
        let nameText = name == nil ? "none" : box(name!)
        print("avatar-column-drag: row \(target.id) (preferring bubble:\(Self.preferredRow)) "
              + "=\(box(bubble)) disc=\(box(disc)) bubble-name=\(nameText) "
              + "window=\(box(screen))")

        // The column at the row's middle (the reported aim) and the column at the disc's own
        // centre (the only aim whose touch-down is inside the avatar's frame).
        var aims: [(String, CGPoint)] = [
            ("column at the row's middle", CGPoint(x: disc.midX, y: bubble.midY)),
            ("the disc's measured centre", CGPoint(x: disc.midX, y: disc.midY)),
        ]
        if let name {
            // The 22 pt pair, derived the way the sweep derived it: start on the bubble body far
            // enough left that the drag *finishes* inside the name, which is what made the name
            // the thing under test rather than the column. Same x for both, so the only
            // difference between them is the 22 pt of height that separates the name's band from
            // the row around it.
            let bodyX = max(Self.bubbleBodyX, name.midX - Self.distance)
            XCTAssertGreaterThanOrEqual(bodyX, Self.bubbleBodyX,
                                        "the name trials start on the bubble body, not in the "
                                        + "avatar column they exist to rule out")
            XCTAssertTrue(name.contains(CGPoint(x: bodyX + Self.distance, y: name.midY)),
                          "the name trial's drag finishes inside bubble-name \(box(name)); a drag "
                          + "that misses it tests nothing and must not read as the name standing down")
            aims.append(("body at the row's middle", CGPoint(x: bodyX, y: bubble.midY)))
            aims.append(("body level with the name", CGPoint(x: bodyX, y: name.midY)))
        } else {
            // Said out loud rather than skipped in silence: a pair that did not run is not a pair
            // that found nothing, and the name half of this case is half its point.
            print("avatar-column-drag: no bubble-name on \(target.id) — the 22 pt pair did not run")
        }

        for (label, point) in aims {
            XCTAssertTrue(chatIsFrontmost(app), "the chat is in front before \"\(label)\"")
            // The assertion this case was missing. Every aim is arithmetic on a measured frame, so
            // an off-window frame gives an off-window aim; the finger then touches nothing, nothing
            // opens, and a vacuum reads exactly like a pass. Gate 41828659420 is that run.
            XCTAssertTrue(screen.contains(point),
                          "the aim for \"\(label)\" at (\(Int(point.x)), \(Int(point.y))) is inside "
                          + "the window \(box(screen)); an aim outside it touches nothing, and a "
                          + "trial that touches nothing opens nothing")
            XCTAssertTrue(screen.contains(CGPoint(x: point.x + Self.distance, y: point.y)),
                          "the end of the \(Int(Self.distance)) pt drag from \"\(label)\" is inside "
                          + "the window \(box(screen)) as well")
            XCTAssertLessThan(point.x + Self.distance, bubble.maxX,
                              "the whole \(Int(Self.distance)) pt drag from \"\(label)\" stays on "
                              + "the row, so the drag measured is the one asked for and not a "
                              + "clamped one")
            print("avatar-column-drag: \"\(label)\" at (\(Int(point.x)), \(Int(point.y))) "
                  + "in row \(rowContaining(point, rows)), dragging \(Int(Self.distance)) pt right")
            let start = screenPoint(app, point)
            // Every parameter is the one that fired 11 of 11 on 41812227199. A faster drag or a
            // shorter hold is a different gesture and would not reproduce the thing being guarded.
            start.press(forDuration: 0.05,
                        thenDragTo: start.withOffset(CGVector(dx: Self.distance, dy: 0)),
                        withVelocity: .slow, thenHoldForDuration: 0.5)
            // Long enough for a push animation to have put a page on screen if one was coming.
            sleep(1)

            XCTAssertFalse(profilePageShowing(app),
                           "a \(Int(Self.distance)) pt right drag from \"\(label)\" at "
                           + "(\(Int(point.x)), \(Int(point.y))) opened a profile page")
            XCTAssertTrue(chatBack(app).exists,
                          "a \(Int(Self.distance)) pt right drag from \"\(label)\" left the chat")
            XCTAssertTrue(chatIsFrontmost(app),
                          "a \(Int(Self.distance)) pt right drag from \"\(label)\" took the chat "
                          + "off the front without naming what covered it")
        }
    }

    // MARK: What the fix must not cost
    //
    // Both doors, each on its own launch, because a tap that works takes the chat off the front
    // and there is nothing to measure a second tap against. These are the positive controls: they
    // are what distinguishes the fix from switching the doors off, and without them the case above
    // passes just as well on a transcript where nothing opens a profile at all.

    /// A tap at the measured centre of a disc still opens the sender's page.
    func testATapOnTheMeasuredDiscCentreStillOpensTheProfile() {
        let app = launchGroup()
        let screen = window(app)
        guard let target = onScreenTarget(app, in: screen) else {
            return XCTFail("no row of the group transcript is on screen with a sender avatar to tap")
        }
        let disc = target.disc
        let point = CGPoint(x: disc.midX, y: disc.midY)
        // This was `disc.contains(point)`, which could not fail: the point is the disc's own
        // centre, so containment held by construction and the check was decoration. What can fail
        // is the disc sitting outside the window — gate 41828659420 read it at y −373 to −337 —
        // and then the tap lands nowhere while the frame it came from still looks reasonable.
        XCTAssertTrue(screen.contains(point),
                      "the tap point (\(Int(point.x)), \(Int(point.y))) is inside the window "
                      + "\(box(screen)); the disc read \(box(disc))")
        XCTAssertGreaterThan(point.x, 24,
                             "the disc's centre is clear of the 24 pt the system pop owns, so this "
                             + "is the app's own tap")
        print("avatar-column-drag: tapping the disc \(box(disc)) at (\(Int(point.x)), \(Int(point.y)))")

        screenPoint(app, point).tap()

        XCTAssertTrue(waitForProfile(app),
                      "a tap on the disc at (\(Int(point.x)), \(Int(point.y))) opens the sender's page")
    }

    /// A tap inside `bubble-name`'s measured frame still opens the sender's page.
    ///
    /// Strictly inside the measured box, which the drag trial above could not manage: its name
    /// aim starts on the bubble body at x = 58 so that it has somewhere to drag from, and 58 is
    /// outside the name's own frame. So that trial shows the name's *band* is quiet and this one
    /// shows the name itself still answers.
    func testATapInsideTheMeasuredNameFrameStillOpensTheProfile() {
        let app = launchGroup()
        let screen = window(app)
        guard let target = onScreenTarget(app, in: screen) else {
            return XCTFail("no row of the group transcript is on screen with a sender avatar "
                           + "beside it, so no row's name can be tapped either")
        }
        guard let name = target.name else {
            return XCTFail("\(target.id) is on screen but draws no bubble-name inside the window "
                           + "\(box(screen)) to tap")
        }
        let point = CGPoint(x: name.midX, y: name.midY)
        // Same correction as the disc case: `name.contains(point)` was true by construction, and
        // the window is the frame worth asserting.
        XCTAssertTrue(screen.contains(point),
                      "the tap point (\(Int(point.x)), \(Int(point.y))) is inside the window "
                      + "\(box(screen)); the name read \(box(name))")
        XCTAssertGreaterThanOrEqual(point.x, Self.bubbleBodyX,
                                    "the name's centre is on the bubble body, so this tap is the "
                                    + "name's door and not the avatar column's")
        print("avatar-column-drag: tapping bubble-name \(box(name)) at (\(Int(point.x)), \(Int(point.y)))")

        screenPoint(app, point).tap()

        XCTAssertTrue(waitForProfile(app),
                      "a tap on the name at (\(Int(point.x)), \(Int(point.y))) opens the sender's page")
    }

    /// Wait for a page to name itself over the chat.
    private func waitForProfile(_ app: XCUIApplication) -> Bool {
        let page = app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier BEGINSWITH %@", "profile-"))
            .firstMatch
        return page.waitForExistence(timeout: settle)
    }
}
