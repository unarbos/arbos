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
