import ArbosFFI
import XCTest
@testable import Arbos

/// Jacob, 2026-10-26 ([feedback search-settings-ios-2026-10-26]): the bottom
/// search also finds Settings — every page of the Settings list, its sub-pages
/// and the words people type for them, through the shared index
/// (`arbos_core::settings_search` over ArbosFFI). A Settings section lists
/// after Servers & channels and before Messages, first of all when the query
/// *is* a title or keyword; a title hit says where the page lives, a keyword
/// hit names the keyword; Model · Reasoning · Voice come once per owned agent.
final class SearchSettingsTests: XCTestCase {
    private var agents: [GlobalSearch.AgentRow] {
        [
            GlobalSearch.AgentRow(projectId: "p-arbos", name: "Arbos", model: "gpt-6.1-sol", reasoning: "High", voice: "Default"),
            GlobalSearch.AgentRow(projectId: "p-comms", name: "Comms", model: "gpt-6.1-sol", reasoning: "High", voice: "Rex"),
            GlobalSearch.AgentRow(projectId: "p-affine", name: "Affine", model: "gpt-6.1-sol", reasoning: "High", voice: "Ara"),
        ]
    }

    private func entry(_ kind: GlobalSearch.Kind, _ title: String, when: Int64, rank: GlobalSearch.Rank,
                       target: GlobalSearch.Target) -> GlobalSearch.Entry {
        GlobalSearch.Entry(kind: kind, title: title, subtitle: "", when: when, rank: rank, target: target)
    }

    private func titles(_ groups: [GlobalSearch.Group], _ section: GlobalSearch.Section) -> [String] {
        groups.first { $0.section == section }?.rows.map { $0.title } ?? []
    }

    private func sections(_ groups: [GlobalSearch.Group]) -> [GlobalSearch.Section] {
        groups.map { $0.section }
    }

    /// The targets of the Settings rows the typed query finds (no agents).
    private func targets(_ typed: String) -> [GlobalSearch.Target] {
        GlobalSearch.settingsEntries(query: typed, agents: []).map { $0.target }
    }

    // MARK: Ranking

    /// "dev": Devin (a name hit) stays first; Devices follows in its own section,
    /// after Servers & channels and before Messages.
    func testPrefixHitListsAfterPeopleChatsAndServers() {
        let q = HomeSearch.normalizeQuery("dev")
        var rows = [
            entry(.person, "Devin", when: 10, rank: .of(name: "Devin", handle: "devin", q: q), target: .people("dm-1")),
            entry(.server, "Dev Crew", when: 5, rank: .of(name: "Dev Crew", handle: "", q: q), target: .server("s1")),
            entry(.message, "Arbos", when: 99, rank: .indirect, target: .message(chatId: "c", projectId: "p", messageId: "m")),
        ]
        rows += GlobalSearch.settingsEntries(query: q, agents: agents)
        let groups = GlobalSearch.arrange(rows, queryIsEmpty: false)
        let expected: [GlobalSearch.Section] = [.people, .servers, .settings, .messages]
        XCTAssertEqual(sections(groups), expected)
        XCTAssertEqual(titles(groups, .settings), ["Devices"])
        let devices: GlobalSearch.Entry = groups[2].rows[0]
        XCTAssertEqual(devices.subtitle, "Settings")
        XCTAssertEqual(devices.target, GlobalSearch.Target.setting(.page(.devices)))
        XCTAssertEqual(devices.picture, GlobalSearch.Picture.settingsTile("laptopcomputer"))
    }

