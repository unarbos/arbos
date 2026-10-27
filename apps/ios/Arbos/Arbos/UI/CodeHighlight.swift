// Syntax colouring for the code card: the tokens come from the shared Rust
// highlighter (`arbos_core::highlight` through ArbosFFI), so a fenced block is
// coloured exactly as on the Mac; this file maps them to the theme's palette,
// builds the attributed text, and places the indent guides
// ([feedback code-highlighting-ios-2026-10-26]).

import ArbosFFI
import SwiftUI
import UIKit

/// Grok Bot's six code colours plus the indent guide, per theme (sampled from
/// Jacob's 2026-10-26 reference; the Mac's `Theme::code_*`;
/// `docs/mac-design-rules.md` § Code cards).
struct CodePalette: Equatable {
    let plain: Color
    let keyword: Color
    let function: Color
    let string: Color
    let number: Color
    let comment: Color
    let guide: Color

    static let dark = CodePalette(
        plain: Color(hex: "#D6D6DD")!, keyword: Color(hex: "#95D0CD")!, function: Color(hex: "#E6B387")!,
        string: Color(hex: "#D898D8")!, number: Color(hex: "#F1C972")!, comment: Color(hex: "#626262")!,
        guide: Color.white.opacity(0.07))
    static let light = CodePalette(
        plain: Color(hex: "#1F1F23")!, keyword: Color(hex: "#1F7F7A")!, function: Color(hex: "#B8641A")!,
        string: Color(hex: "#A63FA4")!, number: Color(hex: "#9A6F00")!, comment: Color(hex: "#8C8C8C")!,
        guide: Color.black.opacity(0.09))

    /// The colour a token paints in (`comment` is also italic).
    func color(_ kind: HighlightKind) -> Color {
        switch kind {
        case .plain: return plain
        case .keyword: return keyword
        case .function: return function
        case .string: return string
        case .number: return number
        case .comment: return comment
        }
    }
}

extension Theme {
    var codePalette: CodePalette { isDark ? .dark : .light }
}

enum CodeHighlight {
    /// The card's mono size at Text Size 1 (the Mac's 12.5 pt reads the same at 3×).
    static let fontSize: CGFloat = 14
    /// Body insets; the right inset leaves room for the copy button.
    static let padX: CGFloat = 10
    static let padY: CGFloat = 10
    static let padRight: CGFloat = 40
    static let copySize: CGFloat = 24
    static let copyInset: CGFloat = 8

    /// Load the grammars (≈ 15 ms) off the main thread once at launch.
    static func warm() {
        Task.detached(priority: .utility) { warmHighlighter() }
    }

    /// The shared highlighter's runs for `code`, cached per (tag, code).
    static func tokens(lang: String, code: String) -> [HighlightToken] {
        let key = "\(lang)\u{1}\(code)" as NSString
        if let hit = cache.object(forKey: key) { return hit.tokens }
        let tokens = highlightCode(lang: lang, code: code)
        cache.setObject(TokenBox(tokens), forKey: key)
        return tokens
    }

    private final class TokenBox {
        let tokens: [HighlightToken]
        init(_ tokens: [HighlightToken]) { self.tokens = tokens }
    }

    private static let cache: NSCache<NSString, TokenBox> = {
        let c = NSCache<NSString, TokenBox>()
        c.countLimit = 512
        return c
    }()

    /// `code` as attributed text: each run in the palette colour of its kind,
    /// comments italic; plain code (no tokens) in `plain`.
    static func attributed(lang: String, code: String, palette: CodePalette, font: Font, plain: Color) -> AttributedString {
        let tokens = tokens(lang: lang, code: code)
        guard !tokens.isEmpty else {
            var whole = AttributedString(code)
            whole.swiftUI.foregroundColor = plain
            whole.swiftUI.font = font
            return whole
        }
        var out = AttributedString()
        let utf8 = code.utf8
        var cursor = utf8.startIndex
        for t in tokens {
            let start = utf8.index(utf8.startIndex, offsetBy: Int(t.start), limitedBy: utf8.endIndex) ?? utf8.endIndex
            let end = utf8.index(utf8.startIndex, offsetBy: Int(t.end), limitedBy: utf8.endIndex) ?? utf8.endIndex
            if start > cursor {
                var gap = AttributedString(String(code[cursor..<start]))
                gap.swiftUI.foregroundColor = plain
                gap.swiftUI.font = font
                out.append(gap)
            }
            guard end > start else { continue }
            var piece = AttributedString(String(code[start..<end]))
            piece.swiftUI.foregroundColor = t.kind == .plain ? plain : palette.color(t.kind)
            piece.swiftUI.font = t.kind == .comment ? font.italic() : font
            out.append(piece)
            cursor = end
        }
        if cursor < utf8.endIndex {
            var tail = AttributedString(String(code[cursor...]))
            tail.swiftUI.foregroundColor = plain
            tail.swiftUI.font = font
            out.append(tail)
        }
        return out
    }

    /// The mono glyph advance and line height at `size` (SF Mono's 0.6 em; the
    /// guides and the scroll width are laid out from them).
    static func glyphAdvance(size: CGFloat) -> CGFloat {
        let font = UIFont.monospacedSystemFont(ofSize: size, weight: .regular)
        return ("M" as NSString).size(withAttributes: [.font: font]).width
    }

    static func lineHeight(size: CGFloat) -> CGFloat {
        UIFont.monospacedSystemFont(ofSize: size, weight: .regular).lineHeight
    }

    /// The width the code needs unwrapped: the longest line in glyph advances.
    /// Wider than the card → the body scrolls sideways. (`CodeHighlightTests` checks the
    /// bubble-sort card against the 321 pt bubble.)
    static func contentWidth(_ code: String, advance: CGFloat) -> CGFloat {
        let lines: [Substring] = code.split(omittingEmptySubsequences: false, whereSeparator: { $0 == "\n" })
        let longest = lines.map(\.count).max() ?? 0
        return CGFloat(longest) * advance
    }

    /// Where the indent guides go: `(line, x)` in points from the body's left
    /// inset, one per completed indent step on every non-blank line, a little
    /// past the middle of the step (Grok: column 2.6 of a 4-space step). The step
    /// is the block's smallest indent (tabs count 4), at least 2. The Mac's
    /// `indent_guides`.
    static func indentGuides(_ code: String, advance: CGFloat) -> [(line: Int, x: CGFloat)] {
        let lines: [Substring] = code.split(omittingEmptySubsequences: false, whereSeparator: { $0 == "\n" })
        func indent(_ line: Substring) -> Int {
            line.prefix { $0 == " " || $0 == "\t" }.reduce(0) { $0 + ($1 == "\t" ? 4 : 1) }
        }
        let unit = max(
            lines.filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
                .map(indent).filter { $0 > 0 }.min() ?? 4,
            2)
        var out: [(line: Int, x: CGFloat)] = []
        for (li, line) in lines.enumerated() {
            if line.trimmingCharacters(in: .whitespaces).isEmpty { continue }
            let steps = indent(line) / unit
            for k in 0..<steps {
                out.append((li, (CGFloat(k * unit) + 0.65 * CGFloat(unit)) * advance))
            }
        }
        return out
    }
}
