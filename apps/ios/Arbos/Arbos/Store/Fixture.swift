// Debug-only fixture: the Grok Bot reference conversation mapped onto Arbos
// concepts, so the simulator can be screenshotted without a relay. Enabled by
// `ARBOS_FIXTURE=1` in the launch environment. Not compiled into Release.

import Foundation

#if DEBUG
extension AppModel {
    func loadFixture() {
        isFixture = true
        waitlistPosition = DebugHooks.fixtureWaitlist
        // The signed-in user (D79 initials at home and in Settings; reactions need `myUserId`).
        setFixtureUser(AuthUser(id: "fx-me", login: "jacob", name: "Jacob Steeves"))
        let now = nowMs()
        let comms = Project(id: "fx-comms", name: "Comms", icon: "Co", color: "#1084FE", state: .awake, stateSince: now - 3_600_000,
                            stateDetail: nil, model: "anthropic/claude-haiku-5.5", effort: .low, spendTodayUsd: 0.4, spendCapUsd: 25)
        let affine = Project(id: "fx-affine", name: "Affine", icon: "Af", color: "#10B981", state: .awake, stateSince: now - 3_600_000,
                             stateDetail: nil, model: "anthropic/claude-sonnet-5.5", effort: .medium, spendTodayUsd: 1.2, spendCapUsd: 25)
        let hyper = Project(id: "fx-hyper", name: "Hyper", icon: "Hy", color: "#FF7A00", state: .awake, stateSince: now - 3_600_000,
                            stateDetail: nil, model: "anthropic/claude-haiku-5.5", effort: .low, spendTodayUsd: 0, spendCapUsd: 25)
        let scout = Project(id: "fx-scout", name: "Talent Scout", icon: "Ts", color: "#8E5A3C", state: .awake, stateSince: now,
                            stateDetail: nil, model: "anthropic/claude-haiku-5.5", effort: .low, spendTodayUsd: 0, spendCapUsd: 25)
        let expense = Project(id: "fx-expense", name: "Expense Manager", icon: "Em", color: "#FF2D9B", state: .awake, stateSince: now,
                              stateDetail: nil, model: "anthropic/claude-haiku-5.5", effort: .low, spendTodayUsd: 0, spendCapUsd: 25)
        let invoices = Project(id: "fx-invoices", name: "Invoice Collector", icon: "Ic", color: "#8E8E93", state: .awake, stateSince: now,
                               stateDetail: nil, model: "anthropic/claude-haiku-5.5", effort: .low, spendTodayUsd: 0, spendCapUsd: 25)
        var list = [comms, affine, hyper, scout, expense, invoices]
        list[0].description = "Keeps your Discord and email in your voice: short, casual, no sign-offs. Drafts before it sends."
        list[0].machineState = "awake"; list[0].machineClass = "daytona"; list[0].machineSnapshot = "arbos-agent-0.1.11"; list[0].machineAlwaysOn = true
        // Distinct palette colours (D79) so screenshots read well.
        let tints = ["fx-comms": 5, "fx-affine": 3, "fx-hyper": 1, "fx-expense": 6, "fx-invoices": 4, "fx-scout": 2]
        for i in list.indices { list[i].avatarColorIndexEffective = tints[list[i].id] }
        setFixture(projects: list, now: now)
        if DebugHooks.fixtureServers {
            setFixtureServers(now: now)
            // `ARBOS_OPEN_SERVER` lands on a server / channel; the live path applies it
            // after `refreshServers`, which the fixture skips (back-swipe UI tests).
            applyOpenServerHook()
        }
    }

    /// Three servers for the home list's Servers segment (D88 servers otherwise come from the
    /// relay only). No Matrix rows: the preview falls back to the server's own line.
    private func setFixtureServers(now: Int64) {
        let seeds: [(String, String, Int, String)] = [
            ("fx-server-hq", "Arbos HQ", 1, "owner"),
            ("fx-server-affine", "Affine", 2, "member"),
            ("fx-server-hyper", "Hyper", 3, "member"),
        ]
        servers = seeds.enumerated().map { i, seed in
            let (id, name, tint, role) = seed
            let created = now - Int64(i + 1) * 2_700_000
            let general = ServerChannel(id: "\(id)-general", serverId: id, name: "general", topic: nil,
                                        restricted: false, position: 0, roomId: nil, createdAt: created)
            return Server(id: id, name: name, imageKey: nil, ownerUserId: role == "owner" ? "fx-me" : "fx-other",
                          spaceRoomId: nil, createdAt: created, myRole: role, memberCount: 4 + i, channels: [general],
                          generalChannelId: general.id, avatarColorIndex: tint, isOpen: false, invite: nil, members: [])
        }
        serversAvailable = true
    }

