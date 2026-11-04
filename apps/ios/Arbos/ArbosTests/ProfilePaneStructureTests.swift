import XCTest
import SwiftUI
import UIKit
@testable import Arbos

/// The shared profile pane (D93): every kind lays out hero, then the tab
/// pill, then the selected tab's options (then a footer when it has one) —
/// kinds differ in their tab sets, not in that structure (Jacob, 2026-10-22).
/// The Mac twin is `crates/arbos-mac/src/ui/profiles/pane.rs` `tests`.
final class ProfilePaneStructureTests: XCTestCase {
    func testEveryKindLaysOutHeroThenPillThenOptions() {
        for kind in PaneKinds.all {
            XCTAssertGreaterThanOrEqual(kind.tabs.count, 2, "\(kind.name): a pane has at least two tabs")
            XCTAssertEqual(ProfilePane.layout(tabs: kind.tabs.count, footerSections: 1), [.hero, .tabPill, .options, .footer], kind.name)
            XCTAssertEqual(ProfilePane.layout(tabs: kind.tabs.count, footerSections: 0), [.hero, .tabPill, .options], kind.name)
        }
        // One tab: no pill, the options follow the hero directly.
        XCTAssertEqual(ProfilePane.layout(tabs: 1, footerSections: 0), [.hero, .options])
    }

    func testKindsKeepTheirOwnTabSets() {
        // Grok Bot's agent tabs, the same on the Mac (#1679) and the iPhone
        // ([feedback iphone-agent-pane-tabs-2026-10-25]).
        XCTAssertEqual(PaneKinds.agent, ["Details", "Library", "Computer"])
        XCTAssertEqual(PaneKinds.ownProfile, ["Profile", "Settings"])
        XCTAssertEqual(PaneKinds.person, ["Links", "Groups"])
        XCTAssertEqual(PaneKinds.group, ["Members", "Library"])
        XCTAssertEqual(PaneKinds.server, ["Channels", "Members", "Settings"])
        XCTAssertEqual(PaneKinds.channel, ["Profile", "Settings"])
        for kind in PaneKinds.all {
            XCTAssertEqual(Set(kind.tabs).count, kind.tabs.count, "\(kind.name): a tab label appears once")
        }
    }

    /// The agent pane's Details tab is the Settings card alone — no "Who may message", no
    /// "Permissions", no "Profile" (Jacob 2026-10-25, [feedback
    /// agent-details-remove-sections-ios-2026-10-25]); Coding / Routines come back ahead of
    /// it only while the flag is on (routines for the owner alone). The copy is the Mac's
    /// (`agent::tests` in `profiles/agent.rs`).
    func testAgentDetailsIsSettingsAloneAndReadsLikeTheMac() {
        XCTAssertFalse(AgentPane.agentDetailsShowCodingRoutines, "Coding and Routines stay hidden by default")
        XCTAssertEqual(AgentPane.detailsOrder(mine: true), [.settings])
        XCTAssertEqual(AgentPane.detailsOrder(mine: false), [.settings])
        XCTAssertEqual(AgentPane.detailsOrder(mine: true, showCodingRoutines: true), [.coding, .routines, .settings])
        XCTAssertEqual(AgentPane.detailsOrder(mine: false, showCodingRoutines: true), [.coding, .settings])
        XCTAssertEqual(AgentPane.subtitle(description: "", mine: true, ownerHandle: nil), "Add a label")
        XCTAssertEqual(AgentPane.subtitle(description: " Drafts before it sends. ", mine: true, ownerHandle: nil), "Drafts before it sends.")
        XCTAssertEqual(AgentPane.subtitle(description: "", mine: false, ownerHandle: "@sami"), "Run by @sami")
        XCTAssertEqual(AgentPane.subtitle(description: "", mine: false, ownerHandle: "@sami", ownerName: "Sami Kassab"), "Run by Sami Kassab")

        let working = BoxWorkspace(projectId: "p", name: "Arbos", healthy: true, running: true, elapsedMs: 42_000, toolCalls: 7)
        XCTAssertEqual(AgentPane.codingStatus(streaming: false, workspace: working, machine: .awake, alwaysOn: false), "Working · 42 s · 7 tool calls")
        XCTAssertEqual(AgentPane.codingStatus(streaming: true, workspace: nil, machine: .awake, alwaysOn: false), "Working")
        XCTAssertEqual(AgentPane.codingStatus(streaming: false, workspace: nil, machine: .awake, alwaysOn: false), "Idle")
        XCTAssertEqual(AgentPane.codingStatus(streaming: false, workspace: nil, machine: .hibernatedLight, alwaysOn: false), "Machine asleep")

        let now: Int64 = 1_000_000_000
        let standup = Routine(id: "r", projectId: "p", name: "Stand-up", trigger: .cron(expr: "0 9 * * 1-5", tz: "UTC"), enabled: true, lastRunAt: nil, nextRunAt: now + 2 * 3_600_000)
        XCTAssertEqual(AgentPane.routineStatus(standup, now: now), "Weekdays 09:00 · next in 2 h")
        var paused = standup
        paused.enabled = false
        XCTAssertEqual(AgentPane.routineStatus(paused, now: now), "Paused")
        let hook = Routine(id: "w", projectId: "p", name: "Deploy", trigger: .webhook, enabled: true, lastRunAt: now - 40 * 60_000, nextRunAt: nil)
        XCTAssertEqual(AgentPane.routineStatus(hook, now: now), "Webhook · last 40 min ago")
    }

