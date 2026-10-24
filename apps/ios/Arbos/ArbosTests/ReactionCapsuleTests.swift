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

    func testMenuUnderTheCapsuleCarriesNoEmoji() {
        XCTAssertEqual(MessageMenu.items.last, .delete)
        for item in MessageMenu.items {
            XCTAssertNil(ReactionCapsule.pickedEmoji(item.label), "\(item) label is an emoji")
        }
    }
}