    private func setFixture(projects list: [Project], now: Int64) {
        projects = list
        var allChats: [Id: [Chat]] = [:]
        var allMessages: [Id: [Message]] = [:]
        let previews: [Id: (String, Int64)] = [
            "fx-affine": ("Shortlist of 6 candidates ready.", now - 19 * 60_000),
            "fx-hyper": ("A/B copy variants ready to review.", now - 36 * 60_000),
            "fx-scout": ("Outreach drafts queued for approval.", now - 26 * 3_600_000),
            "fx-expense": ("Receipts coded — one needs a look.", now - 27 * 3_600_000),
            "fx-invoices": ("Pulled 9 invoices from vendor portals.", now - 3 * 86_400_000),
        ]
        for p in list {
            let chat = Chat(id: "\(p.id)-main", projectId: p.id, title: p.name, isPinned: true, unread: p.id == "fx-affine",
                            lastMessageAt: p.id == "fx-comms" ? now - 60_000 : (previews[p.id]?.1 ?? now))
            allChats[p.id] = [chat]
            if let (text, at) = previews[p.id] {
                allMessages[chat.id] = [assistant("\(p.id)-m1", chat.id, text, at: at)]
            }
            caughtUp.insert(chat.id)
        }
        // An agent-hosted group chat (F6): Comms hosts, Affine and Hyper were added.
        let g = "fx-comms-group-launch"
        allChats["fx-comms", default: []].append(Chat(id: g, projectId: "fx-comms", title: "Launch copy", isPinned: false, unread: true,
                                                      lastMessageAt: now - 9 * 60_000, kind: "group", participants: ["fx-affine", "fx-hyper"]))
        allMessages[g] = [
            user("g0", g, "Affine, Hyper — align the launch copy with the Discord tone Comms set.", at: now - 30 * 60_000),
            // Two hand-offs in a row merge into one Grok line: "4 messages with ◉ Affine and ◉ Hyper".
            handoff("g0a", g, name: "Affine", at: now - 29 * 60_000),
            handoff("g0b", g, name: "Hyper", at: now - 28 * 60_000),
            assistant("g1", g, "Got it. Three variants in the shared doc, all short and casual.", at: now - 9 * 60_000),
        ]
        caughtUp.insert(g)
        let c = "fx-comms-main"
        let t0 = now - 14 * 60_000
        allMessages[c] = [
            user("c1", c, "That was really great what we set up with Alan. However, I think we need to be a little more careful about the way in which we type in Discord, not to write massive, verbose responses. Try to write in my style do things that make it seem like it's not an agent that's writing the response. so maybe you can rewrite messages that go through our Discord.", at: t0),
            assistant("c2", c, "Makes sense. I'll pull a sample of your own Discord messages to get your style, then set it so everything going through Discord gets rewritten to sound like you first.", at: t0 + 20_000),
            handoff("c3", c, name: "Affine", at: t0 + 40_000, messages: 1),
            assistant("c4", c, "From now on, anything I send on your Discord gets rewritten to sound like you first. That means short, casual one-liners like \"Can be done in a day\" or \"nice work with 10\", sometimes split into two or three quick messages. It also means no lists, no long blocks, and no \"— Arbos\" sign-off. I've told Affine to keep its answers to the key points too, and I'll stick to this style for anything else I send on your Discord.", at: t0 + 90_000),
            user("c5", c, "Hey", at: t0 + 120_000),
            streaming("c6", c, at: t0 + 125_000),
        ]
        // #1544: the Comms agent's destructive-shell card with its four standing rows, when the
        // rig asks for it (`ARBOS_FIXTURE_SHELL_CARD=1`); Comms also holds one standing grant.
        if DebugHooks.fixtureShellCard {
            allMessages[c]?.removeAll { $0.id == "c6" }
            allMessages[c]?.append(user("c7", c, "clone arbos into /tmp and list the top level", at: t0 + 130_000))
            allMessages[c]?.append(assistant("c8", c, "On it.", at: t0 + 131_000))
            allMessages[c]?.append(shellCard("c9", c, at: t0 + 132_000))
        }
        // Secret hand-off rig (`ARBOS_FIXTURE_SECRET_HANDOFF=offer|saved|ghost|detect`).
        if let mode = DebugHooks.fixtureSecretHandoff {
            allMessages[c]?.removeAll { $0.id == "c6" }
            switch mode {
            case "offer", "saved":
                allMessages[c]?.append(user("h1", c, "Can I give you a secret? It's the Stripe key for the refunds report.", at: t0 + 130_000))
                allMessages[c]?.append(assistant("h2", c, "Yes. Use the box below. It goes straight to your vault, never into this chat.", at: t0 + 131_000))
                allMessages[c]?.append(secretOfferCard("h3", c, saved: mode == "saved", at: t0 + 132_000))
                if mode == "saved" {
                    allMessages[c]?.append(assistant("h4", c, "Got it. I'll use STRIPE_KEY for the refunds report and keep the value out of this chat.", at: t0 + 140_000))
                }
            case "ghost":
                allMessages[c]?.append(user("h1", c, "Let's do the refunds report next.", at: t0 + 130_000))
                allMessages[c]?.append(assistant("h2", c, "Sure — I need the Stripe key for that.", at: t0 + 131_000))
                allMessages[c]?.append(secretGhost("h3", c, name: "STRIPE_KEY", at: t0 + 132_000))
                allMessages[c]?.append(assistant("h4", c, "Saved as STRIPE_KEY and removed from this chat — also from my memory and working context. I can use it; the value stays in your vault. Running the refunds report now.", at: t0 + 140_000))
            case "detect":
                allMessages[c]?.append(user("h1", c, "Let's do the refunds report next.", at: t0 + 130_000))
                allMessages[c]?.append(assistant("h2", c, "Sure — I need the Stripe key for that.", at: t0 + 131_000))
                allMessages[c]?.append(user("h3", c, "sk_" + "test_FIXTURE51HxQ9mT3vK2pLnR8wZ4eYc7bD1fG6hJ0kM9nP5qS2tU8vW3xA1", at: t0 + 132_000))
                allMessages[c]?.append(secretDetectedCard("h4", c, at: t0 + 133_000))
            default:
                break
            }
        }
        shellGrants["fx-comms"] = [ShellGrant(id: "grant-scratch", ruleClass: "file-delete", pathClass: "scratch", chatId: nil,
                                              grantedBy: "dev-iphone", createdAt: now - 3 * 3_600_000,
                                              label: "File deletes in /tmp", detail: "scratch only: /tmp and /work/tmp")]
        // Affine's chat shows the card types: file, voice memo, question, stop.
        let a = "fx-affine-main"
        let a0 = now - 19 * 60_000
        allMessages[a] = [
            user("a1", a, "Pull the board deck numbers from Salesforce, Mercury and QuickBooks and send me the deck.", at: a0 - 15 * 60_000),
            assistant("a2", a, "Updated the numbers based on today's pull from Salesforce, Mercury, QuickBooks, and Hex.", at: a0 - 14 * 60_000),
            attachment("a3", a, name: "Q3 board deck v4.pdf", mime: "application/pdf", at: a0 - 13 * 60_000),
            user("a4", a, "Can you add a slide for headcount and pull from Rippling", at: a0 - 10 * 60_000),
            assistant("a5", a, "Pulled and added.", at: a0 - 9 * 60_000),
            attachment("a6", a, name: "Q3 board deck v5.pdf", mime: "application/pdf", at: a0 - 9 * 60_000 + 5_000),
            attachment("a7", a, name: "Walkthrough.m4a", mime: "audio/m4a", at: a0 - 8 * 60_000),
            // Jacob's case (chat-preview-cards-2026-10-23): a screenshot of the small
            // page the agent made — white, with its own empty top margin.
            user("a7b", a, "Make me a small page with a button that counts clicks.", at: a0 - 6 * 60_000),
            assistant("a7c", a, "Here it is — a small page with a counter button.", at: a0 - 5 * 60_000),
            attachment("a7d", a, name: "Screenshot 2026-10-23 at 7.55.41.png", mime: "image/png", at: a0 - 5 * 60_000 + 3_000),
            question("a8", a, "Send the deck to the whole team or just the board list?",
                     options: ["Whole team", "Board list only", "Hold off for now"], at: a0 - 2 * 60_000),
            stopped("a9", a, at: a0),
        ]
        chats = allChats
        messages = allMessages
        // `ARBOS_FIXTURE_LONG_REPLY=1`: the opened chat ends with a several-block
        // agent reply, the shape of Jacob's bubble in #1389 (selection / copy rigs).
        if DebugHooks.fixtureLongReply, let c = DebugHooks.openChat {
            messages[c, default: []].append(assistant("fx-long", c, Self.longReply, at: now - 60_000))
        }
        machine = Machine(state: .awake, since: now - 3_600_000, spec: "Daytona · 4 vCPU · 8 GiB · 10 GiB disk · arbos-agent-0.2.0",
                          provider: "daytona", sandboxId: "fixture-box", spendTodayUsd: 1.6, spendCapUsd: 25,
                          workspaces: Dictionary(uniqueKeysWithValues: list.map { ($0.id, $0.id == "fx-comms" ? WorkspaceState.active : .idle) }),
                          alwaysOn: DebugHooks.alwaysOn, machineTodayUsd: 2.88, reported: true)
        // One DM with three unread messages, for the count pill and the notification screens.
        let meMember = ChatMember(id: "fx-me", kind: .user, member: "user:fx-me", role: "owner", handle: "jacob", avatarUrl: nil, name: "Jacob Steeves", status: "member",
                                  joinedAt: now - 40 * 60_000, project: nil, ownerId: nil, ownerHandle: nil, replyPolicy: nil)
        let unconst = ChatMember(id: "gh_32490803", kind: .user, member: "user:gh_32490803", role: "member", handle: "unconst",
                                 avatarUrl: "https://avatars.githubusercontent.com/u/32490803?v=4", name: "Unconst", status: "member",
                                 joinedAt: now - 40 * 60_000, project: nil, ownerId: nil, ownerHandle: nil, replyPolicy: nil,
                                 lastSeenAt: now - 7 * 60_000, online: false)
        // A small group too, for the group page.
        let mira = ChatMember(id: "fx-u3", kind: .user, member: "user:fx-u3", role: "member", handle: "mira-k", avatarUrl: nil, name: "Mira Kaplan", status: "member",
                              joinedAt: now - 3 * 86_400_000, project: nil, ownerId: nil, ownerHandle: nil, replyPolicy: nil, lastSeenAt: now - 30_000, online: true)
        let sam = ChatMember(id: "fx-u4", kind: .user, member: "user:fx-u4", role: "member", handle: "sam", avatarUrl: nil, name: "Sam Okafor", status: "member",
                             joinedAt: now - 3 * 86_400_000, project: nil, ownerId: nil, ownerHandle: nil, replyPolicy: nil, lastSeenAt: now - 2 * 86_400_000, online: false)
        var meOwner = meMember; meOwner.role = "owner"
        let commsAgent = ChatMember(id: "fx-comms", kind: .agent, member: "agent:fx-comms", role: "agent", handle: "comms", avatarUrl: nil, name: "Comms", status: "member",
                                    joinedAt: now - 86_400_000, project: list.first { $0.id == "fx-comms" }, ownerId: "fx-me", ownerHandle: "jacob", replyPolicy: "mention")
        // `ARBOS_FIXTURE_WORKING=<n>` (presence-row rig): the group holds n agents so the
        // row over the composer can show several working at once.
        var groupAgents = [commsAgent]
        // `ARBOS_FIXTURE_FOREIGN_AGENT=1` (#1592): someone else's agent, first so
        // `ARBOS_FIXTURE_WORKING=1` shows it working. Roster record only — not in `projects`.
        if DebugHooks.fixtureForeignAgent {
            var testAgent = Project(id: "fx-foreign-testagent", name: "TestAgent", icon: "T", color: "#8b7cf6", state: .awake, stateSince: nil,
                                    stateDetail: nil, model: "", effort: .low, spendTodayUsd: 0, spendCapUsd: 0, avatar: nil, avatarColor: nil)
            testAgent.avatarLeaf = 3
            testAgent.avatarLeafColor = 7
            testAgent.avatarColorIndexEffective = 3
            groupAgents.insert(ChatMember(id: testAgent.id, kind: .agent, member: "agent:\(testAgent.id)", role: "agent", handle: "testagent", avatarUrl: nil,
                                          name: "TestAgent", status: "member", joinedAt: now - 86_400_000, project: testAgent, ownerId: "gh_459",
                                          ownerHandle: "sami", replyPolicy: "mention"), at: 0)
        }
        for p in list.dropFirst().prefix(max(0, DebugHooks.fixtureWorking - 1)) {
            groupAgents.append(ChatMember(id: p.id, kind: .agent, member: "agent:\(p.id)", role: "agent", handle: mentionSlug(p.name), avatarUrl: nil,
                                          name: p.name, status: "member", joinedAt: now - 86_400_000, project: p, ownerId: "fx-me", ownerHandle: "jacob", replyPolicy: "mention"))
        }
        let group = PeopleChat(id: "fx-group-launch", kind: "group", createdBy: "fx-me", createdAt: now - 3 * 86_400_000, title: "Launch week",
                               pending: false, members: [meOwner, unconst, mira, sam] + groupAgents, headSeq: 40, readSeq: 40, unread: false,
                               lockState: "visible_to_agents", lastActivityAt: now - 50 * 60_000, myRole: "owner", muted: false, silenced: false)
        peopleChats = [group, PeopleChat(id: "fx-dm-unconst", kind: "dm", createdBy: "gh_32490803", createdAt: now - 40 * 60_000, title: "@unconst",
                                  pending: false, members: [unconst, meMember], headSeq: 12, readSeq: 9, unread: true,
                                  lockState: "people_only", lastActivityAt: now - 7 * 60_000, myRole: "member", muted: false, silenced: false)]
        peopleUnread = ["fx-dm-unconst"]
        messages["fx-group-launch"] = [
            Message(id: "g1", chatId: "fx-group-launch", role: .user, sender: "user:fx-u3", status: .done, turn: nil, decision: nil, subagentId: nil,
                    subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: now - 3 * 3_600_000, completedAt: nil, pending: false,
                    parts: [.text(id: "g1t", text: "Launch checklist is here: https://notion.so/arbos/launch-week")], hiddenCount: 0, senderHandle: "mira-k", senderName: "Mira Kaplan"),
            // A mention of an agent and a person: both draw as chips (mention chips, Oct 23).
            Message(id: "g1m", chatId: "fx-group-launch", role: .user, sender: "user:fx-u3", status: .done, turn: nil, decision: nil, subagentId: nil,
                    subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: now - 3 * 3_600_000 + 90_000, completedAt: nil, pending: false,
                    parts: [.text(id: "g1mt", text: "@comms can you draft the embargo note? @unconst has the press list.")], hiddenCount: 0, senderHandle: "mira-k", senderName: "Mira Kaplan"),
            Message(id: "g2", chatId: "fx-group-launch", role: .user, sender: "user:fx-u4", status: .done, turn: nil, decision: nil, subagentId: nil,
                    subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: now - 2 * 3_600_000, completedAt: nil, pending: false,
                    parts: [.attachment(id: "g2a", attachment: Attachment(name: "press-kit-v3.pdf", mime: "application/pdf", blobRef: "fixture-g2a"))], hiddenCount: 0, senderHandle: "sam", senderName: "Sam Okafor"),
            // Four pictures (same set as the Mac fixture) so the group's Library grid and
            // the media viewer's album have something to page through (media-viewer-2026-10-23).
            Message(id: "g2b", chatId: "fx-group-launch", role: .user, sender: "user:fx-u4", status: .done, turn: nil, decision: nil, subagentId: nil,
                    subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: now - 2 * 3_600_000 + 60_000, completedAt: nil, pending: false,
                    parts: [.attachment(id: "g2b1", attachment: Attachment(name: "deck-v3-slide-04.png", mime: "image/png", blobRef: "fixture-g2b1", kind: "image")),
                            .attachment(id: "g2b2", attachment: Attachment(name: "launch-dashboard.png", mime: "image/png", blobRef: "fixture-g2b2", kind: "image")),
                            .attachment(id: "g2b3", attachment: Attachment(name: "launch-day-team.jpg", mime: "image/jpeg", blobRef: "fixture-g2b3", kind: "image")),
                            .attachment(id: "g2b4", attachment: Attachment(name: "signup-funnel.png", mime: "image/png", blobRef: "fixture-g2b4", kind: "image")),
                            .attachment(id: "g2b5", attachment: Attachment(name: "launch-teaser.mp4", mime: "video/mp4", blobRef: "fixture-g2b5", kind: "video")),
                            .text(id: "g2bt", text: "Deck, dashboard and the press kit for the run-through.")],
                    hiddenCount: 0, senderHandle: "sam", senderName: "Sam Okafor"),
            Message(id: "g3", chatId: "fx-group-launch", role: .user, sender: "user:fx-u3", status: .done, turn: nil, decision: nil, subagentId: nil,
                    subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: now - 50 * 60_000, completedAt: nil, pending: false,
                    parts: [.text(id: "g3t", text: "And the signup form: https://arbos.life/i/launch")], hiddenCount: 0, senderHandle: "mira-k", senderName: "Mira Kaplan"),
            // What the relay writes on a membership change (#1349): a system line, device `relay`.
            Message(id: "g4", chatId: "fx-group-launch", role: .system, sender: "system", status: .done, turn: nil, decision: nil, subagentId: nil,
                    subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: now - 40 * 60_000, completedAt: nil, pending: false,
                    parts: [.text(id: "g4t", text: "Jacob added Comms")], hiddenCount: 0, senderHandle: nil, senderName: nil),
            // A member's portrait phone screenshot with a long note — the shape of Jacob's
            // Arbonites report ([feedback image-caption-bubble-ios-2026-10-25]): the caption
            // must wrap at the bubble's width, not the tall picture's.
            Message(id: "g6", chatId: "fx-group-launch", role: .user, sender: "user:fx-u3", status: .done, turn: nil, decision: nil, subagentId: nil,
                    subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: now - 35 * 60_000, completedAt: nil, pending: false,
                    parts: [.attachment(id: "g6a", attachment: Attachment(name: "IMG_4301.PNG", mime: "image/png", blobRef: "fixture-g6a", kind: "image")),
                            .text(id: "g6t", text: "It's hard to explain, but I feel like your own sent texts could be improved a little. It now seems like it's still waiting for connection before it gets sent out because it's faded. Should be more colored imho @comms")],
                    hiddenCount: 0, senderHandle: "mira-k", senderName: "Mira Kaplan"),
            // A landscape picture with a short note: the picture spans the bubble, the note sits
            // under it at the picture's width (the flush case, unchanged).
            Message(id: "g7", chatId: "fx-group-launch", role: .user, sender: "user:fx-u4", status: .done, turn: nil, decision: nil, subagentId: nil,
                    subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: now - 33 * 60_000, completedAt: nil, pending: false,
                    parts: [.attachment(id: "g7a", attachment: Attachment(name: "launch-hero.png", mime: "image/png", blobRef: "fixture-g7a", kind: "image")),
                            .text(id: "g7t", text: "Hero for the landing page, v2.")],
                    hiddenCount: 0, senderHandle: "sam", senderName: "Sam Okafor"),
        ]
        // Mira's second line replies to her first (quote header on another person's bubble).
        messages["fx-group-launch"]?[2].replyTo = "g1"
        // #1592 rig: a question to someone else's agent and its first bubble — the shape of
        // Jacob's Arbonites screenshot ("@testagent how many messages …" / "On it.").
        if DebugHooks.fixtureForeignAgent {
            messages["fx-group-launch", default: []].append(
                Message(id: "g5", chatId: "fx-group-launch", role: .user, sender: "user:fx-u4", status: .done, turn: nil, decision: nil, subagentId: nil,
                        subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: now - 30 * 60_000, completedAt: nil, pending: false,
                        parts: [.text(id: "g5t", text: "@testagent how many messages are there in this chat?")], hiddenCount: 0,
                        senderHandle: "sam", senderName: "Sam Okafor"))
            var reply = assistant("g6", "fx-group-launch", "On it.", at: now - 29 * 60_000)
            reply.sender = "agent:fx-foreign-testagent"
            reply.senderHandle = "testagent"
            reply.senderName = "TestAgent"
            reply.inReplyTo = "g5"
            messages["fx-group-launch", default: []].append(reply)
        }
        if let rows = DebugHooks.fixtureLongRows, rows > 0 {
            messages["fx-group-launch"] = Self.longGroupTranscript(rows: rows, before: now - 4 * 3_600_000)
                + (messages["fx-group-launch"] ?? [])
        }
        // `ARBOS_FIXTURE_DOC=<path>`: the fixture group becomes a real chat doc (a Loro
        // snapshot exported from the relay), so the transcript has the true shape —
        // members, avatars, mentions, replies, media, system rows (long-chat perf rig).
        if let path = DebugHooks.fixtureDoc, let data = FileManager.default.contents(atPath: path) {
            let started = Date()
            if let doc = try? CoreChatDoc(key: "fx-group-launch", snapshot: data) {
                let list = doc.readAll()
                LaunchClock.mark("fixture-doc", detail: "messages=\(list.count) bytes=\(data.count) in \(Int(Date().timeIntervalSince(started) * 1000)) ms")
                var seen = Set<String>()
                var members: [ChatMember] = [meOwner]
                for m in list where m.sender.hasPrefix("user:") && m.sender != "user:fx-me" {
                    let id = String(m.sender.dropFirst(5))
                    guard seen.insert(id).inserted, let handle = m.senderHandle else { continue }
                    members.append(ChatMember(id: id, kind: .user, member: m.sender, role: "member", handle: handle, avatarUrl: m.senderAvatar,
                                              name: m.senderName, status: "member", joinedAt: now - 3 * 86_400_000, project: nil, ownerId: nil,
                                              ownerHandle: nil, replyPolicy: nil, lastSeenAt: now - 30_000, online: true))
                }
                for m in list where m.sender.hasPrefix("agent:") {
                    let id = String(m.sender.dropFirst(6))
                    guard seen.insert(id).inserted, let handle = m.senderHandle else { continue }
                    members.append(ChatMember(id: id, kind: .agent, member: m.sender, role: "agent", handle: handle, avatarUrl: m.senderAvatar,
                                              name: m.senderName, status: "member", joinedAt: now - 86_400_000, project: nil, ownerId: nil,
                                              ownerHandle: nil, replyPolicy: "mention"))
                }
                if let i = peopleChats.firstIndex(where: { $0.id == "fx-group-launch" }) {
                    peopleChats[i].members = members
                    peopleChats[i].title = "Arbonites (export)"
                }
                messages["fx-group-launch"] = list
            }
        }
        if let replay = DebugHooks.perfReplay {
            perfReplay(checkpointPath: replay.checkpoint, rowsPath: replay.rows, rooms: DebugHooks.perfReplayRooms,
                       gapMs: DebugHooks.perfReplayGapMs, afterMs: DebugHooks.perfReplayAfterMs)
        }
        // `ARBOS_FIXTURE_WORKING=<n>`: the first n agents of the group (and the opened
        // agent chat's one agent) stream a row with no words yet — "<name> is working".
        if DebugHooks.fixtureWorking > 0 {
            for (i, a) in groupAgents.prefix(DebugHooks.fixtureWorking).enumerated() {
                var m = streaming("fx-working-\(i)", "fx-group-launch", at: now - 5_000 + Int64(i))
                m.sender = a.member
                m.senderHandle = a.handle
                m.senderName = a.name
                messages["fx-group-launch", default: []].append(m)
            }
            if let c = DebugHooks.openChat, c != "fx-group-launch", c != "fx-dm-unconst" {
                messages[c, default: []].append(streaming("fx-working-agent", c, at: now - 5_000))
            }
        }
        // `ARBOS_FIXTURE_JOBS=<n>`: background jobs open in the opened agent chat — each with
        // its hidden `job` row (what agentd writes) and its entry in the relay's
        // `activity.jobs[]` (what the poll would answer): the tasks chip and sheet.
        if DebugHooks.fixtureJobs > 0, let c = DebugHooks.openChat, let projectId = chat(c)?.projectId,
           peopleChat(c) == nil {
            let ask = messages[c]?.last { $0.role == .user }?.id
            let seeds: [(id: String, title: String, state: JobState, step: String, elapsed: Int)] = [
                ("fx-job-surf", "Searching for surf schools", .running, "Reading tripadvisor.com", 41),
                ("fx-job-tests", "Fixing the flaky tests", .running, "", 125),
                ("fx-job-rename", "Renaming the config module", .queued, "Waiting for Fixing the flaky tests", 12),
            ]
            let n = DebugHooks.fixtureJobs
            var list: [Job] = []
            for (i, seed) in seeds.prefix(n).enumerated() {
                let activity = seed.step.isEmpty ? seed.title : seed.step
                var row = streaming("fx-job-row-\(i)", c, at: now - Int64(seed.elapsed) * 1000)
                row.job = JobRef(["id": seed.id, "title": seed.title])
                row.worker = true
                row.hidden = true
                row.inReplyTo = ask
                row.activity = n > 1 ? "\(activity) · \(n) tasks" : activity
                messages[c, default: []].append(row)
                list.append(Job(id: seed.id, title: seed.title, state: seed.state, step: seed.step, elapsedS: seed.elapsed,
                                askMessageId: ask, chatId: c))
            }
            applyJobs(projectId: projectId, list)
        }
        // `ARBOS_FIXTURE_READS=1`: the opened agent chat ends like Bubble's — a hidden job
        // row that read a source file (its page as a link, the file attached) and a finished
        // reply naming ten files and a docs page ([feedback library-chat-only-ios-2026-10-25]).
        if DebugHooks.fixtureReads, let c = DebugHooks.openChat, peopleChat(c) == nil {
            var job = streaming("fx-read-job", c, at: now - 40_000)
            job.job = JobRef(["id": "fx-job-reads", "title": "Reading the repo"])
            job.worker = true
            job.hidden = true
            job.parts = [.text(id: "fx-read-job-t", text: "Read https://github.com/unarbos/arbos/blob/main/crates/arbos-mac/src/app.rs"),
                         .attachment(id: "fx-read-job-a", attachment: Attachment(name: "app.rs", mime: "text/plain", blobRef: "fixture-read-app"))]
            messages[c, default: []].append(job)
            let files = ["app.rs", "calls.rs", "auth.rs", "update.rs", "search.rs", "onboarding.rs", "transcript.rs", "relay.rs", "agent.rs", "people.rs"]
            var reply = streaming("fx-read-reply", c, at: now - 20_000)
            reply.status = .done
            reply.completedAt = now - 19_000
            reply.parts = [.text(id: "fx-read-reply-t", text: "I read \(files.joined(separator: ", ")) — nothing to change. Notes on the layout API: https://docs.rs/gpui")]
            messages[c, default: []].append(reply)
        }
        // `ARBOS_FIXTURE_TYPING=1|<handle>[,<handle>…]`: those people type in every fixture
        // people chat for the whole launch (`1` = the DM's other person / the group's first).
        if let spec = DebugHooks.fixtureTyping {
            for chat in peopleChats {
                let handles: [String] = spec == "1"
                    ? [chat.members.first { !$0.isAgent && $0.id != "fx-me" }?.handle].compactMap { $0 }
                    : spec.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
                var marks: [String: Int64] = [:]
                for (i, h) in handles.enumerated() { marks[h] = now + 3_600_000 - Int64(i) }
                if !marks.isEmpty { typing[chat.id] = marks }
            }
        }
        caughtUp.insert("fx-group-launch")
        // The DM carries two lines so the rig can show a 1:1 chat (no sender names,
        // sender-name-in-bubble-2026-10-23).
        messages["fx-dm-unconst"] = [
            Message(id: "d1", chatId: "fx-dm-unconst", role: .user, sender: "user:gh_32490803", status: .done, turn: nil, decision: nil, subagentId: nil,
                    subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: now - 9 * 60_000, completedAt: nil, pending: false,
                    parts: [.text(id: "d1t", text: "Hey — are you around? I want to go over the beta invite copy before it ships.")], hiddenCount: 0,
                    senderHandle: "unconst", senderName: "Unconst"),
            Message(id: "d2", chatId: "fx-dm-unconst", role: .user, sender: "user:fx-me", status: .done, turn: nil, decision: nil, subagentId: nil,
                    subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: now - 8 * 60_000, completedAt: nil, pending: false,
                    parts: [.text(id: "d2t", text: "Here now. Paste what you have and I'll read it.")], hiddenCount: 0, senderHandle: "jacob", senderName: "Jacob Steeves"),
            Message(id: "d3", chatId: "fx-dm-unconst", role: .user, sender: "user:gh_32490803", status: .done, turn: nil, decision: nil, subagentId: nil,
                    subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: now - 7 * 60_000, completedAt: nil, pending: false,
                    parts: [.text(id: "d3t", text: "Sending in a sec. Also: can we add your Arbos agent here so it can pull the numbers while we talk?")], hiddenCount: 0,
                    senderHandle: "unconst", senderName: "Unconst"),
            // A second line of mine after unconst's last one (rig: two own bubbles in a 1:1 shot).
            Message(id: "d4", chatId: "fx-dm-unconst", role: .user, sender: "user:fx-me", status: .done, turn: nil, decision: nil, subagentId: nil,
                    subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: now - 6 * 60_000, completedAt: nil, pending: false,
                    parts: [.text(id: "d4t", text: "Sure — add it, and send the copy whenever you're ready.")], hiddenCount: 0, senderHandle: "jacob", senderName: "Jacob Steeves"),
        ]
        // `ARBOS_FIXTURE_REPLY_DM=1` (rig, #1836): rows that answer messages in other chats —
        // unconst replies here to Mira's Launch week line (a group I am in: linked); I reply
        // to Sam's; and one from a chat I am not in (quote only, no link).
        if DebugHooks.fixtureReplyDM {
            var theirs = Message(id: "d5", chatId: "fx-dm-unconst", role: .user, sender: "user:gh_32490803", status: .done, turn: nil, decision: nil, subagentId: nil,
                                 subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: now - 5 * 60_000, completedAt: nil, pending: false,
                                 parts: [.text(id: "d5t", text: "Works")], hiddenCount: 0, senderHandle: "unconst", senderName: "Unconst")
            theirs.replyRef = ReplyRef(chatId: "fx-group-launch", projectId: "people", messageId: "g1", chatTitle: "Launch week",
                                       sender: "user:fx-u3", senderName: "Mira Kaplan", senderHandle: "mira-k",
                                       text: "Launch checklist is here: https://notion.so/arbos/launch-week")
            var mine = Message(id: "d6", chatId: "fx-dm-unconst", role: .user, sender: "user:fx-me", status: .done, turn: nil, decision: nil, subagentId: nil,
                               subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: now - 4 * 60_000, completedAt: nil, pending: false,
                               parts: [.text(id: "d6t", text: "Press kit looks good — ship it.")], hiddenCount: 0, senderHandle: "jacob", senderName: "Jacob Steeves")
            mine.replyRef = ReplyRef(chatId: "fx-group-launch", projectId: "people", messageId: "g2", chatTitle: "Launch week",
                                     sender: "user:fx-u4", senderName: "Sam Okafor", senderHandle: "sam", text: "press-kit-v3.pdf")
            var outside = Message(id: "d7", chatId: "fx-dm-unconst", role: .user, sender: "user:gh_32490803", status: .done, turn: nil, decision: nil, subagentId: nil,
                                  subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: now - 3 * 60_000, completedAt: nil, pending: false,
                                  parts: [.text(id: "d7t", text: "Same thing happened in ours.")], hiddenCount: 0, senderHandle: "unconst", senderName: "Unconst")
            outside.replyRef = ReplyRef(chatId: "fx-not-my-chat", projectId: "people", messageId: "x1", chatTitle: "Design crit",
                                        sender: "user:gh_900", senderName: "Alex Rivera", senderHandle: "alex-rivera",
                                        text: "The pricing slide still shows last quarter's tiers.")
            messages["fx-dm-unconst"]?.append(contentsOf: [theirs, mine, outside])
        }
        caughtUp.insert("fx-dm-unconst")
        directory = [
            Person(id: "fx-u1", handle: "alex-rivera", avatarUrl: nil, name: "Alex Rivera", status: "member"),
            Person(id: "fx-u2", handle: "jen", avatarUrl: nil, name: "Jen Park", status: "member"),
            Person(id: "fx-u3", handle: "mira-k", avatarUrl: nil, name: "Mira Kaplan", status: "member"),
            Person(id: "fx-u4", handle: "sam", avatarUrl: nil, name: "Sam Okafor", status: "invited"),
            Person(id: "fx-u5", handle: "tobias", avatarUrl: nil, name: "Tobias Lund", status: "member"),
        ]
        link = .live
        seedFixtureSecrets(now: now)
    }