    /// `GET /machine/debug` → the Computer tab: Machine, then Resources only when the box
    /// answered, then Running now; sizes and durations read like the Mac's.
    func testAgentComputerFollowsTheReport() {
        let debug: [String: Any] = [
            "relay": ["state": "awake", "awake_for_s": 18_720, "agentd": ["image": "arbos-agent-0.1.7"]],
            "box": ["reachable": true, "state": [
                "uptime_s": 18_720, "load": [0.42], "memory_mb": ["total": 8192, "available": 5325],
                "disk": ["total_mb": 10240, "available_mb": 6861],
                "workspaces": [["project_id": "p", "name": "Arbos", "healthy": true, "turn": ["running": true, "current": ["elapsed_ms": 42_000, "tool_calls": 7]]]],
            ]],
        ]
        let report = BoxReport(debug: debug, now: 0)
        XCTAssertTrue(report.reachable)
        XCTAssertEqual(report.image, "arbos-agent-0.1.7")
        XCTAssertEqual(report.running.map(\.name), ["Arbos"])
        XCTAssertEqual(AgentPane.computerOrder(mine: true, report: report), [.machine, .resources, .running])
        XCTAssertEqual(AgentPane.computerOrder(mine: false, report: report), [.machine])
        XCTAssertEqual(AgentPane.freeOf(available: report.diskAvailableMb, total: report.diskTotalMb), "6.7 GB free of 10 GB")
        XCTAssertEqual(AgentPane.durationS(18_720), "5 h 12 m")

        let down: [String: Any] = ["relay": ["state": "asleep"], "box": ["reachable": false, "error": "box has no url yet (asleep or starting)"]]
        let asleep = BoxReport(debug: down, now: 0)
        XCTAssertFalse(asleep.reachable)
        XCTAssertEqual(asleep.unreachableReason, "box has no url yet (asleep or starting)")
        XCTAssertEqual(AgentPane.computerOrder(mine: true, report: asleep), [.machine, .running])
        XCTAssertEqual(AgentPane.computerOrder(mine: true, report: nil), [.machine, .running])
    }
}

/// #1545 (Jacob, 2026-10-23): after "Block user" the row kept reading Block user. The
/// person pane's footer now derives from the relay's block list, so it flips to Unblock
/// once the list names the person and back once it no longer does. The Mac twin is
/// `crates/arbos-mac/src/app.rs` `block_state_tests`.
final class BlockRowTests: XCTestCase {
    private func person(_ id: String, _ handle: String) -> Person { Person(id: id, handle: handle) }

