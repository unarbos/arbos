import ArbosFFI
import SwiftUI
import UIKit
import XCTest
@testable import Arbos

/// Jacob's "write bubble sort" card ([feedback code-highlighting-ios-2026-10-26]): the
/// shared Rust highlighter classifies the tokens, the theme palette colours them (Grok's
/// six), guides sit one per indent step, long lines scroll, Copy puts the block on the
/// pasteboard. The classification vectors match `crates/arbos-core/src/highlight.rs`.
final class CodeHighlightTests: XCTestCase {
    static let bubbleSort = """
    def bubble_sort(arr):
        n = len(arr)
        for i in range(n):
            swapped = False
            for j in range(0, n - i - 1):
                if arr[j] > arr[j + 1]:
                    arr[j], arr[j + 1] = arr[j + 1], arr[j]
                    swapped = True
            # If no two elements swapped, list is already sorted
            if not swapped:
                break
        return arr

    # Example usage:
    items = [64, 34, 25, 12, 22, 11, 90]
    print(bubble_sort(items))  # Output: [11, 12, 22, 25, 34, 64, 90]
    """

    /// The kind of the `nth` occurrence of `needle` in `code` (one token must cover it).
    private func kind(_ lang: String, _ code: String, _ needle: String, nth: Int = 0) -> HighlightKind? {
        let tokens = CodeHighlight.tokens(lang: lang, code: code)
        var ranges: [Range<String.Index>] = []
        var from = code.startIndex
        while let r = code.range(of: needle, range: from..<code.endIndex) {
            ranges.append(r); from = r.upperBound
        }
        guard nth < ranges.count else { return nil }
        let start = code.utf8.distance(from: code.utf8.startIndex, to: ranges[nth].lowerBound)
        let end = code.utf8.distance(from: code.utf8.startIndex, to: ranges[nth].upperBound)
        return tokens.first { Int($0.start) <= start && end <= Int($0.end) }?.kind
    }

    func testTheBubbleSortCardIsClassifiedLikeGrok() {
        let c = Self.bubbleSort
        XCTAssertEqual(kind("python", c, "def"), .keyword)
        XCTAssertEqual(kind("python", c, "bubble_sort"), .function)
        XCTAssertEqual(kind("python", c, "len"), .function)
        XCTAssertEqual(kind("python", c, "range"), .function)
        XCTAssertEqual(kind("python", c, "for"), .keyword)
        XCTAssertEqual(kind("python", c, " in "), .plain, "Grok paints `in` plain")
        XCTAssertEqual(kind("python", c, "False"), .keyword)
        XCTAssertEqual(kind("python", c, "True"), .keyword)
        XCTAssertEqual(kind("python", c, "not"), .keyword)
        XCTAssertEqual(kind("python", c, "64"), .number)
        XCTAssertEqual(kind("python", c, "# If no two"), .comment)
        XCTAssertEqual(kind("python", c, "print"), .function)
        XCTAssertEqual(kind("python", c, "bubble_sort", nth: 1), .function)
        XCTAssertEqual(kind("python", c, "swapped = "), .plain)
        // Contiguous, covering, merged.
        let tokens = CodeHighlight.tokens(lang: "python", code: c)
        XCTAssertEqual(tokens.first?.start, 0)
        XCTAssertEqual(Int(tokens.last?.end ?? 0), c.utf8.count)
        for (a, b) in zip(tokens, tokens.dropFirst()) {
            XCTAssertEqual(a.end, b.start)
            XCTAssertNotEqual(a.kind, b.kind)
        }
    }