    /// Settings → Secrets demo (D83), the Mac fixture's twin: a Stripe key for Affine and
    /// Comms, a Mailgun key for Comms, a GitHub login for Hyper, one 1Password vault, recent
    /// activity. No value exists anywhere in the fixture.
    private func seedFixtureSecrets(now: Int64) {
        func agent(_ id: String, _ name: String, _ enabled: Bool, _ note: String) -> [String: Any] {
            // Every key the shared `SecretAccessDto` requires (strict Codable since f24098b8).
            var a: [String: Any] = ["project_id": id, "name": name, "access_id": "fx-acc-\(id)-\(note.count)", "enabled": enabled, "note": note, "hosts": [],
                                    "scope": ["kind": "all"], "created_from": "settings", "created_at": now - 10 * 86_400_000]
            if enabled { a["last_used_at"] = now - 120_000 } else { a["disabled_at"] = now - 6 * 86_400_000 }
            return a
        }
        func secret(_ id: String, _ name: String, _ env: String, _ kind: String, _ note: String, _ hosts: [String], _ agents: [[String: Any]], _ daysAgo: Int64) -> [String: Any] {
            var m: [String: Any] = ["id": id, "name": name, "env": env, "kind": kind, "note": note, "hosts": hosts, "ask_every_time": false,
                                    "inject_env": false, "version": 1, "created_at": now - daysAgo * 86_400_000, "updated_at": now - daysAgo * 86_400_000,
                                    "vault_name": "Arbos", "ref": "arbos-secret://\(env)/\(kind == "login" ? "password" : "value")",
                                    "agents": agents, "directors": [], "people": [], "others_ask": "refuse"]
            if daysAgo > 3 { m["last_used_at"] = now - 120_000 }
            if kind == "login" { m["username"] = "unarbos" }
            return m
        }
        func act(_ id: Int, _ action: String, _ name: String, _ project: (String, String)?, _ host: String?, _ by: String, _ minsAgo: Int64) -> [String: Any] {
            var m: [String: Any] = ["id": id, "user_id": "fx-me", "at": now - minsAgo * 60_000, "action": action, "name": name, "by": by, "detail": [:],
                                    "target_kind": "secret", "target_id": "fx-sec-\(name.lowercased())", "target_name": name]
            if let p = project { m["project_id"] = p.0; m["project_name"] = p.1 }
            if let host { m["host"] = host }
            return m
        }
        let stripeAgents = [agent("fx-affine", "Affine", true, "pull refund reports"),
                            agent("fx-comms", "Comms", true, "monthly revenue line in the newsletter"),
                            agent("fx-hyper", "Hyper", false, "")]
        var stripe = secret("fx-sec-stripe", "STRIPE_KEY", "STRIPE_KEY", "key", "Stripe live key — refunds and reports. Never for payouts.", ["api.stripe.com"], stripeAgents, 12)
        stripe["inject_env"] = true
        if DebugHooks.fixtureSecretHandoff != nil {
            // Secret hand-off rig: the key was given to Comms in chat.
            stripe["inject_env"] = false
            stripe["hosts"] = []
            stripe["note"] = "For the refunds report. Read-only until you say otherwise."
            stripe["last_used_at"] = nil
            stripe["agents"] = [agent("fx-comms", "Comms", true, "the refunds report")]
            stripe["source"] = ["kind": "chat", "project_id": "fx-comms", "project_name": "Comms", "chat_id": "fx-comms-main"]
        }
        // The People list (spec §7.7), the Mac fixture's twin: Sam may ask (on), Jen's row is off.
        stripe["people"] = [
            ["user_id": "fx-u4", "handle": "sam", "name": "Sam Okafor", "note": "reports only", "created_at": now - 9 * 86_400_000,
             "enabled": true, "scope": ["kind": "all"], "last_asked_at": now - 120_000],
            ["user_id": "fx-u2", "handle": "jen", "name": "Jen Park", "note": "", "created_at": now - 9 * 86_400_000,
             "enabled": false, "disabled_at": now - 2 * 86_400_000, "scope": ["kind": "all"]],
        ]
        stripe["others_ask"] = "refuse"
        let mailgun = secret("fx-sec-mailgun", "MAILGUN_API_KEY", "MAILGUN_API_KEY", "key", "Newsletter sends.", ["api.mailgun.net"], [agent("fx-comms", "Comms", true, "send the newsletter")], 5)
        let github = secret("fx-sec-github", "GitHub login", "GITHUB_LOGIN", "login", "The unarbos account, for Actions and reviews.", ["github.com", "*.github.com"], [agent("fx-hyper", "Hyper", true, "review failing Actions runs")], 1)
        let vault: [String: Any] = ["id": "fx-vault-1p", "provider": "onepassword", "name": "Arbos — Research", "items_count": 12, "status": "ok", "created_at": now - 20 * 86_400_000,
                                    "env_items": 1, "limits": ["reads_per_hour": 1000, "reads_used_hour": 3, "reads_per_day": 1000, "reads_used_day": 41, "cache_ttl_secs": 300],
                                    "agents": [agent("fx-affine", "Affine", true, "read the Stripe and Postgres items")]]
        var usedBySam = act(1, "used", "STRIPE_KEY", ("fx-affine", "Affine"), "api.stripe.com", "agent:affine", 2)
        usedBySam["detail"] = ["asked_by": "fx-u4", "asked_by_handle": "sam"]
        let activity = [usedBySam,
                        act(2, "access_on", "MAILGUN_API_KEY", ("fx-comms", "Comms"), nil, "user:me", 1_440),
                        act(3, "refused", "STRIPE_KEY", ("fx-hyper", "Hyper"), "stripe.evil.example", "agent:hyper", 4_320)]
        // One pending card: shown on the Secrets root and on Comms's page (same rule on the Mac).
        let request: [String: Any] = ["id": "fx-req-1", "project_id": "fx-comms", "project_name": "Comms", "chat_id": "fx-comms-main", "name": "GITHUB_LOGIN",
                                      "target": ["kind": "secret", "id": "fx-sec-github", "ref": "GITHUB_LOGIN"],
                                      "purpose": "Sign in to post the release notes", "use": "browser_fill", "host": "github.com",
                                      "status": "pending", "created_at": now - 180_000, "expires_at": now + 420_000]
        fixtureVaultItems = [
            ["ref": "op://Arbos — Research/Stripe", "title": "Stripe", "category": "API_CREDENTIAL", "fields": ["credential", "username"], "env": "STRIPE", "inject_env": true],
            ["ref": "op://Arbos — Research/Postgres", "title": "Postgres", "category": "DATABASE", "fields": ["password", "hostname", "port", "database"], "env": "POSTGRES", "inject_env": false],
        ]
        // The vault and the connect flow are seeded only with `ARBOS_FIXTURE_VAULT=1` (rig):
        // the shipped default hides Connect 1Password (`features.vault_connect` off, D83 Phase 2).
        let withVault = DebugHooks.fixtureVault
        secrets = SecretsOverview.parse(["paused": false, "vaults": withVault ? [vault] : [], "secrets": [stripe, mailgun, github],
                                         "requests_pending": 1, "requests": [request], "activity": activity,
                                         "providers": withVault ? ["onepassword"] : [], "features": ["vault_connect": withVault]])
        var stripeAccess = stripeAgents[1]
        stripeAccess["secret"] = stripe
        var vaultAccess = agent("fx-comms", "Comms", true, "read the Stripe item")
        vaultAccess["vault"] = vault
        vaultAccess["target_kind"] = "vault"
        projectSecrets["fx-comms"] = ProjectSecrets.parse(["paused": false, "owner": true, "items": withVault ? [stripeAccess, vaultAccess] : [stripeAccess],
                                                            "requests": [request], "activity": activity])
    }

