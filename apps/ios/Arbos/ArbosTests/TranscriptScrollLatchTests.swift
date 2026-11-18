import XCTest
@testable import Arbos

/// The reader's place survives the keyboard going away under their finger
/// ([feedback typing-anchor-2026-11-08], runs 42545157179 / 42545159346: the head leg of
/// the history pair came back at the live bottom while the base leg stayed up in history,
/// off the same drag from the same place).
///
/// The transcript measures how far it is from the end as
/// `contentSize − contentOffset − containerSize`, which is a distance from the container's
/// bottom edge. `.scrollDismissesKeyboard(.interactively)` retracts the keyboard under the
/// finger, so the container grows by the keyboard's whole height during the one gesture
/// that takes the reader away from the end — and the raw number falls by that height while
/// the reader is going the other way.
///
/// It only became visible when the presence row moved inside the transcript, because a row
/// that lives in the transcript changes the content size while it works, and a growth while
/// `pinned` re-sticks to the end. The latch was always wrong; it had nothing to fire it.
final class TranscriptScrollLatchTests: XCTestCase {
    /// iPhone 17 portrait, the height `BottomBarFadeTests` already models.
    private let keyboard: CGFloat = 336

    func testADragWithNoKeyboardInItReadsExactlyAsItAlwaysDid() {
        for sampled in [CGFloat(0), 60, 119, 120, 121, 400] {
            let held = TranscriptScrollLatch.distance(sampled: sampled, container: 874, gestureStart: 874)
            XCTAssertEqual(held, sampled, "a container that did not move corrects by nothing")
            XCTAssertEqual(TranscriptScrollLatch.away(distance: held), sampled > 120)
        }
    }

    /// The regression. A reader drags 300 pt up into history with the keyboard open; the
    /// same drag retracts the keyboard, so the container grows 336 pt under them.
    func testTheKeyboardRetractingUnderTheFingerDoesNotReadAsArrivingAtTheEnd() {
        let dragged: CGFloat = 300
        let withKeyboard: CGFloat = 874 - 336
        // What the scroll view reports once the keyboard has gone: the same content, a
        // taller container, so 36 pt *past* the end.
        let sampled = dragged - keyboard
        XCTAssertEqual(sampled, -36)
        XCTAssertFalse(TranscriptScrollLatch.away(distance: sampled),
                       "the raw sample is the bug: a reader 300 pt into history reads as pinned")

        let held = TranscriptScrollLatch.distance(sampled: sampled, container: 874, gestureStart: withKeyboard)
        XCTAssertEqual(held, dragged, "measured against the gesture's own container, it is the 300 pt drag")
        XCTAssertTrue(TranscriptScrollLatch.away(distance: held), "so the reader stays away from the end")
    }

    /// A reader who really did come back to the end during the same gesture still latches,
    /// so the correction cannot be used to strand someone at the bottom.
    func testComingBackToTheEndDuringTheSameGestureStillCounts() {
        let withKeyboard: CGFloat = 874 - 336
        let held = TranscriptScrollLatch.distance(sampled: -336, container: 874, gestureStart: withKeyboard)
        XCTAssertEqual(held, 0, "no content between them and the end")
        XCTAssertFalse(TranscriptScrollLatch.away(distance: held))
    }

    /// The threshold is the one the transcript has always used; this suite is about the
    /// measurement reaching it honestly, not about moving it.
    func testTheThresholdIsUnchanged() {
        XCTAssertEqual(TranscriptScrollLatch.awayThreshold, 120)
        XCTAssertFalse(TranscriptScrollLatch.away(distance: 120))
        XCTAssertTrue(TranscriptScrollLatch.away(distance: 120.5))
    }

    // MARK: After the gesture

