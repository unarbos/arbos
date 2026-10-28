import SwiftUI
import XCTest
@testable import Arbos

/// Jacob (2026-10-27): "allow these common chat highlighting stuff for users to use > etc."
/// The iPhone renders chat Markdown through the shared parser (`arbos_core::markdown` over
/// the FFI), so a typed `> hello` is a quote, `**bold**` is bold, `||spoiler||` hides until
/// tapped — and plain text (snake_case, 2*3*4, e-mail addresses) is never mangled, exactly as
/// on the Mac ([feedback chat-markdown-ios-2026-10-27]).
@MainActor
final class ChatMarkdownTests: XCTestCase {
    func testQuotesNestThroughTheFFI() {
        XCTAssertEqual(Markdown.blocks("> hello\n> world\n>\n> again"),
                       [.quote([.paragraph("hello\nworld"), .paragraph("again")])])
        XCTAssertEqual(Markdown.blocks("> outer\n>> inner\n> back\nafter"),
                       [.quote([.paragraph("outer"), .quote([.paragraph("inner")]), .paragraph("back")]),
                        .paragraph("after")])
        // A lone `>` or `>5` (no space) is prose: the rule in docs/mac-design-rules.md.
        XCTAssertEqual(Markdown.blocks(">5 means more than five"), [.paragraph(">5 means more than five")])
        XCTAssertEqual(Markdown.blocks(">"), [.paragraph(">")])
        XCTAssertEqual(Markdown.blocks("> 5 means five"), [.quote([.paragraph("5 means five")])])
    }

    func testBlocksMatchTheMacGrammar() {
        XCTAssertEqual(Markdown.blocks("# One\n#### Four\n#hashtag"),
                       [.heading(level: 1, text: "One"), .heading(level: 3, text: "Four"), .paragraph("#hashtag")])
        XCTAssertEqual(Markdown.blocks("- a\n* b\n+ c"), [.bullets(["a", "b", "c"])])
        XCTAssertEqual(Markdown.blocks("1. x\n2) y"), [.numbered(["x", "y"])])
        XCTAssertEqual(Markdown.blocks("-5 degrees"), [.paragraph("-5 degrees")])
        XCTAssertEqual(Markdown.blocks("```swift\nlet a = **b**\n```"), [.code(lang: "swift", code: "let a = **b**")])
        // Soft newlines stay (Enter is a line break), a blank line splits paragraphs.
        XCTAssertEqual(Markdown.blocks("one\ntwo\n\nthree"), [.paragraph("one\ntwo"), .paragraph("three")])
    }

    func testInlineStylesComeFromTheSharedRuns() {
        let attr = Markdown.inline("**b** *i* ~~s~~ `c` [l](https://a.com/x_y) https://b.com/p_q",
                                   color: .white, link: .blue, codeChip: .gray)
        XCTAssertEqual(String(attr.characters), "b i s c l https://b.com/p_q")
        func run(_ text: String) -> AttributedString.Runs.Run? {
            attr.runs.first { String(attr[$0.range].characters) == text }
        }
        XCTAssertEqual(run("b")?.swiftUI.font, .system(size: Theme.body, weight: .bold))
        XCTAssertEqual(run("i")?.swiftUI.font, .system(size: Theme.body).italic())
        XCTAssertEqual(run("s")?.strikethroughStyle, .single)
        XCTAssertEqual(run("c")?.swiftUI.backgroundColor, .gray)
        XCTAssertEqual(run("l")?.link?.absoluteString, "https://a.com/x_y")
        XCTAssertEqual(run("l")?.swiftUI.foregroundColor, .blue)
        XCTAssertEqual(run("https://b.com/p_q")?.link?.absoluteString, "https://b.com/p_q")
    }