    /// "opacity": no page is called that; Appearance's keyword is an exact hit, so
    /// Settings jumps first and the keyword leads the subtitle.
    func testExactKeywordPutsSettingsFirstAndNamesTheKeyword() {
        let q = HomeSearch.normalizeQuery("Opacity")
        var rows = [entry(.message, "Affine", when: 99, rank: .indirect, target: .message(chatId: "c", projectId: "p", messageId: "m"))]
        rows += GlobalSearch.settingsEntries(query: q, agents: agents)
        let groups = GlobalSearch.arrange(rows, queryIsEmpty: false)
        let expected: [GlobalSearch.Section] = [.settings, .messages]
        XCTAssertEqual(sections(groups), expected)
        let appearance: GlobalSearch.Entry = groups[0].rows[0]
        XCTAssertEqual(appearance.title, "Appearance")
        XCTAssertEqual(appearance.subtitle, "Opacity · Settings")
        XCTAssertEqual(appearance.target, GlobalSearch.Target.setting(.page(.appearance)))
        // Jacob's other examples.
        XCTAssertEqual(targets("theme").first, GlobalSearch.Target.setting(.page(.appearance)))
        XCTAssertEqual(targets("token").first, GlobalSearch.Target.setting(.page(.secrets)))
        XCTAssertEqual(targets("1Password").first, GlobalSearch.Target.setting(.page(.secrets)))
    }

    /// "voice": one row per owned agent in the order given (open chat's agent
    /// first), each with its current value; the title is exact, so first.
    func testAgentSettingsComeOncePerOwnedAgent() {
        let groups = GlobalSearch.arrange(GlobalSearch.settingsEntries(query: "voice", agents: agents), queryIsEmpty: false)
        XCTAssertEqual(groups.first?.section, GlobalSearch.Section.settings)
        let voice: [GlobalSearch.Entry] = groups[0].rows
        XCTAssertEqual(voice.count, 3)
        XCTAssertTrue(voice.allSatisfy { $0.title == "Voice" })
        XCTAssertEqual(voice.map { $0.subtitle }, ["Arbos · Settings · Default", "Comms · Settings · Rex", "Affine · Settings · Ara"])
        XCTAssertEqual(voice[0].target, GlobalSearch.Target.setting(.agent(projectId: "p-arbos", setting: .voice)))
        XCTAssertEqual(voice[0].picture, GlobalSearch.Picture.project("p-arbos"))
        let model: [GlobalSearch.Entry] = GlobalSearch.settingsEntries(query: "model", agents: agents)
        XCTAssertEqual(model.first?.target, GlobalSearch.Target.setting(.agent(projectId: "p-arbos", setting: .model)))
        let reasoning: [GlobalSearch.Entry] = GlobalSearch.settingsEntries(query: "reasoning", agents: agents)
        XCTAssertEqual(reasoning.first?.target, GlobalSearch.Target.setting(.agent(projectId: "p-arbos", setting: .reasoning)))
        XCTAssertTrue(GlobalSearch.settingsEntries(query: "voice", agents: []).isEmpty, "no owned agents, no rows")
    }

    /// Title hits before keyword hits; a section caps at eight like every other.
    func testTitleHitsOutrankKeywordHitsAndNothingShowsWithoutAQuery() {
        XCTAssertEqual(targets("about").first, GlobalSearch.Target.setting(.page(.about)))
        XCTAssertTrue(GlobalSearch.settingsEntries(query: "", agents: agents).isEmpty)
        let recent = GlobalSearch.arrange(GlobalSearch.settingsEntries(query: "", agents: agents), queryIsEmpty: true)
        XCTAssertTrue(recent.isEmpty, "Settings never show in Recent")
        var many = agents
        for i in 0..<10 { many.append(GlobalSearch.AgentRow(projectId: "p-\(i)", name: "A\(i)", model: "", reasoning: "", voice: "")) }
        let capped = GlobalSearch.arrange(GlobalSearch.settingsEntries(query: "voice", agents: many), queryIsEmpty: false)
        XCTAssertEqual(capped[0].rows.count, GlobalSearch.sectionCap)
        XCTAssertEqual(capped[0].rows[0].subtitle, "Arbos · Settings · Default", "the open chat's agent stays first")
    }

    // MARK: Navigation targets and structure

    /// Every regex match in `text`, as its capture groups (group 0 is the whole match).
    private func matches(_ pattern: String, in text: String) throws -> [[String]] {
        try NSRegularExpression(pattern: pattern)
            .matches(in: text, range: NSRange(text.startIndex..., in: text))
            .map { m in
                (0 ..< m.numberOfRanges).map { i in
                    Range(m.range(at: i), in: text).map { String(text[$0]) } ?? ""
                }
            }
    }

