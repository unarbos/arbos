import XCTest
@testable import Arbos

/// The reaction capsule is the quick-react set plus the add-emoji control; the
/// long-press menu under it is still Copy / Reply / Delete.
final class ReactionCapsuleTests: XCTestCase {
    func testPaletteIsQuickReactSetInOrder() {
        XCTAssertEqual(ReactionCapsule.palette, ["❤️", "👍🏽", "💪🏽", "👎🏻", "👌🏾", "🤝", "🔥", "💯"])
        XCTAssertEqual(MessageReaction.palette, ReactionCapsule.palette)
    }

    func testCapsuleWidthCountsTheAddControl() {
        XCTAssertEqual(ReactionCapsule.width, 2 * ReactionCapsule.padding + 9 * ReactionCapsule.cell)
        XCTAssertEqual(ReactionCapsule.height, 2 * ReactionCapsule.padding + ReactionCapsule.cell)
    }

    func testPickedEmojiTakesOnlyAnEmoji() {
        XCTAssertEqual(ReactionCapsule.pickedEmoji("🎉"), "🎉")
        XCTAssertEqual(ReactionCapsule.pickedEmoji("❤️ love"), "❤️")
        XCTAssertEqual(ReactionCapsule.pickedEmoji("👍🏽"), "👍🏽")
        XCTAssertNil(ReactionCapsule.pickedEmoji("ok"))
        XCTAssertNil(ReactionCapsule.pickedEmoji(""))
    }

    /// The emoji keyboard's symbol and keycap emoji are text-default characters it sends
    /// with U+FE0F (and U+20E3); the bare characters are not reactions.
    func testPickedEmojiTakesTheKeyboardsSymbolsAndKeycaps() {
        let symbols = ["©️", "®️", "™️", "‼️", "⁉️", "ℹ️", "⌨️",
                       "↔️", "↕️", "↖️", "↗️", "↘️", "↙️", "↩️", "↪️",
                       "#️⃣", "*️⃣", "0️⃣", "1️⃣", "2️⃣", "3️⃣", "4️⃣", "5️⃣", "6️⃣", "7️⃣", "8️⃣", "9️⃣"]
        for emoji in symbols {
            XCTAssertEqual(ReactionCapsule.pickedEmoji(emoji), emoji, "\(emoji) is a reaction")
        }
        for text in ["1", "#", "*", "©", "™", "↔", "!", "?", "→"] {
            XCTAssertNil(ReactionCapsule.pickedEmoji(text), "\(text) is not a reaction")
        }
    }

    func testMenuUnderTheCapsuleCarriesNoEmoji() {
        XCTAssertEqual(MessageMenu.items.last, .delete)
        for item in MessageMenu.items {
            XCTAssertNil(ReactionCapsule.pickedEmoji(item.label), "\(item) label is an emoji")
        }
    }
}
