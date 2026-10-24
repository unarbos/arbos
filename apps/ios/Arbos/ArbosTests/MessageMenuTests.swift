import XCTest
@testable import Arbos

/// The bubble's long-press menu is Copy / Reply / Forward / Reply in DM /
/// Delete (#1370, #1472, forward feedback), plus Edit on the user's own text
/// messages — no emoji row, no Mention; a non-message row copies only.
final class MessageMenuTests: XCTestCase {
    func testItemsAreExactlyCopyReplyForwardReplyInDMDelete() {
        XCTAssertEqual(MessageMenu.items, [.copy, .reply, .forward, .replyInDM, .delete])
        XCTAssertEqual(MessageMenu.items.map(\.label), ["Copy", "Reply", "Forward", "Reply in DM", "Delete"])
        XCTAssertEqual(MessageMenu.items(forMessage: true), MessageMenu.items)
    }

    func testOwnMessageAddsEditBeforeDelete() {
        XCTAssertEqual(
            MessageMenu.items(forMessage: true, canEdit: true, canDelete: true),
            [.copy, .reply, .forward, .replyInDM, .edit, .delete]
        )
        XCTAssertEqual(
            MessageMenu.items(forMessage: true, canEdit: true, canDelete: true).map(\.label),
            ["Copy", "Reply", "Forward", "Reply in DM", "Edit", "Delete"]
        )
        XCTAssertEqual(
            MessageMenu.items(forMessage: true, canEdit: true, canDelete: false),
            [.copy, .reply, .forward, .replyInDM, .edit]
        )
    }

    func testNoEmojiOrExtraItems() {
        for item in MessageMenuItem.allCases {
            XCTAssertTrue(item.label.unicodeScalars.allSatisfy { $0.isASCII }, "\(item) label carries non-ASCII text")
            XCTAssertFalse(MessageReaction.palette.contains(item.label))
        }
        XCTAssertEqual(MessageMenuItem.allCases.count, 6)
    }

    func testOnlyDeleteIsDestructive() {
        XCTAssertEqual(MessageMenuItem.allCases.filter(\.isDestructive), [.delete])
    }

    func testNonMessageRowsCopyOnly() {
        XCTAssertEqual(MessageMenu.items(forMessage: false), [.copy])
        XCTAssertEqual(MessageMenu.items(forMessage: false, canEdit: true), [.copy])
    }

    /// feedback app-01M5MXQ90MEEWF6C8W090VCZHF: Delete asks first. My own message gets the
    /// plain title; a moderator sees whose message it is; the one action is for everyone.
    func testDeleteConfirmCopy() {
        XCTAssertEqual(MessageDeleteCopy.title(mine: true, sender: "Jacob"), "Delete message?")
        XCTAssertEqual(MessageDeleteCopy.title(mine: false, sender: "Sam Rivera"), "Delete Sam Rivera’s message?")
        XCTAssertEqual(MessageDeleteCopy.title(mine: false, sender: "  "), "Delete message?")
        XCTAssertTrue(MessageDeleteCopy.body.contains("for everyone"))
    }

    /// #1389: a long-press on any block of a bubble reaches the one menu, and
    /// its Copy is the whole message — every paragraph, the list, the code
    /// span — not the block under the finger. The fixture's "rules" reply is
    /// the shape of Jacob's bubble: five blocks, one text part.
    @MainActor func testCopyOfAMultiBlockBubbleIsTheWholeMessage() {
        let text = AppModel.longReply
        let blocks = Markdown.blocks(text)
        XCTAssertGreaterThanOrEqual(blocks.count, 5, "the fixture reply must span several blocks")
        XCTAssertTrue(blocks.contains { if case .bullets = $0 { return true } else { return false } })
        let m = Message(id: "m1", chatId: "c1", role: .assistant, sender: "agent", status: .done, turn: .reply, decision: nil,
                        subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: 0,
                        completedAt: 0, pending: false, parts: [.text(id: "m1-t", text: text)], hiddenCount: 0)
        XCTAssertEqual(m.plainText, text)
        for case .paragraph(let p) in blocks {
            XCTAssertTrue(m.plainText.contains(p), "copy misses paragraph: \(p)")
        }
    }
}
