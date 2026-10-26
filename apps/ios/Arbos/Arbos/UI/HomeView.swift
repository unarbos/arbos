// Home, Grok Bot layout: top bar with the account circle (GitHub avatar, left)
// and the "+" circle (right), then All | Agents | People | Servers and the chat
// list (avatar, name, time, preview). One machine: its state is the small dot
// on the account circle; the machine details live in Settings. The bottom edge
// holds one app-wide search bar and nothing else (Jacob, 2026-10-25, [feedback
// iphone-bottom-search-bar-2026-10-25]; `GlobalSearchView.swift`).

import ArbosFFI
import SwiftUI

/// Selection above the home list: All | Agents | People | Servers. The Mac
/// sidebar carries the same four segments (`sidebar::ListFilter` in
/// crates/arbos-mac); the scope rules here are the shared contract.
enum HomeListFilter: String, CaseIterable, Identifiable {
    case all = "All"
    case agents = "Agents"
    case people = "People"
    case servers = "Servers"
    var id: String { rawValue }

    /// Agent (project) rows are listed under All and Agents.
    var showsProjects: Bool { self == .all || self == .agents }
    /// People chats (DMs, groups) are listed under All and People.
    var showsPeople: Bool { self == .all || self == .people }
    /// Servers are listed under All and Servers.
    var showsServers: Bool { self == .all || self == .servers }

    /// Agents: my agents only — an agent shared in by someone else stays out.
    func admitsProject(shared: Bool) -> Bool {
        showsProjects && !(self == .agents && shared)
    }

    /// Copy for an empty list under this segment (no query typed).
    var emptyCopy: String {
        switch self {
        case .all: return "No conversations yet"
        case .agents: return "No agents yet"
        case .people: return "No people chats yet"
        case .servers: return "No servers yet"
        }
    }
}