    /// The agent pane's Routines card (the Mac fixture's three: a weekday stand-up, a paused
    /// digest, a webhook that last fired 40 min ago). Comms owns them; other agents have none.
    func fixtureRoutines(_ projectId: Id) -> [Routine] {
        guard projectId == "fx-comms" else { return [] }
        let now = nowMs()
        return [
            Routine(id: "fx-rt-standup", projectId: projectId, name: "Morning stand-up",
                    trigger: .cron(expr: "0 9 * * 1-5", tz: "Europe/Berlin"), enabled: true,
                    lastRunAt: now - 20 * 3_600_000, nextRunAt: now + 4 * 3_600_000),
            Routine(id: "fx-rt-digest", projectId: projectId, name: "Open PR digest",
                    trigger: .cron(expr: "0 * * * *", tz: "UTC"), enabled: false, lastRunAt: nil, nextRunAt: nil),
            Routine(id: "fx-rt-deploy", projectId: projectId, name: "Deploy finished",
                    trigger: .webhook, enabled: true, lastRunAt: now - 40 * 60_000, nextRunAt: nil),
        ]
    }

    /// `GET /machine/debug` as the rig answers it: an awake 4-core box up 5 h 12 m, one
    /// workspace per agent, Comms mid-turn (42 s, 7 tool calls) and — with
    /// `ARBOS_FIXTURE_WORKING=n` — the next agents too. Goes through the real parser.
    func fixtureBoxReport() -> BoxReport {
        let working = Set(["fx-comms"] + projects.dropFirst().prefix(max(0, DebugHooks.fixtureWorking - 1)).map(\.id))
        let nearLimit = DebugHooks.fixtureMachineNearLimit
        let workspaces: [[String: Any]] = projects.filter { $0.id != peopleSlot }.enumerated().map { i, p in
            let on = working.contains(p.id)
            var turn: [String: Any] = ["running": on]
            if on {
                turn["current"] = ["elapsed_ms": p.id == "fx-comms" ? 42_000 : 128_000, "tool_calls": p.id == "fx-comms" ? 7 : 3]
            }
            // Settings › Machine's per-agent breakdown: a share per workspace, the busy ones
            // biggest ([feedback machine-page-ios-2026-10-26]).
            let usage: [String: Any] = on
                ? ["cpu_pct": p.id == "fx-comms" ? 61.0 : 9.0, "mem_mb": p.id == "fx-comms" ? 1900 : 1100, "pids": 40, "disk_mb": p.id == "fx-comms" ? 2300 : 900]
                : ["cpu_pct": 0.0, "mem_mb": 200 + 50 * i, "pids": 12, "disk_mb": i == 1 ? 1600 : 100 + 150 * i]
            return ["project_id": p.id, "name": p.name, "healthy": true, "turn": turn,
                    "jobs_running": on && p.id == "fx-comms" ? 1 : 0, "usage": usage]
        }
        // `ARBOS_FIXTURE_MACHINE=near-limit`: disk 92 % and memory 88 % used, CPU 71 %.
        let (diskAvail, memUsed, cpuPct, rx, tx, rxToday, txToday): (Int, Int, Double, Double, Double, Int, Int) = nearLimit
            ? (819, 7209, 71.0, 4_800_000, 300_000, 6_100_000_000, 300_000_000)
            : (6861, 2867, 23.0, 1_200_000, 80_000, 2_000_000_000, 100_000_000)
        let debug: [String: Any] = [
            "relay": ["state": "awake", "awake_for_s": 5 * 3600 + 12 * 60, "provider": "gvisor",
                      "agentd": ["image": "arbos-agent-0.1.7"]],
            "box": ["reachable": true, "state": [
                "uptime_s": 5 * 3600 + 12 * 60, "load": [38.5, 36.9, 32.1], "load_scope": "host",
                "memory_mb": ["total": 8192, "available": 8192 - memUsed, "source": "cgroup"],
                "disk": ["total_mb": 10240, "available_mb": diskAvail],
                "resources": [
                    "sampled_at": nowMs(), "load_scope": "host",
                    "cpu": ["cores": 4.0, "pct": cpuPct, "source": "cgroup"],
                    "memory": ["total_mb": 8192, "used_mb": memUsed, "source": "cgroup"],
                    "disk": ["total_mb": 10240, "used_mb": 10240 - diskAvail, "available_mb": diskAvail,
                             "inodes_total": 131_072, "inodes_avail": nearLimit ? 1_200 : 93_000],
                    "net": ["rx_bps": rx, "tx_bps": tx, "rx_bytes_today": rxToday, "tx_bytes_today": txToday],
                ],
                "workspaces": workspaces,
            ]],
        ]
        return BoxReport(debug: debug, now: nowMs())
    }