    func testSpoilersHideUntilTappedForThisViewer() {
        let text = "the ||butler|| did it"
        let hidden = Markdown.inline(text, color: .white, link: .blue, codeChip: .gray)
        let pill = hidden.runs.first { String(hidden[$0.range].characters) == "butler" }
        XCTAssertNotNil(pill)
        XCTAssertEqual(pill?.swiftUI.foregroundColor, .clear, "glyphs hidden")
        XCTAssertEqual(pill?.swiftUI.backgroundColor, Color.white.opacity(Markdown.spoilerHiddenAlpha))
        let url = try! XCTUnwrap(pill?.link)
        XCTAssertEqual(SpoilerState.key(from: url), SpoilerState.key(text: text, index: 1))
        // The copy carries the words; only the paint hides them.
        XCTAssertEqual(String(hidden.characters), "the butler did it")

        SpoilerState.shared.toggle(SpoilerState.key(from: url)!)
        defer { SpoilerState.shared.toggle(SpoilerState.key(from: url)!) }
        let open = Markdown.inline(text, color: .white, link: .blue, codeChip: .gray)
        let shown = open.runs.first { String(open[$0.range].characters) == "butler" }
        XCTAssertEqual(shown?.swiftUI.foregroundColor, .white)
        XCTAssertNil(shown?.link)
        XCTAssertEqual(shown?.swiftUI.backgroundColor, Color.white.opacity(Markdown.spoilerOpenAlpha))
        // A mention chip's URL is not a spoiler's.
        XCTAssertNil(SpoilerState.key(from: URL(string: "arbos://mention/user/abc")!))
    }

    func testPlainTextIsNeverMangled() {
        for text in [
            "snake_case_word and __init__",
            "2*3*4 = 24 and 5 * 3 = 15",
            "https://example.com/a_b_c and jacob_s@example.com",
            "<b>html</b> stays <i>text</i>",
            "I paid $5 and $10",
            "** not bold ** and * not italic *",
            "unterminated **bold and *italic",
            "a || b and x||y and a ~~ b",
            "C++ C# #1 fan 100% 3.14",
        ] {
            let attr = Markdown.inline(text, color: .white, link: .blue, codeChip: .gray)
            XCTAssertEqual(String(attr.characters), text)
            XCTAssertEqual(Markdown.plain(text), text)
            for run in attr.runs where run.link == nil {
                XCTAssertNil(run.swiftUI.font, "\(text): \(String(attr[run.range].characters)) styled")
                XCTAssertNil(run.strikethroughStyle)
            }
            XCTAssertEqual(Markdown.blocks(text), [.paragraph(text)])
        }
    }

    func testUnsafeLinksStayLiteral() {
        for text in ["[x](javascript:alert(1))", "[x](data:text/html,hi)", "[x](relative)"] {
            let attr = Markdown.inline(text, color: .white, link: .blue, codeChip: .gray)
            XCTAssertEqual(String(attr.characters), text)
            XCTAssertTrue(attr.runs.allSatisfy { $0.link == nil }, text)
        }
    }

    func testEscapesAndMathSurvive() {
        XCTAssertEqual(Markdown.plain("\\*not italic\\* and \\> not a quote"), "*not italic* and > not a quote")
        XCTAssertEqual(Markdown.plain("runs in $O(n^2)$ time"), "runs in O(n²) time")
        XCTAssertEqual(Markdown.blocks("\\> not a quote"), [.paragraph("\\> not a quote")])
    }

    func testOneLinePreviewFlattensQuotes() {
        XCTAssertEqual(Quote.oneLine("> **hello**\n> there\n\nreply"), "hello there reply")
        XCTAssertEqual(Quote.oneLine("||secret|| ~~gone~~"), "secret gone")
    }

    func testLinkHostsSeeInsideQuotes() {
        XCTAssertEqual(LinkIcons.hosts(in: "> see [Docs](https://docs.arbos.life/x)\n- [Site](https://arbos.life)"),
                       ["docs.arbos.life", "arbos.life"])
    }
}
