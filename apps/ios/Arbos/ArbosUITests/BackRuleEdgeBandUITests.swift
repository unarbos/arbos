import XCTest

/// Where UIKit's interactive pop still claims a rightward drag, and what that costs the app's
/// own back rule.
///
/// **The question.** `BackSwipe.edgeZoneWidth` is 24: a drag beginning left of 24 pt is left to
/// the system's `interactivePopGestureRecognizer`, and everything right of it is the app's, to
/// be decided by `BackSwipe.fullScreenPops` — which needs `absX > 70`. Those two numbers only
/// describe the app's intent. The system recognizer's own reach is UIKit's, and the app cannot
/// narrow it. If it reaches further than 24 pt there is a band where both recognizers are
/// eligible for the same touch, they are mutually exclusive (`BackSwipe.sharesTouch` refuses
/// every pan, and UIKit's is a `UIScreenEdgePanGestureRecognizer`), and which one begins first
/// decides what a drag does. In that band the app's 70 pt rule is not the rule.
///
/// **Why this is not theory.** Gate 41759504677 and gate 41749001603 both popped the chat on a
/// 50 pt rightward drag begun at x = 40. 50 pt cannot pop through the app: both pop paths —
/// `TranscriptSwipeHost.handle`'s `.back` case and `FullScreenBackHost.handle` — are gated on
/// `fullScreenPops`, whose `absX > 70` guard 50 cannot clear. Gate 41563318547 ran the same
/// 50 pt drag from the same x against byte-identical `BackSwipe.swift` and did **not** pop. One
/// event, one build, two outcomes: a race, which is what two eligible exclusive recognizers
/// look like.
///
/// **What this file measures.** One fixed 50 pt distance — deliberately under the app's rule,
/// so *any* pop is the system's — swept across start x, **repeated at each point**. Every
/// trial relaunches, because a pop changes the screen. The sweep prints a line per trial and a
/// tally per point, so the log carries the whole table even when the verdict is a single
/// assertion, and then fails naming every x that ever popped.
///
/// **Repeats, not one shot, because the thing being measured is a race.** A drag that does not
/// pop shows only that the app's recogniser won that touch; it is not evidence UIKit cannot
/// claim there. Reading a single quiet observation as "clear" is what made the recorded
/// hand-sweep's lone `x = 28` result look like proof that UIKit's reach stopped short of 40.
/// A pop is sound evidence of reach; a run of non-pops is weak evidence of clearance, and
/// weakens no further than the trial count allows.
///
/// **Why the Comms chat.** The drag has to begin on bare transcript, or the measurement is of
/// something else. A group row draws a sender avatar across 12–48 pt (`Theme.memberAvatar`,
/// #2527) — a `Button`, which since #2530 shares the touch with the back recognizers — and
/// `g3` carries an autolinked URL (`https://arbos.life/i/launch`), which is an interactive
/// link. Both sit exactly in the band under test. In the Comms chat every `.user` row is the
/// signed-in user's and renders trailing-aligned, so the leading half of the row is empty
/// gutter: no avatar, no link, no button. That is the control column, and it is the one the
/// earlier hand sweep used.
///
/// Companion to `AvatarBackSwipeUITests`, which holds the avatar case, and to
/// `SwipeDirectionsUITests`, which holds the direction split. This file holds neither: it holds
/// the boundary between the app's gesture and the system's.
final class BackRuleEdgeBandUITests: XCTestCase {
    /// The shared Mac runs the gate and releases at the same time; waits are generous.
    private let appear: TimeInterval = 90
    private let settle: TimeInterval = 20

    /// The Comms agent chat: no avatar lane beside my own messages, so the leading gutter is
    /// bare transcript.
    private static let comms = "fx-comms-main"

    /// Under `BackSwipe.fullScreenPops`' `absX > 70`, so the app's own rule cannot pop it.
    /// Any pop at this distance came from UIKit.
    private static let distance: CGFloat = 50

    /// Start points, in screen points from the leading edge.
    ///
    /// Spread rather than dense, because trials per point buy more here than points do (see
    /// `trials`). 24 is `edgeZoneWidth` itself; 40 is where the gates popped and so is the
    /// known-contested control; 80 is `SwipeDirectionsUITests.appOwnedStartX`, chosen on the
    /// assumption that nothing but the app can answer there, which is the assumption under
    /// test; 120 is the far control — a pop there would mean this is not an edge band at all.
    private static let starts: [CGFloat] = [16, 24, 32, 40, 48, 56, 72, 80, 96, 120]

    /// Observations per start point.
    ///
    /// One is not enough, and that is the whole lesson of this investigation. The two
    /// recognisers race, so a drag that does not pop shows only that the app won *that* touch
    /// — not that UIKit cannot claim there. At x = 40 three gates split one stay to two pops,
    /// so the app takes roughly a third of them; a single trial would mis-read a contested
    /// point about that often.
    ///
    /// The signal is therefore **ever popped**, not popped. Five trials put a contested point
    /// surviving as all-stayed under a percent at that win rate.
    private static let trials = 5

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func chatBack(_ app: XCUIApplication) -> XCUIElement {
        app.buttons["chat-back:\(Self.comms)"]
    }

    /// Whether the chat is still the screen in front.
    ///
    /// Two separate things can take the chat off screen and they must not be confused. A pop
    /// removes it from the stack. A pushed page — the sender's profile, say — covers it while
    /// leaving its elements queryable with plausible frames (`AvatarBackSwipeUITests`,
    /// `ChatSearchResultJumpUITests`), so the honest read of "in front" is whether the chat's
    /// own header control can be touched.
    private func chatIsFrontmost(_ app: XCUIApplication) -> Bool {
        app.descendants(matching: .any)["chat-search-button"].firstMatch.isHittable
    }

