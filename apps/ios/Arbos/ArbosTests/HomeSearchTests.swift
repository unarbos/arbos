import ArbosFFI
import UIKit
import XCTest
@testable import Arbos

/// Home search (Jacob, Mac + iPhone, 2026-10-23): "carro" showed only the group
/// Testing, not the person Carro; "robertalan" showed Carro (handle) and Testing
/// again. Mirrors crates/arbos-mac `search::tests`.
final class HomeSearchTests: XCTestCase {
    private func none() -> [HomeSearch.MessageText] { [] }

    private func msg(_ chat: String, _ id: String, _ at: Int64, _ text: String) -> HomeSearch.MessageText {
        HomeSearch.MessageText(chatId: chat, messageId: id, createdAt: at, text: text)
    }

    func testDisplayNameFindsThePerson() {
        let hit = HomeSearch.dmHit(name: "Carro", handle: "robertalanm", messages: none, q: HomeSearch.normalizeQuery("carro"))
        XCTAssertEqual(hit, HomeSearch.Hit(kind: .person), "the row's own name explains the hit")
    }

    func testHandlePrefixFindsThePersonWithOrWithoutAt() {
        for typed in ["robertalan", "@robertalan", "ROBERTALAN", " robertalanm "] {
            let hit = HomeSearch.dmHit(name: "Carro", handle: "robertalanm", messages: none, q: HomeSearch.normalizeQuery(typed))
            XCTAssertEqual(hit?.kind, .person, typed)
        }
    }

    func testUnrelatedGroupDoesNotAppear() {
        let members = [HomeSearch.Member(handle: "kenobijon", name: "Kenobi")]
        let messages = { [self.msg("c1", "m1", 10, "Missed call from Kenobi")] }
        XCTAssertNil(HomeSearch.titledHit(title: "Testing", members: members, messages: messages, q: HomeSearch.normalizeQuery("carro")))
        XCTAssertNil(HomeSearch.titledHit(title: "Testing", members: members, messages: messages, q: HomeSearch.normalizeQuery("robertalan")))
    }

    func testGroupWithThePersonAsMemberSaysSo() {
        let members = [HomeSearch.Member(handle: "kenobijon", name: "Kenobi"), HomeSearch.Member(handle: "robertalanm", name: "Carro")]
        let hit = HomeSearch.titledHit(title: "Testing", members: members, messages: none, q: HomeSearch.normalizeQuery("carro"))
        XCTAssertEqual(hit?.kind, .member)
        XCTAssertEqual(hit?.reason, "Member · Carro @robertalanm")
        let byHandle = HomeSearch.titledHit(title: "Testing", members: members, messages: none, q: HomeSearch.normalizeQuery("robertalan"))
        XCTAssertEqual(byHandle?.reason, "Member · Carro @robertalanm")
        let noName = HomeSearch.titledHit(title: "Testing", members: [HomeSearch.Member(handle: "sam", name: nil)], messages: none, q: "sam")
        XCTAssertEqual(noName?.reason, "Member · @sam", "no name → handle only")
    }

    func testMessageBodyHitNamesTheMessage() {
        let messages = {
            [self.msg("c1", "old", 10, "ask carro about the deck"),
             self.msg("c1", "new", 20, "Carro said the launch is Tuesday, so plan for it")]
        }
        let hit = HomeSearch.titledHit(title: "Testing", members: [], messages: messages, q: HomeSearch.normalizeQuery("carro"))
        XCTAssertEqual(hit?.kind, .message)
        XCTAssertEqual(hit?.chatId, "c1")
        XCTAssertEqual(hit?.messageId, "new", "newest match wins")
        XCTAssertEqual(hit?.reason, "Message · Carro said the launch is Tuesday, so plan for it")
    }

    func testRankingPeopleThenTitlesThenMembersThenMessages() {
        let q = HomeSearch.normalizeQuery("carro")
        let kinds: [HomeSearch.Kind] = [
            HomeSearch.titledHit(title: "Research", members: [], messages: { [self.msg("c", "m", 1, "ping carro")] }, q: q)!.kind,
            HomeSearch.titledHit(title: "Testing", members: [HomeSearch.Member(handle: "robertalanm", name: "Carro")], messages: none, q: q)!.kind,
            HomeSearch.titledHit(title: "Carro's launch", members: [], messages: none, q: q)!.kind,
            HomeSearch.dmHit(name: "Carro", handle: "robertalanm", messages: none, q: q)!.kind,
        ]
        XCTAssertEqual(kinds.sorted(), [.person, .title, .member, .message])
        XCTAssertEqual(kinds, Array(kinds.sorted().reversed()))
    }

