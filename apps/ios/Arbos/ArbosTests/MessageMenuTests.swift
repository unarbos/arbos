import XCTest
@testable import Arbos

/// The bubble's long-press menu is Copy / Reply / Forward / Reply in DM /
/// Delete (#1370, #1472, forward feedback), plus View thread when the message
/// has replies, Edit on the user's own text, and Pin / Unpin in a group whose
/// policy lets this member pin (#1547) — no emoji row, no Mention; a
/// non-message row copies only.
final class MessageMenuTests: XCTestCase {
    func testItemsAreExactlyCopyReplyForwardReplyInDMDelete() {
        XCTAssertEqual(MessageMenu.items, [.copy, .reply, .forward, .replyInDM, .delete])
        XCTAssertEqual(MessageMenu.items.map(\.label), ["Copy", "Reply", "Forward", "Reply in DM", "Delete"])
        XCTAssertEqual(MessageMenu.items(forMessage: true), MessageMenu.items)
        XCTAssertEqual(MessageMenu.items(forMessage: true, canDelete: true, hasReplies: true), MessageMenu.itemsWithThread)
        XCTAssertEqual(MessageMenu.itemsWithThread.map(\.label),
                       ["Copy", "Reply", "View thread", "Forward", "Reply in DM", "Delete"])
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
        XCTAssertEqual(
            MessageMenu.items(forMessage: true, canEdit: true, canDelete: true, hasReplies: true),
            [.copy, .reply, .viewThread, .forward, .replyInDM, .edit, .delete]
        )
    }

    func testNoEmojiOrExtraItems() {
        for item in MessageMenuItem.allCases {
            XCTAssertTrue(item.label.unicodeScalars.allSatisfy { $0.isASCII }, "\(item) label carries non-ASCII text")
            XCTAssertFalse(MessageReaction.palette.contains(item.label))
        }
        XCTAssertEqual(MessageMenuItem.allCases.count, 10)
    }

    /// #1547: Pin only where the group's policy lets this member pin, and the
    /// held message decides whether Pin or Unpin is offered.
    func testPinAppearsOnlyWhenTheMemberMayPin() {
        XCTAssertEqual(
            MessageMenu.items(forMessage: true, canPin: true),
            [.copy, .reply, .forward, .replyInDM, .pin, .delete]
        )
        XCTAssertEqual(
            MessageMenu.items(forMessage: true, canPin: true, isPinned: true),
            [.copy, .reply, .forward, .replyInDM, .unpin, .delete]
        )
        XCTAssertEqual(MessageMenu.items(forMessage: true, canPin: false), MessageMenu.items)
        // isPinned alone never shows an item: the policy decides.
        XCTAssertEqual(MessageMenu.items(forMessage: true, isPinned: true), MessageMenu.items)
        XCTAssertEqual(MessageMenu.items(forMessage: false, canPin: true, isPinned: true), [.copy])
    }

    /// Pin sits between Edit / Remove preview and Delete, so Delete stays last.
    func testPinSitsJustBeforeDelete() {
        let items = MessageMenu.items(forMessage: true, canEdit: true, canDelete: true,
                                      hasReplies: true, canPin: true)
        XCTAssertEqual(items, [.copy, .reply, .viewThread, .forward, .replyInDM, .edit, .pin, .delete])
        XCTAssertEqual(items.last, .delete)
    }

    func testOnlyDeleteIsDestructive() {
        XCTAssertEqual(MessageMenuItem.allCases.filter(\.isDestructive), [.delete])
    }

    func testNonMessageRowsCopyOnly() {
        XCTAssertEqual(MessageMenu.items(forMessage: false), [.copy])
        XCTAssertEqual(MessageMenu.items(forMessage: false, canEdit: true), [.copy])
    }

    /// #1871: already in a private / 1:1 chat — no Reply in DM (it would open the same chat).
    func testPrivateChatOmitsReplyInDM() {
        XCTAssertEqual(
            MessageMenu.items(forMessage: true, inPrivateChat: true),
            [.copy, .reply, .forward, .delete]
        )
        XCTAssertEqual(
            MessageMenu.items(forMessage: true, canEdit: true, canDelete: true, hasReplies: true, inPrivateChat: true),
            [.copy, .reply, .viewThread, .forward, .edit, .delete]
        )
        XCTAssertFalse(MessageMenu.items(forMessage: true, inPrivateChat: true).contains(.replyInDM))
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
