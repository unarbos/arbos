import SwiftUI
import XCTest
@testable import Arbos

/// Emoji-only display names beside / over their `@handle`
/// ([feedback emoji-name-handle-offset-ios-2026-10-28]): the name's ink is raised by the
/// Apple Color Emoji cell's descent, the layout is not touched.
final class EmojiNameTests: XCTestCase {
    private let text = "Jacob"
    private let emoji = "🧙🏽\u{200d}♂\u{fe0f}"
    private let mixed = "Cato 🌳"

    func testEmojiOnlyKnowsEmojiFromText() {
        for name in [emoji, "🌳", "🌳🌲", "🙂 🙃", "👍🏽", "🇩🇪", "1️⃣", "❤️", "⭐️", "✨",
                     "👨\u{200d}👩\u{200d}👧", "🏴\u{e0067}\u{e0062}\u{e0073}\u{e0063}\u{e0074}\u{e007f}"] {
            XCTAssertTrue(EmojiName.isEmojiOnly(name), "\(name) is an emoji-only name")
        }
        for name in [text, mixed, "", " ", "@mage", "🌳!", "x🌳", "→", "1", "#", "✓ done", "❤"] {
            XCTAssertFalse(EmojiName.isEmojiOnly(name), "\(name.debugDescription) is not an emoji-only name")
        }
    }

    func testTheLiftIsTheEmojiCellsDescentAndOnlyForEmojiNames() {
        // Apple Color Emoji's cell: −0.25 em … +1.0 em (`CTRunGetImageBounds`, 15 pt: y −3.75, h 18.75).
        XCTAssertEqual(EmojiName.cellDescentEm, 0.25)
        XCTAssertEqual(EmojiName.lift(emoji, size: 17), 4.25)
        XCTAssertEqual(EmojiName.lift(emoji, size: 28), 7)
        XCTAssertEqual(EmojiName.lift(text, size: 17), 0)
        XCTAssertEqual(EmojiName.lift(mixed, size: 17), 0)
        XCTAssertEqual(EmojiName.lift("", size: 17), 0, "a row with no subtitle passes an empty name: no lift")
    }

    /// A name beside its handle (the home row) and over it (a member row, the hero): the
    /// lift is a visual offset, so the pair's layout size does not change when it is applied —
    /// the handle and the row stay where they were. (The line box itself may differ between
    /// an emoji and a text name on iOS — UIKit grows it for the emoji font — which is why the
    /// comparison is lifted vs. unlifted, not emoji vs. text.)
    @MainActor
    func testTheLiftChangesNoLayout() {
        func beside(_ name: String, lifted: Bool) -> some View {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(name).font(.system(size: 17, weight: .semibold)).lineLimit(1)
                    .emojiNameLift(lifted ? name : "", size: 17)
                Text("@dawizzaa").font(.system(size: 13)).lineLimit(1)
            }
            .fixedSize()
        }
        func over(_ name: String, lifted: Bool) -> some View {
            VStack(alignment: .leading, spacing: 2) {
                Text(name).font(.system(size: 17, weight: .medium)).lineLimit(1)
                    .emojiNameLift(lifted ? name : "", size: 17)
                Text("@dawizzaa").font(.system(size: 15)).lineLimit(1)
            }
            .fixedSize()
        }
        func size<V: View>(_ v: V) -> CGSize {
            let host = UIHostingController(rootView: v)
            host.view.layoutIfNeeded()
            return host.sizeThatFits(in: CGSize(width: 400, height: 400))
        }
        for name in [text, emoji, mixed] {
            XCTAssertEqual(size(beside(name, lifted: true)), size(beside(name, lifted: false)),
                           "beside: the lift leaves \(name)'s pair the same size")
            XCTAssertEqual(size(over(name, lifted: true)), size(over(name, lifted: false)),
                           "over: the lift leaves \(name)'s pair the same size")
        }
        XCTAssertGreaterThan(EmojiName.lift(emoji, size: 17), 0, "and the emoji name really is lifted")
    }
}
