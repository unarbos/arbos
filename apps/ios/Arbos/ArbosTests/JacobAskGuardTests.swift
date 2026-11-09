import SwiftUI
import XCTest
@testable import Arbos

/// Guards for the Jacob asks that had no test of their own.
///
/// Every `[feedback …]` tag in the app is one of Jacob's asks. Most already have a dedicated
/// suite — `ListTopFadeTests`, `GroupedMessagesTests`, `PresenceRowTests` and the rest — and the
/// audit in `docs/product/jacob-asks.md` found these had nothing holding them at all. An ask with
/// no test is an ask that regresses silently and is noticed in a screenshot weeks later, which is
/// the failure this file exists to stop.
///
/// Each case names its tag, so `scripts/ci/jacob-asks-check.py` can tie registry entry to guard.
final class JacobAskGuardTests: XCTestCase {
    /// [feedback agent-bubble-inset-2026-10-22] — the agent bubble leaves 76 pt, not the 71 pt it
    /// was. Measured off Grok dark at 3 px/pt: 16 pt own margin + 60 pt on the far side.
    func testTheAgentBubbleKeepsItsSeventySixPointInset() {
        XCTAssertEqual(Theme.bubbleInset, 76, accuracy: 0.001,
                       "the agent bubble inset is the measured Grok number; 71 was the old one")
    }

    /// [feedback in-call-bar] — the in-call strip's chip is a recorded exception to the one
    /// app-wide control height, so a sweep that unifies control sizes must not quietly take it.
    func testTheInCallChipKeepsItsRecordedException() {
        XCTAssertEqual(Theme.voiceButton, 36, accuracy: 0.001,
                       "the in-call chip is 36 pt by exception, not Controls.height")
        XCTAssertNotEqual(Theme.voiceButton, Theme.fieldHeight,
                          "if these ever match, the exception has been absorbed by a sweep")
    }

    /// [feedback mention-chip-hover-2026-10-24] — a mention chip is accent on accent at 14 %, and
    /// 30 % while the finger is down. The two must stay distinct or the chip stops answering touch.
    func testTheMentionChipStillAnswersTheFinger() {
        XCTAssertEqual(ChipLinkStyle.restAlpha, 0.14, accuracy: 0.001)
        XCTAssertEqual(ChipLinkStyle.pressedAlpha, 0.30, accuracy: 0.001)
        XCTAssertGreaterThan(ChipLinkStyle.pressedAlpha, ChipLinkStyle.restAlpha,
                             "pressed must read heavier than rest, or the chip looks dead")
    }

    /// [feedback connection-pill] — when several signals disagree the worst one is shown, and a
    /// healthy link says nothing at all. Ordering is the whole contract.
    func testTheConnectionPillShowsTheWorstSignalAndStaysQuietWhenLive() {
        let worstFirst: [LinkState] = [.offline, .relayDown, .reconnecting, .syncing, .restartSoon, .live]
        for (worse, better) in zip(worstFirst, worstFirst.dropFirst()) {
            XCTAssertGreaterThan(worse, better, "\(worse) must outrank \(better) in the pill")
        }
        XCTAssertNil(LinkState.live.label, "a live link has nothing to say")
        for state in worstFirst where state != .live {
            XCTAssertNotNil(state.label, "\(state) must give the pill something to show")
        }
    }

    /// [feedback math-render-ios-2026-10-26] — display math on its own line, in either delimiter,
    /// reaches the transcript as a math block rather than as literal dollars and brackets.
    func testDisplayMathSurvivesBothDelimiters() {
        for source in ["$$x^2 + y^2$$", "\\[x^2 + y^2\\]"] {
            let blocks = Markdown.blocks(source)
            let isMath = blocks.contains { if case .math = $0 { return true }; return false }
            XCTAssertTrue(isMath, "\(source) must parse to a math block, got \(blocks)")
        }
    }

    /// [feedback blocked-state-relay-2026-10-26] — the relay's refusal reason reaches the chat as
    /// a sentence a person can read, and the machine code is carried through untouched.
    func testARefusedPushExplainsItselfInWords() {
        let why = PushRefusalWhy(code: "not_available", reason: "blocked",
                                 byHandle: "sam", byName: "Sam Okafor")
        XCTAssertEqual(why.code, "not_available", "the code the relay sent is never rewritten")
        XCTAssertFalse(why.sentence().isEmpty, "a refusal the chat cannot explain is a blank bubble")
    }
}