    /// The hole the latch left behind, and the reason the chevron was not evidence.
    ///
    /// The gesture ends with the finger; the keyboard's retraction does not. The last of it
    /// lands as a container resize with no content growth, and the reporter used to look only
    /// at content — so `away` kept whatever the gesture set and described a state that had
    /// since moved. Run 42558663139 is what that cost: the transcript was back at the live
    /// bottom and the chevron was still up, so a test asking "is the chevron there" passed
    /// over the bug it was written to catch.
    ///
    /// The proposition is that a container-only change is read again, not that some gate
    /// changed shape: `.hold` here is the old behaviour and is what must not come back.
    func testAContainerGrowthWithNoContentGrowthReadsWhereTheReaderIsAgain() {
        let withKeyboard: CGFloat = 874 - 336

        // The keyboard finishes leaving and the end comes back to the reader: say so.
        XCTAssertEqual(TranscriptScrollLatch.settled(grew: false, oldContainer: withKeyboard,
                                                     newContainer: 874, distance: 0, pinned: false),
                       .reread(away: false),
                       "the reader is at the end again and `away` has to stop saying otherwise")

        // The same resize with the reader genuinely still up in history: unchanged answer,
        // reached by reading rather than by keeping.
        XCTAssertEqual(TranscriptScrollLatch.settled(grew: false, oldContainer: withKeyboard,
                                                     newContainer: 874, distance: 400, pinned: false),
                       .reread(away: true),
                       "400 pt of transcript below them is still history")
    }

    /// The keyboard rising under a reader at the end (TestFlight reports, 2026-11-13).
    ///
    /// The numbers are the simulator's: the sample for the rise has the container 301 pt
    /// shorter and the offset where it was, so the distance reads 364 pt for a reader who was
    /// 63 pt from the end and did not move. Reading that as "away" unpinned them on every
    /// keyboard open — the chevron over the live bottom, and the last bubble left under the
    /// composer by whatever changed next.
    func testTheKeyboardRisingUnderAReaderAtTheEndKeepsThemThere() {
        XCTAssertEqual(TranscriptScrollLatch.settled(grew: false, oldContainer: 778, newContainer: 477,
                                                     distance: 364, pinned: true),
                       .stick,
                       "they were following the end before the resize and still are")

        // A reader who had gone up into history keeps their place through the same rise.
        XCTAssertEqual(TranscriptScrollLatch.settled(grew: false, oldContainer: 778, newContainer: 477,
                                                     distance: 900, pinned: false),
                       .reread(away: true))
    }

    /// The keyboard opening (tester report, 2026-11-14: "I'd prefer it just stays where you
    /// were"). At the end it still carries the reader to the newest row, which is #3051; up
    /// in history it leaves them where they are.
    ///
    /// "At the end" is the measurement and takes no latch: a reader the chevron brought to
    /// the end while a flick was still coasting is left `away` by that flick's last sample,
    /// and is at the end all the same.
    func testTheKeyboardOpeningFollowsTheEndForAReaderMeasuredThere() {
        let slack = TranscriptScrollLatch.keyboardSlack
        func follows(_ fromEnd: CGFloat) -> Bool {
            // The reader's own scrolling put them here: touched, nothing animating, and the
            // latch in either state.
            let pinned = TranscriptScrollLatch.keyboardFollowsEnd(fromEnd: fromEnd, pinned: true, held: true, animating: false, coastingToEnd: false, revealing: false)
            let unpinned = TranscriptScrollLatch.keyboardFollowsEnd(fromEnd: fromEnd, pinned: false, held: true, animating: false, coastingToEnd: false, revealing: false)
            XCTAssertEqual(pinned, unpinned, "the latch does not change the answer at \(fromEnd)")
            return pinned
        }
        XCTAssertTrue(follows(0), "at rest on the newest row")
        XCTAssertTrue(follows(slack), "the newest row still clear of the fade over the composer")
        XCTAssertFalse(follows(slack + 1), "scrolled up past that: keep the place")
        XCTAssertFalse(follows(44), "one short bubble up is already past it")
        XCTAssertFalse(follows(900), "far up, or taken to an old message by a reveal")
        XCTAssertEqual(slack, Theme.transcriptTail, "the slack is the transcript's own tail, not a device's number")
    }

    /// What the reporter samples rests on the top inset at the end, so the rule is given the
    /// distance with that taken off and means the same on every phone.
    func testTheDistanceToTheEndDoesNotIncludeTheTopInset() {
        for inset in [CGFloat(20), 47, 59, 62] {
            XCTAssertEqual(TranscriptScrollLatch.fromEnd(sampled: inset, topInset: inset), 0,
                           "at rest on the end with a \(inset) pt inset")
            XCTAssertEqual(TranscriptScrollLatch.fromEnd(sampled: inset + 300, topInset: inset), 300)
        }
    }