    func testBlockedMatchesByIdThenHandle() {
        let ken = person("gh_1", "kenobijon")
        XCTAssertNotNil(BlockRow.entry(in: [ken], id: ken.id, handle: ken.handle))
        // The pane may open from a row that only knows the handle.
        XCTAssertNotNil(BlockRow.entry(in: [ken], id: "", handle: "KenobiJon"))
        XCTAssertNil(BlockRow.entry(in: [ken], id: "gh_2", handle: "sami"))
        XCTAssertNil(BlockRow.entry(in: [], id: ken.id, handle: ken.handle))
    }

    func testBlockRowFlipsWithTheList() {
        let ken = person("gh_1", "kenobijon")
        var list: [Person] = []
        XCTAssertEqual(BlockRow.title(blocked: BlockRow.entry(in: list, id: ken.id, handle: ken.handle) != nil), "Block user")
        list.append(ken)
        XCTAssertEqual(BlockRow.title(blocked: BlockRow.entry(in: list, id: ken.id, handle: ken.handle) != nil), "Unblock")
        list.removeAll { $0.id == ken.id }
        XCTAssertEqual(BlockRow.title(blocked: BlockRow.entry(in: list, id: ken.id, handle: ken.handle) != nil), "Block user")
    }

    /// On someone else's agent the "Run by …" line is a link to the owner's page (Jacob
    /// 2026-10-25, [feedback agent-pane-run-by-link-ios-2026-10-25]): the display name when the
    /// owner has one, "@handle" otherwise; never on my own agent or without an owner. The Mac
    /// twin is `agent::run_by_tests` in `profiles/agent.rs`.
    func testForeignAgentRunByLineIsAnOwnerLink() {
        let named = AgentPane.ownerLink(mine: false, ownerHandle: "@sami", ownerName: "Sami Kassab")
        XCTAssertEqual(named?.label, "Run by Sami Kassab")
        XCTAssertEqual(named?.handle, "sami")
        let bare = AgentPane.ownerLink(mine: false, ownerHandle: "sami", ownerName: "  ")
        XCTAssertEqual(bare?.label, "Run by @sami")
        XCTAssertNil(AgentPane.ownerLink(mine: true, ownerHandle: "sami", ownerName: "Sami Kassab"), "my own agent has no owner line")
        XCTAssertNil(AgentPane.ownerLink(mine: false, ownerHandle: nil, ownerName: nil), "no owner, no link")
        XCTAssertEqual(AgentPane.runByLabel(name: nil, handle: "sami"), "Run by @sami")
        XCTAssertEqual(ChipLinkStyle.restAlpha, 0.14)
        XCTAssertEqual(ChipLinkStyle.pressedAlpha, 0.30)
        // The agent page pushes the owner inside the root sheet's stack, like member rows do.
        XCTAssertTrue(ProfileTarget.agent(projectId: "p").pushesPeople)
    }
}

/// Jacob 2026-10-25 ("Add a message button to a user"): the person pane's hero carries one
/// Message pill — for another person I have not blocked, never on my own pane, never once I
/// blocked them. The Mac twin is `crates/arbos-mac/src/ui/profiles/user.rs`
/// `tests::the_message_pill_is_for_other_unblocked_people`
/// ([feedback profile-message-button-ios-2026-10-25]).
final class MessageButtonTests: XCTestCase {
    func testTheMessagePillIsForOtherUnblockedPeople() {
        XCTAssertTrue(MessageButton.shows(isMe: false, blocked: false))
        XCTAssertFalse(MessageButton.shows(isMe: true, blocked: false), "not on my own pane")
        XCTAssertFalse(MessageButton.shows(isMe: false, blocked: true), "not once I blocked them")
        XCTAssertFalse(MessageButton.shows(isMe: true, blocked: true))
        XCTAssertEqual(MessageButton.title, "Message")
    }