    func testCaseAndDiacriticsFoldBothWays() {
        for typed in ["zoe", "ZOË", "martin", "Martín", "zoë martin"] {
            let hit = HomeSearch.dmHit(name: "Zoë Martín", handle: "zoe-m", messages: none, q: HomeSearch.normalizeQuery(typed))
            XCTAssertEqual(hit?.kind, .person, typed)
        }
        XCTAssertNotNil(HomeSearch.dmHit(name: "Jose", handle: "jose", messages: none, q: HomeSearch.normalizeQuery("José")))
    }

    func testAgentRowsMatchNameChatTitlesAndAgents() {
        let q = HomeSearch.normalizeQuery("research")
        XCTAssertEqual(HomeSearch.titledHit(title: "Research", members: [], messages: none, q: q)?.kind, .title)
        let chats = [HomeSearch.ChatTitle(id: "c1", title: "Interface research"), HomeSearch.ChatTitle(id: "c2", title: "Budget")]
        let hit = HomeSearch.titledHit(title: "Arbos", members: [], chats: chats, messages: none, q: q)
        XCTAssertEqual(hit, HomeSearch.Hit(kind: .title, reason: "Chat · Interface research", chatId: "c1"))
        let agent = HomeSearch.titledHit(title: "Launch week", members: [HomeSearch.Member(handle: "arbos", name: "Arbos", isAgent: true)],
                                         messages: none, q: HomeSearch.normalizeQuery("arbos"))
        XCTAssertEqual(agent?.reason, "Agent · Arbos")
    }

    func testEmptyQueryShowsEverythingAndNothingMatchesGibberish() {
        XCTAssertNotNil(HomeSearch.dmHit(name: "Carro", handle: "robertalanm", messages: none, q: HomeSearch.normalizeQuery("")))
        XCTAssertNotNil(HomeSearch.dmHit(name: "Carro", handle: "robertalanm", messages: none, q: HomeSearch.normalizeQuery("@")))
        XCTAssertNil(HomeSearch.dmHit(name: "Carro", handle: "robertalanm", messages: none, q: HomeSearch.normalizeQuery("zzqx")))
    }

    func testServersMatchNameOrChannel() {
        XCTAssertEqual(HomeSearch.serverHit(name: "Bittensor", channels: ["general"], q: "bitt")?.reason, nil)
        XCTAssertEqual(HomeSearch.serverHit(name: "Bittensor", channels: ["general"], q: "gener")?.reason, "#general")
        XCTAssertNil(HomeSearch.serverHit(name: "Bittensor", channels: ["general"], q: "carro"))
    }

    func testSnippetsCutAroundTheMatch() {
        let long = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty twenty-one twenty-two"
        let s = HomeSearch.snippet(long, q: "ten")!
        XCTAssertTrue(s.hasPrefix("…"), s)
        XCTAssertTrue(s.contains("ten"), s)
        XCTAssertTrue(s.hasSuffix("…"), s)
        XCTAssertEqual(HomeSearch.snippet("Zoë\nwas here", q: "zoe"), "Zoë was here")
        XCTAssertNil(HomeSearch.snippet("plain", q: "x"))
    }
}

/// The home list filter — All · Agents · People · Servers (Mac parity,
/// 2026-10-24; the Mac twin is `sidebar::strip_tests` in crates/arbos-mac).
final class HomeListFilterTests: XCTestCase {
    func testTheFourSegmentsInOrder() {
        XCTAssertEqual(HomeListFilter.allCases.map(\.rawValue), ["All", "Agents", "People", "Servers"])
    }

    func testAllShowsEverything() {
        XCTAssertTrue(HomeListFilter.all.showsProjects)
        XCTAssertTrue(HomeListFilter.all.showsPeople)
        XCTAssertTrue(HomeListFilter.all.showsServers)
        XCTAssertTrue(HomeListFilter.all.admitsProject(shared: true))
    }

    func testAgentsShowsMyAgentsOnly() {
        XCTAssertTrue(HomeListFilter.agents.admitsProject(shared: false))
        XCTAssertFalse(HomeListFilter.agents.admitsProject(shared: true), "a shared-in agent stays out")
        XCTAssertFalse(HomeListFilter.agents.showsPeople)
        XCTAssertFalse(HomeListFilter.agents.showsServers)
    }