    /// A scroll to the end that code started is animated, and half way through it the
    /// measurement is a number from the middle of history. The keyboard follows where it is
    /// going: the chevron's hop, a send, a tall message arriving under a reader at the end.
    func testTheKeyboardOpeningFollowsAScrollToTheEndThatIsStillInFlight() {
        XCTAssertTrue(TranscriptScrollLatch.keyboardFollowsEnd(fromEnd: 1500, pinned: true, held: false, animating: false, coastingToEnd: false, revealing: false),
                      "the chevron pinned and the hop has not produced a sample yet")
        XCTAssertTrue(TranscriptScrollLatch.keyboardFollowsEnd(fromEnd: 700, pinned: true, held: false, animating: true, coastingToEnd: false, revealing: false),
                      "mid-hop")
        XCTAssertTrue(TranscriptScrollLatch.keyboardFollowsEnd(fromEnd: 90, pinned: true, held: true, animating: true, coastingToEnd: false, revealing: false),
                      "a reader who scrolled to the end themselves, mid-scroll to a tall new message")
        XCTAssertFalse(TranscriptScrollLatch.keyboardFollowsEnd(fromEnd: 700, pinned: false, held: false, animating: true, coastingToEnd: false, revealing: false),
                       "a reveal animates too, and is not going to the end")
        XCTAssertFalse(TranscriptScrollLatch.keyboardFollowsEnd(fromEnd: 44, pinned: true, held: true, animating: false, coastingToEnd: false, revealing: false),
                       "pinned by the latch's wider threshold, by their own scrolling, and at rest: not in flight")
        XCTAssertTrue(TranscriptScrollLatch.keyboardFollowsEnd(fromEnd: 44, pinned: true, held: true, animating: false, coastingToEnd: true, revealing: false),
                      "the same 44 pt, but their flick is still carrying them to the end")
        XCTAssertFalse(TranscriptScrollLatch.keyboardFollowsEnd(fromEnd: 400, pinned: false, held: true, animating: false, coastingToEnd: true, revealing: false),
                       "a flick toward the end that is still far out has not arrived")
    }

    /// Below iOS 18 nothing measures, the size-change anchor cannot be given up, and the
    /// bottom anchor moves the reader to the end on its own. Saying "keep" there would leave
    /// them at the end with a latch that says they are not.
    func testWithNothingMeasuredTheKeyboardFollowsTheEndAsItAlwaysDid() {
        XCTAssertTrue(TranscriptScrollLatch.keyboardFollowsEnd(fromEnd: nil, pinned: false, held: true, animating: false, coastingToEnd: false, revealing: false))
    }

    /// A reveal in progress is going to its row. Right after the row window slides the
    /// transcript sits on that window's end, which measures as the end; following from there
    /// would cancel the reveal.
    func testTheKeyboardOpeningDoesNotFollowTheEndDuringAReveal() {
        XCTAssertFalse(TranscriptScrollLatch.keyboardFollowsEnd(fromEnd: 0, pinned: false, held: false,
                                                                animating: false, coastingToEnd: false, revealing: true))
        XCTAssertFalse(TranscriptScrollLatch.keyboardFollowsEnd(fromEnd: 0, pinned: true, held: false,
                                                                animating: true, coastingToEnd: false, revealing: true))
    }

    /// The composer tapped while a flick toward the end is still coasting. Once the flick's
    /// own samples have latched the reader at the end, the keyboard's resize keeps them there
    /// rather than putting the offset back under it. Decided on the resize itself: a keyboard
    /// shown for the first time can arrive long after the tap.
    func testAKeyboardRisingOnACoastSticksOnlyForAReaderTheFlickHasLatchedAtTheEnd() {
        func rose(coasting: Bool = true, from old: CGFloat = 778, to new: CGFloat = 477, pinned: Bool) -> Bool {
            TranscriptScrollLatch.keyboardRoseOnACoastAtTheEnd(coasting: coasting, oldContainer: old,
                                                               newContainer: new, pinned: pinned)
        }
        XCTAssertTrue(rose(pinned: true), "arrived: stay on the newest row")
        XCTAssertFalse(rose(pinned: false), "still in history: the offset is put back as before")
        XCTAssertFalse(rose(coasting: false, pinned: true), "a finger on the glass owns the offset")
        XCTAssertFalse(rose(from: 477, to: 778, pinned: true),
                       "the keyboard leaving is the reader's own drag finishing: #3034, untouched")
        XCTAssertFalse(rose(from: 778, to: 778, pinned: true), "no resize, nothing to decide")
    }