    private func launchComms() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1", "ARBOS_OPEN_PROJECT": "fx-comms"]
        app.launch()
        XCTAssertTrue(chatBack(app).waitForExistence(timeout: appear), "the Comms chat opened")
        return app
    }

    /// A y inside the transcript that is clear of the header and the composer bar, and a drag
    /// that begins in the composer is the composer's by design (`fullScreenLock` declines a
    /// start at or below `composerTop`).
    ///
    /// Taken from the window rather than from a row: this sweep must not depend on a bubble
    /// being resolved, since the row-resolution work is a different question on a different
    /// branch and a dependency on it would make a stale frame look like an edge-band result.
    private func transcriptY(_ app: XCUIApplication) -> CGFloat {
        app.windows.firstMatch.frame.midY
    }

    /// What one 50 pt rightward drag from `x` did.
    private enum Outcome: String {
        /// The chat is gone from the stack: something popped it, and at 50 pt that is UIKit.
        case popped
        /// The chat is still there and still in front: nobody acted. This is the app's rule.
        case stayed
        /// The chat is still on the stack but something is over it. Not a pop; recorded
        /// separately so it can never be read as one.
        case covered
    }

    /// Fail rather than measure a drag that began on a control.
    ///
    /// Everything this sweep claims rests on the start point being bare transcript. An avatar
    /// is a `Button` that shares the touch with the back recognizers since #2530, and a
    /// `Button` under the finger changes which recognizers are in the race — so a point inside
    /// one would be measuring #2530's simultaneity, not UIKit's reach, and would read as an
    /// edge-band result. Checked per point rather than once: the transcript is free to relayout
    /// between launches, and a frame read before that is stale rather than wrong.
    private func assertBareTranscript(_ app: XCUIApplication, at point: CGPoint,
                                      file: StaticString = #filePath, line: UInt = #line) {
        let avatars = app.descendants(matching: .any).matching(identifier: "sender-avatar")
        for frame in avatars.allElementsBoundByIndex.map(\.frame) where frame.contains(point) {
            XCTFail("the start point \(point) is inside a sender avatar at \(frame), so this "
                    + "point would measure the avatar's press and not the edge band",
                    file: file, line: line)
        }
    }

    private func outcome(startX: CGFloat) -> Outcome {
        let app = launchComms()
        let y = transcriptY(app)
        assertBareTranscript(app, at: CGPoint(x: startX, y: y))
        let start = app.coordinate(withNormalizedOffset: .zero)
            .withOffset(CGVector(dx: startX, dy: y))
        start.press(forDuration: 0.05,
                    thenDragTo: start.withOffset(CGVector(dx: Self.distance, dy: 0)),
                    withVelocity: .slow, thenHoldForDuration: 0.5)
        sleep(1)
        if !chatBack(app).exists { return .popped }
        return chatIsFrontmost(app) ? .stayed : .covered
    }

    /// Sweep the start point and name every x where a 50 pt drag ever popped the chat.
    ///
    /// On a build whose back rule holds everywhere, every trial at every point stays: 50 pt is
    /// under 70 pt, so nothing should go back from anywhere. A block of points that pop at
    /// least once, running from the leading edge up to some x, is UIKit's reach — and the gap
    /// between `edgeZoneWidth` and the top of that block is how far the app is wrong about
    /// where it stops being in charge.
    ///
    /// Asymmetry worth stating, because it decides how the table may be read: a pop is sound
    /// evidence that UIKit claims the point, since no app path can pop 50 pt. A point that
    /// never pops across `trials` attempts is *weak* evidence it is clear — the more trials,
    /// the less weak, and never proof.
    func testNoFiftyPointDragEverPopsTheChatFromAnywhere() {
        var rows: [(x: CGFloat, pops: Int, covers: Int)] = []
        for x in Self.starts {
            var pops = 0, covers = 0
            for trial in 1...Self.trials {
                let result = outcome(startX: x)
                if result == .popped { pops += 1 }
                if result == .covered { covers += 1 }
                print("edge-band sweep: x=\(Int(x)) trial \(trial)/\(Self.trials) "
                      + "distance=\(Int(Self.distance)) -> \(result.rawValue)")
            }
            rows.append((x, pops, covers))
            print("edge-band sweep: x=\(Int(x)) popped \(pops)/\(Self.trials)")
        }

        let printed = rows.map { "\(Int($0.x)):\($0.pops)/\(Self.trials)" }.joined(separator: " ")
        print("edge-band sweep table (pops per start x): \(printed)")

        let popped = rows.filter { $0.pops > 0 }.map { Int($0.x) }
        let covered = rows.filter { $0.covers > 0 }.map { Int($0.x) }
        XCTAssertTrue(covered.isEmpty,
                      "a 50 pt drag put something over the chat at x = \(covered) — not a pop, "
                      + "and not the app's back rule either. Whole sweep: \(printed)")
        XCTAssertTrue(popped.isEmpty,
                      "a 50 pt drag popped the chat at x = \(popped), which "
                      + "BackSwipe.fullScreenPops cannot do (absX > 70). Those start points are "
                      + "inside UIKit's interactive pop, so BackSwipe.edgeZoneWidth (24) is "
                      + "narrower than the strip the app actually has to stand aside for. "
                      + "Whole sweep: \(printed)")
    }
}
