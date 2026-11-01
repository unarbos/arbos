import SwiftUI
import XCTest
@testable import Arbos

/// Jacob, 2026-10-25 22:34 UTC: every named link in a reply — bullets, prose, table cells — carries
/// its site's small rounded icon, as in Grok Bot; his example Bubble's New York Post headline list.
/// The structure: seven named-link runs, each to nypost.com; one fetch per host; the picture or the
/// placeholder in front of each.
@MainActor
final class LinkIconTests: XCTestCase {
    override func setUp() {
        super.setUp()
        LinkIcons.shared.reset()
        LinkIcons.shared.fetch = nil
    }

    func testHostsOfNamedLinksOnlyAndEachOnce() {
        XCTAssertEqual(LinkIcons.host(of: URL(string: "https://user@Sub.Example.com:8443/p?q#f")!), "sub.example.com")
        XCTAssertNil(LinkIcons.host(of: URL(string: "http://10.0.0.1/x")!))
        XCTAssertNil(LinkIcons.host(of: URL(string: "mailto:a@b.com")!))
        XCTAssertNil(LinkIcons.host(of: URL(string: "https://localhost/")!))
        XCTAssertTrue(LinkIcons.labelIsURL("https://x.com/a"))
        XCTAssertTrue(LinkIcons.labelIsURL("www.x.com"))
        XCTAssertFalse(LinkIcons.labelIsURL("El Virrey"))

        let text = "[A](https://a.com) and [B](https://b.org/x) and https://bare.net/y and [https://c.io](https://c.io)\n\n\(AppModel.nyPostReply)\n\n| k | v |\n| --- | --- |\n| [C](https://c.net) | [A again](https://a.com/2) |\n\n```\n[D](https://d.io)\n```"
        XCTAssertEqual(LinkIcons.hosts(in: text), ["a.com", "b.org", "nypost.com", "c.net"],
                       "named links in prose, lists and cells; a bare URL and a URL-as-label have no icon; code is skipped")
    }

    func testTheNYPostListHasSevenNamedLinksToOneHostAndOneFetch() async throws {
        let blocks = Markdown.blocks(AppModel.nyPostReply)
        guard case .bullets(let items) = blocks[0] else { return XCTFail("a bullet list first: \(blocks)") }
        XCTAssertEqual(items.count, 7)
        var fetched: [String] = []
        LinkIcons.shared.fetch = { host in
            fetched.append(host)
            return LinkIconTests.png(side: 64)
        }
        for item in items {
            let attr = Markdown.inline(item, color: .white, link: .blue, codeChip: .gray)
            let links = LinkIcons.namedLinks(in: attr)
            XCTAssertEqual(links.count, 1, item)
            XCTAssertEqual(links.first?.host, "nypost.com")
            // Building the Text asks for the icon: the placeholder while loading.
            _ = LinkIcons.shared.text(attr, lineHeight: 20)
            XCTAssertEqual(LinkIcons.shared.image(for: "nypost.com").size, LinkIcons.placeholder.size)
        }
        // One ask per host, however many links point at it.
        try await Self.waitUntilAnswered("nypost.com")
        XCTAssertEqual(fetched, ["nypost.com"])
        let icon = LinkIcons.shared.image(for: "nypost.com")
        XCTAssertEqual(icon.size, CGSize(width: LinkIcons.side + LinkIcons.gap, height: LinkIcons.side))
        XCTAssertFalse(icon === LinkIcons.placeholder, "the fetched picture replaced the placeholder")
    }

    func testASiteWithoutAnIconKeepsThePlaceholderAndIsNotAskedAgain() async throws {
        var asks = 0
        LinkIcons.shared.fetch = { _ in asks += 1; return nil }
        _ = LinkIcons.shared.image(for: "none.example")
        try await Self.waitUntilAnswered("none.example")
        XCTAssertTrue(LinkIcons.shared.image(for: "none.example") === LinkIcons.placeholder)
        _ = LinkIcons.shared.image(for: "none.example")
        try await Task.sleep(for: .milliseconds(20))
        XCTAssertEqual(asks, 1)
        XCTAssertNil(LinkIcons.rounded(Data("not a picture".utf8)))
        XCTAssertNotNil(LinkIcons.rounded(LinkIconTests.png(side: 32)))
    }

    /// The Lima table's first column: four named links to four hosts, in the cells — and the
    /// fixture ships each site's icon (the shots' source), decodable into the rounded picture.
    func testTheLimaTableCellsCarryTheirSitesAndTheFixtureHasTheirIcons() {
        let hosts = LinkIcons.hosts(in: MarkdownTableTests.lima)
        XCTAssertEqual(hosts, ["www.elvirrey.com", "www.communitas.pe", "www.tangol.com", "www.assouline.com"])
        for host in hosts + ["nypost.com"] {
            let data = LinkIcons.fixtureIcon(host: host)
            XCTAssertNotNil(data, "fixture icon for \(host)")
            XCTAssertNotNil(data.flatMap(LinkIcons.rounded), "fixture icon for \(host) decodes")
        }
        XCTAssertNil(LinkIcons.fixtureIcon(host: "nowhere.example"))
    }

    /// Waits until `host`'s answer (picture or none) is published, which is when it becomes a key
    /// of `images`. The fetch, the detached decode and the one-frame publish have no fixed bound
    /// on a loaded simulator clone, so a fixed sleep is not a wait.
    static func waitUntilAnswered(_ host: String, timeout: Duration = .seconds(10)) async throws {
        let clock = ContinuousClock()
        let end = clock.now + timeout
        while !LinkIcons.shared.images.keys.contains(host) {
            guard clock.now < end else { return XCTFail("no answer for \(host) within \(timeout)") }
            try await Task.sleep(for: .milliseconds(10))
        }
    }

    static func png(side: Int) -> Data {
        let r = UIGraphicsImageRenderer(size: CGSize(width: side, height: side))
        return r.pngData { ctx in
            UIColor.red.setFill()
            ctx.fill(CGRect(x: 0, y: 0, width: side, height: side))
        }
    }
}
