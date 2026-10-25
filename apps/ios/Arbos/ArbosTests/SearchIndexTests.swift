import XCTest
@testable import Arbos

/// The phone's local search index ([feedback iphone-bottom-search-bar-2026-10-25]),
/// the Mac's `search_index::tests` twin: message text, attachments and links from
/// every loaded chat, folded once per chat version, scanned on each keystroke.
final class SearchIndexTests: XCTestCase {
    private func msg(_ id: String, chat: String, at: Int64, parts: [Part], role: Role = .user,
                     sender: String = "user:fx-me", hidden: Bool = false) -> Message {
        var m = Message(id: id, chatId: chat, role: role, sender: sender, status: .done, turn: nil, decision: nil, subagentId: nil,
                        subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: at, completedAt: nil,
                        pending: false, parts: parts, hiddenCount: 0, senderHandle: "me", senderName: "Me")
        m.hidden = hidden
        return m
    }

    private func text(_ id: String, _ s: String) -> Part { .text(id: "\(id)-t", text: s) }

    private var sam: [Message] {
        [
            msg("m1", chat: "dm-sam", at: 10, parts: [text("m1", "Three decisions: keep the canvas infinite")]),
            msg("m2", chat: "dm-sam", at: 20, parts: [text("m2", "Here is the deck https://www.notion.so/launch-week"),
                                                      .attachment(id: "a1", attachment: Attachment(name: "Q3 plan.pdf", mime: "application/pdf", blobRef: "b1", bytes: 1_900_000))]),
            msg("sys", chat: "dm-sam", at: 30, parts: [text("sys", "Zoë joined")], role: .system),
            msg("job", chat: "dm-sam", at: 40, parts: [text("job", "hidden decisions row")], hidden: true),
        ]
    }

    func testFindsMessageTextFoldedWithASnippet() {
        var index = SearchIndex()
        index.sync(chatId: "dm-sam", projectId: "people", version: 1, messages: { sam }, who: { _ in "Sam Rivera" })
        let hits = index.textHits(q: HomeSearch.normalizeQuery("DECISIONS"))
        XCTAssertEqual(hits.map(\.text.messageId), ["m1"], "system lines and hidden rows never count")
        XCTAssertEqual(hits[0].text.who, "Sam Rivera")
        XCTAssertEqual(hits[0].snippet, "Three decisions: keep the canvas infinite")
        XCTAssertEqual(index.textHits(q: HomeSearch.normalizeQuery("zoe")).count, 0, "the system line is not searchable")
        XCTAssertEqual(index.textHits(q: "").count, 0, "an empty query finds no message")
    }

    func testFindsAttachmentsAndLinksByName() {
        var index = SearchIndex()
        index.sync(chatId: "dm-sam", projectId: "people", version: 1, messages: { sam }, who: { _ in "Sam Rivera" })
        let files = index.contentHits(q: "q3")
        XCTAssertEqual(files.count, 1)
        XCTAssertEqual(files[0].kind, .file)
        XCTAssertEqual(files[0].name, "Q3 plan.pdf")
        XCTAssertEqual(files[0].itemId, "m2#a1", "the Library item's id")
        XCTAssertEqual(files[0].bytes, 1_900_000)
        let links = index.contentHits(q: "notion")
        XCTAssertEqual(links.count, 1)
        XCTAssertEqual(links[0].kind, .link)
        XCTAssertEqual(links[0].url?.host(), "www.notion.so")
        XCTAssertEqual(index.contentCount, 2)
    }

    /// Typing never re-reads a chat: `sync` is a no-op until the chat's version moves,
    /// a moved version replaces the chat's entries, and a removed chat drops out.
    func testSyncIsLazyPerChatVersion() {
        var index = SearchIndex()
        var reads = 0
        let read: () -> [Message] = { reads += 1; return self.sam }
        XCTAssertTrue(index.sync(chatId: "dm-sam", projectId: "people", version: 1, messages: read, who: { _ in "Sam" }))
        XCTAssertFalse(index.sync(chatId: "dm-sam", projectId: "people", version: 1, messages: read, who: { _ in "Sam" }))
        XCTAssertEqual(reads, 1, "same version: not read again")
        XCTAssertEqual(index.version(of: "dm-sam"), 1)
        let newer = [msg("m9", chat: "dm-sam", at: 99, parts: [text("m9", "decisions, revisited")])]
        XCTAssertTrue(index.sync(chatId: "dm-sam", projectId: "people", version: 2, messages: { newer }, who: { _ in "Sam" }))
        XCTAssertEqual(index.textHits(q: "decisions").map(\.text.messageId), ["m9"], "the new version replaced the old rows")
        index.sync(chatId: "g-launch", projectId: "people", version: 1, messages: { [self.msg("g1", chat: "g-launch", at: 5, parts: [self.text("g1", "decisions in the group")])] }, who: { _ in "Jules" })
        XCTAssertEqual(index.textHits(q: "decisions").map(\.text.messageId), ["m9", "g1"], "across chats, newest first")
        index.retain(chatIds: ["dm-sam"])
        XCTAssertEqual(index.chatCount, 1)
        index.remove(chatId: "dm-sam")
        XCTAssertEqual(index.textCount, 0)
    }
}
