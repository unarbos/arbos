import UIKit
import XCTest
@testable import Arbos

/// The back swipe follows the finger (Jacob, 2026-11-03: "The swipe is still not always
/// registering and it seems finicky. Sometimes i swipe and then it doesn't register until i
/// let go… Should register on first touch and then as i slide my finger across the chat
/// slides out with my finger").
///
/// Away from the screen's left edge the gesture used to read one number, once, when the
/// touch was lifted: `BackSwipe.fullScreenPops` on the final translation. Every sample in
/// between did nothing, so the screen stood still for the whole drag and then jumped — which
/// is exactly the report. `BackDrag` is the rule that replaced it, and these cases are
/// written to tell the two apart rather than to describe the new one: for each drag they
/// check what the screen did **while the finger was still down**, not only where it ended up.
///
/// Separate from `BackSwipeTests` on purpose: that file is being edited in another lane
/// (#2386, the swipe-direction split).
final class BackDragTests: XCTestCase {
    /// An iPhone 17's width, the travel of a complete pop.
    private let width: CGFloat = 393

    /// One touch sample: how far the finger has moved from where it went down, and how fast
    /// it is going there.
    private struct Sample {
        let translation: CGSize
        let velocity: CGSize

        init(_ dx: CGFloat, _ dy: CGFloat = 0, speed: CGFloat = 600) {
            translation = CGSize(width: dx, height: dy)
            velocity = CGSize(width: speed, height: 0)
        }
    }

    /// Play a whole touch: the first sample is the one the recognizer claimed the drag on,
    /// the rest are moves, and `release` ends it. The result is what the screen did, in
    /// order, with the release's answer last.
    private func play(_ samples: [Sample], release: UIGestureRecognizer.State = .ended) -> [BackDrag.Step] {
        let drag = BackDrag(width: width)
        var steps = samples.enumerated().map { (index, sample) -> BackDrag.Step in
            let state: UIGestureRecognizer.State = index == 0 ? .began : .changed
            return drag.step(for: state, translation: sample.translation, velocity: sample.velocity)
        }
        if let last = samples.last {
            steps.append(drag.step(for: release, translation: last.translation, velocity: last.velocity))
        }
        return steps
    }

    /// How far out the screen was at each sample taken before the touch was lifted. Fails
    /// when any of them resolved the gesture instead of moving the screen.
    private func travelUnderTheFinger(_ steps: [BackDrag.Step],
                                      file: StaticString = #filePath, line: UInt = #line) -> [CGFloat] {
        steps.dropLast().map { (step) -> CGFloat in
            guard case .track(let progress) = step else {
                XCTFail("the screen answered \(step) while the finger was still down; only the release may finish or cancel",
                        file: file, line: line)
                return 0
            }
            return progress
        }
    }

    // MARK: The report: a swipe that only registers on touch-up

    /// The heart of it. A plain rightward drag, sampled nine times on the way out: every one
    /// of those samples moves the screen, each one further than the last, and the release is
    /// the first and only step that decides anything.
    func testTheScreenMovesOnEverySampleAndNotOnlyOnTheRelease() {
        let samples = [12, 24, 40, 60, 85, 110, 140, 175, 210].map { Sample(CGFloat($0)) }
        let steps = play(samples)

        let travel = travelUnderTheFinger(steps)
        XCTAssertEqual(travel.count, samples.count, "every sample under the finger moved the screen")
        XCTAssertGreaterThan(travel.first ?? 0, 0, "the screen is already out on the first sample")
        for (earlier, later) in zip(travel, travel.dropFirst()) {
            XCTAssertGreaterThan(later, earlier, "the screen kept up with the finger")
        }
        XCTAssertEqual(steps.last, .finish, "letting go at 210 pt of a 393 pt screen goes back")
    }