    func testPeopleShowsPeopleChatsOnly() {
        XCTAssertTrue(HomeListFilter.people.showsPeople)
        XCTAssertFalse(HomeListFilter.people.showsProjects)
        XCTAssertFalse(HomeListFilter.people.admitsProject(shared: false))
        XCTAssertFalse(HomeListFilter.people.showsServers)
    }

    func testServersShowsServersOnly() {
        XCTAssertTrue(HomeListFilter.servers.showsServers)
        XCTAssertFalse(HomeListFilter.servers.showsProjects)
        XCTAssertFalse(HomeListFilter.servers.showsPeople)
        XCTAssertEqual(HomeListFilter.servers.emptyCopy, "No servers yet")
    }

    // MARK: the row does not move when the selection does

    private let labels = HomeListFilter.allCases.map(\.rawValue)

    private func labelWidth(_ text: String, size: CGFloat, weight: UIFont.Weight) -> CGFloat {
        (text as NSString).size(withAttributes: [.font: UIFont.systemFont(ofSize: size, weight: weight)]).width
    }

    /// Home's filter row as `PaneTabs` lays it out: four labels, each in
    /// `Theme.body` at the Text Size `scale`, `labelPad` on both sides of the
    /// label and `gap` between two capsules.
    ///
    /// `reserveSelectedWidth` is the rule this change introduces — every tab is
    /// as wide as its label in the selected (semibold) weight. `false` is what
    /// shipped before it: only the selected label was semibold, so the row's
    /// width followed the selection.
    private func rowWidth(selected: String, scale: CGFloat, reserveSelectedWidth: Bool) -> CGFloat {
        let size = (Theme.body * scale * 2).rounded() / 2
        let text = labels.reduce(CGFloat(0)) { sum, t in
            let bold = reserveSelectedWidth || t == selected
            return sum + labelWidth(t, size: size, weight: bold ? .semibold : .regular)
        }
        let n = CGFloat(labels.count)
        return text + n * 2 * PaneTabsMetrics.labelPad + n * PaneTabsMetrics.gap
    }

    /// Scout, 2026-10-31: selecting a filter made its label bolder, so the tab
    /// grew and pushed the others sideways. Reserving the semibold width for
    /// every tab holds the row still whichever one is on.
    func testSelectingATabDoesNotMoveTheRow() {
        for scale in TextScale.steps {
            let widths = labels.map { rowWidth(selected: $0, scale: scale, reserveSelectedWidth: true) }
            for (t, w) in zip(labels, widths) {
                XCTAssertEqual(w, widths[0], accuracy: 0.01,
                               "\(t) selected at \(scale)×: the row is the width it is with All selected")
            }
            let before = labels.map { rowWidth(selected: $0, scale: scale, reserveSelectedWidth: false) }
            XCTAssertGreaterThan((before.max() ?? 0) - (before.min() ?? 0), 0.5,
                                 "the old rule did move the row at \(scale)× — this test can tell them apart")
        }
    }

    /// What holding the row still costs in width: the row gets wider, and only
    /// barely. The cost is the three unselected labels' regular → semibold
    /// delta, so it is bounded here as a *fraction* of the row rather than in
    /// points, at every Text Size.
    ///
    /// Deliberately no absolute width and no screen-width comparison. Two
    /// earlier versions of this test asserted the row fits a named phone; both
    /// were guesses at SF Pro's metrics written on a machine with no Mac and no
    /// Swift toolchain to measure on, and the gate failed both. How many points
    /// wide the row is, is a measurement, not an invariant. The invariant is
    /// that reserving the selected weight does not balloon the row, and that
    /// holds whatever the face. (`PaneTabs` is a bare `HStack` with no scroll,
    /// so a row wider than the screen is squeezed — that predates this change;
    /// see § Chat-list filter in docs/mac-design-rules.md.)
    func testReservingTheSelectedWidthBarelyWidensTheRow() {
        for scale in TextScale.steps {
            let after = rowWidth(selected: "All", scale: scale, reserveSelectedWidth: true)
            let before = rowWidth(selected: "All", scale: scale, reserveSelectedWidth: false)
            XCTAssertGreaterThanOrEqual(after, before, "at \(scale)× the reserved row is never the narrower one")
            XCTAssertLessThan(after - before, before / 20,
                              "at \(scale)× reserving the semibold width costs under 5% of the row")
        }
    }
}

