import SwiftUI

/// Display names that are only emoji, drawn beside or over their `@handle`
/// ([feedback emoji-name-handle-offset-ios-2026-10-28], the Mac's `ui::identity::name`).
///
/// SwiftUI aligns a name and its handle on the text baseline (`.firstTextBaseline`) and
/// the layout is right: an emoji name's line box is a text name's line box. What differs
/// is the ink. Apple Color Emoji draws every glyph in one cell that hangs
/// `cellDescentEm` under the baseline and rises 1.0 em over it (`CTRunGetImageBounds` of a
/// 15 pt run: y −3.75, height 18.75), where SF's capitals end at 0.70 em. The handle sits
/// on the baseline, so beside an emoji it reads as hanging mid-glyph, with the emoji's
/// feet 0.25 em under the handle's bottom — Scout's report, 2026-10-28.
///
/// The fix moves the ink, not the layout: an emoji-only name is drawn raised by
/// `size × cellDescentEm` (a visual offset — the frame, the baseline guide the handle
/// aligns to and the row's height stay where a text name puts them), so the cell rests on
/// the baseline like a capital, the same bottom edge the handle has against text. Mixed
/// names ("Cato 🌳") are left as Core Text sets them; a lone centred name (the chat title)
/// keeps Core Text's placement, which already sits level with capitals.
enum EmojiName {
    /// How far the Apple Color Emoji cell hangs under the text baseline, in em.
    static let cellDescentEm: CGFloat = 0.25

    /// `true` when every grapheme of `name` is an emoji (skin tones, ZWJ sequences, flags,
    /// keycaps and VS16 included) — spaces between them allowed, nothing else.
    static func isEmojiOnly(_ name: String) -> Bool {
        var sawEmoji = false
        for ch in name {
            if ch.unicodeScalars.allSatisfy({ $0.properties.isWhitespace }) { continue }
            guard isEmojiGrapheme(ch) else { return false }
            sawEmoji = true
        }
        return sawEmoji
    }

    /// One grapheme cluster that renders as a single emoji: a scalar with emoji
    /// presentation by default (🌳, a flag's regional indicators), or a text-default
    /// scalar made emoji by what follows it (❤️ with VS16, 1️⃣ with the keycap, ☺️).
    static func isEmojiGrapheme(_ ch: Character) -> Bool {
        let scalars = Array(ch.unicodeScalars)
        guard let first = scalars.first else { return false }
        if first.properties.isEmojiPresentation { return true }
        guard first.properties.isEmoji, scalars.count > 1 else { return false }
        return scalars.dropFirst().contains { s in
            s.value == 0xFE0F || s.value == 0x20E3 || s.value == 0x200D
                || s.properties.isEmojiModifier || s.properties.isEmojiPresentation
        }
    }

    /// How far (pt) a name drawn at `size` is raised so an emoji cell rests on the text
    /// baseline: `size × cellDescentEm` for an emoji-only name, 0 for text and mixed names.
    static func lift(_ name: String, size: CGFloat) -> CGFloat {
        isEmojiOnly(name) ? size * cellDescentEm : 0
    }
}

extension View {
    /// Raise an emoji-only `name` drawn at `size` onto the text baseline (see `EmojiName`).
    /// A visual offset only: layout, alignment guides and the row's height are untouched,
    /// so the `@handle` beside or under the name stays where a text name has it.
    func emojiNameLift(_ name: String, size: CGFloat) -> some View {
        offset(y: -EmojiName.lift(name, size: size))
    }
}