    /// The screen is under the finger, not behind it: at every sample it is out by exactly
    /// the distance the finger has travelled.
    func testTheScreenFollowsTheFingerOneToOne() {
        for dx in [CGFloat(12), 40, 100, 196.5, 300] {
            XCTAssertEqual(BackDrag.progress(dx: dx, width: width) * width, dx, accuracy: 0.001,
                           "at \(dx) pt of finger the screen is \(dx) pt out")
        }
        XCTAssertEqual(BackDrag.progress(dx: 500, width: width), 1, "the screen stops when it is all the way out")
        XCTAssertEqual(BackDrag.progress(dx: -40, width: width), 0, "a leftward drag moves it nowhere")
    }

    /// A drag the old rule would have thrown away whole. 60 pt let go gently is under
    /// `fullScreenPops`' 70 pt, so on the release-only rule nothing happened at any point in
    /// the touch — the screen never moved and the chat stayed, with no sign a gesture had
    /// been read. Now the screen goes out with the finger and comes back when it is let go.
    ///
    /// This is the case that separates "tracks from the first movement" from "resolves on
    /// release": both end with the chat still on screen, and only the middle tells them apart.
    func testADragTooShortToGoBackStillMovesTheScreenTheWholeWay() {
        let samples = [12, 25, 40, 52, 60].map { Sample(CGFloat($0), 4, speed: 90) }
        XCTAssertFalse(BackSwipe.fullScreenPops(translation: samples.last!.translation, velocity: samples.last!.velocity),
                       "the old release-only rule reads this drag as nothing at all")

        let steps = play(samples)

        let travel = travelUnderTheFinger(steps)
        XCTAssertEqual(travel.count, samples.count)
        XCTAssertEqual(travel.last ?? 0, 60 / width, accuracy: 0.001, "the screen was 60 pt out when the finger stopped")
        XCTAssertEqual(steps.last, .cancel, "and it goes back where it was on the release")
    }

    // MARK: Slowly, and on the diagonal

    /// Jacob's "finicky" case: a slow drag. Speed is nothing to the tracking — the screen
    /// goes where the finger is — and it only reaches the release, where 240 pt sideways and
    /// nothing vertical goes back however slowly it got there.
    func testASlowDragTracksJustTheSame() {
        let samples = [12, 18, 26, 38, 55, 80, 120, 180, 240].map { Sample(CGFloat($0), 0, speed: 45) }
        let steps = play(samples)

        let travel = travelUnderTheFinger(steps)
        XCTAssertEqual(travel.count, samples.count, "a slow finger is sampled and answered like any other")
        for (sample, progress) in zip(samples, travel) {
            XCTAssertEqual(progress * width, sample.translation.width, accuracy: 0.001)
        }
        XCTAssertEqual(steps.last, .finish, "a slow 240 pt is still the release rule's answer, not the tracking's")
    }

    /// A drag that starts on the diagonal — the finger comes down and goes out at the same
    /// time, which is what a thumb reaching across a phone actually does. The screen tracks
    /// its horizontal travel from the first sample and ignores the vertical part.
    func testADiagonalDragTracksFromItsFirstSample() {
        // About 30° below the horizontal, inside the 1.15× cone the recognizer claims on.
        let samples = [(14, 8), (30, 17), (55, 31), (90, 52), (130, 75), (185, 107)]
            .map { Sample(CGFloat($0.0), CGFloat($0.1), speed: 70) }
        let steps = play(samples)

        let travel = travelUnderTheFinger(steps)
        XCTAssertEqual(travel.count, samples.count, "a diagonal drag moves the screen on every sample too")
        XCTAssertGreaterThan(travel.first ?? 0, 0, "including the first")
        for (sample, progress) in zip(samples, travel) {
            XCTAssertEqual(progress * width, sample.translation.width, accuracy: 0.001,
                           "the screen follows the sideways part of the drag and nothing else")
        }
    }

    /// The same diagonal, let go early: it still moved all the way out and back.
    func testADiagonalDragLetGoEarlyStillMovedTheScreen() {
        let samples = [(14, 8), (30, 17), (48, 28)].map { Sample(CGFloat($0.0), CGFloat($0.1), speed: 60) }
        let steps = play(samples)

        XCTAssertEqual(travelUnderTheFinger(steps).last ?? 0, 48 / width, accuracy: 0.001)
        XCTAssertEqual(steps.last, .cancel)
    }