    /// Two sample message notifications (an agent, a person) for screenshots.
    func postSampleNotifications() {
        let affine = projects.first { $0.id == "fx-affine" }
        let agentInfo = MessagePush.userInfo(
            chatId: "fx-affine-main", projectId: "fx-affine", title: "Affine",
            body: "Shortlist of 6 candidates is ready — two look very strong.",
            senderKind: "agent", senderId: "fx-affine", senderName: "Affine", senderHandle: "affine",
            avatar: nil, avatarColor: nil, avatarUrl: nil)
        let personInfo = MessagePush.userInfo(
            chatId: "fx-dm-unconst", projectId: peopleSlot, title: "Unconst",
            body: "@main who is in the standup tomorrow?",
            senderKind: "user", senderId: "gh_32490803", senderName: "Unconst", senderHandle: "unconst",
            avatar: nil, avatarColor: nil, avatarUrl: "https://avatars.githubusercontent.com/u/32490803?v=4")
        Task {
            await PushManager.shared.postMessage(id: "sample-agent", info: agentInfo)
            try? await Task.sleep(nanoseconds: 2_500_000_000)
            await PushManager.shared.postMessage(id: "sample-person", info: personInfo)
            PushManager.shared.setBadge(3)
        }
    }

    /// Sends in fixture mode append locally so the composer can be exercised,
    /// then a scripted reply lands as complete bubbles (ack → working → result).
    /// Fixture `openDM`: the existing DM with `handle`, else a new one built from the member
    /// as the group lists them (Reply in DM rig and tests, #1836). No relay.
    func fixtureOpenDM(handle: String, navigate: Bool) -> PeopleChat? {
        let handle = handle.trimmingCharacters(in: CharacterSet(charactersIn: "@ "))
        if let dm = peopleChats.first(where: { $0.kind == "dm" && $0.humans.contains { $0.handle == handle } }) {
            if navigate { path = [.chat(chatId: dm.id, projectId: peopleSlot)] }
            return dm
        }
        guard let peer = peopleChats.flatMap(\.members).first(where: { !$0.isAgent && $0.handle == handle }),
              let me = peopleChats.flatMap(\.members).first(where: { $0.member == myUserId })
        else { return nil }
        let id = "fx-dm-\(handle)"
        let dm = PeopleChat(id: id, kind: "dm", createdBy: me.id, createdAt: nowMs(), title: "@\(handle)", pending: false,
                            members: [peer, me], headSeq: 0, readSeq: 0, unread: false, lockState: "people_only",
                            lastActivityAt: nowMs(), myRole: "owner", muted: false, silenced: false)
        peopleChats.insert(dm, at: 0)
        messages[id] = []
        caughtUp.insert(id)
        if navigate { path = [.chat(chatId: id, projectId: peopleSlot)] }
        return dm
    }

