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

    // MARK: - AF-233 diagnostic: where a long message loses its text

    private static let longSentence =
        "The export step fails before the upload is ever attempted, so the build number rises "
        + "while nothing reaches a tester, and the only way to tell the two apart from the "
        + "outside is to read the job log rather than the check mark, which is the whole reason "
        + "this sentence is long enough to need a dozen lines inside a phone bubble."

    private static let longParagraph = longSentence + " " + longSentence

    /// What `RowView.maxBubble` hands an agent bubble on an iPhone 17 (393 pt wide).
    private var cap: CGFloat { 393 - Theme.bubbleInset }
    /// The width the blocks inside the bubble are proposed.
    private var contentWidth: CGFloat { cap - 2 * Theme.bubbleHPad }
    /// The width a list item's text is proposed: the bullet column plus the HStack gap.
    private var itemWidth: CGFloat { contentWidth - 22 }

    private func height<V: View>(_ view: V) -> CGFloat {
        UIHostingController(rootView: view).sizeThatFits(in: CGSize(width: 393, height: 20_000)).height
    }

    /// An agent bubble's chrome around one block, as `RowView`'s `.assistant` case builds it.
    private func bubble<V: View>(_ content: V) -> some View {
        VStack(alignment: .leading, spacing: 8) { content }
            .padding(.horizontal, Theme.bubbleHPad)
            .padding(.vertical, Theme.bubbleVPad)
            .frame(maxWidth: cap, alignment: .leading)
    }

    /// Fully wrapped height of `text` at `width`: the ground truth every bubble is measured against.
    private func wrapped(_ text: String, at width: CGFloat) -> CGFloat {
        height(Text(text)
            .font(.system(size: Theme.body))
            .frame(width: width, alignment: .leading)
            .fixedSize(horizontal: false, vertical: true))
    }

    private var bullet: some View {
        Text("•").font(.system(size: Theme.body)).frame(minWidth: 16, alignment: .trailing)
    }

    private var itemText: Text {
        Markdown.inlineText(Self.longSentence, color: .white, link: .blue, codeChip: .gray, mentions: nil)
            .font(.system(size: Theme.body))
    }

    private var paragraphText: Text {
        Markdown.inlineText(Self.longParagraph, color: .white, link: .blue, codeChip: .gray, mentions: nil)
            .font(.system(size: Theme.body))
    }

    /// `BlockView`'s list row as main draws it, and as the shipped build drew it. The two are
    /// written out rather than switched on a flag: a `_ConditionalContent` around the text
    /// would itself change what the `HStack` proposes, in both arms, and the A/B would say
    /// nothing.
    private var listRowWithPills: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            bullet
            itemText.tint(.blue).mentionPills().fixedSize(horizontal: false, vertical: true)
        }
    }

    private var listRowWithoutPills: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            bullet
            itemText.tint(.blue).fixedSize(horizontal: false, vertical: true)
        }
    }

    private var paragraphWithPills: some View {
        paragraphText.lineSpacing(Theme.bodyLineSpacing).tint(.blue).mentionPills()
    }

    private var paragraphWithoutPills: some View {
        paragraphText.lineSpacing(Theme.bodyLineSpacing).tint(.blue)
    }

    /// Every block kind a long message is made of, measured in a real bubble. A block whose
    /// bubble is barely taller than one line has dropped its text; the printed table says which.
    func testLongMessageBlocksKeepTheirTextInABubble() {
        let chrome = 2 * Theme.bubbleVPad
        let oneLine = wrapped("one line", at: contentWidth)
        let paragraphTruth = wrapped(Self.longParagraph, at: contentWidth)
        let itemTruth = wrapped(Self.longSentence, at: itemWidth)

        var measured: [(String, CGFloat)] = []
        func measure<V: View>(_ name: String, _ view: V) -> CGFloat {
            let h = height(bubble(view)) - chrome
            measured.append((name, h))
            return h
        }

        let table = MentionTable()
        let paragraph = measure("BlockView .paragraph", BlockView(block: .paragraph(Self.longParagraph), live: false, mentions: table))
        let bullets = measure("BlockView .bullets", BlockView(block: .bullets([Self.longSentence, Self.longSentence]), live: false, mentions: table))
        let numbered = measure("BlockView .numbered", BlockView(block: .numbered([Self.longSentence]), live: false, mentions: table))
        let quote = measure("BlockView .quote", BlockView(block: .quote([.paragraph(Self.longParagraph)]), live: false, mentions: table))
        let pillParagraph = measure("paragraph with pills", paragraphWithPills)
        let plainParagraph = measure("paragraph without pills", paragraphWithoutPills)
        let pillItem = measure("list item with pills", listRowWithPills)
        let plainItem = measure("list item without pills", listRowWithoutPills)

        print("AF-233 cap=\(cap) content=\(contentWidth) itemWidth=\(itemWidth) oneLine=\(oneLine)")
        print("AF-233 wrapped truth: paragraph=\(paragraphTruth) item=\(itemTruth)")
        for (name, h) in measured { print("AF-233 \(name) = \(h) pt (\(h / oneLine) lines)") }

        // The whole text reaches the renderer: nothing on the way in shortens the body.
        XCTAssertEqual(Markdown.blocks(Self.longParagraph), [.paragraph(Self.longParagraph)])

        // Each block is as tall as its text needs. 0.9 allows for line spacing differences
        // between the reference Text and the block; it does not allow a dropped line.
        XCTAssertGreaterThan(paragraph, paragraphTruth * 0.9, "paragraph lost lines")
        XCTAssertGreaterThan(bullets, itemTruth * 1.8, "two long bullets lost lines")
        XCTAssertGreaterThan(numbered, itemTruth * 0.9, "numbered item lost lines")
        XCTAssertGreaterThan(quote, paragraphTruth * 0.9, "quoted paragraph lost lines")

        // The A/B: the text renderer is the only difference between these pairs.
        XCTAssertEqual(pillParagraph, plainParagraph, accuracy: 1, "the text renderer changed a paragraph's height")
        XCTAssertEqual(pillItem, plainItem, accuracy: 1, "the text renderer changed a list item's height")
    }
}