struct HomeView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @State private var showNewChat = false
    /// Settings is a sheet from the account circle (as before the 2026-10-23 tab bar).
    @State private var showSettings = false
    /// The bottom bar's text; the results replace the list while it is focused or filled.
    @State private var query = ""
    @State private var slowConnect = false
    /// "Offline · saved chats": the list came from the on-device store and the
    /// relay has not answered for a few seconds (#929).
    @State private var offlineHint = false
    /// Own-agent Share sheet from the home long-press menu.
    /// Own-agent delete confirmation from the home long-press / swipe menu.
    /// People chat delete confirmation from the home long-press / swipe menu.
    /// "Hidden chats N ›" footer → the list of archived agents and people chats.
    @State private var showHidden = false
    /// Goals list (long-running objectives) from the home sidebar section.
    @State private var showGoals = false
    @State private var openGoalId: String?
    /// Home list scope: All mixes agents, people and servers; Agents is your
    /// own agents only (no people chats / other users); People is DMs and groups.
    @State private var listFilter: HomeListFilter = .all

    private var hiddenProjects: [Project] {
        model.projects.filter { model.isArchived($0.id) }.sorted { lastActivity($0) > lastActivity($1) }
    }

    private var hiddenPeople: [PeopleChat] {
        model.peopleChats.filter { model.isArchived(peopleChatId: $0.id) }
            .sorted { peopleActivity($0) > peopleActivity($1) }
    }

    private var hiddenServers: [Server] {
        model.servers.filter { model.isArchived(serverId: $0.id) }
    }

    private var hiddenCount: Int { hiddenProjects.count + hiddenPeople.count + hiddenServers.count }
    @FocusState private var searchFocused: Bool

    /// Folded query (case + diacritics, leading `@` stripped) so "@robert" and "Zoe" find their rows.
    private var searchQuery: String { HomeSearch.normalizeQuery(query) }
    /// The bar is focused or holds text: the results are up instead of the list.
    private var searching: Bool { searchFocused || !query.isEmpty }

    var body: some View {
        NavigationStack(path: $model.path) {
            ZStack(alignment: .top) {
                theme.bg.ignoresSafeArea()
                if searching {
                    GlobalSearchResults(query: query, folded: searchQuery, onOpen: closeSearch)
                        .transition(.opacity)
                } else {
                    homeList
                }
                topBar
            }
            .safeAreaInset(edge: .bottom, spacing: 0) { bottomBar }
            .animation(.easeOut(duration: 0.18), value: searching)
            .task(id: model.link) {
                // Quiet for 4 s after a from-disk launch → say so; live again → hide.
                guard model.showingSavedCopy else {
                    withAnimation(.easeOut(duration: 0.2)) { offlineHint = false }
                    return
                }
                try? await Task.sleep(nanoseconds: 4_000_000_000)
                if !Task.isCancelled, model.showingSavedCopy {
                    withAnimation(.easeOut(duration: 0.2)) { offlineHint = true }
                }
            }
            .toolbar(.hidden, for: .navigationBar)
            .navigationDestination(for: AppModel.Route.self) { route in
                switch route {
                case .chat(let chatId, let projectId):
                    // `.id(chatId)` forces a fresh ChatView when switching threads of the
                    // same agent from the Chats sheet; without it NavigationStack reuses
                    // the destination and the tap looks like it does nothing.
                    ChatView(chatId: chatId, projectId: projectId)
                        .id(chatId)
                case .server(let serverId):
                    // D88: the server's channel list (the slid-in left pane).
                    ServerChannelsView(serverId: serverId)
                        .id(serverId)
                }
            }
            .navigationDestination(isPresented: $showHidden) {
                HiddenChatsView().environmentObject(model).environment(\.theme, theme)
            }
            .navigationDestination(isPresented: $showGoals) {
                GoalsPage().environmentObject(model).environment(\.theme, theme)
            }
            .navigationDestination(isPresented: Binding(
                get: { openGoalId != nil },
                set: { if !$0 { openGoalId = nil } }
            )) {
                GoalDetailPage(goalId: openGoalId ?? "")
                    .environmentObject(model)
                    .environment(\.theme, theme)
            }
            .sheet(isPresented: $showNewChat) {
                NewChatSheet().environmentObject(model).environment(\.theme, theme)
            }
            .sheet(isPresented: $showSettings) {
                SettingsView().environmentObject(model).environment(\.theme, theme)
            }
        }
        .task {
            if DebugHooks.demo, model.isFixture {
                try? await Task.sleep(nanoseconds: 2_600_000_000)
                if let p = model.project("fx-comms") { model.open(project: p) }
            }
            if DebugHooks.liveDemo, let id = DebugHooks.openProject {
                while model.project(id) == nil || model.machine.reported == false { try? await Task.sleep(nanoseconds: 300_000_000) }
                try? await Task.sleep(nanoseconds: 5_500_000_000)
                if let p = model.project(id) { model.open(project: p) }
            }
            if let id = DebugHooks.openPeople {
                while !model.peopleChats.contains(where: { $0.id == id }) { try? await Task.sleep(nanoseconds: 300_000_000) }
                try? await Task.sleep(nanoseconds: 800_000_000)
                if let c = model.peopleChats.first(where: { $0.id == id }) { model.open(peopleChat: c) }
            }
        }
        .task { await model.refreshGoals() }
        .onAppear {
            // Rig: the New Message sheet, which hosts the Create screens (`newagent`,
            // `newgroup`, `newserver`) and the group member picker (`newgroup2`).
            if ["newchat", "newagent", "newgroup", "newgroup2", "newserver"].contains(DebugHooks.show ?? ""),
               DebugHooks.openProject == nil {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { showNewChat = true }
            }
            if DebugHooks.show == "hidden", DebugHooks.openProject == nil {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { showHidden = true }
            }
            if DebugHooks.show == "settings", DebugHooks.openProject == nil {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { showSettings = true }
            }
            // Rig: `ARBOS_HOME_FILTER=all|agents|people|servers` selects a segment at launch.
            if let raw = DebugHooks.homeFilter,
               let f = HomeListFilter.allCases.first(where: { $0.rawValue.lowercased() == raw.lowercased() }) {
                listFilter = f
            }
            // Rig: `ARBOS_SEARCH=<text>` opens the bottom search with that query
            // (`ARBOS_SEARCH=-` focuses the empty bar).
            if let q = DebugHooks.search, DebugHooks.openProject == nil {
                model.paintTranscriptsForSearch()
                query = q == "-" ? "" : q
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { searchFocused = true }
            }
            #if DEBUG
            if DebugHooks.show == "notify", DebugHooks.openProject == nil {
                DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { model.postSampleNotifications() }
            }
            #endif
            if !DebugHooks.liveDemo, DebugHooks.openProject == peopleSlot, let cid = DebugHooks.openChat, model.path.isEmpty, let c = model.peopleChat(cid) {
                model.open(peopleChat: c)
            } else if !DebugHooks.liveDemo, let id = DebugHooks.openProject, model.path.isEmpty, let p = model.project(id) {
                if let cid = DebugHooks.openChat, let c = model.chat(cid) { model.open(chat: c) } else { model.open(project: p) }
            }
        }
        .onChange(of: model.projects.count) { _, _ in
            if !DebugHooks.liveDemo, let id = DebugHooks.openProject, model.path.isEmpty, let p = model.project(id) {
                if let cid = DebugHooks.openChat, let c = model.chat(cid) { model.open(chat: c) } else { model.open(project: p) }
            }
        }
        .onChange(of: model.peopleChats.count) { _, _ in
            if DebugHooks.openProject == peopleSlot, model.path.isEmpty, let cid = DebugHooks.openChat, let c = model.peopleChat(cid) {
                model.open(peopleChat: c)
            }
        }
        .onChange(of: searchFocused) { _, focused in
            // Rooms may finish a checkpoint while the field is focused — paint
            // any still-unpublished transcripts so message search can see them;
            // and load the directory so people without a DM can be found.
            guard focused else { return }
            model.paintTranscriptsForSearch()
            Task { await model.loadDirectory() }
        }
        .onChange(of: query) { _, _ in
            if !searchQuery.isEmpty { model.paintTranscriptsForSearch() }
        }
    }

    /// Clear the bar and drop the keyboard: back to the list.
    private func closeSearch() {
        query = ""
        searchFocused = false
    }

    /// The one control on Home's bottom edge: the app-wide search capsule, 20 pt
    /// side margins like the header, over a fade so rows slide under it. It sits in
    /// the bottom safe-area inset, so it rides the keyboard and clears the home indicator.
    private var bottomBar: some View {
        VStack(spacing: 0) {
            LinearGradient(colors: [theme.bg.opacity(0), theme.bg], startPoint: .top, endPoint: .bottom)
                .frame(height: 18)
                .allowsHitTesting(false)
            HomeSearchBar(query: $query, focused: $searchFocused, active: searching, onClose: closeSearch)
                .padding(.horizontal, 20)
                .padding(.bottom, 8)
                .background(theme.bg)
        }
        .accessibilityIdentifier("home-bottom-bar")
    }

    private var homeList: some View {
                List {
                    Color.clear.frame(height: 66)
                        .listRowInsets(EdgeInsets())
                        .listRowSeparator(.hidden)
                        .listRowBackground(Color.clear)
                        .accessibilityHidden(true)
                    if model.projects.isEmpty && model.peopleChats.isEmpty && model.servers.isEmpty {
                        emptyState
                            .listRowInsets(EdgeInsets())
                            .listRowSeparator(.hidden)
                            .listRowBackground(Color.clear)
                    } else if homeRows.isEmpty {
                        listFilterBar
                            .listRowInsets(EdgeInsets(top: 4, leading: 20, bottom: 8, trailing: 20))
                            .listRowSeparator(.hidden)
                            .listRowBackground(theme.bg)
                        Text(listFilter.emptyCopy)
                            .font(theme.font(Theme.small))
                            .foregroundStyle(theme.muted)
                            .frame(maxWidth: .infinity)
                            .padding(.top, 80)
                            .listRowInsets(EdgeInsets())
                            .listRowSeparator(.hidden)
                            .listRowBackground(Color.clear)
                            .accessibilityIdentifier("home-filter-empty")
                    } else {
                        listFilterBar
                            .listRowInsets(EdgeInsets(top: 4, leading: 20, bottom: 8, trailing: 20))
                            .listRowSeparator(.hidden)
                            .listRowBackground(theme.bg)
                        if let update = model.availableUpdate {
                            UpdateCard(update: update)
                                .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 22))
                                .listRowSeparator(.hidden)
                                .listRowBackground(theme.bg)
                        }
                        if let position = model.waitlistPosition {
                            WaitlistCard(position: position)
                                .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 22))
                                .listRowSeparator(.hidden)
                                .listRowBackground(theme.bg)
                        }
                        if !plannedTasks.isEmpty {
                            plannedTasksSection
                        }
                        ForEach(homeRows) { entry in
                            switch entry.row {
                            case .project(let project):
                                let pinned = model.isHomePinned(projectId: project.id)
                                // borderless (not .plain): .plain steals the list swipe /
                                // long-press on recent iOS, so archive and the menu fail.
                                Button { openSearchHit(project, hit: entry.hit) } label: {
                                    AgentRow(project: project, preview: entry.hit.reason ?? preview(project),
                                             previewIcon: entry.hit.reason == nil ? previewIcon(project) : nil, time: timeLabel(project),
                                             streaming: model.isStreaming(project: project.id),
                                             unread: model.chats(of: project.id).contains { model.chatHasUnread($0) },
                                             pinned: pinned)
                                }
                                .buttonStyle(.borderless)
                                .contextMenu {
                                    // Long-press: Pin (also on leading swipe), reorder when
                                    // pinned, Fork (owned), Archive (also on trailing swipe),
                                    // then Delete for owned agents. Profile is the chat header.
                                    Button {
                                        withAnimation {
                                            if pinned { model.unpinHome(projectId: project.id) }
                                            else { model.pinHome(projectId: project.id) }
                                        }
                                    } label: {
                                        Label(pinned ? "Unpin" : "Pin", systemImage: pinned ? "pin.slash" : "pin")
                                    }
                                    if let pinIndex = model.homePinIndex(projectId: project.id) {
                                        if pinIndex > 0 {
                                            Button {
                                                withAnimation { model.moveHomePinToTop(projectId: project.id) }
                                            } label: {
                                                Label("Move to top", systemImage: "arrow.up.to.line")
                                            }
                                            Button {
                                                withAnimation { model.moveHomePinUp(projectId: project.id) }
                                            } label: {
                                                Label("Move up", systemImage: "arrow.up")
                                            }
                                        }
                                        if pinIndex + 1 < model.pinnedHomeIds.count {
                                            Button {
                                                withAnimation { model.moveHomePinDown(projectId: project.id) }
                                            } label: {
                                                Label("Move down", systemImage: "arrow.down")
                                            }
                                        }
                                    }
                                    if !project.shared {
                                        Button { model.duplicateProject(project) } label: {
                                            Label("Fork", systemImage: "arrow.triangle.branch")
                                        }
                                    }
                                    Button {
                                        withAnimation { model.archive(project.id) }
                                    } label: {
                                        Label("Archive", systemImage: "archivebox")
                                    }
                                    if !project.shared {
                                        Divider()
                                        Button(role: .destructive) { model.deleteProject(project.id) } label: {
                                            Label("Delete", systemImage: "trash")
                                        }
                                    }
                                }
                                .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 22))
                                .listRowSeparator(.hidden)
                                .listRowBackground(theme.bg)
                                .swipeActions(edge: .leading, allowsFullSwipe: true) {
                                    Button {
                                        withAnimation {
                                            if pinned { model.unpinHome(projectId: project.id) }
                                            else { model.pinHome(projectId: project.id) }
                                        }
                                    } label: {
                                        Label(pinned ? "Unpin" : "Pin", systemImage: pinned ? "pin.slash" : "pin")
                                    }
                                    .tint(Color(hex: "#1084FE")!)
                                }
                                // Trailing = swipe left. Full-swipe archives (Telegram-easy).
                                // Archive alone so a destructive Delete cannot steal the full swipe.
                                .swipeActions(edge: .trailing, allowsFullSwipe: true) {
                                    Button {
                                        withAnimation { model.archive(project.id) }
                                    } label: {
                                        Label("Archive", systemImage: "archivebox")
                                    }
                                    .tint(Color(hex: "#636366")!)
                                }
                            case .people(let chat):
                                let pinned = model.isHomePinned(peopleChatId: chat.id)
                                Button { model.open(peopleChat: chat) } label: {
                                    PeopleRow(chat: chat, preview: entry.hit.reason ?? peoplePreview(chat), time: timeLabel(ms: peopleActivity(chat)),
                                              unread: model.peopleUnread.contains(chat.id) || model.unreadCount(for: chat) > 0,
                                              count: model.unreadCount(for: chat), me: model.user?.id,
                                              pinned: pinned)
                                }
                                .buttonStyle(.borderless)
                                .contextMenu {
                                    // Long-press: Pin, reorder when pinned, Archive,
                                    // Mute (groups), Profile/Group info, Delete.
                                    Button {
                                        withAnimation {
                                            if pinned { model.unpinHome(peopleChatId: chat.id) }
                                            else { model.pinHome(peopleChatId: chat.id) }
                                        }
                                    } label: {
                                        Label(pinned ? "Unpin" : "Pin", systemImage: pinned ? "pin.slash" : "pin")
                                    }
                                    if let pinIndex = model.homePinIndex(peopleChatId: chat.id) {
                                        if pinIndex > 0 {
                                            Button {
                                                withAnimation { model.moveHomePinToTop(peopleChatId: chat.id) }
                                            } label: {
                                                Label("Move to top", systemImage: "arrow.up.to.line")
                                            }
                                            Button {
                                                withAnimation { model.moveHomePinUp(peopleChatId: chat.id) }
                                            } label: {
                                                Label("Move up", systemImage: "arrow.up")
                                            }
                                        }
                                        if pinIndex + 1 < model.pinnedHomeIds.count {
                                            Button {
                                                withAnimation { model.moveHomePinDown(peopleChatId: chat.id) }
                                            } label: {
                                                Label("Move down", systemImage: "arrow.down")
                                            }
                                        }
                                    }
                                    Button {
                                        withAnimation { model.archive(peopleChatId: chat.id) }
                                    } label: {
                                        Label("Archive", systemImage: "archivebox")
                                    }
                                    if chat.isGroup {
                                        if model.isPeopleChatMuted(chat.id) {
                                            Button { model.unmutePeopleChat(chat.id) } label: {
                                                Label("Unmute", systemImage: "bell")
                                            }
                                        } else {
                                            Menu {
                                                ForEach(ChatMuteDuration.allCases) { duration in
                                                    Button(duration.title) {
                                                        model.mutePeopleChat(chat.id, for: duration)
                                                    }
                                                }
                                            } label: {
                                                Label("Mute", systemImage: "bell.slash")
                                            }
                                        }
                                    }
                                    if chat.kind == "dm", let other = chat.counterpart(me: model.user?.id) {
                                        Button { model.showProfile(.user(ProfilePerson(other))) } label: { Label("Profile", systemImage: "person.crop.circle") }
                                    } else {
                                        Button { model.showProfile(.group(chatId: chat.id, projectId: nil)) } label: { Label("Group info", systemImage: "person.2") }
                                    }
                                    Button(role: .destructive) { model.deleteOrLeavePeopleChat(chat) } label: {
                                        Label("Delete", systemImage: "trash")
                                    }
                                }
                                .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 22))
                                .listRowSeparator(.hidden)
                                .listRowBackground(theme.bg)
                                .swipeActions(edge: .leading, allowsFullSwipe: true) {
                                    Button {
                                        withAnimation {
                                            if pinned { model.unpinHome(peopleChatId: chat.id) }
                                            else { model.pinHome(peopleChatId: chat.id) }
                                        }
                                    } label: {
                                        Label(pinned ? "Unpin" : "Pin", systemImage: pinned ? "pin.slash" : "pin")
                                    }
                                    .tint(Color(hex: "#1084FE")!)
                                }
                                // Trailing = swipe left. Full-swipe archives (same as agents).
                                // Archive alone so Delete stays on long-press / Hidden chats.
                                .swipeActions(edge: .trailing, allowsFullSwipe: true) {
                                    Button {
                                        withAnimation { model.archive(peopleChatId: chat.id) }
                                    } label: {
                                        Label("Archive", systemImage: "archivebox")
                                    }
                                    .tint(Color(hex: "#636366")!)
                                }
                            case .server(let server):
                                // D88 rule 1: a server row is a chat row — avatar, name, preview,
                                // time, unread dot; no counts, badges or disclosure. Tap → its
                                // channel list; the user then opens a channel.
                                Button { model.open(server: server) } label: {
                                    ServerRow(server: server, preview: entry.hit.reason ?? model.serverPreview(server),
                                              time: timeLabel(ms: model.serverActivity(server)),
                                              unread: model.serverUnread(server))
                                }
                                .buttonStyle(.borderless)
                                .contextMenu {
                                    Button { model.showProfile(.server(serverId: server.id)) } label: {
                                        Label("Server info", systemImage: "person.2")
                                    }
                                    Button {
                                        withAnimation { model.archive(serverId: server.id) }
                                    } label: {
                                        Label("Archive", systemImage: "archivebox")
                                    }
                                }
                                .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 22))
                                .listRowSeparator(.hidden)
                                .listRowBackground(theme.bg)
                            }
                        }
                        if hiddenCount > 0 {
                            hiddenFooter
                                .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 22))
                                .listRowSeparator(.hidden)
                                .listRowBackground(theme.bg)
                        }
                        // Launch timing: the populated list is on screen.
                        Color.clear.frame(height: 1)
                            .listRowInsets(EdgeInsets())
                            .listRowSeparator(.hidden)
                            .listRowBackground(Color.clear)
                            .accessibilityHidden(true)
                            .onAppear { LaunchClock.mark("first-content", detail: "source=\(model.contentOrigin) rows=\(homeRows.count)") }
                    }
                }
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
                .background(theme.bg)
                // Fade under the top bar as an overlay — a List `.mask` clips the
                // system scroll indicator on the right edge (feedback
                // app-01M5MY2Y9HTFR019WAEPPDCNYS). Same treatment as ChatView's
                // headerBlur: keep the soft edge, leave the scroller free.
                .scrollIndicators(.visible)
                .overlay(alignment: .top) {
                    LinearGradient(colors: [theme.bg, theme.bg.opacity(0)], startPoint: .top, endPoint: .bottom)
                        .frame(height: 100)
                        .allowsHitTesting(false)
                }
                .refreshable { await model.refreshRegistry() }
    }

    // MARK: Top bar


    /// One planned task on Home: an agent's unfinished plan step (or next step / goal title).
    private struct PlannedTask: Identifiable {
        let id: String
        let goalId: String
        let projectId: String
        let agentName: String
        let text: String
    }

    /// Open goals → one planned task each (doing step, else first todo, else next_step, else title).
    private var plannedTasks: [PlannedTask] {
        model.openGoals.prefix(6).map { goal in
            let agent = goal.projectName
                ?? model.project(goal.projectId)?.name
                ?? "Agent"
            let doing = goal.plan.first { $0.status == "doing" }
            let todo = goal.plan.first { $0.status == "todo" }
            let text = doing?.title
                ?? todo?.title
                ?? (goal.nextStep.isEmpty ? goal.title : goal.nextStep)
            return PlannedTask(
                id: goal.id,
                goalId: goal.id,
                projectId: goal.projectId,
                agentName: agent,
                text: text
            )
        }
    }

    /// Planned tasks by agent: agent name, task text — tap opens the goal.
    private var plannedTasksSection: some View {
        Section {
            ForEach(plannedTasks) { task in
                Button { openGoalId = task.goalId } label: {
                    HStack(spacing: 12) {
                        if let project = model.project(task.projectId) {
                            AgentAvatar(project: project, theme: theme, size: 28, still: true)
                        } else {
                            Image(systemName: "checklist")
                                .font(.system(size: 14, weight: .semibold))
                                .foregroundStyle(theme.accent)
                                .frame(width: 28, height: 28)
                        }
                        VStack(alignment: .leading, spacing: 2) {
                            Text(task.agentName)
                                .font(theme.font(Theme.body, weight: .medium))
                                .foregroundStyle(theme.text)
                                .lineLimit(1)
                            Text(task.text)
                                .font(theme.font(Theme.tiny))
                                .foregroundStyle(theme.muted)
                                .lineLimit(1)
                        }
                        Spacer(minLength: 0)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.borderless)
                .listRowInsets(EdgeInsets(top: 6, leading: 20, bottom: 6, trailing: 22))
                .listRowSeparator(.hidden)
                .listRowBackground(theme.bg)
                .accessibilityLabel("\(task.agentName), \(task.text)")
            }
            Button { showGoals = true } label: {
                HStack(spacing: 8) {
                    Text("All goals")
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                    Image(systemName: "chevron.right")
                        .font(theme.font(11, weight: .semibold))
                        .foregroundStyle(theme.placeholder)
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .listRowInsets(EdgeInsets(top: 4, leading: 20, bottom: 10, trailing: 22))
            .listRowSeparator(.hidden)
            .listRowBackground(theme.bg)
            .accessibilityLabel("All goals")
        } header: {
            Text("Planned tasks")
                .font(theme.font(Theme.tiny, weight: .medium))
                .foregroundStyle(theme.muted)
                .textCase(nil)
        }
    }

    /// Grok's "Hidden chats  N  ›" line under the list (measured: 15 pt muted,
    /// the count a shade dimmer, a small chevron, 27 pt above the bottom).
    private var hiddenFooter: some View {
        Button { showHidden = true } label: {
            HStack(spacing: 8) {
                Text("Hidden chats")
                    .font(theme.font(Theme.small))
                    .foregroundStyle(theme.muted)
                Text("\(hiddenCount)")
                    .font(theme.font(Theme.small))
                    .foregroundStyle(theme.placeholder)
                Image(systemName: "chevron.right")
                    .font(theme.font(11, weight: .semibold))
                    .foregroundStyle(theme.placeholder)
                Spacer(minLength: 0)
            }
            .padding(.top, 14)
            .padding(.bottom, 20)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Hidden chats, \(hiddenCount)")
    }

    /// Account circle (→ Settings) left, the + circle right. Search lives in the
    /// bottom bar, so the header carries no search disc.
    private var topBar: some View {
        HStack(spacing: 10) {
            Button { showSettings = true } label: {
                ZStack(alignment: .bottomTrailing) {
                    PersonAvatar(url: model.user?.avatarUrl, handle: model.user?.login ?? "?", size: Theme.control, name: model.user?.name, id: model.user?.id, colorIndex: model.user?.avatarColorIndexEffective)
                    Circle()
                        .fill(machineDotColor(model.machine.state, theme: theme))
                        .frame(width: 11, height: 11)
                        .overlay(Circle().strokeBorder(theme.bg, lineWidth: 2))
                        .offset(x: 2, y: 2)
                }
                .frame(width: Theme.control, height: Theme.control)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Account, machine and settings · \(model.machine.label)")
            .accessibilityIdentifier("home-account")
            Spacer()
                .overlay {
                    // Between the account circle and the button, where nothing else lives.
                    if offlineHint {
                        Text("Offline · showing saved chats")
                            .font(theme.font(Theme.small))
                            .foregroundStyle(theme.muted)
                            .lineLimit(1)
                            .minimumScaleFactor(0.8)
                            .transition(.opacity)
                            .accessibilityIdentifier("offline-hint")
                    }
                }
            Button { showNewChat = true } label: {
                GlassCircle {
                    Image(systemName: "plus")
                        .font(theme.font(Controls.glyph, weight: .regular))
                        .foregroundStyle(theme.text)
                }
            }
            .buttonStyle(.plain)
            .accessibilityLabel("New message")
            .accessibilityIdentifier("home-new")
        }
        // One top bar for every screen: `Controls.inset` / `Controls.top` (was Grok's 20 here
        // and 16 in chats; [feedback iphone-header-composer-sizing-2026-10-26]).
        .controlBar()
        .zIndex(1)
    }

    // MARK: Empty / error

    private var emptyState: some View {
        VStack(spacing: 10) {
            if let err = model.registryError {
                Text("Could not reach the relay")
                    .font(theme.font(Theme.body, weight: .semibold))
                    .foregroundStyle(theme.text)
                Text(err)
                    .font(theme.font(Theme.small))
                    .foregroundStyle(theme.muted)
                    .multilineTextAlignment(.center)
                Button("Try again") { Task { await model.refreshRegistry() } }
                    .font(theme.font(Theme.small, weight: .semibold))
                    .foregroundStyle(theme.text)
            } else if model.link == .connecting && !model.isFixture {
                ProgressView().tint(theme.muted)
                Text(slowConnect ? "Still connecting…" : "Loading your agents…")
                    .font(theme.font(Theme.small))
                    .foregroundStyle(theme.muted)
                if slowConnect {
                    Button("Retry") {
                        slowConnect = false
                        Task { await model.refreshRegistry() }
                    }
                    .font(theme.font(Theme.small, weight: .semibold))
                    .foregroundStyle(theme.text)
                }
            } else {
                Text("No agents yet")
                    .font(theme.font(Theme.body, weight: .semibold))
                    .foregroundStyle(theme.text)
                Text("Tap + to create one. The relay sets up its workspace on your machine.")
                    .font(theme.font(Theme.small))
                    .foregroundStyle(theme.muted)
                    .multilineTextAlignment(.center)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 160)
        .padding(.horizontal, 24)
        .task(id: model.link) {
            // "Loading your agents…" must not spin forever: after 10 s offer a retry.
            slowConnect = false
            guard model.link == .connecting else { return }
            try? await Task.sleep(nanoseconds: 10_000_000_000)
            if !Task.isCancelled && model.link == .connecting { slowConnect = true }
        }
    }

    // MARK: Row data

    enum HomeRow: Identifiable {
        case project(Project)
        case people(PeopleChat)
        case server(Server)
        var id: String {
            switch self {
            case .project(let p): return "p:" + p.id
            case .people(let c): return "c:" + c.id
            case .server(let s): return "s:" + s.id
            }
        }
        var activity: Int64 { 0 }
    }

    /// One home row. Every row is a plain `title` hit: the list is no longer
    /// narrowed by typed text (search is the bottom bar, as the Mac's palette);
    /// the segment alone scopes it.
    struct HomeEntry: Identifiable {
        var row: HomeRow
        var hit: HomeSearch.Hit
        var id: String { row.id }
    }

    /// Projects, DMs and servers in one list: pinned first (in the user's pin
    /// order), then newest activity. `listFilter` scopes the rows: Agents hides
    /// people and servers, People hides agents and servers, Servers shows only
    /// servers (`HomeListFilter`). Archived rows stay out (Hidden chats).
    private var homeRows: [HomeEntry] {
        // pinRank: strip index when pinned (0 = top), Int.max when not.
        var rows: [(Int, Int64, HomeEntry)] = []
        let hit = HomeSearch.Hit(kind: .title)
        if listFilter.showsProjects {
            for p in model.projects {
                // Agents: my agents only (`!shared`) — no shared-in agents.
                if !listFilter.admitsProject(shared: p.shared) { continue }
                if model.isArchived(p.id) { continue }
                let pinRank = model.homePinIndex(projectId: p.id) ?? Int.max
                rows.append((pinRank, lastActivity(p), HomeEntry(row: .project(p), hit: hit)))
            }
        }
        if listFilter.showsPeople {
            for c in model.peopleChats where !model.isArchived(peopleChatId: c.id) {
                let pinRank = model.homePinIndex(peopleChatId: c.id) ?? Int.max
                rows.append((pinRank, peopleActivity(c), HomeEntry(row: .people(c), hit: hit)))
            }
        }
        if listFilter.showsServers {
            // D88: servers sort among the chats by last activity (rule 1); the
            // Servers segment shows them alone (Mac parity, 2026-10-24).
            for s in model.servers where !model.isArchived(serverId: s.id) {
                rows.append((Int.max, model.serverActivity(s), HomeEntry(row: .server(s), hit: hit)))
            }
        }
        return rows.sorted {
            if $0.0 != $1.0 { return $0.0 < $1.0 }
            return $0.1 > $1.1
        }.map(\.2)
    }

    /// All | Agents | People | Servers — same capsule pill as profile panes (`PaneTabs`).
    private var listFilterBar: some View {
        let selected = Binding(
            get: { listFilter.rawValue },
            set: { raw in
                guard let next = HomeListFilter(rawValue: raw) else { return }
                Haptics.pick()
                withAnimation(.snappy(duration: 0.2)) { listFilter = next }
            }
        )
        return PaneTabs(tabs: HomeListFilter.allCases.map(\.rawValue), selected: selected)
            .accessibilityIdentifier("home-list-filter")
    }

    /// Open the chat the row points at, else the agent's landing chat.
    private func openSearchHit(_ project: Project, hit: HomeSearch.Hit) {
        if let chatId = hit.chatId, let chat = model.chat(chatId) {
            model.open(chat: chat)
            return
        }
        model.open(project: project)
    }

    private func chatActivity(_ c: Chat) -> Int64 {
        max(c.lastMessageAt, model.messages(of: c.id).last?.createdAt ?? 0)
    }

    private func chatPreview(_ c: Chat) -> String {
        guard let m = model.messages(of: c.id).last(where: { !$0.voice && !$0.plainText.isEmpty }) else { return "No messages yet" }
        let line = m.plainText.split(separator: "\n").first.map(String.init) ?? ""
        let who = m.role == .user ? "You" : (m.senderHandle.map { "@\($0)" } ?? (model.project(c.projectId)?.name ?? ""))
        return (who.isEmpty ? "" : who + ": ") + HomeView.plain(line)
    }

    private func peopleActivity(_ c: PeopleChat) -> Int64 {
        max(c.lastActivityAt, model.messages(of: c.id).last?.createdAt ?? 0)
    }

    private func peoplePreview(_ c: PeopleChat) -> String {
        guard let m = model.messages(of: c.id).last(where: { !$0.voice && !$0.plainText.isEmpty }) else {
            return c.pending ? "Invited · opens when they sign in" : "Say hi"
        }
        let line = m.plainText.split(separator: "\n").first.map(String.init) ?? ""
        let who = m.sender == model.myUserId ? "You" : (m.senderHandle.map { "@\($0)" } ?? "")
        return (who.isEmpty ? "" : who + ": ") + HomeView.plain(line)
    }

    /// Newest message with prose (or an open question) across the project's chats.
    private func latest(_ p: Project) -> Message? {
        var best: Message? = nil
        for chat in model.chats(of: p.id) {
            if let m = model.messages(of: chat.id).last(where: { !$0.voice && (!$0.plainText.isEmpty || $0.openQuestion != nil) }) {
                if best == nil || m.createdAt > best!.createdAt { best = m }
            }
        }
        return best
    }

    /// Grok puts a small glyph before some previews: a checklist when the bot is
    /// waiting on a question, a paper plane when it messaged another bot.
    private func previewIcon(_ p: Project) -> String? {
        guard let m = latest(p) else { return nil }
        if m.openQuestion != nil { return "checklist" }
        if m.role != .user, m.turn == .new, m.subagentName != nil { return "paperplane" }
        return nil
    }

    private func lastActivity(_ p: Project) -> Int64 {
        if let m = latest(p), m.createdAt > 0 { return m.createdAt }
        return model.chats(of: p.id).map(\.lastMessageAt).max() ?? 0
    }

    private func preview(_ p: Project) -> String {
        guard let m = latest(p) else {
            return model.isStreaming(project: p.id) ? "\(p.name) is working" : "No messages yet"
        }
        if let q = m.openQuestion { return HomeView.plain(q.question) }
        let line = m.plainText.split(separator: "\n").first.map(String.init) ?? ""
        if m.role != .user, m.turn == .new, let name = m.subagentName {
            return line.isEmpty ? "Messaged \(name)" : "Messaged \(name): \(HomeView.plain(line))"
        }
        return (m.role == .user ? "You: " : "") + HomeView.plain(line)
    }

    /// Strip inline markdown markers for one-line previews.
    static func plain(_ s: String) -> String {
        var t = s
        for mark in ["**", "__", "`", "*", "_", "#"] { t = t.replacingOccurrences(of: mark, with: "") }
        return t.trimmingCharacters(in: .whitespaces)
    }

    private func timeLabel(_ p: Project) -> String { timeLabel(ms: lastActivity(p)) }

    private func timeLabel(ms: Int64) -> String { HomeView.timeLabel(ms: ms) }

    /// The home row's time column; the server channel list uses the same rule.
    static func timeLabel(ms: Int64) -> String {
        guard ms > 0 else { return "" }
        let date = Date(timeIntervalSince1970: Double(ms) / 1000)
        let cal = Calendar.current
        if cal.isDateInToday(date) { return date.formatted(date: .omitted, time: .shortened) }
        if cal.isDateInYesterday(date) { return "Yesterday" }
        if let week = cal.date(byAdding: .day, value: -6, to: Date()), date > week {
            return date.formatted(.dateTime.weekday(.wide))
        }
        return date.formatted(.dateTime.month(.abbreviated).day())
    }
}

/// "Update available" — a newer TestFlight build is out (continuous delivery, D85). One tap opens
/// TestFlight on Arbos; "Later" hides the card for this build.
struct UpdateCard: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let update: AvailableBuild

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                GlassCircle(size: 36) { // control-size: exception rowIconTiles
                    Image(systemName: "arrow.down.circle.fill").font(.system(size: 17)).foregroundStyle(theme.text)
                }
                VStack(alignment: .leading, spacing: 2) {
                    Text("Arbos update available").font(theme.font(Theme.body, weight: .semibold)).foregroundStyle(theme.text)
                    Text("TestFlight build \(update.build) · you run \(AppModel.runningBuild)").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                }
                Spacer()
            }
            HStack(spacing: 10) {
                Button { model.dismissUpdate() } label: {
                    Text("Later").font(theme.font(Theme.small, weight: .semibold)).foregroundStyle(theme.text)
                        .frame(maxWidth: .infinity).frame(height: 38)
                        .background(Capsule().fill(theme.field)).overlay(Capsule().strokeBorder(theme.fieldBorder, lineWidth: 1)) // control-size: exception inlineChips
                }
                .buttonStyle(.plain)
                Button { model.openTestFlight() } label: {
                    Text("Update").font(theme.font(Theme.small, weight: .semibold)).foregroundStyle(theme.onPrimary)
                        .frame(maxWidth: .infinity).frame(height: 38)
                        .background(Capsule().fill(theme.primary))
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("home-update-button")
            }
        }
        .padding(14)
        .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(theme.bubbleAssistant))
        .padding(.top, 10)
        .accessibilityIdentifier("home-update-card")
    }
}

/// D89: the host that runs everyone's computer is full; this account is in line. Chats and
/// people work meanwhile; agents start once the machine is ready.
struct WaitlistCard: View {
    @Environment(\.theme) private var theme
    let position: Int

    var body: some View {
        HStack(spacing: 10) {
            GlassCircle(size: 36) { // control-size: exception rowIconTiles
                Image(systemName: "hourglass").font(.system(size: 16)).foregroundStyle(theme.text)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text("You're number \(position) in line for a computer").font(theme.font(Theme.body, weight: .semibold)).foregroundStyle(theme.text)
                Text("We'll let you know when it's ready. Chats and people work now; agents start then.").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
            }
            Spacer()
        }
        .padding(14)
        .background(RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous).fill(theme.bubbleAssistant))
        .padding(.top, 10)
        .accessibilityIdentifier("home-waitlist-card")
    }
}