    func testThePillFollowsTheBlockList() {
        let doug = Person(id: "gh_346940006", handle: "DaWizzaa")
        var list: [Person] = []
        XCTAssertTrue(MessageButton.shows(isMe: false, blocked: BlockRow.entry(in: list, id: doug.id, handle: doug.handle) != nil))
        list.append(doug)
        XCTAssertFalse(MessageButton.shows(isMe: false, blocked: BlockRow.entry(in: list, id: doug.id, handle: doug.handle) != nil))
        list.removeAll { $0.id == doug.id }
        XCTAssertTrue(MessageButton.shows(isMe: false, blocked: BlockRow.entry(in: list, id: doug.id, handle: doug.handle) != nil), "back once unblocked")
    }

    /// The hero keeps its order with the pill: picture, name, subtitle, then the action.
    func testTheHeroActionSitsUnderTheSubtitle() {
        XCTAssertEqual(PaneKit.heroActionGap, 6)
        XCTAssertEqual(Theme.control, Controls.diameter, "the pill is the control family's height")
    }
}

/// Jacob 2026-10-27 ([feedback add-your-agent-row-removal-ios-2026-10-27]; Mac
/// `[feedback add-your-agent-row-removal-mac-2026-10-27]`): every info pane that adds
/// members has exactly ONE add row — "Add members" — whose picker adds people and agents
/// through an "Agents" section; no "Add agent" / "Add your agent" row anywhere (group,
/// server, person pane, empty chat). The Mac twin is `profiles::group::render_tests`.
final class OneAddRowPerPaneTests: XCTestCase {
    private func source(_ rel: String) throws -> String {
        let file = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Arbos/\(rel)")
        guard FileManager.default.fileExists(atPath: file.path) else {
            throw XCTSkip("sources not beside the test bundle (CI without the tree)")
        }
        return try String(contentsOf: file, encoding: .utf8)
    }

    /// The body of one `struct <name>` in a source file (up to the next top-level struct).
    private func body(of name: String, in text: String) -> String {
        let after = text.components(separatedBy: "struct \(name): View").dropFirst().joined()
        return after.components(separatedBy: "\nstruct ").first ?? after
    }

    private func member(_ id: String, _ handle: String, agent: Bool = false, owner: String? = nil, ownerHandle: String? = nil, name: String? = nil) -> ChatMember {
        ChatMember(id: id, kind: agent ? .agent : .user, member: "\(agent ? "agent" : "user"):\(id)", role: "member",
                   handle: handle, avatarUrl: nil, name: name, status: "member", joinedAt: 0,
                   project: nil, ownerId: owner, ownerHandle: ownerHandle, replyPolicy: nil)
    }

    /// Launch week: me, Sam, Priya; my agent Comms and Priya's agents Ledger and Atlas.
    private func launchWeek() -> PeopleChat {
        PeopleChat(id: "g1", kind: "group", createdBy: "gh_me", createdAt: 0, title: "Launch week", pending: false,
                   members: [member("gh_me", "unconst"), member("gh_2", "samir"), member("gh_3", "priya"),
                             member("a-comms", "comms", agent: true, owner: "gh_me", ownerHandle: "unconst", name: "Comms"),
                             member("a-ledger", "ledger", agent: true, owner: "gh_3", ownerHandle: "priya", name: "Ledger"),
                             member("a-atlas", "atlas", agent: true, owner: "gh_3", ownerHandle: "priya", name: "Atlas")],
                   headSeq: 0, readSeq: 0, unread: false, lockState: "people_only", lastActivityAt: 0, myRole: "owner", muted: false, silenced: false)
    }

    func testEveryPaneDrawsOneAddMembersRowAndNoAddAgentRow() throws {
        let pages = try source("UI/ProfilePages.swift")
        let addRow = "PaneRow(leading: .glyph(\"person.badge.plus\"), title: \"Add members\")"
        for (pane, count) in [("GroupProfileView", 1), ("ServerProfileView", 1), ("UserProfileView", 0)] {
            let text = body(of: pane, in: pages)
            XCTAssertFalse(text.isEmpty, "\(pane) is in ProfilePages.swift")
            XCTAssertEqual(text.components(separatedBy: addRow).count - 1, count, "\(pane): Add members rows")
            for gone in ["Add agent", "Add your agent", "One of yours", "Bring one of yours", ".glyph(\"sparkles\")", ".tile(\"sparkles\")"] {
                XCTAssertFalse(text.contains(gone), "\(pane): `\(gone)` is gone for good")
            }
        }
        // The group's description is a labelled row in the same card as Add members, never a bare block.
        let group = body(of: "GroupProfileView", in: pages)
        XCTAssertTrue(group.contains("title: \"Description\""), "a labelled Description row")
        // The empty chat offers no "Add agent" control either.
        let chat = try source("UI/ChatView.swift")
        XCTAssertFalse(chat.contains("\"Add agent\""), "no Add agent control in the chat pane")
    }

