import UIKit
import XCTest
@testable import Arbos

/// Picking an emoji from the capsule's add-emoji keyboard adds exactly one reaction
/// ([feedback iphone-emoji-picker-reaction-2026-10-28]: the pick closed the overlay and
/// left nothing on the message). UIKit reports one keyboard insert to the hidden field
/// twice — `shouldChangeCharactersIn`, then `.editingChanged` — and both reports used to
/// toggle the reaction: on, then off again.
final class ReactionPickTests: XCTestCase {
    /// The two delegate reports UIKit sends for one typed character, in UIKit's order.
    private func typeOnKeyboard(_ emoji: String, into field: UITextField, coordinator: EmojiKeyboardField.Coordinator) {
        _ = coordinator.textField(field, shouldChangeCharactersIn: NSRange(location: 0, length: 0), replacementString: emoji)
        field.text = emoji
        coordinator.changed(field)
    }

    func testOneKeyboardInsertReportedTwicePicksOnce() {
        var picks: [String] = []
        let coordinator = EmojiKeyboardField.Coordinator { picks.append($0) }
        typeOnKeyboard("🔥", into: UITextField(), coordinator: coordinator)
        XCTAssertEqual(picks, ["🔥"])
    }

    func testOnlyTheFirstEmojiOfAFieldCounts() {
        var pick = ReactionPick()
        XCTAssertNil(pick.take(""), "nothing typed yet")
        XCTAssertNil(pick.take("ok"), "letters are not a reaction")
        XCTAssertEqual(pick.take("🔥"), "🔥")
        XCTAssertNil(pick.take("🔥"), "the same insert reported again")
        XCTAssertNil(pick.take("🎉"), "a later emoji in the same field")
        XCTAssertEqual(pick.emoji, "🔥")
    }

    /// The picked emoji, driven through the field exactly as the keyboard drives it, ends
    /// up as one reaction on the chat doc row — the row the relay syncs and every device
    /// reads back.
    func testPickedFireReachesTheDocRowOnce() throws {
        let doc = CoreChatDoc(key: "react-pick")
        try doc.insertUserMessage(id: "m1", text: "ship it", deviceId: "dev", author: .init(sender: "user:ken"))
        let coordinator = EmojiKeyboardField.Coordinator { emoji in
            try? doc.toggleReaction(messageId: "m1", emoji: emoji, sender: "user:me")
            doc.commit()
        }
        typeOnKeyboard("🔥", into: UITextField(), coordinator: coordinator)

        let row = try XCTUnwrap(doc.readAll().first)
        XCTAssertEqual(row.reactions.map(\.emoji), ["🔥"])
        XCTAssertEqual(row.reactions.first?.senders, ["user:me"])
    }

    /// What the bug did: the same pick toggled twice leaves the row empty.
    func testTwoTogglesOfOnePickWouldClearTheRow() throws {
        let doc = CoreChatDoc(key: "react-pick-twice")
        try doc.insertUserMessage(id: "m1", text: "ship it", deviceId: "dev", author: .init(sender: "user:ken"))
        try doc.toggleReaction(messageId: "m1", emoji: "🔥", sender: "user:me")
        try doc.toggleReaction(messageId: "m1", emoji: "🔥", sender: "user:me")
        doc.commit()
        XCTAssertTrue(try XCTUnwrap(doc.readAll().first).reactions.isEmpty)
    }
}
