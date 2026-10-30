import ArbosFFI
import XCTest
@testable import Arbos

/// Jacob, 2026-10-25 ([feedback iphone-bottom-search-bar-2026-10-25]): the iPhone's
/// bottom row (Contacts · Calls · Chats · Settings) is gone; one app-wide search
/// bar sits there and nothing else, and it searches like the Mac palette
/// (`crates/arbos-mac/src/palette.rs` `tests`): results grouped by kind in a fixed
/// order, name hits before indirect hits, newest first, capped per section.
final class BottomSearchBarTests: XCTestCase {
    private func entry(_ kind: GlobalSearch.Kind, _ title: String, when: Int64, indirect: Bool = false,
                       target: GlobalSearch.Target? = nil) -> GlobalSearch.Entry {
        GlobalSearch.Entry(kind: kind, title: title, subtitle: "", when: when, rank: indirect ? .indirect : .direct(kind: .wordStart, field: .name),
                           target: target ?? .project("id-\(title)"))
    }

    private var fixture: [GlobalSearch.Entry] {
        [
            entry(.agent, "Arbos", when: 50),
            entry(.person, "Carro", when: 40, target: .people("dm-1")),
            entry(.group, "Testing", when: 60, target: .people("g-1")),
            // A DM found through a member / chat title: a Chats row, not a person row.
            entry(.person, "Sam", when: 70, indirect: true, target: .people("dm-2")),
            entry(.server, "Launch Crew", when: 30, target: .server("s1")),
            entry(.channel, "#general", when: 80, target: .channel(serverId: "s1", channelId: "c1")),
            entry(.file, "plan.pdf", when: 90, target: .item(chatId: "ch", projectId: "p", itemId: "m#p")),
            entry(.link, "github.com", when: 20, target: .link(URL(string: "https://github.com/unarbos/arbos")!)),
            // A message-text hit: its own section.
            entry(.message, "Arbonites", when: 85, indirect: true, target: .message(chatId: "c", projectId: "people", messageId: "m9")),
            // A person met in a group, no DM yet: People, opens a DM.
            entry(.person, "@DaWizzaa", when: 15, target: .person(handle: "dawizzaa")),
        ]
    }

    // MARK: Structure

    /// The bottom edge carries the search bar alone — no tab bar, no button beside it.
    func testBottomEdgeIsOneSearchBarAndNothingElse() {
        XCTAssertEqual(HomeChrome.bottom, [.searchBar])
        XCTAssertFalse(HomeChrome.bottom.contains(.tabBar), "the Contacts · Calls · Chats · Settings bar is gone")
        XCTAssertFalse(HomeChrome.bottom.contains { if case .button = $0 { return true } else { return false } },
                       "no grid+ or any other control beside the bar")
        XCTAssertEqual(HomeChrome.searchPlaceholder, "Search", "the Mac palette's word")
    }

    /// Every former tab has a home that already existed.
    func testFormerTabsKeepAHome() {
        let names = HomeChrome.homes.map(\.0)
        for want in ["Settings", "Contacts", "Calls", "Unread", "New chat"] {
            XCTAssertTrue(names.contains(want), want)
        }
        for (_, home) in HomeChrome.homes {
            XCTAssertFalse(home.lowercased().contains("bottom"), "nothing new on the bottom edge: \(home)")
        }
    }

    // MARK: Grouping (the Mac's `palette::tests`)

    /// People/agents → chats → servers/channels → messages → files/media/links, each
    /// section newest first; a server *and* its channel both appear.
    func testSectionsComeInJacobsOrderWithAChannelAndAFile() {
        let arranged = GlobalSearch.arrange(fixture, queryIsEmpty: false)
        XCTAssertEqual(arranged.map(\.section), [.people, .chats, .servers, .messages, .content])
        XCTAssertEqual(arranged.map(\.section.label),
                       ["People & agents", "Chats", "Servers & channels", "Messages", "Files, media & links"])
        func titles(_ s: GlobalSearch.Section) -> [String] { arranged.first { $0.section == s }?.rows.map(\.title) ?? [] }
        XCTAssertEqual(titles(.people), ["Arbos", "Carro", "@DaWizzaa"])
        XCTAssertEqual(titles(.chats), ["Testing", "Sam"])
        XCTAssertEqual(titles(.servers), ["#general", "Launch Crew"])
        XCTAssertEqual(titles(.messages), ["Arbonites"])
        XCTAssertEqual(titles(.content), ["plan.pdf", "github.com"])
        let flat = GlobalSearch.flatten(arranged)
        XCTAssertTrue(flat.contains { if case .channel = $0.target { return true } else { return false } })
        XCTAssertTrue(flat.contains { if case .item = $0.target { return true } else { return false } })
        XCTAssertTrue(flat.contains { if case .message = $0.target { return true } else { return false } }, "a message row opens the chat at the row")
    }