/// The shared search matcher ([feedback shared-search-matcher-ios-2026-10-26]):
/// Jacob, Mac v0.6.268, 2026-10-26 — "Give a person access" with "aff" typed
/// listed only "Andreas F @aff"; every picker had its own handle-or-substring
/// filter. One rule now, in the Rust core (`arbos_core::search`) through
/// ArbosFFI (`EntitySearch`), for people, agents, groups, servers and
/// channels on both platforms. Twin of crates/arbos-mac `picker_sites_tests`.
final class SharedSearchTests: XCTestCase {
    private func person(_ handle: String, _ name: String?) -> Person {
        Person(id: "gh_\(handle)", handle: handle, name: name)
    }

    private var directory: [Person] {
        [person("doug", "Doug"), person("team-affine", "Team Affine"), person("aff", "Andreas F"),
         person("affine-foundation", "Affine"), person("robertalanm", "Carro"), person("affteam", "AffineTeam")]
    }

    func testAffFindsDisplayNamesAndTheHandleRanked() {
        let rows = PeoplePicker.people(directory: directory, remote: [], me: nil, query: "aff")
        XCTAssertEqual(rows.map(\.handle), ["aff", "affine-foundation", "affteam", "team-affine"],
                       "exact @aff, then the names that start with aff (directory order), then the word-start")
        // The relay's handle-prefix answer alone is what Jacob saw.
        XCTAssertEqual(PeoplePicker.people(directory: [], remote: [person("aff", "Andreas F")], me: nil, query: "aff").map(\.handle), ["aff"])
    }

    func testRemoteHitsJoinTheDirectoryOnceAndNeverMe() {
        let remote = [person("aff", "Andreas F"), person("affable", "Affable Bot"), person("me", "Affine Me")]
        let rows = PeoplePicker.people(directory: directory, remote: remote, me: "gh_me", query: "@Aff")
        XCTAssertEqual(rows.map(\.handle), ["aff", "affine-foundation", "affteam", "affable", "team-affine"])
        XCTAssertEqual(rows.filter { $0.handle == "aff" }.count, 1, "one row per person")
    }

    func testEmptyQueryKeepsDirectoryOrderAndDiacriticsFold() {
        XCTAssertEqual(PeoplePicker.people(directory: directory, remote: [], me: nil, query: "").map(\.handle), directory.map(\.handle))
        let dir = [person("zoe-m", "Zoë Martín"), person("jose", "José")]
        XCTAssertEqual(PeoplePicker.people(directory: dir, remote: [], me: nil, query: "martin").map(\.handle), ["zoe-m"])
        XCTAssertEqual(PeoplePicker.people(directory: dir, remote: [], me: nil, query: "JOSÉ").map(\.handle), ["jose"])
        XCTAssertEqual(PeoplePicker.people(directory: dir, remote: [], me: nil, query: "zoe mar").map(\.handle), ["zoe-m"])
        XCTAssertTrue(PeoplePicker.people(directory: dir, remote: [], me: nil, query: "oe").isEmpty, "no mid-word substrings")
    }

    func testMatcherRanksExactPrefixWordStartNameBeforeHandle() {
        let exactHandle = EntitySearch.match(name: "Andreas F", handle: "aff", query: "aff")
        let prefixName = EntitySearch.match(name: "Affine", handle: "affine-foundation", query: "aff")
        let wordStart = EntitySearch.match(name: "Team Affine", handle: "team", query: "aff")
        XCTAssertEqual(exactHandle?.kind, .exact); XCTAssertEqual(exactHandle?.field, .handle)
        XCTAssertEqual(prefixName?.kind, .prefix); XCTAssertEqual(prefixName?.field, .name)
        XCTAssertEqual(wordStart?.kind, .wordStart)
        XCTAssertLessThan(exactHandle!.order, prefixName!.order)
        XCTAssertLessThan(prefixName!.order, wordStart!.order)
        XCTAssertEqual(EntitySearch.match(name: "Sam", handle: "sam", query: "sam")?.field, .name, "name before handle on a tie")
        XCTAssertNil(EntitySearch.match(name: "Carro", handle: "robertalanm", query: "alan"))
        XCTAssertEqual(EntitySearch.match(name: "Andreas F", handle: "aff", query: "an f")?.kind, .wordStart, "multi-word, in order")
        XCTAssertNil(EntitySearch.match(name: "Andreas F", handle: "aff", query: "f an"))
        XCTAssertEqual(EntitySearch.match(name: "Comms", handle: "comms", title: "Research assistant", query: "res")?.field, .title)
        XCTAssertEqual(EntitySearch.fold("Øyvind Ångström"), "oyvind angstrom")
        XCTAssertEqual(EntitySearch.normalize("@Zoë  B"), "zoe b")
        XCTAssertNotNil(EntitySearch.match(name: "A", handle: "a", query: " "), "empty query matches everything")
    }