    func testEveryLanguageClassifiesItsLandmarks() {
        let cases: [(String, String, [(String, HighlightKind)])] = [
            ("js", "const add = (a, b) => a + b;\nconsole.log(add(1, 2), true); // done",
             [("const", .keyword), ("add", .function), ("log", .function), ("1", .number), ("true", .keyword), ("// done", .comment)]),
            ("ts", "interface P { x: number }\nconst f = (p: P): string => `v${p.x}`;", [("const", .keyword), ("`v", .string)]),
            ("swift", "struct Greeter {\n    let name: String\n    func greet() -> String { return \"Hi \\(name)\" } // hi\n}\nprint(Greeter(name: \"J\").greet())",
             [("struct", .keyword), ("Greeter", .function), ("let", .keyword), ("func", .keyword), ("greet", .function), ("\"Hi \\(name)\"", .string), ("// hi", .comment), ("print", .function)]),
            ("rust", "fn main() {\n    let mut v = vec![1];\n    println!(\"hi {}\", v.len()); // end\n}",
             [("fn", .keyword), ("main", .function), ("let", .keyword), ("vec!", .function), ("1", .number), ("\"hi {}\"", .string), ("len", .function), ("// end", .comment)]),
            ("go", "func main() {\n    x := len(os.Args)\n    fmt.Println(\"hi\", x, true) // out\n}",
             [("func", .keyword), ("len", .function), ("Println", .function), ("\"hi\"", .string), ("true", .keyword), ("// out", .comment)]),
            ("bash", "#!/bin/bash\nfor f in *.txt; do\n  echo \"file $f\" | grep -c foo\ndone",
             [("#!/bin/bash", .comment), ("for", .keyword), ("do", .keyword), ("echo", .function), ("\"file ", .string), ("grep", .function), ("done", .keyword)]),
            ("json", "{\n  \"name\": \"arbos\",\n  \"count\": 3,\n  \"ok\": true\n}", [("\"name\"", .string), ("3", .number), ("true", .keyword), ("{", .plain)]),
            ("sql", "SELECT name, COUNT(*) FROM users WHERE age > 21; -- top",
             [("SELECT", .keyword), ("COUNT", .function), ("FROM", .keyword), ("21", .number), ("-- top", .comment), ("users", .plain)]),
            ("c", "#include <stdio.h>\nint main(void) {\n    printf(\"%d\\n\", 42); // hi\n    return 0;\n}",
             [("#include", .keyword), ("<stdio.h>", .string), ("int", .keyword), ("main", .function), ("printf", .function), ("42", .number), ("// hi", .comment), ("return", .keyword)]),
            ("cpp", "#include <iostream>\nint main() {\n    std::cout << \"hi\" << std::endl; /* c */\n    return 0;\n}",
             [("int", .keyword), ("main", .function), ("\"hi\"", .string), ("/* c */", .comment), ("return", .keyword)]),
            ("yaml", "name: arbos\nversion: 1.2\nflags:\n  - fast\n  - debug: true  # yes",
             [("name", .function), ("arbos", .string), ("1.2", .number), ("true", .keyword), ("# yes", .comment)]),
            ("html", "<div class=\"a\">Hi <b>there</b></div>\n<!-- note -->",
             [("div", .keyword), ("class", .function), ("\"a\"", .string), ("Hi ", .plain), ("<!-- note -->", .comment)]),
            ("css", ".card { color: #fff; margin: 4px 0; } /* box */",
             [("card", .function), ("color", .function), ("4", .number), ("/* box */", .comment)]),
            ("markdown", "# Title\nSome **bold** and `code` and [link](http://x).\n- item",
             [("Title", .keyword), ("bold", .keyword), ("code", .string), ("http://x", .function), ("Some ", .plain)]),
        ]
        for (lang, code, expect) in cases {
            for (needle, want) in expect {
                XCTAssertEqual(kind(lang, code, needle), want, "\(lang): \(needle)")
            }
        }
    }

    func testDetectionFallsBackFromTagToFirstLineToKeywordsToPlain() {
        XCTAssertEqual(detectCodeLanguage(lang: "py", code: ""), "python")
        XCTAssertEqual(detectCodeLanguage(lang: "rust", code: "def f():\n  pass"), "rust", "the tag wins")
        XCTAssertEqual(detectCodeLanguage(lang: "", code: "#!/usr/bin/env python3\nprint(1)"), "python")
        XCTAssertEqual(detectCodeLanguage(lang: "", code: "<!DOCTYPE html>\n<p>x</p>"), "html")
        XCTAssertEqual(detectCodeLanguage(lang: "", code: Self.bubbleSort), "python")
        XCTAssertEqual(detectCodeLanguage(lang: "", code: "const x = 1;\nconsole.log(x === 1);"), "javascript")
        XCTAssertEqual(detectCodeLanguage(lang: "", code: "{\"a\": [1, 2]}"), "json")
        XCTAssertEqual(detectCodeLanguage(lang: "", code: "SELECT id FROM users WHERE active = 1"), "sql")
        XCTAssertNil(detectCodeLanguage(lang: "", code: "Just a sentence with no code in it."))
        XCTAssertTrue(CodeHighlight.tokens(lang: "", code: "Just a sentence with no code in it.").isEmpty)
        XCTAssertNil(detectCodeLanguage(lang: "brainfuck", code: "+++"))
    }