    // MARK: What the release decides, which this lane does not change

    /// The release asks exactly the question it always asked. `BackDrag` holds no threshold of
    /// its own, so where the swipe commits stays Jacob's to set — about three quarters of the
    /// way across with a velocity escape, as constants he can feel on his own phone — and it
    /// stays a separate build. This case is what keeps the two apart.
    func testTheReleaseIsStillFullScreenPopsAndNothingElse() {
        let cases: [(CGSize, CGSize)] = [
            (CGSize(width: 210, height: 0), CGSize(width: 600, height: 0)),      // long and brisk
            (CGSize(width: 60, height: 4), CGSize(width: 90, height: 0)),        // short and gentle
            (CGSize(width: 90, height: 8), CGSize(width: 1400, height: 40)),     // a flick
            (CGSize(width: 196, height: 300), CGSize(width: 40, height: 900)),   // far out, but dragged down
            (CGSize(width: 350, height: 10), .zero),                             // pushed across and paused
        ]
        for (translation, velocity) in cases {
            let expected: BackDrag.Step =
                BackSwipe.fullScreenPops(translation: translation, velocity: velocity) ? .finish : .cancel
            XCTAssertEqual(BackDrag(width: width).step(for: .ended, translation: translation, velocity: velocity),
                           expected, "the release agrees with fullScreenPops at \(translation)")
        }
    }

    /// A deliberate flick goes back from well short of the screen, as it always has.
    func testAShortFlickStillGoesBack() {
        XCTAssertTrue(BackSwipe.fullScreenPops(translation: CGSize(width: 90, height: 8),
                                               velocity: CGSize(width: 1400, height: 40)))
        XCTAssertEqual(play([Sample(30), Sample(60), Sample(90, 8, speed: 1400)]).last, .finish)
    }

    /// A leftward drag can never go back, at any distance: `fullScreenPops` has always
    /// required `dx > 0`, and the release is still the only thing that asks it. That is the
    /// half of #2386's "right is back, left is reply" guarantee this lane must not weaken.
    func testALeftwardDragNeverGoesBack() {
        for dx in stride(from: CGFloat(-12), through: -400, by: -8) {
            let translation = CGSize(width: dx, height: 0)
            let velocity = CGSize(width: -2000, height: 0)
            XCTAssertFalse(BackSwipe.fullScreenPops(translation: translation, velocity: velocity),
                           "a \(Int(-dx)) pt swipe left is not back")
            XCTAssertEqual(BackDrag(width: width).step(for: .ended, translation: translation, velocity: velocity),
                           .cancel, "and letting go of one puts the screen back")
        }
    }

    /// A touch taken away mid-drag — a call banner, a second finger, the recognizer losing
    /// its claim — puts the screen back rather than leaving it half out.
    func testATakenAwayTouchPutsTheScreenBack() {
        for state in [UIGestureRecognizer.State.cancelled, .failed] {
            let steps = play([Sample(20), Sample(90), Sample(160)], release: state)
            XCTAssertEqual(steps.last, .cancel, "a \(state.rawValue) touch does not go back")
            XCTAssertEqual(travelUnderTheFinger(steps).count, 3, "and the screen tracked until it was taken away")
        }
    }

    /// Before the recognizer claims the drag there is nothing to draw.
    func testNothingHappensBeforeTheDragIsClaimed() {
        let drag = BackDrag(width: width)
        XCTAssertEqual(drag.step(for: .possible, translation: CGSize(width: 4, height: 2), velocity: .zero), .idle)
    }

    /// A screen with no width yet (a host measured before layout) must not divide by zero or
    /// report the screen as fully out.
    func testAnUnmeasuredScreenTracksNothing() {
        XCTAssertEqual(BackDrag.progress(dx: 100, width: 0), 0)
        XCTAssertEqual(BackDrag(width: 0).step(for: .changed, translation: CGSize(width: 100, height: 0), velocity: .zero),
                       .track(progress: 0))
    }
}
