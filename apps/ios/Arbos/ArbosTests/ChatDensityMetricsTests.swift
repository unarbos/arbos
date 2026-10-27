import SwiftUI
import XCTest
@testable import Arbos

/// Chat density in people chats ([feedback chat-density-ios-2026-10-26], Jacob
/// 2026-10-26: "the pfp chat bubble takes up too much space — go for Discord
/// standard size and use more of the screen both ways, left and right per chat
/// bubble"). The numbers are pinned here; the drawing reads them from `Theme`.
/// Screen: iPhone 17, 402 pt, 3 px/pt; the transcript list pads 16 pt each side.
final class ChatDensityMetricsTests: XCTestCase {
    private let screen: CGFloat = 402
    private let listPad: CGFloat = 16

    func testSenderAvatarIsDiscordStandardAndTheGapIsTheControlGap() {
        // Discord: 40 pt in a 72 pt gutter (desktop cozy), 32 pt on mobile; 36 is the pick.
        XCTAssertEqual(Theme.memberAvatar, 36)
        XCTAssertEqual(Theme.memberAvatarGap, Controls.gap)
        XCTAssertEqual(Theme.memberAvatarGap, 10)
        XCTAssertLessThan(Theme.memberAvatar, 42, "smaller than the 42 pt disc Jacob saw")
    }

    func testTheAvatarColumnStaysCentredOnThePlusAndSharesTheStatusPicturesLeftEdge() {
        for composerInset in [Theme.composerInset, Theme.composerInsetKeyboard] {
            let avatarLeft = listPad + Theme.avatarColumnInset(composerInset: composerInset)
            let plusCenter = composerInset + Theme.plusButton / 2
            XCTAssertEqual(avatarLeft + Theme.memberAvatar / 2, plusCenter, accuracy: 0.001,
                           "inset \(composerInset): the disc is centred on the `+`")
            // A 36 disc centred on the 48 `+` starts 6 inside it — exactly where the
            // status row's picture starts (`presenceAvatarOffset`): one left edge for
            // every picture in the chat.
            XCTAssertEqual(avatarLeft, composerInset + Theme.presenceAvatarOffset, accuracy: 0.001)
            XCTAssertEqual(avatarLeft, listPad + Theme.presenceInset(composerInset: composerInset), accuracy: 0.001)
        }
        // Keyboard up: disc 24–60, body at 70 (was 21–63 / 76). Down: 37–73, body at 83 (was 34–76 / 89).
        XCTAssertEqual(listPad + Theme.avatarColumnInset(composerInset: Theme.composerInsetKeyboard), 24)
        XCTAssertEqual(listPad + Theme.memberLane(composerInset: Theme.composerInsetKeyboard), 70)
        XCTAssertEqual(listPad + Theme.avatarColumnInset(composerInset: Theme.composerInset), 37)
        XCTAssertEqual(listPad + Theme.memberLane(composerInset: Theme.composerInset), 83)
    }

    func testPeopleChatBubblesKeepAThirtyTwoPointFarMarginAndAgentChatsKeepGroks() {
        XCTAssertEqual(Theme.bubbleOppositeMargin, 32)
        XCTAssertEqual(Theme.peopleBubbleInset, 48)
        XCTAssertEqual(Theme.bubbleInset, 76, "agent 1:1 chats keep Grok's screen − 76")
        XCTAssertEqual(Theme.bubbleInset(people: true), 48)
        XCTAssertEqual(Theme.bubbleInset(people: false), 76)

        // My bubble: right edge 16 from the screen, at most 354 wide → left edge ≥ 32.
        let mine = RowView.maxBubble(width: screen, people: true)
        XCTAssertEqual(mine, 354)
        XCTAssertEqual(screen - listPad - mine, Theme.bubbleOppositeMargin)
        XCTAssertEqual(RowView.maxBubble(width: screen, people: false), 326, "agent chat unchanged")

        // Someone else's bubble: after the lane, at most 300 wide → right edge ≤ 370.
        for composerInset in [Theme.composerInset, Theme.composerInsetKeyboard] {
            let lane = Theme.memberLane(composerInset: composerInset)
            let incoming = mine - lane
            let bodyLeft = listPad + lane
            XCTAssertEqual(bodyLeft + incoming, screen - Theme.bubbleOppositeMargin, accuracy: 0.001,
                           "inset \(composerInset): an incoming bubble may run to 32 from the right edge")
            XCTAssertGreaterThan(incoming, 253, "wider than before (253 keyboard down)")
        }
        XCTAssertEqual(mine - Theme.memberLane(composerInset: Theme.composerInsetKeyboard), 300)
        XCTAssertEqual(mine - Theme.memberLane(composerInset: Theme.composerInset), 287)
    }

    func testThePhotoBubbleWidthIsUntouchedByTheWiderCaps() {
        // #1840: a photo + caption bubble is at most 280 wide whatever the cap.
        let people = RowView.maxBubble(width: screen, people: true) - Theme.memberLane(composerInset: Theme.composerInset)
        XCTAssertEqual(min(280, people), 280)
    }
}