    func testThePickerHasAnAgentsSectionMineFirstThenTheOthersAlreadyIn() throws {
        XCTAssertEqual(AddMembersAgents.sectionTitle, "Agents")
        XCTAssertEqual(AddMembersAgents.peopleTitle, "People")
        let sheet = body(of: "NewGroupView", in: try source("UI/PeopleSheets.swift"))
        XCTAssertFalse(sheet.contains("YOUR AGENTS"), "the section is Agents, not Your agents")
        let agents = sheet.range(of: "AddMembersAgents.sectionTitle.uppercased()")
        let people = sheet.range(of: "AddMembersAgents.peopleTitle.uppercased()")
        XCTAssertNotNil(agents); XCTAssertNotNil(people)
        if let agents, let people { XCTAssertLessThan(agents.lowerBound, people.lowerBound, "Agents above People") }
        XCTAssertTrue(sheet.contains("ForEach(Array(othersAgents.enumerated())"), "the other members' agents follow mine")

        // The other members' agents already in the chat: not mine, by name, narrowed by the query.
        let chat = launchWeek()
        XCTAssertEqual(AddMembersAgents.others(in: chat, meId: "gh_me", query: "").map(\.id), ["a-atlas", "a-ledger"])
        XCTAssertEqual(AddMembersAgents.others(in: chat, meId: "gh_me", query: "led").map(\.id), ["a-ledger"])
        XCTAssertEqual(AddMembersAgents.others(in: chat, meId: "gh_me", query: "@ATLAS").map(\.id), ["a-atlas"])
        XCTAssertEqual(AddMembersAgents.others(in: chat, meId: "gh_3", query: "").map(\.id), ["a-comms"], "Priya sees mine, not hers")
        XCTAssertEqual(AddMembersAgents.others(in: nil, meId: "gh_me", query: ""), [], "New group: no chat, no others")
        XCTAssertEqual(AddMembersAgents.note(for: chat.agents[1]), "run by @priya")
        XCTAssertNil(AddMembersAgents.note(for: member("x", "x", agent: true)))
    }
}

/// The tab pill's geometry and its selected capsule ([feedback grok-tab-bold-2026-11-03]).
/// Jacob, 2026-11-03: "on grok these items dont bold when you click on them." `PaneTabs` used
/// to draw the selected tab semibold, so that tab was wider than the same tab unselected and
/// every tab after it slid sideways as the selection moved. The Mac twin is
/// `crates/arbos-mac/src/ui/profiles/mod.rs` `tab_tests`.
final class PaneTabsLayoutTests: XCTestCase {
    /// Every tab set the control carries, plus Home's filter row.
    private var rows: [[String]] { PaneKinds.all.map(\.tabs) + [HomeListFilter.allCases.map(\.rawValue)] }

    private func textWidth(_ s: String, _ weight: UIFont.Weight) -> CGFloat {
        (s as NSString).size(withAttributes: [.font: UIFont.systemFont(ofSize: Theme.body, weight: weight)]).width
    }

    /// Where each tab starts and how wide it is, for one selection, under `weight` for the
    /// selected tab and `.regular` for the rest — the row `PaneTabs` lays out.
    private func boxes(_ tabs: [String], selected: Int, selectedWeight: UIFont.Weight) -> [(CGFloat, CGFloat)] {
        var x: CGFloat = 0
        return tabs.enumerated().map { i, t in
            let w = textWidth(t, i == selected ? selectedWeight : .regular) + 2 * PaneTabsMetrics.labelPad + PaneTabsMetrics.gap
            defer { x += w }
            return (x, w)
        }
    }

