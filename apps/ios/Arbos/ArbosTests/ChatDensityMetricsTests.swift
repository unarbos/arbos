import SwiftUI
import XCTest
@testable import Arbos

/// Chat density in people chats ([feedback chat-density-ios-2026-10-26], Jacob
/// 2026-10-26: "the pfp chat bubble takes up too much space — go for Discord
/// standard size and use more of the screen both ways, left and right per chat
/// bubble"; [feedback group-chat-spacing-ios-2026-11-01], Jacob 2026-11-01:
/// "Fix the chat spacing so that it works like this in group chats. More width
/// and tighter"). The numbers are pinned here; the drawing reads them from `Theme`.
/// Screen: iPhone 17, 402 pt, 3 px/pt; the transcript list pads 16 pt each side.
/// Jacob's 2026-11-01 reference is an iPhone Air (420 pt) at 2.03 px/pt, so the
/// widths he measured against are pinned at 420 as well.
final class ChatDensityMetricsTests: XCTestCase {
    private let screen: CGFloat = 402
    private let air: CGFloat = 420
    private let listPad: CGFloat = 16

    func testSenderAvatarIsDiscordStandardAndTheGapToTheBubbleIsFour() {
        // Discord: 40 pt in a 72 pt gutter (desktop cozy), 32 pt on mobile; 36 is the pick.
        XCTAssertEqual(Theme.memberAvatar, 36)
        XCTAssertLessThan(Theme.memberAvatar, 42, "smaller than the 42 pt disc Jacob saw")
        // 2026-11-01: the gap left the control family (was `Controls.gap`, 10). His discs
        // end 48 pt from the screen and the bubbles start at 52.
        XCTAssertEqual(Theme.memberAvatarGap, 4)
        XCTAssertLessThan(Theme.memberAvatarGap, Controls.gap)
    }

    func testTheAvatarColumnIsFixedAtTwelveAndNoLongerFollowsTheComposer() {
        // 2026-11-01: the column was centred on the composer's `+` and moved with the
        // keyboard; it is now 12 pt from the screen in both states, so an incoming
        // bubble keeps its width when the keyboard opens.
        XCTAssertEqual(listPad + Theme.avatarColumnInset, 12, "the disc starts 12 pt from the screen")
        for composerInset in [Theme.composerInset, Theme.composerInsetKeyboard] {
            XCTAssertEqual(listPad + Theme.memberLane(composerInset: composerInset), 52,
                           "inset \(composerInset): the bubble body starts at 52")
        }
        // The disc reaches 4 pt back into the transcript's 16 pt pad, and the lane gives
        // the same 4 pt back, so a row still fits its container exactly.
        XCTAssertEqual(Theme.avatarColumnInset, -4)
        XCTAssertEqual(Theme.memberLane(composerInset: Theme.composerInset), 36)
        // The status row over the composer keeps its own rule: it still follows the `+`.
        XCTAssertEqual(listPad + Theme.presenceInset(composerInset: Theme.composerInset),
                       Theme.composerInset + Theme.presenceAvatarOffset, accuracy: 0.001)
    }

    func testPeopleChatBubblesKeepASixteenPointFarMarginAndAgentChatsKeepGroks() {
        XCTAssertEqual(Theme.bubbleOppositeMargin, 16, "was 32 before Jacob's 2026-11-01 shot")
        XCTAssertEqual(Theme.peopleBubbleInset, 32)
        XCTAssertEqual(Theme.bubbleInset, 76, "agent 1:1 chats keep Grok's screen − 76")
        XCTAssertEqual(Theme.bubbleInset(people: true), 32)
        XCTAssertEqual(Theme.bubbleInset(people: false), 76)

        // My bubble: right edge 16 from the screen, at most 370 wide → left edge ≥ 16.
        let mine = RowView.maxBubble(width: screen, people: true)
        XCTAssertEqual(mine, 370)
        XCTAssertEqual(screen - listPad - mine, Theme.bubbleOppositeMargin)
        XCTAssertEqual(RowView.maxBubble(width: screen, people: false), 326, "agent chat unchanged")

        // Someone else's bubble: after the lane, at most 334 wide → right edge ≤ 386.
        for composerInset in [Theme.composerInset, Theme.composerInsetKeyboard] {
            let lane = Theme.memberLane(composerInset: composerInset)
            let incoming = mine - lane
            let bodyLeft = listPad + lane
            XCTAssertEqual(bodyLeft + incoming, screen - Theme.bubbleOppositeMargin, accuracy: 0.001,
                           "inset \(composerInset): an incoming bubble may run to 16 from the right edge")
            XCTAssertEqual(incoming, 334, "inset \(composerInset): wider than the 287–300 it had")
        }
    }

    func testJacobsOwnScreenMatchesTheWidthsHeMeasured() {
        // iPhone Air, 420 pt: his bubbles run to 404 (16 from the right edge), his discs
        // span 12–48 and his incoming bubbles start at 52.
        let mine = RowView.maxBubble(width: air, people: true)
        XCTAssertEqual(mine, 388, "was 372")
        XCTAssertEqual(air - listPad - mine, 16)
        let bodyLeft = listPad + Theme.memberLane(composerInset: Theme.composerInset)
        XCTAssertEqual(bodyLeft, 52)
        XCTAssertEqual(mine - Theme.memberLane(composerInset: Theme.composerInset), 352, "was 305")
        XCTAssertEqual(bodyLeft + 352, air - 16)
    }

    func testThePhotoBubbleWidthIsUntouchedByTheWiderCaps() {
        // #1840: a photo + caption bubble is at most 280 wide whatever the cap.
        let people = RowView.maxBubble(width: screen, people: true) - Theme.memberLane(composerInset: Theme.composerInset)
        XCTAssertEqual(min(280, people), 280)
    }
}