    func fixtureAppend(chatId: Id, text: String, attachments: [Attachment], replyTo: Id? = nil,
                       forwardedFrom: String? = nil) {
        var list = messages[chatId] ?? []
        // Finish the fixture's stuck "working" turn so the demo reads cleanly.
        if let i = list.lastIndex(where: { $0.isStreaming }) {
            list[i].status = .done
            list[i].parts = [.text(id: "\(list[i].id)-t", text: "Got it.")]
        }
        var m = user(newId(), chatId, text, at: nowMs())
        m.replyTo = replyTo
        m.forwardedFrom = forwardedFrom
        m.parts = attachments.map { .attachment(id: newId(), attachment: $0) } + (text.isEmpty ? [] : m.parts)
        list.append(m)
        messages[chatId] = list
        Task { await fixtureReply(chatId: chatId) }
    }

    /// Rig (`ARBOS_SHOW=reply-long`): one finished agent reply appended as the chat's last
    /// message, so the composer's reply preview can be armed on a long one.
    func fixtureAppendAssistant(chatId: Id, text: String) {
        var list = messages[chatId] ?? []
        if let i = list.lastIndex(where: { $0.isStreaming }) {
            list[i].status = .done
            list[i].parts = [.text(id: "\(list[i].id)-t", text: "Got it.")]
        }
        list.append(assistant(newId(), chatId, text, at: nowMs()))
        messages[chatId] = list
    }

    /// Rig (`ARBOS_SHOW=table`): Jacob's "Find me a book store in lima" turn as agentd
    /// wrote it on 2026-10-25 — the ask, then the three `send_message` bubbles (conclusion,
    /// GFM table of linked stores, details) — for the table renderer's shots.
    func fixtureLimaReply(chatId: Id, bubbles: Int = AppModel.limaReply.count) {
        var list = messages[chatId] ?? []
        if let i = list.lastIndex(where: { $0.isStreaming }) {
            list[i].status = .done
            list[i].parts = [.text(id: "\(list[i].id)-t", text: "Got it.")]
        }
        let t = nowMs()
        list.append(user(newId(), chatId, "Find me a book store in lima", at: t))
        for (i, text) in AppModel.limaReply.prefix(bubbles).enumerated() {
            list.append(assistant(newId(), chatId, text, at: t + Int64(1_000 * (i + 1))))
        }
        messages[chatId] = list
    }

    /// Rig (`ARBOS_SHOW=agent-message` / `agent-message-received`): the agent-to-agent
    /// messaging rows exactly as agentd writes them — sender side: "send Affine a hello" →
    /// the hand-off row ("2 messages with ◉ Affine") → "Sent to Affine." → the relayed reply;
    /// receiving side: Comms's words as a `project` row (member bubble labelled "Comms"),
    /// the reply, "Replied to ◉ Comms".
    func fixtureAgentMessage(chatId: Id, received: Bool) {
        var list = messages[chatId] ?? []
        if let i = list.lastIndex(where: { $0.isStreaming }) {
            list[i].status = .done
            list[i].parts = [.text(id: "\(list[i].id)-t", text: "Got it.")]
        }
        let t = nowMs()
        if received {
            var from = Message(id: newId(), chatId: chatId, role: .project, sender: "project:fx-comms", status: .done, turn: nil,
                               decision: nil, subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil,
                               createdAt: t, completedAt: t, pending: false,
                               parts: [.text(id: newId(), text: "Hello, Affine — Jacob asked me to pass along a hello.")], hiddenCount: 0)
            from.senderName = "Comms"
            from.senderHandle = "comms"
            var reply = assistant(newId(), chatId, "Hello back, Jacob — and hi Comms! Ready whenever you want the shortlist sent.", at: t + 15_000)
            reply.agentReplyTo = "Comms"
            list += [from, reply]
        } else {
            let sent = Message(id: newId(), chatId: chatId, role: .assistant, sender: "agent", status: .done, turn: .spawn, decision: "new",
                               subagentId: nil, subagentName: "Affine", handoffMessages: 2, inReplyTo: nil, usage: [:], model: nil,
                               createdAt: t + 5_000, completedAt: t + 5_000, pending: false, parts: [], hiddenCount: 1)
            list += [
                user(newId(), chatId, "send Affine a hello", at: t),
                sent,
                assistant(newId(), chatId, "Sent to Affine.", at: t + 6_000),
                assistant(newId(), chatId, "Affine says hello back — and it's ready whenever you want the shortlist sent.", at: t + 40_000),
            ]
        }
        messages[chatId] = list
    }

    /// Rig (`ARBOS_SHOW=code-block`): the Mac fixture's `REPLY_CODE` reply.
    static let codeReply = """
    The transcript is a bottom-aligned list with one row per markdown block:

    ```rust
    let list = ListState::new(0, ListAlignment::Bottom, px(320.0));
    list(list.clone(), cx.processor(Self::render_row))
    ```

    Rows are keyed by `{msg}#{part}.{block}` and re-spliced only when their bytes change.
    """

    /// Rig (`ARBOS_SHOW=math`): Jacob's "write bubble sort" turn (2026-10-26 10:50, his Mac
    /// screenshot) — a Python code card, then prose with inline TeX — for the math renderer's shots.
    func fixtureBubbleSortReply(chatId: Id) {
        var list = messages[chatId] ?? []
        if let i = list.lastIndex(where: { $0.isStreaming }) {
            list[i].status = .done
            list[i].parts = [.text(id: "\(list[i].id)-t", text: "Got it.")]
        }
        let t = nowMs()
        list.append(user(newId(), chatId, "write bubble sort in python", at: t))
        list.append(assistant(newId(), chatId, AppModel.bubbleSortReply, at: t + 1_000))
        messages[chatId] = list
    }

    /// The reply to "write bubble sort in python" from Jacob's 2026-10-26 screenshot, byte for
    /// byte (the Mac fixture's `ARBOS_FIXTURE_REPLY=math`): `$O(n^2)$` rendered as raw text.
    static let bubbleSortReply = """
    Here is a standard implementation of bubble sort in Python:

    ```python
    def bubble_sort(arr):
        n = len(arr)
        for i in range(n):
            swapped = False
            for j in range(0, n - i - 1):
                if arr[j] > arr[j + 1]:
                    arr[j], arr[j + 1] = arr[j + 1], arr[j]
                    swapped = True
            # If no two elements swapped, list is already sorted
            if not swapped:
                break
        return arr

    # Example usage:
    items = [64, 34, 25, 12, 22, 11, 90]
    print(bubble_sort(items))  # Output: [11, 12, 22, 25, 34, 64, 90]
    ```

    It runs in $O(n^2)$ time in the worst and average cases, and $O(n)$ time when the array is already sorted thanks to the `swapped` flag. Space complexity is $O(1)$.
    """

    /// Rig (`ARBOS_SHOW=links`): Jacob's "Read the latest from the New York post" turn — seven
    /// linked headlines as bullets, then one line of prose — for the link icons' shots.
    func fixtureNYPostReply(chatId: Id) {
        var list = messages[chatId] ?? []
        if let i = list.lastIndex(where: { $0.isStreaming }) {
            list[i].status = .done
            list[i].parts = [.text(id: "\(list[i].id)-t", text: "Got it.")]
        }
        let t = nowMs()
        list.append(user(newId(), chatId, "Read the latest from the New York post", at: t))
        list.append(assistant(newId(), chatId, AppModel.nyPostReply, at: t + 1_000))
        messages[chatId] = list
    }

