import XCTest
import SwiftUI
@testable import Arbos

/// Link previews ([feedback link-embeds-ios-2026-10-27]): the `link_preview` part parses, becomes
/// a card under the text (or the whole bubble for a bare link), is hidden by `preview: "off"`
/// and the global switch, joins an agent's bubble as its last block, and the menu offers
/// Remove preview only where Delete is offered too.
final class LinkPreviewCardTests: XCTestCase {
    private let chat = "fx-affine-main"
    private let t0: Int64 = 1_700_000_000_000
    private let url = "https://open.spotify.com/track/7eiYNCojnoUVnz14kjBcQH?si=163bbf4a6caf4b39"

    private func part(_ extra: [String: Any] = [:]) -> [String: Any] {
        var p: [String: Any] = [
            "id": "lp", "kind": "link_preview", "specVersion": 1,
            "url": url, "canonicalUrl": "https://open.spotify.com/track/7eiYNCojnoUVnz14kjBcQH",
            "provider": "spotify", "providerName": "Spotify", "type": "song",
            "title": "Midnight City", "subtitle": "M83 · Hurry Up, We're Dreaming",
            "facts": ["Song", "4:03"], "siteHost": "open.spotify.com",
            "image": ["ref": "lp-abc.jpg", "width": 300, "height": 300, "mime": "image/jpeg", "art": true],
            "layout": "compact", "fetchedAt": t0,
        ]
        for (k, v) in extra { p[k] = v }
        return p
    }

    private func userMessage(_ text: String, preview: String? = nil) -> [String: Any] {
        var m: [String: Any] = [
            "id": "m1", "role": "user", "sender": "user:gh_me", "status": "done", "createdAt": t0,
            "parts": [["id": "t", "kind": "text", "text": text], part()] as [[String: Any]],
        ]
        if let preview { m["preview"] = preview }
        return m
    }

    private func userRow(_ rows: [Row]) throws -> (text: String, preview: LinkPreview?) {
        for row in rows {
            if case .user(let text, _, _, let preview) = row.kind { return (text, preview) }
        }
        throw XCTSkip("no user row")
    }

    override func setUp() {
        super.setUp()
        Transcript.linkPreviewsEnabled = true
    }

    func testThePartParsesAndUnknownFieldsAreIgnored() throws {
        let m = try XCTUnwrap(DocJSON.message(userMessage("x"), chatId: chat))
        guard case .linkPreview(let id, let p) = m.parts[1] else { return XCTFail("link preview part: \(m.parts[1])") }
        XCTAssertEqual(id, "lp")
        XCTAssertEqual(p.provider, "spotify")
        XCTAssertEqual(p.type, "song")
        XCTAssertEqual(p.title, "Midnight City")
        XCTAssertEqual(p.facts, ["Song", "4:03"])
        XCTAssertEqual(p.image?.ref, "lp-abc.jpg")
        XCTAssertEqual(p.image?.art, true)
        XCTAssertEqual(p.siteHost, "open.spotify.com")
        XCTAssertNil(p.siteIcon, "reserved for the shared tile")
        XCTAssertFalse(p.expanded)
        XCTAssertEqual(p.oneLine, "Midnight City — Spotify")
        XCTAssertEqual(p.openURL?.absoluteString, "https://open.spotify.com/track/7eiYNCojnoUVnz14kjBcQH")
        XCTAssertNotNil(m.linkPreview)

        // A newer field is ignored; a part without a title is nothing.
        let newer: [String: Any] = ["id": "m9", "role": "user", "status": "done",
                                    "parts": [["id": "z", "kind": "link_preview", "title": "T", "newerField": ["a": 1]],
                                              ["id": "y", "kind": "link_preview", "url": "https://a.test"]] as [[String: Any]]]
        let n = try XCTUnwrap(DocJSON.message(newer, chatId: chat))
        guard case .linkPreview(_, let np) = n.parts[0] else { return XCTFail("parsed") }
        XCTAssertEqual(np.title, "T")
        guard case .unknown = n.parts[1] else { return XCTFail("no title → unknown") }
    }

