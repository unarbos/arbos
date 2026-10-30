import XCTest
@testable import Arbos

/// The Library tab's structure ([feedback library-list-ios-2026-10-25]): one list
/// by day, five rows then Show more, compact times, the day words, the preview's
/// type label and the long-press actions — the Mac's `library::tests` twin.
final class LibraryListTests: XCTestCase {
    private var cal: Calendar {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = TimeZone(identifier: "UTC")!
        return c
    }

    private func date(_ y: Int, _ m: Int, _ d: Int, _ h: Int = 12) -> Date {
        cal.date(from: DateComponents(year: y, month: m, day: d, hour: h))!
    }

    private func item(_ id: String, kind: ChatItem.Kind, title: String, at: Date, media: MediaItem? = nil) -> ChatItem {
        ChatItem(id: id, kind: kind, title: title, subtitle: "", url: URL(string: "https://www.arbos.life/launch/"),
                 at: Int64(at.timeIntervalSince1970 * 1000), fromHandle: "samir", mime: "", media: media)
    }

    func testRelativeTimesAreCompact() {
        let now: Int64 = 1_000_000_000_000
        let min: Int64 = 60_000
        XCTAssertEqual(LibraryList.relTime(now - 20_000, now: now), "now")
        XCTAssertEqual(LibraryList.relTime(now - 42 * min, now: now), "42m")
        XCTAssertEqual(LibraryList.relTime(now - 13 * 60 * min, now: now), "13h")
        XCTAssertEqual(LibraryList.relTime(now - 2 * 24 * 60 * min, now: now), "2d")
        XCTAssertEqual(LibraryList.relTime(now - 20 * 24 * 60 * min, now: now), "2w")
        XCTAssertEqual(LibraryList.relTime(now - 100 * 24 * 60 * min, now: now), "3mo")
        XCTAssertEqual(LibraryList.relTime(now - 800 * 24 * 60 * min, now: now), "2y")
        XCTAssertEqual(LibraryList.relTime(now + 5000, now: now), "now", "a clock ahead of ours")
    }

    func testDayLabelsAreTheHomeListsWords() {
        let today = date(2026, 10, 25) // a Sunday
        XCTAssertEqual(LibraryList.dayLabel(date(2026, 10, 25, 9), today: today, calendar: cal), "Today")
        XCTAssertEqual(LibraryList.dayLabel(date(2026, 10, 24, 22), today: today, calendar: cal), "Yesterday")
        XCTAssertEqual(LibraryList.dayLabel(date(2026, 10, 23), today: today, calendar: cal), "Friday")
        XCTAssertEqual(LibraryList.dayLabel(date(2026, 10, 19), today: today, calendar: cal), "Monday")
        XCTAssertEqual(LibraryList.dayLabel(date(2026, 10, 18), today: today, calendar: cal), "Oct 18")
        XCTAssertEqual(LibraryList.dayLabel(date(2025, 12, 31), today: today, calendar: cal), "Dec 31, 2025")
        XCTAssertEqual(LibraryList.stamp(Int64(date(2026, 10, 24, 22).timeIntervalSince1970 * 1000), now: today, calendar: cal), "Yesterday 10:00 PM")
        XCTAssertEqual(LibraryList.stamp(Int64(date(2026, 10, 3, 11).timeIntervalSince1970 * 1000), now: today, calendar: cal), "Oct 3, 11:00 AM")
    }

    func testOneListByDayFiveRowsThenShowMore() {
        let today = date(2026, 10, 25, 15)
        var items: [ChatItem] = []
        for i in 0..<6 {
            items.append(item("t\(i)", kind: .media, title: "slide-\(i).png", at: date(2026, 10, 25, 13 - i)))
        }
        items.append(item("l", kind: .link, title: "arbos.life", at: date(2026, 10, 25, 14)))
        items.append(item("y", kind: .file, title: "q3-review.pdf", at: date(2026, 10, 24, 9)))
        items.append(item("f", kind: .file, title: "notes.zip", at: date(2026, 10, 23, 9)))
        let plan = LibraryList.plan(items.shuffled(), now: today, calendar: cal)
        XCTAssertEqual(plan.map(\.label), ["Today", "Yesterday", "Friday"], "newest day first")
        XCTAssertEqual(plan.map(\.shown), [5, 1, 1], "five of today's seven, then the short days whole")
        XCTAssertEqual(plan.map(\.more), [true, false, false])
        let unfolded = LibraryList.plan(items, expanded: ["Today"], now: today, calendar: cal)
        XCTAssertEqual(unfolded[0].shown, 7, "Show more unfolds the day in place")
        // Within a day the items are newest first and of every kind together.
        let days = LibraryList.days(items, now: today, calendar: cal)
        XCTAssertEqual(Array(days[0].items.map(\.id).prefix(3)), ["l", "t0", "t1"])
        XCTAssertEqual(Set(days[0].items.map(\.kind)), [.media, .link])
        XCTAssertEqual(LibraryList.visibleRows(5, expanded: false), 5)
        XCTAssertEqual(LibraryList.visibleRows(6, expanded: false), 5)
        XCTAssertEqual(LibraryList.showMoreAfter, 5)
    }

