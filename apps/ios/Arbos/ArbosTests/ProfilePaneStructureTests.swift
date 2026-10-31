import XCTest
@testable import Arbos

/// The shared profile pane (D93): every kind lays out hero, then the tab
/// pill, then the selected tab's options (then a footer when it has one) —
/// kinds differ in their tab sets, not in that structure (Jacob, 2026-10-22).
/// The Mac twin is `crates/arbos-mac/src/ui/profiles/pane.rs` `tests`.
final class ProfilePaneStructureTests: XCTestCase {
    func testEveryKindLaysOutHeroThenPillThenOptions() {
        for (kind, tabs) in PaneKinds.all {
            XCTAssertGreaterThanOrEqual(tabs.count, 2, "\(kind): a pane has at least two tabs")
            XCTAssertEqual(ProfilePane.layout(tabs: tabs.count, footerSections: 1), [.hero, .tabPill, .options, .footer], kind)
            XCTAssertEqual(ProfilePane.layout(tabs: tabs.count, footerSections: 0), [.hero, .tabPill, .options], kind)
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
        for (kind, tabs) in PaneKinds.all {
            XCTAssertEqual(Set(tabs).count, tabs.count, "\(kind): a tab label appears once")
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

    /// Jacob 2026-10-31 ([feedback chat-info-pane-icon-defaults-2026-10-31]: "fix the
    /// settings page icons so that they are all classic defaults"): every row on the chat
    /// info pane leads with the stock SF Symbol Apple's own apps put on that action, drawn
    /// at the row label's own point size, weight and colour. The muted tint is what made
    /// the glyphs read lighter than the text beside them. The Mac twin is
    /// `profiles::kit::tests::an_action_rows_icon_has_the_menus_room_around_it`.
    func testChatInfoRowsLeadWithStockSymbolsDrawnLikeTheirLabel() throws {
        let pages = try source("UI/ProfilePages.swift")
        let group = body(of: "GroupProfileView", in: pages)
        for (row, symbol) in [("Add members", "person.badge.plus"), ("Description", "note.text"),
                              ("Share invite link", "square.and.arrow.up"), ("Activity", "clock.arrow.circlepath")] {
            XCTAssertTrue(group.contains(".glyph(\"\(symbol)\")"), "\(row) leads with \(symbol)")
        }
        // A text-alignment control and a filled list rectangle: neither is the symbol
        // Apple would put on a description or on a history, and both sat heavier or
        // lighter than the rest of the set.
        for gone in ["text.alignleft", "list.bullet.rectangle"] {
            XCTAssertFalse(group.contains(gone), "\(gone) is gone from the chat info pane")
        }
        // The icon is drawn like the label it sits beside, and the box matches the Mac's.
        XCTAssertEqual(PaneKit.glyphPoint, Theme.body)
        XCTAssertEqual(PaneKit.glyphWeight, .medium)
        XCTAssertEqual(PaneKit.glyphRadius, 10)
        let box = body(of: "PaneGlyphBox", in: pages)
        XCTAssertTrue(box.contains("Image(systemName: symbol)"), "the system symbol, not a drawn glyph")
        XCTAssertTrue(box.contains("foregroundStyle(theme.text)"), "the label's colour")
        XCTAssertFalse(box.contains("theme.muted"), "never the muted tint")
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