    func testTextPlusLinkKeepsTheTextAndABareLinkBecomesTheCard() throws {
        let withText = try XCTUnwrap(DocJSON.message(userMessage("listen to this \(url)"), chatId: chat))
        let r1 = try userRow(Transcript.rows([withText], projectId: "people:\(chat)"))
        XCTAssertTrue(r1.text.hasPrefix("listen to this https://"), r1.text)
        XCTAssertNotNil(r1.preview)
        XCTAssertEqual(r1.preview?.slot, "people:\(chat)", "the card loads its picture from the chat's slot")

        let bare = try XCTUnwrap(DocJSON.message(userMessage(url), chatId: chat))
        let r2 = try userRow(Transcript.rows([bare], projectId: "people:\(chat)"))
        XCTAssertEqual(r2.text, "", "a bare link shows the card alone")
        XCTAssertNotNil(r2.preview)

        let canonical = try XCTUnwrap(DocJSON.message(userMessage("https://open.spotify.com/track/7eiYNCojnoUVnz14kjBcQH"), chatId: chat))
        let r3 = try userRow(Transcript.rows([canonical], projectId: "people:\(chat)"))
        XCTAssertEqual(r3.text, "")
    }

    func testPreviewOffAndTheGlobalSwitchHideTheCard() throws {
        let off = try XCTUnwrap(DocJSON.message(userMessage(url, preview: "off"), chatId: chat))
        XCTAssertNil(off.linkPreview)
        let r = try userRow(Transcript.rows([off], projectId: "p"))
        XCTAssertNil(r.preview)
        XCTAssertTrue(r.text.hasPrefix("https://"), "the plain link is back")

        Transcript.linkPreviewsEnabled = false
        defer { Transcript.linkPreviewsEnabled = true }
        let on = try XCTUnwrap(DocJSON.message(userMessage(url), chatId: chat))
        let r2 = try userRow(Transcript.rows([on], projectId: "p"))
        XCTAssertNil(r2.preview)
        XCTAssertTrue(r2.text.hasPrefix("https://"))
    }

    func testAnAgentsLinkPreviewIsTheBubblesLastBlock() throws {
        let m: [String: Any] = [
            "id": "m2", "role": "assistant", "sender": "box:1", "status": "done", "createdAt": t0, "inReplyTo": "m1",
            "parts": [
                ["id": "t", "kind": "text", "text": "Mira posted the count this morning: https://x.com/mira/status/1"],
                part(["provider": "x", "providerName": "X", "type": "post", "title": "Mira Kaplan", "subtitle": "@mirakaplan",
                      "description": "Subnet 120 just crossed 1,000 validators.", "facts": ["Oct 27, 2026"], "image": NSNull(), "layout": "compact"]),
            ] as [[String: Any]],
        ]
        let message = try XCTUnwrap(DocJSON.message(m, chatId: chat))
        let rows = Transcript.rows([message], projectId: "fx-affine")
        let bubbles = rows.compactMap { row -> [Block]? in
            if case .assistant(let blocks, _, _, _) = row.kind { return blocks }
            return nil
        }
        XCTAssertEqual(bubbles.count, 1, "the card joins the prose's bubble")
        XCTAssertEqual(bubbles[0].count, 2)
        guard case .paragraph = bubbles[0][0] else { return XCTFail("prose first") }
        guard case .linkPreview(let partId, let p) = bubbles[0][1] else { return XCTFail("then the card") }
        XCTAssertEqual(partId, "lp")
        XCTAssertEqual(p.type, "post")
        XCTAssertTrue(p.isPost)
        XCTAssertEqual(p.slot, "fx-affine")
        XCTAssertEqual(p.oneLine, "Mira Kaplan — X", "a copy carries title and site")
    }