    func testAgentAndServerPickersUseTheMatcher() {
        let project = { (name: String, description: String) -> Project in
            var p = Project(id: "p-\(name)", name: name, icon: "A", color: "#000000", state: .awake, stateSince: nil, stateDetail: nil,
                            model: "", effort: .low, spendTodayUsd: 0, spendCapUsd: 0)
            p.description = description
            return p
        }
        let agents = [project("Comms", "Research assistant"), project("Research Bot", ""), project("Affine Research", ""), project("Doug", "")]
        XCTAssertEqual(PeoplePicker.agents(agents, query: "res").map(\.name), ["Research Bot", "Affine Research", "Comms"])
        XCTAssertEqual(PeoplePicker.agents(agents, query: "@affine-res").map(\.name), ["Affine Research"])
        XCTAssertEqual(PeoplePicker.agents(agents, query: "").map(\.name), agents.map(\.name))
        XCTAssertEqual(PeoplePicker.titled(["Deploy token", "Token zoe", "Zoë's key"], query: "zoe") { $0 }, ["Zoë's key", "Token zoe"],
                       "prefix before word-start; diacritics fold")
    }

    func testMentionPickerRanksNameBeforeHandle() {
        func human(_ id: String, _ handle: String, _ name: String?) -> ChatMember {
            ChatMember(id: id, kind: .user, member: "user:\(id)", role: "member", handle: handle, avatarUrl: nil,
                       name: name, status: "member", joinedAt: 0, project: nil, ownerId: nil, ownerHandle: nil, replyPolicy: nil)
        }
        let members = [human("gh_aff", "aff", "Andreas F"), human("gh_affine", "affine-foundation", "Affine"), human("gh_zoe", "zoe-m", "Zoë Martín")]
        XCTAssertEqual(MentionDraft.candidates(query: "aff", members: members, me: nil).map(\.id), ["gh_aff", "gh_affine"], "exact handle, then prefix name")
        XCTAssertEqual(MentionDraft.candidates(query: "affi", members: members, me: nil).map(\.id), ["gh_affine"])
        XCTAssertEqual(MentionDraft.candidates(query: "martin", members: members, me: nil).map(\.id), ["gh_zoe"], "a word of the name, diacritics folded")
        XCTAssertTrue(MentionDraft.candidates(query: "ffin", members: members, me: nil).isEmpty)
    }

    // MARK: - Every picker routes through the matcher (structure)

    private static let appRoot: URL = {
        // apps/ios/Arbos/ArbosTests/ThisFile.swift → apps/ios/Arbos/Arbos
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Arbos")
    }()

    private func source(_ rel: String) throws -> String {
        try String(contentsOf: Self.appRoot.appendingPathComponent(rel), encoding: .utf8)
    }

    /// The text of the first declaration containing `anchor`, up to the next `}` at its indentation.
    private func body(_ src: String, _ anchor: String) -> String {
        guard let start = src.range(of: anchor) else { return "" }
        let lineStart = src[..<start.lowerBound].lastIndex(of: "\n").map { src.index(after: $0) } ?? src.startIndex
        let indent = src[lineStart..<start.lowerBound].prefix { $0 == " " }
        let closing = "\n\(indent)}"
        let end = src.range(of: closing, range: start.upperBound..<src.endIndex)?.upperBound ?? src.endIndex
        return String(src[lineStart..<end])
    }