/// Archived ("hidden") chats, Grok's "Hidden chats" page: agents, people and servers;
/// tap opens, swipe/long-press Unarchive, Delete removes for good.
struct HiddenChatsView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss

    private enum HiddenRow: Identifiable {
        case project(Project)
        case people(PeopleChat)
        case server(Server)
        var id: String {
            switch self {
            case .project(let p): return "p:" + p.id
            case .people(let c): return "c:" + c.id
            case .server(let s): return "s:" + s.id
            }
        }
    }

    private var hidden: [HiddenRow] {
        let projects = model.projects.filter { model.isArchived($0.id) }
            .sorted { $0.name < $1.name }
            .map { HiddenRow.project($0) }
        let people = model.peopleChats.filter { model.isArchived(peopleChatId: $0.id) }
            .sorted { peopleTitle($0) < peopleTitle($1) }
            .map { HiddenRow.people($0) }
        let servers = model.servers.filter { model.isArchived(serverId: $0.id) }
            .sorted { $0.name < $1.name }
            .map { HiddenRow.server($0) }
        return projects + people + servers
    }

    var body: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            List {
                Color.clear.frame(height: 66)
                    .listRowInsets(EdgeInsets())
                    .listRowSeparator(.hidden)
                    .listRowBackground(Color.clear)
                    .accessibilityHidden(true)
                if hidden.isEmpty {
                    Text("No hidden chats")
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                        .frame(maxWidth: .infinity)
                        .padding(.top, 80)
                        .listRowInsets(EdgeInsets())
                        .listRowSeparator(.hidden)
                        .listRowBackground(Color.clear)
                }
                ForEach(hidden) { row in
                    switch row {
                    case .project(let project):
                        Button { model.open(project: project) } label: {
                            AgentRow(project: project, preview: HiddenChatsView.preview(project, in: model), time: "",
                                     streaming: model.isStreaming(project: project.id), unread: false)
                        }
                        .buttonStyle(.borderless)
                        .contextMenu {
                            Button { withAnimation { model.unarchive(project.id) } } label: {
                                Label("Unarchive", systemImage: "tray.and.arrow.up")
                            }
                            if !project.shared {
                                Button(role: .destructive) { model.deleteProject(project.id) } label: {
                                    Label("Delete", systemImage: "trash")
                                }
                            }
                        }
                        .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 22))
                        .listRowSeparator(.hidden)
                        .listRowBackground(theme.bg)
                        // Unarchive first = full-swipe restores; Delete needs a tap.
                        .swipeActions(edge: .trailing, allowsFullSwipe: true) {
                            Button { withAnimation { model.unarchive(project.id) } } label: {
                                Label("Unarchive", systemImage: "tray.and.arrow.up")
                            }
                            .tint(theme.unread)
                            if !project.shared {
                                Button(role: .destructive) { model.deleteProject(project.id) } label: {
                                    Label("Delete", systemImage: "trash")
                                }
                            }
                        }
                    case .people(let chat):
                        Button { model.open(peopleChat: chat) } label: {
                            PeopleRow(chat: chat, preview: HiddenChatsView.peoplePreview(chat, in: model), time: "",
                                      unread: false, count: 0, me: model.user?.id)
                        }
                        .buttonStyle(.borderless)
                        .contextMenu {
                            Button { withAnimation { model.unarchive(peopleChatId: chat.id) } } label: {
                                Label("Unarchive", systemImage: "tray.and.arrow.up")
                            }
                            Button(role: .destructive) { model.deleteOrLeavePeopleChat(chat) } label: {
                                Label("Delete", systemImage: "trash")
                            }
                        }
                        .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 22))
                        .listRowSeparator(.hidden)
                        .listRowBackground(theme.bg)
                        // Unarchive first = full-swipe restores; Delete needs a tap.
                        .swipeActions(edge: .trailing, allowsFullSwipe: true) {
                            Button { withAnimation { model.unarchive(peopleChatId: chat.id) } } label: {
                                Label("Unarchive", systemImage: "tray.and.arrow.up")
                            }
                            .tint(theme.unread)
                            Button(role: .destructive) { model.deleteOrLeavePeopleChat(chat) } label: {
                                Label("Delete", systemImage: "trash")
                            }
                        }
                    case .server(let server):
                        Button { model.open(server: server) } label: {
                            ServerRow(server: server, preview: model.serverPreview(server), time: "", unread: false)
                        }
                        .buttonStyle(.borderless)
                        .contextMenu {
                            Button { withAnimation { model.unarchive(serverId: server.id) } } label: {
                                Label("Unarchive", systemImage: "tray.and.arrow.up")
                            }
                        }
                        .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 22))
                        .listRowSeparator(.hidden)
                        .listRowBackground(theme.bg)
                        .swipeActions(edge: .trailing, allowsFullSwipe: true) {
                            Button { withAnimation { model.unarchive(serverId: server.id) } } label: {
                                Label("Unarchive", systemImage: "tray.and.arrow.up")
                            }
                            .tint(theme.unread)
                        }
                    }
                }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .background(theme.bg)
            .scrollIndicators(.visible)
            // Grok page header: back circle left, title centred in a pill.
            HStack {
                Button { dismiss() } label: {
                    GlassCircle {
                        Image(systemName: "chevron.left")
                            .font(theme.font(Controls.glyph, weight: .semibold))
                            .foregroundStyle(theme.text)
                    }
                }
                .buttonStyle(.plain)
                Spacer()
                GlassPill {
                    Text("Hidden chats")
                        .font(theme.font(Theme.body, weight: .semibold))
                        .foregroundStyle(theme.text)
                        .padding(.horizontal, Controls.textInset)
                        .frame(height: Controls.height)
                }
                Spacer()
                Color.clear.frame(width: Theme.control, height: Theme.control)
            }
            .controlBar()
        }
        .toolbar(.hidden, for: .navigationBar)
        .toolbar(.hidden, for: .tabBar)
    }


    private func peopleTitle(_ c: PeopleChat) -> String {
        if c.isGroup { return c.title.isEmpty ? "Group" : c.title }
        return c.counterpart(me: model.user?.id)?.displayName ?? c.title
    }

    /// Last line of prose in the agent's chats, as on home.
    static func preview(_ p: Project, in model: AppModel) -> String {
        var best: Message? = nil
        for chat in model.chats(of: p.id) {
            if let m = model.messages(of: chat.id).last(where: { !$0.voice && !$0.plainText.isEmpty }),
               best == nil || m.createdAt > best!.createdAt { best = m }
        }
        guard let m = best else { return "No messages yet" }
        let line = m.plainText.split(separator: "\n").first.map(String.init) ?? ""
        return (m.role == .user ? "You: " : "") + HomeView.plain(line)
    }

    static func peoplePreview(_ c: PeopleChat, in model: AppModel) -> String {
        guard let m = model.messages(of: c.id).last(where: { !$0.voice && !$0.plainText.isEmpty }) else {
            return c.pending ? "Invited · opens when they sign in" : "Say hi"
        }
        let line = m.plainText.split(separator: "\n").first.map(String.init) ?? ""
        let who = m.sender == model.myUserId ? "You" : (m.senderHandle.map { "@\($0)" } ?? "")
        return (who.isEmpty ? "" : who + ": ") + HomeView.plain(line)
    }
}