    func testThePaletteMapsEveryKindAndTheThemesDiffer() {
        for palette in [CodePalette.dark, CodePalette.light] {
            let kinds: [HighlightKind] = [.plain, .keyword, .function, .string, .number, .comment]
            let colors: [Color] = kinds.map(palette.color)
            XCTAssertEqual(Set(colors.map { "\($0)" }).count, 6, "six distinct colours")
        }
        XCTAssertNotEqual(CodePalette.dark.keyword, CodePalette.light.keyword)
        XCTAssertEqual(Theme.dark.codePalette, .dark)
        XCTAssertEqual(Theme.light.codePalette, .light)
        XCTAssertEqual(CodePalette.dark.color(.keyword), Color(hex: "#95D0CD")!)
        XCTAssertEqual(CodePalette.dark.color(.comment), Color(hex: "#626262")!)
        // The attributed text paints the runs: `def` teal, the comment italic, plain in the body colour.
        let font = Theme.dark.font(CodeHighlight.fontSize, design: .monospaced)
        let attr = CodeHighlight.attributed(lang: "python", code: Self.bubbleSort, palette: .dark, font: font, plain: .white)
        XCTAssertEqual(String(attr.characters), Self.bubbleSort)
        let runs = attr.runs.map { (String(attr[$0.range].characters), $0.swiftUI.foregroundColor, $0.swiftUI.font) }
        XCTAssertTrue(runs.contains { $0.0 == "def" && $0.1 == CodePalette.dark.keyword })
        XCTAssertTrue(runs.contains { $0.0.hasPrefix("# If no two") && $0.1 == CodePalette.dark.comment && $0.2 == font.italic() })
        XCTAssertTrue(runs.contains { $0.0.hasPrefix("(arr)") && $0.1 == Color.white })
        // Plain prose: one run in the body colour.
        let plain = CodeHighlight.attributed(lang: "", code: "hello there", palette: .dark, font: font, plain: .white)
        XCTAssertEqual(plain.runs.count, 1)
    }

    func testIndentGuidesAndSidewaysScroll() {
        let advance = CodeHighlight.glyphAdvance(size: CodeHighlight.fontSize)
        XCTAssertGreaterThan(advance, 6)
        let guides = CodeHighlight.indentGuides(Self.bubbleSort, advance: advance)
        func count(_ line: Int) -> Int { guides.filter { $0.line == line }.count }
        XCTAssertEqual(count(0), 0, "def: no guide")
        XCTAssertEqual(count(1), 1)
        XCTAssertEqual(count(3), 2)
        XCTAssertEqual(count(6), 4)
        XCTAssertEqual(count(12), 0, "blank line")
        XCTAssertEqual(guides.first { $0.line == 1 }!.x, 2.6 * advance, accuracy: 0.01)
        XCTAssertEqual(CodeHighlight.indentGuides("a:\n  b:\n    c", advance: advance).filter { $0.line == 2 }.count, 2, "two-space steps")
        // The longest line (65 glyphs) is wider than the phone's bubble: the body scrolls.
        let width = CodeHighlight.contentWidth(Self.bubbleSort, advance: advance)
        XCTAssertEqual(width, 65 * advance, accuracy: 0.01)
        XCTAssertGreaterThan(width + CodeHighlight.padX + CodeHighlight.padRight, 321, "wider than the 321 pt bubble → sideways scroll")
        XCTAssertLessThan(CodeHighlight.contentWidth("x = 1", advance: advance), 100)
    }

    func testCopyPutsTheBlockOnThePasteboard() {
        UIPasteboard.general.string = ""
        CodeCard.copyToPasteboard(Self.bubbleSort)
        XCTAssertEqual(UIPasteboard.general.string, Self.bubbleSort)
        XCTAssertEqual(CodeCard.copiedFor, .seconds(1.5))
    }
}