    /// A keyboard dragged part of the way down and let go comes back up during the coast.
    /// The reader is 250 pt up in history; the raw sample, taken against the container the
    /// drag had enlarged, says 112, inside the threshold. The latch is measured against the
    /// container the gesture began with and says away, and it is the latch that decides.
    func testAnAbortedKeyboardDragDoesNotSnapAReaderInHistoryToTheEnd() {
        let keyboardUp: CGFloat = 477, pulledDown: CGFloat = 677
        let raw: CGFloat = 250 + 62 - (pulledDown - keyboardUp)
        XCTAssertEqual(raw, 112)
        XCTAssertFalse(TranscriptScrollLatch.away(distance: raw), "the raw sample is the trap")
        let latched = TranscriptScrollLatch.distance(sampled: raw, container: pulledDown, gestureStart: keyboardUp)
        XCTAssertTrue(TranscriptScrollLatch.away(distance: latched), "the latch has them in history")
        XCTAssertFalse(TranscriptScrollLatch.keyboardRoseOnACoastAtTheEnd(
            coasting: true, oldContainer: pulledDown, newContainer: keyboardUp,
            pinned: !TranscriptScrollLatch.away(distance: latched)))
    }

    /// A chevron tap or a send during a coast clears `held`; what is left of the coast must
    /// not write the latch back. A finger on the glass always may.
    func testWhatIsLeftOfACoastDoesNotUndoAPinThatCodeTook() {
        XCTAssertTrue(TranscriptScrollLatch.coastOvertaken(coasting: true, held: false))
        XCTAssertFalse(TranscriptScrollLatch.coastOvertaken(coasting: true, held: true), "an ordinary coast")
        XCTAssertFalse(TranscriptScrollLatch.coastOvertaken(coasting: false, held: false), "a finger, or nothing moving")
    }

    /// A sample where nothing moved must not disturb the latch — the reporter fires on every
    /// scroll sample, so "read it again" has to mean "when something moved it".
    func testASampleWithNothingMovingLeavesTheAnswerAlone() {
        XCTAssertEqual(TranscriptScrollLatch.settled(grew: false, oldContainer: 874,
                                                     newContainer: 874, distance: 400, pinned: false),
                       .hold)
    }

    // MARK: Who owns the offset during a resize

    /// The line that lets the fix be narrow: a container resize is only put back when the
    /// reader's own finger is on the scroll view.
    ///
    /// Leg 42604426988 is why this does not ask `away` instead. The keyboard starts retracting
    /// as soon as the finger moves, when the reader is a few points in and under the 120 pt
    /// threshold, so a condition on `away` reads `false` at exactly the instant that matters
    /// and the resize goes unhandled — which is how the conditional size-change anchor failed.
    /// Who is dragging is knowable then; how far they have got is not yet.
    func testOnlyAResizeUnderTheReadersOwnFingerIsPutBack() {
        let withKeyboard: CGFloat = 874 - 336

        XCTAssertTrue(TranscriptScrollLatch.resizedUnderTheFinger(userDriven: true,
                                                                  oldContainer: withKeyboard,
                                                                  newContainer: 874),
                      "the keyboard retracting out of the reader's own drag moves content they hold")

        // The guards depend on this one. The keyboard rising or falling at the live bottom is
        // a tap, not a drag, so the bottom anchor keeps doing its job and a chat still opens
        // and stays on its newest row.
        XCTAssertFalse(TranscriptScrollLatch.resizedUnderTheFinger(userDriven: false,
                                                                   oldContainer: withKeyboard,
                                                                   newContainer: 874),
                       "the same resize with nobody dragging is the anchor's to handle")

        XCTAssertFalse(TranscriptScrollLatch.resizedUnderTheFinger(userDriven: true,
                                                                   oldContainer: 874,
                                                                   newContainer: 874),
                       "a drag with no resize in it needs nothing put back")
    }

    /// Content growth keeps the behaviour it had: follow the end for a reader held at it,
    /// and leave a reader in history where they are.
    func testContentGrowthStillSticksForAReaderAtTheEndAndNotForOneInHistory() {
        XCTAssertEqual(TranscriptScrollLatch.settled(grew: true, oldContainer: 874, newContainer: 874,
                                                     distance: 0, pinned: true),
                       .stick)
        XCTAssertEqual(TranscriptScrollLatch.settled(grew: true, oldContainer: 874, newContainer: 874,
                                                     distance: 400, pinned: false),
                       .reread(away: true))
    }
}