struct AgentRow: View {
    @Environment(\.theme) private var theme
    let project: Project
    let preview: String
    /// SF Symbol drawn before the preview (Grok: checklist / paper plane).
    var previewIcon: String? = nil
    let time: String
    let streaming: Bool
    let unread: Bool
    var pinned: Bool = false

    var body: some View {
        // Grok home row (measured): 48 pt avatar box, text 15 pt after it, name 17
        // semibold over a 15 pt grey preview, time right, 10 pt unread dot at the far
        // right of the preview line, 80 pt row pitch.
        HStack(spacing: 15) {
            // No "agent is working" indicator on rows (Jacob, 2026-10-22): the working
            // state lives in the chat header / activity line. The right edge keeps the
            // unread dot.
            AgentAvatar(project: project, theme: theme, size: Theme.rowAvatar)
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline) {
                    Text(project.name)
                        .font(theme.font(Theme.body, weight: .semibold))
                        .foregroundStyle(theme.text)
                        .lineLimit(1)
                    Spacer()
                    if pinned {
                        Image(systemName: "pin.fill")
                            .font(theme.font(11, weight: .semibold))
                            .foregroundStyle(theme.muted)
                            .accessibilityLabel("Pinned")
                    }
                    Text(time)
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                        .lineLimit(1)
                        .fixedSize()
                }
                HStack(spacing: 6) {
                    if let previewIcon {
                        Image(systemName: previewIcon)
                            .font(theme.font(13, weight: .regular))
                            .foregroundStyle(theme.muted)
                            .accessibilityHidden(true)
                    }
                    Text(preview)
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                    if unread {
                        Circle().fill(theme.unread).frame(width: Theme.unreadDot, height: Theme.unreadDot)
                    }
                }
            }
        }
        .padding(.vertical, Theme.rowVPad)
        .contentShape(Rectangle())
    }
}