    func testIsLinkOnlyMatchesTheTypedOrCanonicalUrlAlone() {
        var p = LinkPreview(url: "https://a.test/x?si=1", canonicalUrl: "https://a.test/x", provider: "web", providerName: "a.test", type: "page", title: "T")
        XCTAssertTrue(p.isLinkOnly("https://a.test/x?si=1"))
        XCTAssertTrue(p.isLinkOnly("  https://a.test/x  "))
        XCTAssertTrue(p.isLinkOnly("https://a.test/x?si=1/"))
        XCTAssertFalse(p.isLinkOnly("see https://a.test/x?si=1"))
        XCTAssertFalse(p.isLinkOnly(""))
        p.layout = "expanded"
        XCTAssertFalse(p.expanded, "expanded needs a picture")
        p.image = LinkPreview.Picture(ref: "lp-1.jpg", width: 1280, height: 720, mime: "image/jpeg", art: false)
        XCTAssertTrue(p.expanded)
    }

    func testTheMenuOffersRemovePreviewOnlyWhereDeleteIsOffered() {
        let labels: ([MessageMenuItem]) -> [String] = { $0.map(\.label) }
        XCTAssertEqual(labels(MessageMenu.items(forMessage: true, canDelete: true, hasPreview: true)),
                       ["Copy", "Reply", "Forward", "Reply in DM", "Remove preview", "Delete"])
        XCTAssertEqual(labels(MessageMenu.items(forMessage: true, canDelete: false, hasPreview: true)),
                       ["Copy", "Reply", "Forward", "Reply in DM"])
        XCTAssertEqual(labels(MessageMenu.items(forMessage: true, canDelete: true, hasPreview: false)),
                       ["Copy", "Reply", "Forward", "Reply in DM", "Delete"])
        XCTAssertEqual(MessageMenuItem.removePreview.systemImage, "link")
        XCTAssertFalse(MessageMenuItem.removePreview.isDestructive)
    }

    /// The card renders in a hosting controller without touching the network: a compact card
    /// with art, and an expanded video card, both at the bubble's width.
    @MainActor
    func testTheCardHostsCompactAndExpanded() {
        let model = AppModel()
        var p = LinkPreview(url: url, canonicalUrl: "https://open.spotify.com/track/7eiYNCojnoUVnz14kjBcQH", provider: "spotify", providerName: "Spotify", type: "song", title: "Midnight City")
        p.subtitle = "M83"
        p.facts = ["Song", "4:03"]
        let compact = UIHostingController(rootView: LinkPreviewCard(preview: p, chatId: chat, messageId: "m1", own: true)
            .environmentObject(model).environment(\.theme, Theme.dark).frame(width: 275))
        compact.view.frame = CGRect(x: 0, y: 0, width: 275, height: 400)
        compact.view.layoutIfNeeded()
        let h1 = compact.sizeThatFits(in: CGSize(width: 275, height: 400)).height
        XCTAssertGreaterThan(h1, 60)
        XCTAssertLessThan(h1, 120, "a compact card is one short row")

        var v = p
        v.type = "video"; v.layout = "expanded"; v.duration = "18:24"
        v.image = LinkPreview.Picture(ref: "lp-yt.jpg", width: 1280, height: 720, mime: "image/jpeg", art: false)
        let expanded = UIHostingController(rootView: LinkPreviewCard(preview: v, chatId: chat, messageId: "m1", own: false)
            .environmentObject(model).environment(\.theme, Theme.dark).frame(width: 255))
        expanded.view.frame = CGRect(x: 0, y: 0, width: 255, height: 500)
        expanded.view.layoutIfNeeded()
        let h2 = expanded.sizeThatFits(in: CGSize(width: 255, height: 500)).height
        XCTAssertGreaterThan(h2, 255 * 9 / 16 + 50, "the picture on top, the text under it")
    }
}