    /// The fix: the drawn weight does not take the selection at all.
    func testTheTabWeightIsOneConstantForSelectedAndUnselected() {
        XCTAssertEqual(PaneTabsMetrics.labelWeight, .regular)
    }

    /// Whichever tab is selected, every tab keeps its origin and its width — the row does not
    /// reflow, so nothing slides out from under the finger when a tab is tapped.
    func testMovingTheSelectionMovesNoTab() {
        for tabs in rows {
            let first = boxes(tabs, selected: 0, selectedWeight: .regular)
            for selected in 1..<tabs.count {
                let now = boxes(tabs, selected: selected, selectedWeight: .regular)
                for (i, (box, was)) in zip(now, first).enumerated() {
                    XCTAssertEqual(box.0, was.0, accuracy: 0.001, "\(tabs): \(tabs[i]) moved when \(tabs[selected]) was selected")
                    XCTAssertEqual(box.1, was.1, accuracy: 0.001, "\(tabs): \(tabs[i]) resized when \(tabs[selected]) was selected")
                }
            }
        }
    }

    /// The test above is not vacuous: under the old selected-is-semibold rule the same rows
    /// did move. This is the reported bug, measured — the worst shift is reported on failure.
    func testTheOldSemiboldRuleDidMoveTheRow() {
        var worst: CGFloat = 0
        for tabs in rows {
            for selected in tabs.indices {
                let now = boxes(tabs, selected: selected, selectedWeight: .regular)
                let before = boxes(tabs, selected: selected, selectedWeight: .semibold)
                for (a, b) in zip(now, before) { worst = max(worst, abs(a.0 - b.0)) }
                worst = max(worst, abs(now[selected].1 - before[selected].1))
            }
        }
        XCTAssertGreaterThan(worst, 0.5, "semibold and regular measure the same, so testMovingTheSelectionMovesNoTab proves nothing")
    }

    /// Grok's capsule is grey 27 ([feedback grok-tab-bold-2026-11-03]); ours was the
    /// assistant-bubble grey, 32. It owns both its shades now, so moving the assistant
    /// bubble cannot drag it in either appearance. Light keeps the 242 it has always had,
    /// which means light's two values are still equal and equality cannot tell an alias
    /// from a pin — the pinned number is the assertion instead. When the bubble moves,
    /// this stays.
    func testTheSelectedCapsuleIsItsOwnGrey() {
        XCTAssertEqual(grey(Theme.dark.tabOnFill), 27)
        XCTAssertNotEqual(Theme.dark.tabOnFill, Theme.dark.bubbleAssistant, "the capsule owns its grey")
        XCTAssertEqual(grey(Theme.light.tabOnFill), 242, "light is unmeasured, so unchanged — but pinned")
    }

    /// The capsule and the label colour are the only selected signals left now that the tab
    /// does not go bold, so the capsule must still step off the page it sits on. Every
    /// `PaneTabs` call site puts the row straight on `bg` — Home, the profile pane's pinned
    /// block and the FunPicker sheet all paint it — so `bg` is the surface to measure.
    func testTheSelectedCapsuleStillStepsOverThePage() {
        for theme in [Theme.dark, Theme.light] {
            let step = abs(grey(theme.tabOnFill) - grey(theme.bg))
            XCTAssertGreaterThanOrEqual(step, 6, "capsule \(grey(theme.tabOnFill)) on page \(grey(theme.bg)) is invisible")
        }
    }

    /// A grey token's 0–255 channel value, the way Jacob reads these colours.
    private func grey(_ c: Color) -> Int {
        var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
        UIColor(c).getRed(&r, green: &g, blue: &b, alpha: &a)
        XCTAssertEqual(r, g, accuracy: 0.002, "not a grey")
        XCTAssertEqual(g, b, accuracy: 0.002, "not a grey")
        return Int((r * 255).rounded())
    }
}
