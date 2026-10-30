import XCTest
@testable import Arbos

/// Jacob, "Find me a book store in lima" (iPhone, 2026-10-25 21:31 UTC): the agent's table —
/// plain GFM, store names as `[Name](https://…)` links — was drawn as a monospace block of raw
/// `[…](https://…)` cut off at the bubble's right edge. The table is a real table: one header row,
/// one body row per store, the same columns in every row, and every name cell an inline link.
final class MarkdownTableTests: XCTestCase {
    /// The reply's table bubble, byte for byte (relay T3, chat 01M53KX3FYC0J72S23YAD99ZHH).
    static let lima = """
    | Store | Area | Sunday | What they sell |
    | --- | --- | --- | --- |
    | [Librería El Virrey](https://www.elvirrey.com/) | Miraflores, Calle Bolognesi 510 | 11am–7pm | Spanish literature, essays, history, art; some English at a higher price |
    | [Communitas](https://www.communitas.pe/) | San Isidro, Av. 2 de Mayo 1690 | 10:30am–7:30pm | Humanities, literature, law, history, science, in Spanish |
    | [Vallejo Librería Café](https://www.tangol.com/blog/en/the-3-best-bookstores-in-lima/n-15210) | San Isidro, Av. Camino Real 1119 | not listed | Books plus coffee or tea |
    | [Assouline Lima](https://www.assouline.com/pages/store/lima) | Miraflores, street not listed | not listed | Luxury and design books |
    """

    func testTheLimaTableIsOneTableOfLinkedRows() throws {
        let blocks = Markdown.blocks(Self.lima)
        XCTAssertEqual(blocks.count, 1)
        guard case .table(let header, let rows) = blocks[0] else { return XCTFail("not a table: \(blocks)") }
        XCTAssertEqual(header, ["Store", "Area", "Sunday", "What they sell"])
        XCTAssertEqual(rows.count, 4)
        XCTAssertEqual(rows.map(\.count), [4, 4, 4, 4], "every row has the header's columns")
        XCTAssertEqual(rows[0][1], "Miraflores, Calle Bolognesi 510")
        XCTAssertEqual(rows[3][3], "Luxury and design books")

        // The first column renders as links: name shown, page as the target, no markup left.
        let names = ["Librería El Virrey", "Communitas", "Vallejo Librería Café", "Assouline Lima"]
        let pages = ["https://www.elvirrey.com/", "https://www.communitas.pe/",
                     "https://www.tangol.com/blog/en/the-3-best-bookstores-in-lima/n-15210",
                     "https://www.assouline.com/pages/store/lima"]
        for (i, row) in rows.enumerated() {
            let attr = Markdown.inline(row[0], color: .white, link: .blue, codeChip: .gray)
            XCTAssertEqual(String(attr.characters), names[i])
            let links = attr.runs.compactMap(\.link)
            XCTAssertEqual(links.map(\.absoluteString), [pages[i]], "row \(i) links to its page")
            XCTAssertFalse(String(attr.characters).contains("]("))
        }
        XCTAssertEqual(Markdown.plain(rows[0][0]), "Librería El Virrey")
    }

    func testColumnsLineUpAndTheWideColumnCaps() {
        let blocks = Markdown.blocks(Self.lima)
        guard case .table(let header, let rows) = blocks[0] else { return XCTFail("not a table") }
        let widths = TableBlock.columnWidths(header: header, rows: rows)
        XCTAssertEqual(widths.count, 4)
        XCTAssertEqual(widths[3], TableBlock.columnMax, "\"What they sell\" caps and wraps")
        XCTAssertLessThan(widths[2], widths[1], "\"Sunday\" is narrower than \"Area\"")
        XCTAssertTrue(widths.allSatisfy { $0 >= TableBlock.columnMin })
        // Wider than any iPhone bubble: the frame scrolls sideways instead of cutting off.
        XCTAssertGreaterThan(widths.reduce(0, +), 360)
        XCTAssertEqual(TableBlock.fitted(["a"], to: 3), ["a", "", ""])
        XCTAssertEqual(TableBlock.fitted(["a", "b", "c", "d"], to: 3), ["a", "b", "c"])
    }

    /// A row of pipes without the `| --- |` separator is not a GFM table: it degrades to one
    /// monospace block (the code card, which scrolls sideways), never prose with raw pipes. An
    /// indented or trailing-space table still parses; a lone pipe row is prose.
    func testPipeRowsWithoutASeparatorDegradeToAMonoBlock() {
        var lines = Self.lima.split(separator: "\n").map(String.init)
        lines.remove(at: 1)
        let headless = lines.joined(separator: "\n")
        let blocks = Markdown.blocks(headless)
        XCTAssertEqual(blocks.count, 1)
        guard case .code(let lang, let code) = blocks[0] else { return XCTFail("not a mono block: \(blocks)") }
        XCTAssertEqual(lang, "")
        XCTAssertEqual(code, headless)

        let indented = Self.lima.split(separator: "\n").map { "    \($0)  " }.joined(separator: "\n")
        XCTAssertEqual(Markdown.blocks(indented), Markdown.blocks(Self.lima))

        XCTAssertEqual(Markdown.blocks("| one | pipe row |\n\nthen prose"),
                       [.paragraph("| one | pipe row |"), .paragraph("then prose")])
    }

    /// Prose around the table stays prose; the one-line preview reads the names, not the markup.
    func testTableInsideAReplyKeepsItsNeighboursAndPreviewsAsWords() {
        let reply = "Go to Librería El Virrey in Miraflores.\n\n\(Self.lima)\n\nHours were not checked on a second source."
        let blocks = Markdown.blocks(reply)
        XCTAssertEqual(blocks.count, 3)
        XCTAssertEqual(blocks[0], .paragraph("Go to Librería El Virrey in Miraflores."))
        guard case .table = blocks[1] else { return XCTFail("middle block is the table") }
        XCTAssertEqual(blocks[2], .paragraph("Hours were not checked on a second source."))
        let preview = Quote.oneLine(Self.lima)
        XCTAssertTrue(preview.hasPrefix("Store Area Sunday What they sell Librería El Virrey Miraflores"), preview)
        XCTAssertFalse(preview.contains("https://"))
        XCTAssertFalse(preview.contains("|"))
    }
}