    /// (file, anchor of the site, markers that mean "through the matcher").
    private let sites: [(String, String, [String])] = [
        // Home / app-wide search rows: person, title, member, agent, chat title, server, channel.
        ("Model/HomeSearch.swift", "static func dmHit(", ["EntitySearch.matches("]),
        ("Model/HomeSearch.swift", "static func titledHit(", ["EntitySearch.matches(", "EntitySearch.matchText("]),
        ("Model/HomeSearch.swift", "static func serverHit(", ["EntitySearch.matchText("]),
        ("Store/SearchStore.swift", "func globalSearchEntries(", ["EntitySearch.matchText(", "Rank.of(name:"]),
        ("Model/GlobalSearch.swift", "static func of(name: String, handle: String", ["EntitySearch.match("]),
        // New message sheet: agents, servers, people.
        ("UI/PeopleSheets.swift", "private var agents: [Project]", ["PeoplePicker.agents("]),
        ("UI/PeopleSheets.swift", "private var servers: [Server]", ["PeoplePicker.servers("]),
        ("UI/PeopleSheets.swift", "private var people: [Person]", ["PeoplePicker.people("]),
        // New group / Add members / Add server members / Give a person access.
        ("UI/PeopleSheets.swift", "private var agentCandidates: [Project]", ["PeoplePicker.agents("]),
        ("UI/PeopleSheets.swift", "private var candidates: [Person]", ["PeoplePicker.people("]),
        // Project members: invite suggestions.
        ("UI/PeopleSheets.swift", "private var suggestions: [Person]", ["PeoplePicker.people("]),
        // The picker helpers themselves.
        ("Model/HomeSearch.swift", "static func people(directory:", ["EntitySearch.filter("]),
        ("Model/HomeSearch.swift", "static func agents(_ agents:", ["EntitySearch.filter("]),
        ("Model/HomeSearch.swift", "static func servers(_ servers:", ["EntitySearch.filter("]),
        ("Model/HomeSearch.swift", "static func titled<T>(", ["EntitySearch.filter("]),
        // Secrets: a vault's items by title.
        ("UI/SecretsViews.swift", "private var filtered: [[String: Any]]", ["PeoplePicker.titled("]),
        // Secrets: the agent picker (New secret, Connect 1Password, Give access on a secret / a vault).
        ("UI/AgentPicker.swift", "static func candidates(_ projects:", ["PeoplePicker.agents("]),
        // @mention completion.
        ("UI/MentionComposer.swift", "static func candidates(query:", ["EntitySearch.match("]),
        // Forward-to chat picker.
        ("UI/MediaViewer.swift", "func forwardTargets(matching", ["EntitySearch.filter("]),
        // The fixture's people search.
        ("Store/AppModel.swift", "func searchPeople(", ["PeoplePicker.people("]),
    ]

    private let adHoc = ["lowercased().contains(q", "lowercased().contains(needle", "localizedCaseInsensitiveContains(search", "fold(", ".hasPrefix(q)"]

    func testEveryPickerRoutesThroughTheSharedMatcher() throws {
        for (file, anchor, markers) in sites {
            let src = try source(file)
            let text = body(src, anchor)
            XCTAssertFalse(text.isEmpty, "\(file): \(anchor) not found")
            for m in markers {
                XCTAssertTrue(text.contains(m), "\(file) \(anchor): expected \(m) in\n\(text)")
            }
            for bad in adHoc where !(file == "Model/HomeSearch.swift" && bad == "fold(") {
                XCTAssertFalse(text.contains(bad), "\(file) \(anchor): ad-hoc filter \(bad)")
            }
        }
    }

    func testNoPickerFileKeepsAnAdHocFilter() throws {
        for file in ["UI/PeopleSheets.swift", "UI/MentionComposer.swift", "UI/SecretsViews.swift", "Store/SearchStore.swift", "Model/GlobalSearch.swift"] {
            let src = try source(file)
            for (n, line) in src.components(separatedBy: "\n").enumerated() {
                for bad in ["lowercased().contains(q", "HomeSearch.fold(", "localizedCaseInsensitiveContains("] {
                    XCTAssertFalse(line.contains(bad), "\(file):\(n + 1): ad-hoc filter \(bad): \(line)")
                }
            }
        }
    }
}

/// Home pins synced through the relay ([feedback sidebar-pinning-ios-2026-10-28]): the
/// one-time migration of the device-local list, the row keys, and the list's rules on the
/// model (top insert, cap, unpin, reorder, archive unpins, the relay's answer wins).
final class HomePinsTests: XCTestCase {
    override func setUp() {
        super.setUp()
        for key in [HomePins.cacheKey, HomePins.legacyKey, HomePins.migratedKey] {
            UserDefaults.standard.removeObject(forKey: key)
        }
    }