    /// The reply to "Read the latest from the New York post" (agent Bubble, 2026-10-25, Jacob's
    /// screenshot), byte for byte in shape (the Mac fixture's `ARBOS_FIXTURE_REPLY=links`).
    static let nyPostReply = """
    - [Syringe attack: lawyer claims mistaken identity](https://nypost.com/2026/10/25/us-news/syringe-attack-lawyer-claims-mistaken-identity/)
    - [Arrest story: a second Jewish victim minutes earlier](https://nypost.com/2026/10/25/us-news/arrest-story-second-jewish-victim-minutes-earlier/)
    - [Ex-NYPD cop charged in a George Washington Bridge killing, free on $1M bail](https://nypost.com/2026/10/25/us-news/ex-nypd-cop-charged-in-gw-bridge-killing-free-on-1m-bail/)
    - [Thomas Markle home search, child pornography, Philippine official’s comment](https://nypost.com/2026/10/25/world-news/thomas-markle-home-search-philippine-official-comment/)
    - [Musk says SpaceX cut Ukraine’s Starlink access](https://nypost.com/2026/10/25/business/musk-says-spacex-cut-ukraines-starlink-access/)
    - [Giants beat the Texans in overtime in J. J. McCarthy’s first start](https://nypost.com/2026/10/25/sports/giants-beat-texans-in-overtime-mccarthy-first-start/)
    - [McCarthy’s wife after that win](https://nypost.com/2026/10/25/sports/mccarthys-wife-after-that-win/)

    No times or bylines were on the page text I read. Want one of these opened?
    """

    /// The reply to "Find me a book store in lima" (agent Bubble, 2026-10-25 21:31 UTC),
    /// one string per bubble, byte for byte (the Mac fixture's `ARBOS_FIXTURE_REPLY=table`).
    static let limaReply: [String] = [
        "Go to Librería El Virrey in Miraflores. Their site lists Sunday hours as 11am–7pm, so it should still be open this afternoon.",
        """
        | Store | Area | Sunday | What they sell |
        | --- | --- | --- | --- |
        | [Librería El Virrey](https://www.elvirrey.com/) | Miraflores, Calle Bolognesi 510 | 11am–7pm | Spanish literature, essays, history, art; some English at a higher price |
        | [Communitas](https://www.communitas.pe/) | San Isidro, Av. 2 de Mayo 1690 | 10:30am–7:30pm | Humanities, literature, law, history, science, in Spanish |
        | [Vallejo Librería Café](https://www.tangol.com/blog/en/the-3-best-bookstores-in-lima/n-15210) | San Isidro, Av. Camino Real 1119 | not listed | Books plus coffee or tea |
        | [Assouline Lima](https://www.assouline.com/pages/store/lima) | Miraflores, street not listed | not listed | Luxury and design books |
        """,
        "El Virrey’s phone is (511) 713 0505. Their own page says the shop dates to late 1973; a travel write-up said 1972. Communitas is the other store with Sunday hours on its site, but it is in San Isidro, not Miraflores, and the page does not clearly say it is a walk-in shop, so check before you go. Vallejo is a bookstore-café; I only have a travel blog for it, and hours were not listed. Assouline is a luxury boutique, not a general bookstore. La Casa de Kanu in Barranco was called a favorite on an expat guide, but I have no street address or hours.\n\nHours were not checked on a second source, and I did not confirm anyone is open right now. No used-book shop and no central Lima store made this list with an address I could verify.\n\nWant me to check whether El Virrey is open right now, or look for an English-language shop?",
    ]

    /// The shape of Jacob's Oct 23 screenshot — a heading, bold labels and bullets over
    /// twenty-odd lines (the Mac fixture's `ARBOS_FIXTURE_REPLY=long` carries the same text).
    static let longReview = """
    Arbos, from where I sat:

    **What worked**
    - Authority held: three guest agents deferred to the owner's call every time.
    - The handoff lines read cleanly; nobody asked who was speaking.
    - Replies landed inside two seconds on every box.

    **What did not**
    - The group lost the thread twice when two agents answered at once.
    - One attachment never rendered on the phone.
    - The working line stayed up after a stopped turn.

    **Next**
    1. Serialize agent replies in groups.
    2. Re-check the attachment pipeline on iPhone.
    3. Clear the working line on stop.

    Tell me which of these you want first.
    """

    /// Grok Bot reply flow for a cloning exercise: one short ack, a working
    /// line, then a conversational result — no thoughts, no milestone boilerplate.
    private func fixtureReply(chatId: Id) async {
        func append(_ m: Message) { messages[chatId, default: []].append(m) }
        func replace(_ m: Message) { if let i = messages[chatId]?.firstIndex(where: { $0.id == m.id }) { messages[chatId]?[i] = m } }
        func bubble(_ text: String, done: Bool = true) -> Message {
            let id = newId()
            return Message(id: id, chatId: chatId, role: .assistant, sender: "agent", status: done ? .done : .streaming, turn: .reply, decision: nil,
                           subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: nowMs(),
                           completedAt: done ? nowMs() : nil, pending: false, parts: [.text(id: "\(id)-t", text: text)], hiddenCount: 0)
        }
        let request = messages[chatId]?.last(where: { $0.role == .user })?.plainText.lowercased() ?? ""
        let clone = request.contains("pull") || request.contains("clone") || request.contains("subtensor")
        try? await Task.sleep(nanoseconds: 900_000_000)
        // 1. Ack — one short present-tense sentence, nothing else.
        append(bubble(clone ? "Cloning it now." : "On it."))
        // 2. Working line only (no milestone bubble for a quick clone).
        let workId = newId()
        var work = Message(id: workId, chatId: chatId, role: .assistant, sender: "agent", status: .streaming, turn: .act, decision: nil,
                           subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: nowMs(),
                           completedAt: nil, pending: false, parts: [.status(id: "\(workId)-s", code: .working, message: "Working")], hiddenCount: 0)
        append(work)
        let steps = clone
            ? ["Cloning RaoFoundation/subtensor"]
            : ["Reading your Discord style", "Drafting the rewrite rule"]
        for step in steps {
            try? await Task.sleep(nanoseconds: clone ? 2_200_000_000 : 1_600_000_000)
            work.activity = step
            replace(work)
        }
        try? await Task.sleep(nanoseconds: clone ? 800_000_000 : 1_400_000_000)
        if clone {
            // Quick clone: drop the working row; result is the only follow-up.
            if let i = messages[chatId]?.firstIndex(where: { $0.id == workId }) {
                messages[chatId]?.remove(at: i)
            }
        } else {
            // Longer act: working row becomes a short progress note.
            work.activity = nil
            work.status = .done
            work.completedAt = nowMs()
            work.parts = [.text(id: "\(workId)-t", text: "Got your style from 40 recent messages.")]
            replace(work)
        }
        // 3. Result as one complete bubble — no token stream (feedback AFyd_3zYZ2uaRwPk9_7_rMs).
        try? await Task.sleep(nanoseconds: clone ? 500_000_000 : 700_000_000)
        let resultText = clone
            ? "Got `subtensor/` on main. What do you want to do with it?"
            : "Done. Short replies from now on, and I'll keep anything that goes out on Discord to one or two casual lines in your voice."
        append(bubble(resultText))
    }

    /// A several-paragraph agent answer with a list and a code span — the shape
    /// of Jacob's bubble in #1389 (`ARBOS_FIXTURE_LONG_REPLY`, tests).
    static let longReply = """
    I can't see anything in my records that points to an app bug, so I wouldn't blame the app without more evidence.

    The likeliest cause is a prompt that never told the agents to wait for everyone and to keep quiet between turns:

    - post only the number
    - call `I won` only for the lowest
    - wait for every player before the next round

    The app side plays a part too: several agents answer the same message at once, and some of my filings went out before you could stop them.

    Do you want me to post a clear rule summary to the group before the next round?
    """

    private func user(_ id: Id, _ chat: Id, _ text: String, at: Int64) -> Message {
        Message(id: id, chatId: chat, role: .user, sender: "user", status: .done, turn: nil, decision: nil, subagentId: nil,
                subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: at, completedAt: at, pending: false,
                parts: [.text(id: "\(id)-t", text: text)], hiddenCount: 0)
    }

    private func assistant(_ id: Id, _ chat: Id, _ text: String, at: Int64) -> Message {
        Message(id: id, chatId: chat, role: .assistant, sender: "agent", status: .done, turn: .reply, decision: "reply", subagentId: nil,
                subagentName: nil, inReplyTo: nil, usage: [:], model: "anthropic/claude-haiku-5.5", createdAt: at, completedAt: at + 2_000,
                pending: false, parts: [.text(id: "\(id)-t", text: text)], hiddenCount: 0)
    }

    private func handoff(_ id: Id, _ chat: Id, name: String, at: Int64, messages: Int = 2) -> Message {
        Message(id: id, chatId: chat, role: .assistant, sender: "agent", status: .done, turn: .spawn, decision: "new", subagentId: "sub-1",
                subagentName: name, handoffMessages: messages, inReplyTo: nil, usage: [:], model: nil, createdAt: at, completedAt: at + 5_000, pending: false,
                parts: [.status(id: "\(id)-s", code: .handoff, message: "Handed to subagent \(name)")], hiddenCount: 2)
    }

    private func attachment(_ id: Id, _ chat: Id, name: String, mime: String, at: Int64) -> Message {
        Message(id: id, chatId: chat, role: .assistant, sender: "agent", status: .done, turn: .reply, decision: "reply", subagentId: nil,
                subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: at, completedAt: at, pending: false,
                parts: [.attachment(id: "\(id)-a", attachment: Attachment(name: name, mime: mime, blobRef: "fixture-\(id)",
                                                                       bytes: mime == "application/pdf" ? 1_840_000 : nil))], hiddenCount: 0)
    }

    /// As agentd writes it: a `question` part with a choice widget plus the legacy
    /// `waiting_user` status line (the card wins, the line is hidden).
    private func question(_ id: Id, _ chat: Id, _ text: String, options: [String], at: Int64) -> Message {
        let opts = options.enumerated().map { ChoiceOption(id: "opt-\($0.offset)", label: $0.element) }
        let q = Question(questionId: "\(id)-question", question: text, options: opts, multi: false, widgetType: "choice", answer: [], open: true)
        return Message(id: id, chatId: chat, role: .assistant, sender: "agent", status: .done, turn: .reply, decision: "reply", subagentId: nil,
                       subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: at, completedAt: at, pending: false,
                       parts: [.question(id: "\(id)-w", question: q), .status(id: "\(id)-q", code: .askUser, message: text)], hiddenCount: 0)
    }