    /// Empty query: recent chats and servers by activity — no files, links,
    /// messages or DM-less people.
    func testTheEmptyQueryShowsRecents() {
        let arranged = GlobalSearch.arrange(fixture, queryIsEmpty: true)
        XCTAssertEqual(arranged.count, 1)
        XCTAssertEqual(arranged[0].section, .recent)
        XCTAssertEqual(arranged[0].rows.map(\.title), ["Sam", "Testing", "Arbos", "Carro", "Launch Crew"])
    }

    /// A name hit outranks an indirect hit inside a section, whatever the age.
    func testNameHitsComeBeforeMemberAndMessageHits() {
        let rows = [entry(.group, "Old by name", when: 10), entry(.group, "New by member", when: 99, indirect: true)]
        XCTAssertEqual(GlobalSearch.arrange(rows, queryIsEmpty: false)[0].rows.map(\.title), ["Old by name", "New by member"])
    }

    func testASectionIsCapped() {
        let rows = (0..<20).map { entry(.person, "p\($0)", when: Int64($0), target: .people("dm-\($0)")) }
        let arranged = GlobalSearch.arrange(rows, queryIsEmpty: false)
        XCTAssertEqual(arranged[0].rows.count, GlobalSearch.sectionCap)
        XCTAssertEqual(arranged[0].rows[0].title, "p19", "newest first")
    }

    /// Rank per type (Jacob): a name that starts with the query beats one where
    /// only a later word starts with it, whatever their age; indirect hits last.
    /// Through the shared matcher a handle hit sorts after a name hit of the same
    /// kind, and a substring inside a word is no hit at all.
    func testPrefixBeatsWordStartBeatsIndirect() {
        XCTAssertEqual(GlobalSearch.Rank.of("Scout", q: "sc"), .direct(kind: .prefix, field: .name))
        XCTAssertEqual(GlobalSearch.Rank.of("Affinity Scout", q: "sc"), .direct(kind: .wordStart, field: .name))
        XCTAssertEqual(GlobalSearch.Rank.of("Doug", q: "sc"), .indirect)
        XCTAssertEqual(GlobalSearch.Rank.of("Affinity", q: "ffin"), .indirect)
        XCTAssertEqual(GlobalSearch.Rank.of(name: "Carro", handle: "robertalanm", q: "rob"), .direct(kind: .prefix, field: .handle))
        XCTAssertEqual(GlobalSearch.Rank.of(name: "Carro", handle: "robertalanm", q: "carro"), .direct(kind: .exact, field: .name))
        XCTAssertLessThan(GlobalSearch.Rank.direct(kind: .exact, field: .handle), GlobalSearch.Rank.direct(kind: .prefix, field: .name))
        XCTAssertLessThan(GlobalSearch.Rank.direct(kind: .prefix, field: .name), GlobalSearch.Rank.direct(kind: .prefix, field: .handle))
        XCTAssertLessThan(GlobalSearch.Rank.direct(kind: .wordStart, field: .handle), GlobalSearch.Rank.direct(kind: .exact, field: .title))
        XCTAssertLessThan(GlobalSearch.Rank.direct(kind: .wordStart, field: .title), GlobalSearch.Rank.indirect)
        var wordStart = entry(.agent, "Affinity Scout", when: 99); wordStart.rank = .of("Affinity Scout", q: "sc")
        var prefix = entry(.agent, "Scout", when: 1); prefix.rank = .of("Scout", q: "sc")
        XCTAssertEqual(GlobalSearch.arrange([wordStart, prefix], queryIsEmpty: false)[0].rows.map(\.title), ["Scout", "Affinity Scout"])
    }

    func testSubtitlesReadNaturally() {
        XCTAssertEqual(GlobalSearch.serverSubtitle(channels: 1, members: 1), "Server · 1 channel · 1 member")
        XCTAssertEqual(GlobalSearch.serverSubtitle(channels: 3, members: 0), "Server · 3 channels")
        XCTAssertEqual(GlobalSearch.groupSubtitle(members: 4), "Group · 4 members")
        XCTAssertEqual(GlobalSearch.linkHost("https://www.github.com/unarbos/arbos?x=1"), "github.com")
        XCTAssertEqual(GlobalSearch.linkHost("http://relay.arbos.life"), "relay.arbos.life")
        XCTAssertEqual(GlobalSearch.emptyCopy(for: " carro "), "No results for \u{201C}carro\u{201D}")
    }
}