    func testLegacyLocalPinsBecomeRelayRowKeysOnceInOrder() {
        let legacy = ["p:01A", "c:01B", "p:01A", "x:junk", "p:", "server:01S", "c:01C", "p:01D"]
        XCTAssertEqual(
            HomePins.migrateLegacy(legacy, cap: 5),
            ["agent:01A", "chat:01B", "server:01S", "chat:01C", "agent:01D"],
            "p: → agent:, c: → chat:, duplicates folded, junk and empty ids dropped, order kept"
        )
        XCTAssertEqual(HomePins.migrateLegacy(legacy, cap: 2), ["agent:01A", "chat:01B"], "capped like the relay")
        XCTAssertEqual(HomePins.migrateLegacy([], cap: 5), [])

        // Before `/me/pins` answers, a fresh build shows the legacy list translated — nothing vanishes.
        UserDefaults.standard.set(["p:01A", "c:01B"], forKey: HomePins.legacyKey)
        XCTAssertEqual(HomePins.cached(), ["agent:01A", "chat:01B"])
        // Once the relay confirmed a list, that list is what launches show.
        HomePins.cache(["server:01S"])
        XCTAssertEqual(HomePins.cached(), ["server:01S"])
        XCTAssertFalse(HomePins.migrated())
        HomePins.markMigrated()
        XCTAssertTrue(HomePins.migrated())
    }

    func testRowKeysAreTheMacsAndTheRelays() {
        XCTAssertEqual(HomePins.key(projectId: "01A"), "agent:01A")
        XCTAssertEqual(HomePins.key(peopleChatId: "01B"), "chat:01B")
        XCTAssertEqual(HomePins.key(serverId: "01S"), "server:01S")
    }

    @MainActor
    func testPinsLandOnTopStopAtTheCapAndReorder() {
        let model = AppModel()
        model.isFixture = true
        model.pinHome(projectId: "a1")
        model.pinHome(peopleChatId: "c1")
        model.pinHome(serverId: "s1")
        XCTAssertEqual(model.pinnedHomeIds, ["server:s1", "chat:c1", "agent:a1"], "a new pin lands at the top")
        XCTAssertTrue(model.isHomePinned(serverId: "s1"))
        XCTAssertEqual(model.homePinIndex(projectId: "a1"), 2)
        XCTAssertNil(model.homePinIndex(projectId: "nope"))

        model.pinHome(projectId: "a2")
        model.pinHome(projectId: "a3")
        model.pinHome(projectId: "a4")
        XCTAssertEqual(model.pinnedHomeIds.count, 5, "the cap refuses the sixth")
        XCTAssertFalse(model.isHomePinned(projectId: "a4"))
        XCTAssertTrue(model.toasts.contains { $0.error?.sentence.contains("pin up to 5") == true }, "one sentence says why")

        model.moveHomePinToTop(projectId: "a1")
        XCTAssertEqual(model.pinnedHomeIds.first, "agent:a1")
        model.moveHomePinDown(projectId: "a1")
        XCTAssertEqual(model.pinnedHomeIds[1], "agent:a1")
        model.moveHomePinUp(projectId: "a1")
        XCTAssertEqual(model.pinnedHomeIds.first, "agent:a1")

        model.unpinHome(peopleChatId: "c1")
        XCTAssertFalse(model.isHomePinned(peopleChatId: "c1"))
        model.archive(serverId: "s1")
        XCTAssertFalse(model.isHomePinned(serverId: "s1"), "archiving unpins")
        XCTAssertEqual(UserDefaults.standard.stringArray(forKey: HomePins.cacheKey), model.pinnedHomeIds, "cached for the next launch")

        model.applyRemotePins(["chat:c9"])
        XCTAssertEqual(model.pinnedHomeIds, ["chat:c9"], "another device's list replaces mine")
    }
}

/// Scout (Arbonites, 2026-10-29): "it also unarchives the chat if you open it"
/// ([feedback archived-chat-stays-archived-2026-10-29]). Opening a row from Hidden chats only
/// views it; new activity from someone else (`unarchiveOnActivity`) and Unarchive bring it back.
/// Mirrors crates/arbos-mac `hidden_chat_open_tests`.
@MainActor
final class HiddenChatOpenTests: XCTestCase {
    override func setUp() {
        super.setUp()
        for key in ["archivedProjects", "archivedPeopleChats", "archivedServers", HomePins.cacheKey] {
            UserDefaults.standard.removeObject(forKey: key)
        }
    }

    private func hiddenModel() -> AppModel {
        let model = AppModel()
        model.isFixture = true
        model.chats = ["a1": [Chat(id: "a1-main", projectId: "a1", title: "Main", isPinned: true, unread: false, lastMessageAt: 0)]]
        model.peopleChats = [PeopleChat(id: "dm1", kind: "dm", createdBy: "gh_me", createdAt: 0, title: "", pending: false,
                                        members: [], headSeq: 0, readSeq: 0, unread: false, lockState: "people_only",
                                        lastActivityAt: 0, myRole: "owner", muted: false, silenced: false)]
        model.archive("a1")
        model.archive(peopleChatId: "dm1")
        return model
    }