    func testTypeLabelsAndMenus() {
        let picture = MediaItem(id: "m", name: "a.png", mime: "image/png", kind: .image, blobRef: nil, slot: "p", by: "Arbos", at: 0)
        let clip = MediaItem(id: "v", name: "a.mp4", mime: "video/mp4", kind: .video, blobRef: nil, slot: "p", by: "Arbos", at: 0)
        let now = Date()
        XCTAssertEqual(LibraryList.kindLabel(item("a", kind: .media, title: "a.png", at: now, media: picture)), "Image")
        XCTAssertEqual(LibraryList.kindLabel(item("b", kind: .media, title: "a.mp4", at: now, media: clip)), "Video")
        XCTAssertEqual(LibraryList.kindLabel(item("c", kind: .file, title: "press-kit-v3.pdf", at: now)), "PDF")
        XCTAssertEqual(LibraryList.kindLabel(item("d", kind: .file, title: "Makefile", at: now)), "File")
        XCTAssertEqual(LibraryList.kindLabel(item("e", kind: .link, title: "arbos.life", at: now)), "Link")
        XCTAssertEqual(LibraryList.fileExtension("odd.toolongext"), nil)
        XCTAssertEqual(LibraryList.menuItems(item("a", kind: .media, title: "a.png", at: now, media: picture)), ["Open", "Copy", "Forward", "Save to Photos"])
        XCTAssertEqual(LibraryList.menuItems(item("c", kind: .file, title: "a.pdf", at: now)), ["Open", "Copy", "Forward", "Save"])
        XCTAssertEqual(LibraryList.menuItems(item("e", kind: .link, title: "arbos.life", at: now)), ["Open link", "Copy link"])
        XCTAssertEqual(LibraryList.bareURL(URL(string: "https://www.arbos.life/launch/")!), "arbos.life/launch")
    }

    // MARK: - Only what was exchanged in the chat ([feedback library-chat-only-ios-2026-10-25])

    private func row(_ m: [String: Any]) -> Message {
        DocJSON.message(m, chatId: "c1")!
    }

    func testBareFileNamesAndUntypedHostsAreNotLinks() {
        XCTAssertEqual(ChatMedia.links(in: "I looked at app.rs, calls.rs, auth.rs and update.rs.").map(\.absoluteString), [],
                       "the data detector reads app.rs as a Serbian site; a file name is not a link")
        XCTAssertEqual(ChatMedia.links(in: "see www.example.com and https://arbos.life/launch.").map(\.absoluteString),
                       ["https://arbos.life/launch"], "typed http(s) links only")
    }

    @MainActor
    func testToolPartsJobRowsAndTheAgentsPagesNeverEnterTheLibraryButChatSentThingsDo() {
        let model = AppModel()
        let job = row(["id": "j", "role": "assistant", "sender": "box:1", "status": "streaming", "createdAt": 5,
                       "hidden": true, "worker": true,
                       "parts": [["id": "j-t", "kind": "text", "text": "Read https://github.com/unarbos/arbos/blob/main/crates/arbos-mac/src/app.rs"],
                                 ["id": "j-a", "kind": "attachment", "attachment": ["name": "app.rs", "mime": "text/plain", "ref": "blob-1"]]]])
        let worker = row(["id": "w", "role": "assistant", "sender": "agent", "status": "done", "createdAt": 4, "worker": true,
                          "subagentName": "reader", "parts": [["id": "w-t", "kind": "text", "text": "https://docs.rs/gpui"]]])
        let reply = row(["id": "a", "role": "assistant", "sender": "box:1", "status": "done", "createdAt": 3,
                         "parts": [["id": "a-c", "kind": "tool", "call": ["id": "c1", "name": "web_fetch"], "outputSummary": "Fetched https://news.ycombinator.com"],
                                   ["id": "a-t", "kind": "text", "text": "I read calls.rs and auth.rs; source https://github.com/unarbos/arbos. Here is the chart:"],
                                   ["id": "a-a", "kind": "attachment", "attachment": ["name": "signups.png", "mime": "image/png", "ref": "blob-3", "kind": "image"]]]])
        let sent = row(["id": "u", "role": "user", "sender": "user:gh_2", "status": "done", "createdAt": 2, "senderHandle": "samir",
                        "parts": [["id": "u-t", "kind": "text", "text": "Landing page: https://arbos.life/launch — and the deck:"],
                                  ["id": "u-a", "kind": "attachment", "attachment": ["name": "deck-v3.pdf", "mime": "application/pdf", "ref": "blob-2"]]]])
        XCTAssertFalse(ChatMedia.sharedInChat(job))
        XCTAssertFalse(ChatMedia.sharedInChat(worker))
        XCTAssertTrue(ChatMedia.sharedInChat(reply))
        XCTAssertEqual(ChatMedia.libraryLinks(reply), [], "the agent's prose link is its research, not a shared link")
        XCTAssertEqual(ChatMedia.libraryLinks(sent).map(\.absoluteString), ["https://arbos.life/launch"])

        let items = ChatMedia.items(messages: [job, worker, reply, sent], slot: "p1", model: model)
        XCTAssertEqual(Set(items.map(\.title)), ["signups.png", "arbos.life", "deck-v3.pdf"],
                       "the agent's shared chart, the person's link and file — nothing the agent read or fetched: \(items.map(\.title))")
        XCTAssertEqual(items.first?.title, "signups.png", "newest first")
        XCTAssertEqual(Set(items.map(\.kind)), [.media, .link, .file])
    }

    func testTheMocksNumbers() {
        XCTAssertEqual(LibraryList.thumb, 36)
        XCTAssertEqual(LibraryList.previewWidth, 344)
        XCTAssertEqual(LibraryList.previewImageHeight, 206)
    }
}