/// A people chat in the home list: DMs show the other person; groups show the
/// chat title (custom name, or member handles from the relay) and a member count.
struct PeopleRow: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let chat: PeopleChat
    let preview: String
    let time: String
    let unread: Bool
    /// Unread messages (Grok/Telegram-style count pill); 0 → the dot only.
    var count: Int = 0
    let me: String?
    var pinned: Bool = false

    private var other: ChatMember? { chat.counterpart(me: me) }

    /// Groups name the chat, not the first other human (same rule as CallKit).
    private var title: String {
        if chat.isGroup { return chat.title.isEmpty ? "Group" : chat.title }
        return other?.displayName ?? chat.title
    }

    /// Same-line muted label: handle (no @) for a DM, member count for a group.
    private var label: String {
        if chat.isGroup {
            let n = chat.humans.count
            return "\(n) member\(n == 1 ? "" : "s")"
        }
        return other?.handle ?? chat.title
    }

    /// Tokenised blob URL for the group's custom picture, when one is set.
    private var groupImageURL: String? {
        guard chat.isGroup, let key = chat.imageKey, !key.isEmpty else { return nil }
        return model.blobURL(projectId: "\(peopleSlot):\(chat.id)", key: key)?.absoluteString
    }

    var body: some View {
        HStack(spacing: 15) {
            ZStack(alignment: .bottomTrailing) {
                if chat.isGroup {
                    if let groupImageURL {
                        AvatarCluster(members: [], size: Theme.rowAvatar, imageUrl: groupImageURL)
                    } else if chat.isNeutralAvatar {
                        NeutralAvatar(name: chat.title, size: Theme.rowAvatar)
                    } else if let leaf = chat.leafPick, LeafAvatarsFlag.shared.on {
                        // D92 amendment: the leaf picked on the Create screen.
                        LeafAvatar(pick: leaf, size: Theme.rowAvatar)
                    } else {
                        ZStack {
                            Circle().fill(theme.card).frame(width: Theme.rowAvatar, height: Theme.rowAvatar)
                            Image(systemName: "person.2.fill")
                                .font(theme.font(17, weight: .medium))
                                .foregroundStyle(theme.accent)
                        }
                    }
                } else {
                    PersonAvatar(url: other?.avatarUrl, handle: other?.handle ?? "?", size: Theme.rowAvatar, name: other?.name, id: other?.id, colorIndex: other?.avatarColorIndex)
                }
                if let a = chat.agents.first {
                    AgentAvatar(project: model.agent(member: a, chatId: chat.id), theme: theme, size: 18, still: true)
                        .overlay(Circle().strokeBorder(theme.bg, lineWidth: 2))
                        .offset(x: 3, y: 3)
                }
            }
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline) {
                    Text(title)
                        .font(theme.font(Theme.body, weight: .semibold))
                        .foregroundStyle(theme.text)
                        .lineLimit(1)
                    Text(label)
                        .font(theme.font(Theme.tiny))
                        .foregroundStyle(theme.muted)
                        .lineLimit(1)
                    Spacer()
                    if pinned {
                        Image(systemName: "pin.fill")
                            .font(theme.font(11, weight: .semibold))
                            .foregroundStyle(theme.muted)
                            .accessibilityLabel("Pinned")
                    }
                    Text(time).font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                        .lineLimit(1)
                        .fixedSize()
                }
                HStack(spacing: 6) {
                    Text(preview)
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                    if count > 0 {
                        UnreadBadge(count: count)
                    } else if unread {
                        Circle().fill(theme.unread).frame(width: Theme.unreadDot, height: Theme.unreadDot)
                    }
                }
            }
        }
        .padding(.vertical, Theme.rowVPad)
        .contentShape(Rectangle())
    }
}

struct PulseDot: View {
    let color: Color
    @State private var on = false

    var body: some View {
        Circle()
            .fill(color.opacity(on ? 1 : 0.35))
            .frame(width: 7, height: 7)
            .onAppear {
                withAnimation(.easeInOut(duration: 0.9).repeatForever(autoreverses: true)) { on = true }
            }
    }
}

/// Unread count pill (blue, white digits), capped at 99+.
struct UnreadBadge: View {
    @Environment(\.theme) private var theme
    let count: Int

    var body: some View {
        Text(count > 99 ? "99+" : "\(count)")
            .font(theme.font(12, weight: .semibold))
            .foregroundStyle(.white)
            .padding(.horizontal, count > 9 ? 6 : 0)
            .frame(minWidth: 20, minHeight: 20)
            .background(Capsule().fill(theme.unread))
            .accessibilityLabel("\(count) unread")
    }
}