    func testOpeningAHiddenAgentChatOrDMOnlyViewsIt() throws {
        let model = hiddenModel()
        model.open(chat: try XCTUnwrap(model.chats["a1"]?.first))
        XCTAssertEqual(model.path, [.chat(chatId: "a1-main", projectId: "a1")])
        model.open(peopleChat: try XCTUnwrap(model.peopleChat("dm1")))
        XCTAssertEqual(model.path.count, 1)
        XCTAssertTrue(model.isArchived("a1"), "the agent stays in Hidden chats")
        XCTAssertTrue(model.isArchived(peopleChatId: "dm1"), "the DM stays in Hidden chats")
        XCTAssertNotNil(model.peopleChat("dm1")?.archivedAt)
    }

    func testNewActivityAndUnarchiveStillBringTheRowBack() {
        let model = hiddenModel()
        model.unarchiveOnActivity(chatId: "dm1")
        XCTAssertFalse(model.isArchived(peopleChatId: "dm1"), "someone wrote: back in the list")
        model.unarchiveOnActivity(chatId: "a1-main")
        XCTAssertFalse(model.isArchived("a1"))
        model.archive("a1")
        model.unarchive("a1")
        XCTAssertFalse(model.isArchived("a1"))
    }
}

/// Affinity (Arbonites, 2026-10-29): starting a chat from search made a permanent chat and
/// notified both people before anything was sent ([feedback
/// draft-chat-from-search-ios-2026-10-29]). It is a local draft now: no `POST /chats/dm`
/// (the fixture's stand-in is `fixtureOpenDM`) until the first send; backing out drops it.
@MainActor
final class DraftDMTests: XCTestCase {
    private let draft = "draft-dm:mira-k"

    private func fixtureModel() -> AppModel {
        let model = AppModel()
        model.loadFixture()
        return model
    }

    private func dmWithMira(_ model: AppModel) -> PeopleChat? {
        model.peopleChats.first { $0.kind == "dm" && $0.humans.contains { $0.handle == "mira-k" } }
    }

    func testStartingAChatFromSearchCreatesNothing() async {
        let model = fixtureModel()
        let before = model.peopleChats.map(\.id)
        model.open(searchTarget: .person(handle: "mira-k"))
        for _ in 0..<20 where model.draftDM == nil { try? await Task.sleep(for: .milliseconds(20)) }
        XCTAssertEqual(model.path, [.chat(chatId: draft, projectId: peopleSlot)], "the draft opens")
        XCTAssertEqual(model.peopleChats.map(\.id), before, "no chat was made, so nobody was told")
        XCTAssertNil(dmWithMira(model))
        let peer = model.peopleChat(draft)?.counterpart(me: model.user?.id)
        XCTAssertEqual(peer?.handle, "mira-k")
        XCTAssertEqual(peer?.name, "Mira Kaplan", "the header names her")
    }

    func testBackingOutDropsTheDraft() async {
        let model = fixtureModel()
        _ = await model.openDM(handle: "mira-k")
        XCTAssertNotNil(model.draftDM)
        model.setComposerDraft("half a thought", for: draft)
        model.path = []
        XCTAssertNil(model.draftDM, "the draft is gone")
        XCTAssertNil(model.peopleChat(draft))
        XCTAssertEqual(model.composerDraft(for: draft), "", "its typed text too")
        XCTAssertNil(dmWithMira(model), "nothing was made")
    }

    func testTheFirstSendMakesTheDMAndGoesOutThere() async throws {
        let model = fixtureModel()
        _ = await model.openDM(handle: "mira-k")
        model.sendPeople(chatId: draft, text: "hey Mira", replyTo: nil)
        for _ in 0..<50 where model.draftDM != nil { try? await Task.sleep(for: .milliseconds(20)) }
        let dm = try XCTUnwrap(dmWithMira(model), "the first send made the DM")
        XCTAssertNil(model.draftDM)
        XCTAssertEqual(model.path, [.chat(chatId: dm.id, projectId: peopleSlot)], "the screen is the DM now")
        XCTAssertEqual(model.messages(of: dm.id).last?.plainText, "hey Mira")
    }

    func testADMIAlreadyHaveOpensAsBefore() async {
        let model = fixtureModel()
        let dm = await model.openDM(handle: "unconst")
        XCTAssertEqual(dm?.id, "fx-dm-unconst")
        XCTAssertEqual(model.path, [.chat(chatId: "fx-dm-unconst", projectId: peopleSlot)])
        XCTAssertNil(model.draftDM)
    }
}