    /// `apps/ios/Arbos/Arbos/<path>`, from this test file's own location.
    private func appSource(_ path: String) throws -> String {
        try String(
            contentsOf: URL(fileURLWithPath: #filePath)
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .appendingPathComponent("Arbos/" + path),
            encoding: .utf8
        )
    }

    /// The body of `SettingsView.pageDestination`, which is the whole set of pages
    /// the Settings sheet can push.
    private func pageDestinationBody() throws -> String {
        let source = try appSource("UI/SettingsView.swift")
        guard let start = source.range(of: "private func pageDestination("),
              let end = source.range(of: "\n    }", range: start.upperBound ..< source.endIndex)
        else {
            XCTFail("pageDestination not found in SettingsView.swift — this test is reading the wrong thing")
            return ""
        }
        return String(source[start.lowerBound ..< end.upperBound])
    }

    /// Relay shipped with a page, a destination and a deep link but no route, so
    /// search could not find it. That correspondence is now the compiler's job:
    /// `pageDestination` switches over `SettingsPageRoute` exhaustively, so a route
    /// without a page does not build and a page is opened only by naming its route.
    ///
    /// This guards the guarantee rather than the case. A `default` arm would make
    /// the switch compile again with a route that draws nothing — the exact trade
    /// the exhaustive-switch rule exists to refuse — and no compiler would object.
    func testThePageDestinationSwitchCannotFallThrough() throws {
        let body = try pageDestinationBody()

        // The floor: without it this passes just as happily when the read returns
        // something with no arms in it. Measured at 12 when written — the pattern
        // names one route per `case` line, and the vault's line lists three.
        let arms = try matches(#"case \.(\w+)"#, in: body).map { $0[1] }
        XCTAssertGreaterThan(arms.count, 8, "found \(arms.count) destination arms — this is reading the wrong thing")

        XCTAssertFalse(
            body.contains("default:"),
            "pageDestination has a default arm, so a new SettingsPageRoute would silently draw nothing"
        )
        // `_` as a whole arm, not a binding that merely starts with one: `case _foo`
        // is an ordinary pattern and must not read as a catch-all.
        XCTAssertNil(
            try matches(#"case +_\s*[:,]"#, in: body).first,
            "pageDestination has a wildcard arm, so a new SettingsPageRoute would silently draw nothing"
        )
    }

    /// A page can be compiled out of the shipping build while its index entry is
    /// not. `RelayPage` and the Relay row are both inside `#if DEBUG`, so a Release
    /// build has no page to land on, while the shared index carries the entry for
    /// every build. Left alone, Release search offers a result that pushes nothing.
    ///
    /// **The Swift suite builds Debug, so no run of it can observe the Release
    /// configuration.** This reads the source instead, which is the only instrument
    /// here that sees both: a destination compiled out under `#if DEBUG` must have
    /// its route compiled out with it, so the hit is dropped rather than dead.
    ///
    /// **It covers one gating mechanism: `#if DEBUG`.** A destination compiled out
    /// by any other means — an availability or OS conditional, a feature flag, a
    /// custom compilation condition — passes this check while leaving a live index
    /// entry, which is the same defect in different clothes. Read its green as
    /// "no `#if DEBUG` page is routable in Release", not as "no compiled-out page
    /// is".
    func testADebugOnlyPageIsNotRoutableInRelease() throws {
        let body = try pageDestinationBody()
        let route = try appSource("Model/GlobalSearch.swift")

        // Arms whose page only exists in Debug: the `#if DEBUG` regions of the
        // switch, attributed to the `case .x:` they sit under.
        var debugOnly: [String] = []
        var current: String?
        var inDebug = false
        for line in body.split(separator: "\n", omittingEmptySubsequences: false) {
            let text = line.trimmingCharacters(in: .whitespaces)
            if let name = try matches(#"^case \.(\w+)"#, in: text).first?[1] { current = name }
            if text.hasPrefix("#if DEBUG") { inDebug = true }
            if text.hasPrefix("#endif") { inDebug = false }
            if inDebug, let current, !debugOnly.contains(current) { debugOnly.append(current) }
        }

        for page in debugOnly {
            // `settingsRoute(for:)` must answer nil for this page outside Debug;
            // `settingsEntries` already drops a hit whose route is nil.
            let gated = try matches(
                #"#if DEBUG\s*\n\s*case \.\#(page): return \.page\(\.\#(page)\)\s*\n\s*#else\s*\n\s*case \.\#(page): return nil"#,
                in: route
            )
            XCTAssertFalse(
                gated.isEmpty,
                """
                the \(page) page is Debug-only but settingsRoute(for: .\(page)) is not, \
                so a Release build would offer a \(page) search result that pushes an empty view
                """
            )
        }
    }

    /// Every page the Settings list shows is reachable from search with its own
    /// route — and so are the Secrets forms, the feedback sheet and Profile.
    func testEveryListedSettingsPageIsIndexedWithARoute() {
        let index: [SettingsIndexEntry] = settingsIndex()
        for page in SettingsView.listedPages {
            let want: GlobalSearch.Target = .setting(.page(page))
            let routed: [SettingsIndexEntry] = index.filter { entry in
                GlobalSearch.settingsRoute(for: entry.item) == GlobalSearch.SettingsRoute.page(page)
            }
            XCTAssertFalse(routed.isEmpty, "\(page) has no index entry")
            for e in routed {
                XCTAssertTrue(targets(e.title).contains(want), "\(page) is not found by its title \(e.title)")
            }
        }
        XCTAssertTrue(targets("new secret").contains(GlobalSearch.Target.setting(.page(.newSecret))))
        XCTAssertTrue(targets("connect 1password").contains(GlobalSearch.Target.setting(.page(.connectOnePassword))))
        XCTAssertTrue(targets("feedback").contains(GlobalSearch.Target.setting(.feedback)))
        XCTAssertTrue(targets("profile").contains(GlobalSearch.Target.setting(.profile)))
        XCTAssertTrue(targets("on this iphone").contains(GlobalSearch.Target.setting(.page(.root))))
        XCTAssertTrue(targets("text size").contains(GlobalSearch.Target.setting(.page(.textSize))))
        // Every iPhone entry of the shared index maps to an iPhone route (the
        // per-agent rows map through the agent); the Mac-only Updates page is absent.
        for e in index where !e.perAgent {
            XCTAssertNotNil(GlobalSearch.settingsRoute(for: e.item), "\(e.item) has no iPhone route")
            XCTAssertGreaterThanOrEqual(e.keywords.count, 3, "\(e.item) has too few keywords")
        }
        XCTAssertFalse(index.contains { $0.item == SettingsItem.updates })
        XCTAssertTrue(targets("updates").contains(GlobalSearch.Target.setting(.page(.about))), "the iPhone's updates live on About")
        // Not indexed on purpose: Sign out, Pause all secrets.
        XCTAssertTrue(targets("sign out").isEmpty)
        XCTAssertTrue(targets("pause").isEmpty)
        // Every route lands on a page word `SettingsView(landing:)` understands.
        for page in GlobalSearch.SettingsPageRoute.allCases {
            XCTAssertFalse(page.rawValue.isEmpty)
            XCTAssertFalse(GlobalSearch.settingsGlyph(.page(page)).isEmpty)
        }
    }

    /// The section sits between Servers & channels and Messages, and the label is the Mac's.
    func testSectionOrderAndLabel() {
        let expected: [GlobalSearch.Section] = [.people, .chats, .servers, .settings, .messages, .content]
        XCTAssertEqual(GlobalSearch.Section.withQuery, expected)
        XCTAssertEqual(GlobalSearch.Section.settings.label, "Settings")
        XCTAssertEqual(GlobalSearch.keywordLabel("opacity"), "Opacity")
    }
}