    /// #1544: an `approval` part as agentd writes it for `cd /tmp && rm -rf …` outside scratch —
    /// `grant_options` make the card offer A once / B in its workspace / C anywhere / D deny.
    private func shellCard(_ id: Id, _ chat: Id, at: Int64) -> Message {
        let pm: [String: Any] = [
            "id": "\(id)-ap", "kind": "approval", "approvalId": "\(id)-approval", "state": "pending",
            "title": "Run a destructive command (file-delete)", "risk": "destructive",
            "summary": "cd /work/app && rm -rf node_modules && git clone --depth 1 https://github.com/unarbos/arbos arbos-src",
            "payload": [
                "kind": "tool_call", "tool": "bash", "class": "file-delete", "path_class": "workspace",
                "why": "deletes files outside /work/tmp",
                "input": ["command": "cd /work/app && rm -rf node_modules && git clone --depth 1 https://github.com/unarbos/arbos arbos-src"],
                "grant_options": [
                    ["path_class": "workspace", "label": "its workspace", "detail": "the agent's own /work folder, including scratch"],
                    ["path_class": "anywhere", "label": "anywhere", "detail": "any folder of its box"],
                ],
            ],
        ]
        return Message(id: id, chatId: chat, role: .assistant, sender: "agent", status: .done, turn: .reply, decision: "approval", subagentId: nil,
                       subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: at, completedAt: at, pending: false,
                       parts: [DocJSON.part(pm)], hiddenCount: 0)
    }

    /// Secret hand-off: the request card (`payload.kind = secret_offer`, no value field).
    private func secretOfferCard(_ id: Id, _ chat: Id, saved: Bool, at: Int64) -> Message {
        var pm: [String: Any] = [
            "id": "\(id)-ap", "kind": "approval", "approvalId": "\(id)-approval", "state": saved ? "approved" : "pending",
            "title": "Give Comms a secret", "risk": "credential", "summary": "the refunds report",
            "payload": ["kind": "secret_offer", "name": "STRIPE_KEY", "purpose": "the refunds report", "agent": "Comms",
                        "projectId": "fx-comms", "chatId": chat, "answers": []],
        ]
        if saved { pm["decidedBy"] = "user:fx-me"; pm["decidedAt"] = at + 60_000 }
        return Message(id: id, chatId: chat, role: .assistant, sender: "agent", status: .done, turn: .reply, decision: "approval", subagentId: nil,
                       subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: at, completedAt: at, pending: false,
                       parts: [DocJSON.part(pm)], hiddenCount: 0)
    }

    /// Secret hand-off: the offer card for a pasted key (A save / B leave; optional).
    private func secretDetectedCard(_ id: Id, _ chat: Id, at: Int64) -> Message {
        let pm: [String: Any] = [
            "id": "\(id)-ap", "kind": "approval", "approvalId": "\(id)-approval", "state": "pending",
            "title": "That looks like a key. Save it to your vault as STRIPE_KEY and remove it from this chat?",
            "risk": "credential", "summary": "Nothing is saved until you choose.",
            "payload": ["kind": "secret_detected", "optional": true, "name": "STRIPE_KEY", "agent": "Comms",
                        "chatId": chat, "messageId": "h3",
                        "answers": [["id": "save", "label": "Save as STRIPE_KEY and remove"], ["id": "deny", "label": "Leave it here"]]],
        ]
        return Message(id: id, chatId: chat, role: .assistant, sender: "agent", status: .done, turn: .reply, decision: "approval", subagentId: nil,
                       subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: at, completedAt: at, pending: false,
                       parts: [DocJSON.part(pm)], hiddenCount: 0)
    }

    /// Secret hand-off: a user row whose value went into the vault — the ghost.
    private func secretGhost(_ id: Id, _ chat: Id, name: String, at: Int64) -> Message {
        Message(id: id, chatId: chat, role: .user, sender: "user", status: .done, turn: nil, decision: nil, subagentId: nil,
                subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: at, completedAt: at, pending: false,
                parts: [.secretRedacted(id: "\(id)-r", name: name, savedBy: "user:fx-me", agent: "fx-comms")], hiddenCount: 0)
    }

    private func stopped(_ id: Id, _ chat: Id, at: Int64) -> Message {
        Message(id: id, chatId: chat, role: .assistant, sender: "agent", status: .cancelled, turn: .reply, decision: nil, subagentId: nil,
                subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: at, completedAt: at, pending: false,
                parts: [.status(id: "\(id)-s", code: .stopped, message: "Stopped by the user")], hiddenCount: 0)
    }

    private func streaming(_ id: Id, _ chat: Id, at: Int64) -> Message {
        Message(id: id, chatId: chat, role: .assistant, sender: "agent", status: .streaming, turn: .reply, decision: nil, subagentId: nil,
                subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: at, completedAt: nil, pending: false,
                parts: [.status(id: "\(id)-s", code: .working, message: "Working")], hiddenCount: 0)
    }

    /// `ARBOS_FIXTURE_LONG_ROWS`: a long group history shaped like the "Testing" group —
    /// four people and one agent talking, a reply every tenth row, a markdown answer
    /// every seventh, a relay membership line every fortieth (#1386), an image or file
    /// every twenty-fifth, a date break every three hundredth. Deterministic, so two
    /// launches render the same rows. Ends `before`; rows sit 40 s apart.
    nonisolated static func longGroupTranscript(rows: Int, before: Int64) -> [Message] {
        let chat = "fx-group-launch"
        let people: [(sender: String, handle: String, name: String)] = [
            ("user:fx-u3", "mira-k", "Mira Kaplan"), ("user:fx-u4", "sam", "Sam Okafor"),
            ("user:fx-me", "jacob", "Jacob Steeves"), ("user:gh_32490803", "unconst", "Unconst"),
        ]
        let lines = [
            "Can we move the launch call to 10 am? The press embargo lifts at noon.",
            "Done — calendar updated. I also pinged the design channel about the hero asset.",
            "The signup form drops the handle field on iOS 27 when autofill fires. Repro in 2 of 5 tries.",
            "Looks related to the keyboard inset change. Checking the TestFlight build from last night.",
            "Shipping notes are in the shared doc. Shout if the pricing table reads wrong.",
            "Also: the relay restarted twice this morning, both clean 1012 closes. Nothing lost.",
            "https://arbos.life/i/launch is live behind the flag. Please smoke it on a phone.",
            "Agreed. Let's keep the group to launch chatter until Friday.",
        ]
        let markdown = [
            "Three things to check before the embargo lifts:\n\n- the **hero** video loops without a black frame\n- `/download/mac` resolves to the new build\n- the status page shows green for the relay\n\nI'll take the first two.",
            "## Launch day timeline\n\n1. 09:30 final smoke on TestFlight\n2. 10:00 call\n3. 12:00 embargo lifts, post goes out\n4. 13:00 watch the relay dashboard\n\nCode to flip the flag: `arbosctl admin flags set launch on`.",
            "| Step | Owner | State |\n| --- | --- | --- |\n| Hero asset | Mira | done |\n| Pricing copy | Sam | review |\n| Relay deploy | Jacob | queued |",
        ]
        var out: [Message] = []
        out.reserveCapacity(rows)
        let start = before - Int64(rows) * 40_000
        for i in 0..<rows {
            let id = "long-\(i)"
            var at = start + Int64(i) * 40_000
            if i > 0, i % 300 == 0 { at += 2 * 3_600_000 }
            let ts = at
            if i % 40 == 39 {
                // What the relay writes on a membership change (#1349 / #1386).
                let who = people[i % people.count]
                out.append(Message(id: id, chatId: chat, role: .system, sender: "system", status: .done, turn: nil, decision: nil, subagentId: nil,
                                   subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: ts, completedAt: nil, pending: false,
                                   parts: [.text(id: "\(id)-t", text: "Jacob added \(who.name)")], hiddenCount: 0))
                continue
            }
            if i % 7 == 6 {
                let text = markdown[(i / 7) % markdown.count]
                out.append(Message(id: id, chatId: chat, role: .assistant, sender: "agent:fx-comms", status: .done, turn: .reply, decision: "reply",
                                   subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: ts, completedAt: ts + 3_000,
                                   pending: false, parts: [.text(id: "\(id)-t", text: text)], hiddenCount: 0, senderHandle: "comms", senderName: "Comms"))
                continue
            }
            let who = people[(i / 3) % people.count]
            if i % 25 == 24 {
                let image = i % 50 == 24
                let a = image ? Attachment(name: "IMG_\(1000 + i).jpg", mime: "image/jpeg", blobRef: nil, kind: "image")
                              : Attachment(name: "launch-notes-\(i).pdf", mime: "application/pdf", blobRef: nil)
                out.append(Message(id: id, chatId: chat, role: .user, sender: who.sender, status: .done, turn: nil, decision: nil, subagentId: nil,
                                   subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: ts, completedAt: nil, pending: false,
                                   parts: [.attachment(id: "\(id)-a", attachment: a)], hiddenCount: 0, senderHandle: who.handle, senderName: who.name))
                continue
            }
            var m = Message(id: id, chatId: chat, role: .user, sender: who.sender, status: .done, turn: nil, decision: nil, subagentId: nil,
                            subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: ts, completedAt: nil, pending: false,
                            parts: [.text(id: "\(id)-t", text: lines[i % lines.count])], hiddenCount: 0, senderHandle: who.handle, senderName: who.name)
            if i % 10 == 5, i >= 5 { m.replyTo = "long-\(i - 5)" }
            out.append(m)
        }
        return out
    }
}

extension Data {
    /// Bytes from a hex string (the relay's `rows.hex` export).
    init?(hexString: String) {
        let chars = Array(hexString.utf8)
        guard chars.count % 2 == 0 else { return nil }
        var out = Data(capacity: chars.count / 2)
        var i = 0
        while i < chars.count {
            guard let hi = Self.nibble(chars[i]), let lo = Self.nibble(chars[i + 1]) else { return nil }
            out.append(hi << 4 | lo)
            i += 2
        }
        self = out
    }

    private static func nibble(_ c: UInt8) -> UInt8? {
        switch c {
        case 48...57: return c - 48
        case 65...70: return c - 55
        case 97...102: return c - 87
        default: return nil
        }
    }
}
#endif
