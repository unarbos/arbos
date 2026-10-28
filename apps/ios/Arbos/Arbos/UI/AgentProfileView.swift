// Agent pages. `AgentProfileView` is the agent's pane on the shared profile pane
// (Details · Library · Computer, the Mac's `profiles/agent.rs`) opened from anywhere an
// agent shows. `AgentEditSheet` is the older editor the chat's `ARBOS_SHOW=edit` rig still
// opens: tap-to-rename, avatar, model, voice. Name/colour/avatar/description save with
// `PATCH /projects/{id}`.

import SwiftUI

struct AgentEditSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let projectId: Id
    @State private var name = ""
    @State private var editingName = false
    @State private var showModel = false
    @State private var showVoice = false
    @State private var showAvatarPicker = false
    @FocusState private var nameFocused: Bool

    private var project: Project? { model.project(projectId) }

    static let colorHexes = ["#1084FE", "#10B981", "#FF7A00", "#FF3B5C", "#FF2D9B", "#8B5CF6", "#8E5A3C", "#8E8E93", "#FFB000", "#14B8A6"]

    var body: some View {
        NavigationStack {
            ScrollView {
                if let project {
                    VStack(spacing: 0) {
                        hero(project)
                        card {
                            // D92 amendment: the avatar picker sheet (Bot | Generate | Upload).
                            Button { if !project.shared { showAvatarPicker = true } } label: {
                                row(icon: "photo.circle", title: "Avatar", value: project.imageKey != nil ? "Picture" : LeafArt.names[Int(LeafArt.pick(project).leaf)])
                            }
                            .buttonStyle(.plain)
                            .disabled(project.shared)
                        }
                        .padding(.horizontal, 16)
                        .padding(.top, 26)

                        card {
                            Button { showModel = true } label: {
                                row(icon: "cpu", title: "Model", value: "\(project.shortModel) · \(project.effort.label)")
                            }
                            .buttonStyle(.plain)
                            Button { showVoice = true } label: {
                                row(icon: "waveform", title: "Voice", value: voiceLabel(project))
                            }
                            .buttonStyle(.plain)
                        }
                        .padding(.horizontal, 16)
                        .padding(.top, 14)

                        card {
                            row(icon: "desktopcomputer", title: "Workspace", value: workspaceLabel(project), chevron: false)
                            if project.spendCapUsd > 0 {
                                row(icon: "dollarsign.circle", title: "Spend today", value: String(format: "$%.2f of $%.0f", project.spendTodayUsd, project.spendCapUsd), chevron: false)
                            }
                        }
                        .padding(.horizontal, 16)
                        .padding(.top, 14)

                        Text(project.shared
                             ? "A shared agent. Its owner pays for it and can remove you any time."
                             : "Mention @\(mentionSlug(project.name)) in a chat with a person to bring this agent in.")
                            .font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                            .multilineTextAlignment(.center)
                            .padding(.horizontal, 36).padding(.top, 18)
                        Color.clear.frame(height: 40)
                    }
                } else {
                    Text("This agent is gone.").foregroundStyle(theme.muted).padding(.top, 60)
                }
            }
            .scrollDismissesKeyboard(.interactively)
            .background(theme.bg)
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(theme.bg, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    if editingName {
                        Button("Save") { commitName() }.font(theme.font(Theme.body, weight: .semibold))
                    } else {
                        Button("Done") { dismiss() }
                    }
                }
            }
            .sheet(isPresented: $showModel) {
                if let project { ModelPickerView(project: project).environmentObject(model).environment(\.theme, theme) }
            }
            .sheet(isPresented: $showVoice) {
                if let project { VoicePickerView(project: project).environmentObject(model).environment(\.theme, theme) }
            }
            .sheet(isPresented: $showAvatarPicker) {
                AvatarPickerSheet(target: .agent(projectId)).environmentObject(model).environment(\.theme, theme)
            }
            .onAppear { name = project?.name ?? "" }
        }
        .tint(theme.text)
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        .presentationBackground(theme.bg)
        .presentationCornerRadius(28)
        .preferredColorScheme(theme.isDark ? .dark : .light)
    }

    // MARK: Hero

    private func hero(_ project: Project) -> some View {
        VStack(spacing: 12) {
            AgentAvatar(project: project, theme: theme, size: 116, mark: machineMark(model.machine.state), working: model.isStreaming(project: project.id), idle: true)
                .padding(.top, 18)
                .id("\(project.effectiveAvatarColorIndex.map(String.init) ?? "auto")")
                .animation(.snappy, value: project.color)
            if editingName {
                TextField("", text: $name, prompt: Text("Agent name").foregroundStyle(theme.placeholder))
                    .font(theme.font(24, weight: .semibold))
                    .foregroundStyle(theme.text)
                    .multilineTextAlignment(.center)
                    .autocorrectionDisabled()
                    .focused($nameFocused)
                    .submitLabel(.done)
                    .onSubmit(commitName)
                    .controlField()
                    .padding(.horizontal, 48)
            } else {
                Button {
                    guard !project.shared else { return }
                    name = project.name
                    editingName = true
                    nameFocused = true
                } label: {
                    HStack(spacing: 6) {
                        Text(project.name)
                            .font(theme.font(24, weight: .semibold))
                            .foregroundStyle(theme.text)
                            .lineLimit(1)
                        if !project.shared {
                            Image(systemName: "pencil").font(.system(size: 13, weight: .semibold)).foregroundStyle(theme.muted)
                        }
                    }
                }
                .buttonStyle(.plain)
            }
            Text("@\(mentionSlug(editingName ? name : project.name))")
                .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
        }
        .frame(maxWidth: .infinity)
    }

    private func commitName() {
        let clean = name.trimmingCharacters(in: .whitespaces)
        editingName = false
        nameFocused = false
        guard let project, !clean.isEmpty, clean != project.name else { return }
        model.updateProject(projectId, name: clean)
    }

    // MARK: Rows


    /// The hex the avatar really shows: the project's own colour, or — for the
    /// relay default — the palette entry `Theme.projectColor` hashes to
    /// (same FNV-1a over the id, same palette order).
    static func currentHex(_ project: Project) -> String {
        let hex = project.color.lowercased()
        if hex == "#8b7cf6" || hex.isEmpty {
            var h: UInt32 = 2166136261
            for b in project.id.utf8 { h = (h ^ UInt32(b)) &* 16777619 }
            return colorHexes[Int(h % UInt32(colorHexes.count))].lowercased()
        }
        return hex
    }

    /// "Default (Marin)" or the chosen voice's name (§10.3).
    private func voiceLabel(_ project: Project) -> String {
        let catalog = model.voiceCatalog
        if let id = project.voice {
            return catalog?.voices.first { $0.id == id }?.name ?? id.capitalized
        }
        if let d = catalog?.defaultId, let v = catalog?.voices.first(where: { $0.id == d }) { return "Default (\(v.name))" }
        return "Default"
    }

    private func row(icon: String, title: String, value: String, chevron: Bool = true) -> some View {
        PaneRow(leading: .tile(icon), title: title, value: value, chevron: chevron)
    }

    private func card<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        let rows = content()
        return PaneCard(separatorInset: PaneKit.separatorInset(.tile(""))) { rows }
    }

    private func workspaceLabel(_ project: Project) -> String {
        switch model.machine.workspaces[project.id] ?? project.workspace {
        case .active: return "Running"
        case .idle: return "Idle"
        case .error: return "Error"
        case .unknown: return model.machine.reported ? "Not set up yet" : "—"
        }
    }
}


// MARK: - The agent pane: Details · Library · Computer

/// The agent's pane on the shared profile pane (D93) — Grok Bot's content model, the same
/// as the Mac's `profiles/agent.rs` (#1679; iPhone [feedback iphone-agent-pane-tabs-2026-10-25]):
/// hero with the description as the label ("Add a label" for the owner, tap edits it in
/// place; a tap on the name renames), then **Details** (Settings — Coding and Routines ahead
/// of it while `AgentPane.agentDetailsShowCodingRoutines` is on), **Library** (images ·
/// files · voice · links) and **Computer** (Machine · Resources · Running now from
/// `GET /machine/debug`). No Delete footer: the home row's menu deletes, at once. "Who may
/// message", "Permissions" and "Profile" left Details on 2026-10-25 ([feedback
/// agent-details-remove-sections-ios-2026-10-25]).
struct AgentProfileView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let projectId: Id
    /// For an agent that is not in my project list (someone else's agent in a DM).
    var fallback: Project? = nil
    @State private var tab = "Details"
    /// The hero's label (= the description) being edited in place.
    @State private var editingLabel = false
    @State private var labelDraft = ""
    @FocusState private var labelFocused: Bool
    /// The rename alert, from a tap on the hero's name (the owner only).
    @State private var showRename = false
    @State private var renameDraft = ""
    /// The avatar picker sheet (D92 amendment), from the hero's crayon; the rig's
    /// `ARBOS_SHOW=avatarpicker[-generate|-upload]` opens it on that tab.
    @State private var showAvatarPicker: AvatarPickerTab? = nil
    @State private var showModel = false
    @State private var showReasoning = false
    /// The call-voice picker (#1316's `VoicePickerView`) from the Settings card's Voice row.
    @State private var showVoice = false
    /// Connect an agent (#2142): Rotate key… pushes the reveal screen with the fresh key.
    @State private var rotated: ConnectReveal?
    /// Wake URL form ([feedback outside-agents-wake-ios-2026-10-28]): the two drafts, what is in
    /// flight ("saving" | "sending"), the last outcome line, and which agent the drafts were filled for.
    @State private var wakeURLDraft = ""
    @State private var wakeAuthDraft = ""
    @State private var wakeBusy: String?
    @State private var wakeNote: (ok: Bool, text: String)?
    @State private var wakeFilledFor: Id?
    /// Session log card → View all ([feedback sidecar-sessions-ios-2026-10-28]).
    @State private var showLog = false

    private var project: Project? { model.project(projectId) ?? fallback }
    /// A connected (outside) agent: no Computer tab; Connection + Key under Details (#2142).
    private var outside: OutsideAgentInfo? { model.outsideInfo[projectId] }
    private var isMine: Bool { model.project(projectId) != nil && project?.shared != true }
    private var items: [ChatItem] {
        let msgs = model.chats(of: projectId).flatMap { model.messages(of: $0.id) }
        return ChatMedia.items(messages: msgs, slot: projectId, model: model)
    }
    /// Live refresh cadence while the pane shows live content (the registry poll's 15 s).
    private static let livePollSeconds: UInt64 = 15

    var body: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            ScrollViewReader { reader in
                ScrollView {
                    if let project {
                        VStack(spacing: 18) {
                            Color.clear.frame(height: PaneKit.pageTop)
                            ProfilePane(
                                hero: hero(project),
                                tabs: outside != nil ? PaneKinds.outsideAgent : PaneKinds.agent,
                                selected: $tab,
                                body: sections(project)
                            )
                            Color.clear.frame(height: 40).id("pane-end")
                        }
                    } else {
                        Text("This agent is gone.").foregroundStyle(theme.muted).padding(.top, 120)
                    }
                }
                .scrollDismissesKeyboard(.interactively)
                .onAppear {
                    // Rig: `ARBOS_PANE_SCROLL=<anchor>` scrolls to a `pane-<anchor>` id
                    // (`end` is the pane's foot).
                    guard let anchor = DebugHooks.paneScroll else { return }
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) {
                        withAnimation(nil) { reader.scrollTo(anchor == "end" ? "pane-end" : "pane-\(anchor)", anchor: .top) }
                    }
                }
            }
            ProfileTopBar(screen: .agentPage) {
                if editingLabel {
                    Button("Done") { commitLabel() }
                        .font(theme.font(Theme.body, weight: .semibold))
                        .foregroundStyle(theme.text)
                }
            }
        }
        .task { await model.loadVoices() }
        .task(id: projectId) {
            // Outside agents (#2142): the connection for the Details pane; a native agent leaves no entry.
            guard isMine else { return }
            await model.refreshOutsideInfo(projectId)
            if outside != nil, tab == "Computer" { tab = "Details" }
        }
        .task(id: isMine) {
            guard isMine else { return }
            await model.refreshRoutines(projectId)
            await model.refreshBoxReport()
        }
        .navigationDestination(isPresented: Binding(get: { rotated != nil }, set: { if !$0 { rotated = nil } })) {
            if let rotated {
                ConnectRevealView(reveal: rotated, onDone: { self.rotated = nil })
            }
        }
        .navigationDestination(isPresented: $showLog) {
            SessionLogView(projectId: projectId)
        }
        .task(id: "\(tab)-\(model.isStreaming(project: projectId))") {
            // The Coding row's elapsed time and the Computer tab's jobs move: poll while
            // something is running or the Computer tab is up (fetched again on each pick).
            guard isMine else { return }
            if tab == "Computer" { await model.refreshBoxReport() }
            while !Task.isCancelled, tab == "Computer" || model.isStreaming(project: projectId) {
                try? await Task.sleep(nanoseconds: Self.livePollSeconds * 1_000_000_000)
                guard !Task.isCancelled else { return }
                await model.refreshBoxReport()
            }
        }
        .onChange(of: labelFocused) { _, focused in
            if !focused, editingLabel { commitLabel() }
        }
        .sheet(isPresented: $showModel) {
            if let project = model.project(projectId) { ModelPickerView(project: project).environmentObject(model).environment(\.theme, theme) }
        }
        .sheet(item: $showAvatarPicker) { initial in
            AvatarPickerSheet(target: .agent(projectId), initialTab: initial).environmentObject(model).environment(\.theme, theme)
        }
        .onAppear {
            if isMine, let show = DebugHooks.show, show.hasPrefix("avatarpicker") {
                let tab: AvatarPickerTab = show.hasSuffix("generate") ? .generate : (show.hasSuffix("upload") ? .upload : .bot)
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { showAvatarPicker = tab }
            }
            // Rig: `ARBOS_PANEL_TAB=Computer|Library` lands on a tab; `ARBOS_SHOW=agentvoice`
            // also opens the Voice row's picker; `ARBOS_SHOW=agentlabel` edits the label;
            // `ARBOS_SHOW=agentrename` opens the rename alert.
            if let t = DebugHooks.panelTab, PaneKinds.agent.contains(t) { tab = t }
            if isMine, DebugHooks.show == "agentvoice" {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { showVoice = true }
            }
            // A search result for the agent's Model / Reasoning / Voice row
            // ([feedback search-settings-ios-2026-10-26]): the pane opens with that picker up.
            if isMine, let setting = model.pendingAgentSetting {
                model.pendingAgentSetting = nil
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) {
                    switch setting {
                    case .model: showModel = true
                    case .reasoning: showReasoning = true
                    case .voice: showVoice = true
                    }
                }
            }
            if isMine, DebugHooks.show == "agentlabel" {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { startLabelEdit() }
            }
            if isMine, DebugHooks.show == "agentrename" {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { startRename() }
            }
        }
        .sheet(isPresented: $showReasoning) {
            if let project = model.project(projectId) { ReasoningPickerView(project: project).environmentObject(model).environment(\.theme, theme) }
        }
        .sheet(isPresented: $showVoice) {
            if let project = model.project(projectId) { VoicePickerView(project: project).environmentObject(model).environment(\.theme, theme) }
        }
        .alert("Name", isPresented: $showRename) {
            TextField("Agent name", text: $renameDraft)
            Button("Save") { commitRename() }
            Button("Cancel", role: .cancel) { }
        } message: { Text("What people and their agents call it; the @handle follows the name.") }
    }

    // MARK: Hero — picture, name, the label

    private func hero(_ project: Project) -> PaneHero {
        let hasLabel = !project.description.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        let subtitle = AgentPane.subtitle(description: project.description, mine: isMine, ownerHandle: project.ownerHandle, ownerName: project.ownerName)
        // Someone else's agent: "Run by …" is a link to the owner's page (Jacob 2026-10-25).
        let owner = AgentPane.ownerLink(mine: isMine, ownerHandle: project.ownerHandle, ownerName: project.ownerName)
        // The owner's page is pushed over this one by the root sheet's NavigationStack
        // (`ProfileSheet`, `ProfileTarget.pushesPeople`); its top-bar chevron pops back here.
        let openOwner: (() -> Void)? = owner.map { link in { model.showProfile(.user(ownerPerson(project, handle: link.handle))) } }
        return PaneHero(
            picture: .image(AnyView(heroPicture(project))),
            name: project.name,
            nameAction: isMine ? { startRename() } : nil,
            subtitle: subtitle,
            subtitleTint: hasLabel || !isMine ? nil : theme.placeholder,
            subtitleAction: isMine ? { startLabelEdit() } : (hasLabel ? nil : openOwner),
            subtitleLink: !isMine && !hasLabel && owner != nil,
            subtitleEditor: editingLabel ? AnyView(labelEditor) : nil,
            subtitleBelow: (hasLabel && owner != nil) ? AnyView(
                Button(action: { openOwner?() }) { Text(owner!.label).lineLimit(1) }
                    .buttonStyle(ChipLinkStyle())
                    .accessibilityHint("Opens their profile")
            ) : nil
        )
    }

    /// The owner as the person page wants them: the relay's `owner` card when the project
    /// carries it, else the handle resolved through the chats we share.
    private func ownerPerson(_ project: Project, handle: String) -> ProfilePerson {
        ProfilePerson(
            id: project.ownerId ?? model.personId(handle: handle) ?? handle,
            handle: handle,
            name: project.ownerName,
            avatarUrl: project.ownerAvatarUrl
        )
    }

    /// The name is renamed through an alert (a second inline editor would stack on the
    /// label's); since the Profile card left, the hero's name is the one way in.
    private func startRename() {
        guard isMine, let project else { return }
        renameDraft = project.name
        showRename = true
    }

    /// The label field under the name while the label is being edited: Return or Done
    /// commits, leaving the field commits too (the Mac's `label_editor`).
    private var labelEditor: some View {
        TextField("", text: $labelDraft, prompt: Text(AgentPane.labelPlaceholder).foregroundStyle(theme.placeholder))
            .font(theme.font(Theme.small))
            .foregroundStyle(theme.text)
            .multilineTextAlignment(.center)
            .focused($labelFocused)
            .submitLabel(.done)
            .onSubmit(commitLabel)
            .onChange(of: labelDraft) { _, v in if v.count > 500 { labelDraft = String(v.prefix(500)) } }
            .padding(.horizontal, 12)
            .frame(height: 32)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(theme.field)) // control-size: exception inlineChips
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(theme.fieldBorder, lineWidth: 1)) // control-size: exception inlineChips
            .padding(.horizontal, 56)
    }

    private func startLabelEdit() {
        guard isMine, let project else { return }
        labelDraft = project.description
        editingLabel = true
        labelFocused = true
    }

    private func commitLabel() {
        guard editingLabel else { return }
        let clean = labelDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        editingLabel = false
        labelFocused = false
        guard let project, clean != project.description else { return }
        model.updateProject(projectId, description: clean)
    }

    private func commitRename() {
        let clean = renameDraft.trimmingCharacters(in: .whitespaces)
        guard let project, !clean.isEmpty, clean != project.name else { return }
        model.updateProject(projectId, name: clean)
    }

    /// The hero picture: the leaf / photo, with the crayon → avatar picker for my own agent.
    @ViewBuilder
    private func heroPicture(_ project: Project) -> some View {
        if isMine {
            Button { showAvatarPicker = .bot } label: {
                AgentAvatar(project: project, theme: theme, size: PaneKit.heroAvatar, working: model.isStreaming(project: project.id), idle: true)
                    .paneCrayon()
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Change avatar")
        } else {
            // Someone else's agent: its picture opens full-size in the media viewer (a leaf opens nothing).
            ViewablePicture(name: project.name, url: model.projectImageURL(project)?.absoluteString) {
                AgentAvatar(project: project, theme: theme, size: PaneKit.heroAvatar, working: model.isStreaming(project: project.id), idle: true)
            }
        }
    }

    // MARK: Tabs

    private func sections(_ project: Project) -> [PaneSection] {
        switch tab {
        case "Library": return librarySections(project)
        case "Computer" where outside == nil: return computerSections(project)
        default:
            if let outside { return outsideSections(project, outside) }
            return detailsSections(project)
        }
    }

    // MARK: Details for a connected agent — Connection · Key (#2142)

    private func outsideSections(_ project: Project, _ info: OutsideAgentInfo) -> [PaneSection] {
        var out: [PaneSection] = []
        out.append(.title("Connection"))
        let waiting = !info.connected && info.status == "never"
        let statusTrailing: AnyView? = waiting
            ? AnyView(ProgressView().controlSize(.small).tint(theme.accent))
            : nil
        out.append(.card([
            AnyView(PaneRow(title: "Status", value: info.statusLine(), trailing: statusTrailing)
                .accessibilityIdentifier("outside-status")),
            AnyView(PaneRow(title: "Transport", value: info.transportLabel)),
            AnyView(PaneRow(title: "Protocol", value: info.proto)),
        ]))
        // The Session log card (spec §13.3, approved 2026-10-28): under Connection; only on a
        // relay that keeps the log (#2224).
        if info.logSummary != nil {
            out.append(contentsOf: sessionLogSections(project, info))
        }
        // The Can card (spec §4.3, the approved mockup): one switch per scope.
        out.append(.title("Can"))
        out.append(.card(OutsideAgentInfo.scopeRows.map { row -> AnyView in
            let isOn = Binding<Bool>(
                get: { model.outsideInfo[project.id]?.scope(row.name) ?? info.scope(row.name) },
                set: { on in model.setOutsideScope(project.id, row.name, on: on) }
            )
            return AnyView(
                PaneToggleRow(title: row.title, subtitle: row.subtitle, isOn: isOn)
                    .accessibilityIdentifier("outside-scope-\(row.name)")
            )
        }))
        out.append(.footnote("Changes apply at once. The agent sees only what is on."))
        let revoked = info.status == "revoked"
        if info.hasWake, !revoked {
            out.append(contentsOf: wakeSections(project, info))
        }
        out.append(.title("Key"))
        out.append(.card([
            AnyView(
                Button {
                    Task { if let r = await model.rotateOutsideKey(project.id) { rotated = r } }
                } label: {
                    PaneRow(leading: .tile("key"), title: "Rotate key…", chevron: true)
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("outside-rotate")
            ),
            AnyView(
                PaneDangerRow(title: revoked ? "Access revoked" : "Revoke access") {
                    guard !revoked else { return }
                    Task { await model.revokeOutsideAgent(project.id) }
                }
                .disabled(revoked)
                .accessibilityIdentifier("outside-revoke")
            ),
        ]))
        out.append(.footnote("Revoke stops it at once. Its messages stay."))
        return out
    }

    // MARK: Details for a connected agent — Session log ([feedback sidecar-sessions-ios-2026-10-28])

    /// The card: the title with "Last reply 9.8 s · N wakes today", the newest four lines as
    /// two-line rows, View all, a footnote.
    private func sessionLogSections(_ project: Project, _ info: OutsideAgentInfo) -> [PaneSection] {
        let entries = model.outsideLogs[project.id] ?? []
        var out: [PaneSection] = []
        out.append(.custom(AnyView(
            HStack(alignment: .firstTextBaseline) {
                PaneSectionTitle("Session log")
                Spacer(minLength: 8)
                if let line = SessionLogView.headerLine(summary: info.logSummary, entries: entries) {
                    Text(line)
                        .font(theme.font(Theme.tiny))
                        .foregroundStyle(theme.muted)
                        .lineLimit(1)
                        .padding(.trailing, PaneKit.inset * 2)
                }
            }
        )))
        let shown = Array(entries.reversed().filter(\.shown).prefix(4))
        var rows: [AnyView] = shown.map { e in
            AnyView(SessionLogRow(entry: e, chatTitle: e.chat.map { model.outsideLogChatTitle($0, agent: project.id) } ?? ""))
        }
        if shown.isEmpty {
            rows.append(AnyView(PaneRow(title: "No wakes yet", subtitle: "Lines appear here when someone writes to the agent.")))
        }
        rows.append(AnyView(
            Button { showLog = true } label: {
                PaneRow(title: "View all", chevron: true)
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("outside-log-all")
        ))
        out.append(.card(rows))
        out.append(.footnote("Written by the sidecar on the agent's machine. Never message text or secrets."))
        return out
    }

    // MARK: Details for a connected agent — Wake URL ([feedback outside-agents-wake-ios-2026-10-28])

    /// What the line under the Wake URL buttons says: the in-flight word, the last Save / test
    /// outcome, else the relay's last delivery (the Mac's `connect_agent::wake_note`).
    static func wakeNoteLine(busy: String?, note: (ok: Bool, text: String)?, last: OutsideWakeLast?,
                             now: Int64 = Int64(Date().timeIntervalSince1970 * 1000)) -> (ok: Bool, text: String)? {
        if busy == "saving" { return (true, "Saving…") }
        if busy == "sending" { return (true, "Sending…") }
        if let note { return note }
        if let last { return (last.error == nil, "Last wake · \(last.line(now: now))") }
        return nil
    }

    /// **Wake URL** card: URL field, the Authorization value (secure, never shown back), Save and
    /// Send test wake, the outcome line — the braille spinner while something is in flight.
    private func wakeSections(_ project: Project, _ info: OutsideAgentInfo) -> [PaneSection] {
        let busy = wakeBusy != nil
        let canTest = info.wakeURL != nil && info.wakeAuthSet && !busy
        let line = Self.wakeNoteLine(busy: wakeBusy, note: wakeNote, last: info.wakeLast)
        let urlRow = VStack(alignment: .leading, spacing: 6) {
            Text("URL").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
            TextField("", text: $wakeURLDraft, prompt: Text("https://…").foregroundStyle(theme.placeholder))
                .font(theme.font(Theme.body))
                .foregroundStyle(theme.text)
                .keyboardType(.URL)
                .textContentType(.URL)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .controlField()
                .accessibilityIdentifier("wake-url-field")
        }
        .padding(.horizontal, PaneKit.inset).padding(.vertical, 12)
        .onAppear { fillWakeForm(project.id, info) }
        let authRow = VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("Authorization header value").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                Spacer()
                if info.wakeAuthSet {
                    Text("saved · paste again to replace").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                }
            }
            SecureField("", text: $wakeAuthDraft, prompt: Text("Paste the Authorization header value").foregroundStyle(theme.placeholder))
                .font(theme.font(Theme.body))
                .foregroundStyle(theme.text)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .controlField()
                .accessibilityIdentifier("wake-auth-field")
        }
        .padding(.horizontal, PaneKit.inset).padding(.vertical, 12)
        let buttonsRow = VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                PanePillButton(glyph: "checkmark", title: "Save") { saveWake(project.id) }
                    .disabled(busy)
                    .opacity(busy ? 0.5 : 1)
                    .accessibilityIdentifier("wake-save")
                PanePillButton(glyph: "antenna.radiowaves.left.and.right", title: "Send test wake") { testWake(project.id) }
                    .disabled(!canTest)
                    .opacity(canTest ? 1 : 0.5)
                    .accessibilityIdentifier("wake-test")
            }
            if let line {
                HStack(spacing: 6) {
                    if busy { WorkingSpinner() }
                    // A delivery readout ("Last wake · Failed · HTTP 401 · 3 min ago"), muted
                    // like the status rows — not an error note.
                    Text(line.text)
                        .font(theme.font(Theme.tiny))
                        .foregroundStyle(theme.muted)
                        .lineLimit(2)
                }
                .accessibilityIdentifier("wake-note")
            }
        }
        .padding(.horizontal, PaneKit.inset).padding(.vertical, 12)
        return [
            .title("Wake URL"),
            .card([AnyView(urlRow), AnyView(authRow), AnyView(buttonsRow)]),
            .footnote("When a message needs an answer, Arbos posts a small signed nudge here at once — no message text. The value is sent as the Authorization header, verbatim, and is never shown again. Keep a slow poll as the fallback."),
        ]
    }

    /// Fill the two fields the first time the pane shows this agent (the stored URL; the value
    /// field starts empty because the value is never shown back).
    private func fillWakeForm(_ projectId: Id, _ info: OutsideAgentInfo) {
        guard wakeFilledFor != projectId else { return }
        wakeFilledFor = projectId
        wakeURLDraft = info.wakeURL ?? ""
        wakeAuthDraft = ""
        wakeNote = nil
    }

    private func saveWake(_ projectId: Id) {
        guard wakeBusy == nil else { return }
        wakeBusy = "saving"; wakeNote = nil
        let url = wakeURLDraft, auth = wakeAuthDraft
        Task {
            let error = await model.saveOutsideWake(projectId, url: url, auth: auth)
            wakeAuthDraft = ""
            wakeBusy = nil
            wakeNote = error.map { (ok: false, text: $0) } ?? (ok: true, text: "Saved")
        }
    }

    private func testWake(_ projectId: Id) {
        guard wakeBusy == nil else { return }
        wakeBusy = "sending"; wakeNote = nil
        Task {
            let r = await model.testOutsideWake(projectId)
            wakeBusy = nil
            wakeNote = r
        }
    }

    // MARK: Details — Settings (Coding and Routines ahead of it while the flag is on)

    private func detailsSections(_ project: Project) -> [PaneSection] {
        var out: [PaneSection] = []
        for block in AgentPane.detailsOrder(mine: isMine) {
            switch block {
            case .coding:
                out.append(.title("Coding"))
                out.append(.card([AnyView(codingRow(project))], separatorInset: PaneKit.separatorInset(.avatar(AnyView(EmptyView())))))
            case .routines:
                out.append(.title("Routines"))
                let list = model.routines[project.id] ?? []
                let rows: [AnyView] = list.isEmpty
                    ? [AnyView(PaneRow(title: "No routines yet", subtitle: "Ask \(project.name) to schedule one", titleTint: theme.muted))]
                    : list.map { r in AnyView(routineRow(project, r)) }
                out.append(.card(rows))
            case .settings:
                out.append(.title("Settings"))
                out.append(.card(settingsRows(project)))
            }
        }
        return out
    }

    /// "Coding": the agent's workspace — a spinner while a turn runs, the status under the
    /// name; a tap opens the Computer tab.
    private func codingRow(_ project: Project) -> some View {
        let workspace = model.boxReport?.workspace(project.id)
        let streaming = model.isStreaming(project: project.id)
        let working = streaming || workspace?.running == true
        let status = AgentPane.codingStatus(streaming: streaming, workspace: workspace, machine: model.machine.state, alwaysOn: model.machine.alwaysOn)
        return Button { withAnimation(.snappy(duration: 0.2)) { tab = "Computer" } } label: {
            PaneRow(leading: .avatar(AnyView(activityLeading(working))), title: project.name, subtitle: status, chevron: true)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(project.name), \(status)")
    }

    /// The row's picture slot: the system spinner (Grok's 15 pt eight-spoke, accent) while
    /// working, the code glyph when idle.
    @ViewBuilder
    private func activityLeading(_ working: Bool) -> some View {
        if working {
            ProgressView().controlSize(.small).tint(theme.accent)
                .frame(width: PaneKit.avatar, height: PaneKit.avatar)
        } else {
            Image(systemName: "chevron.left.forwardslash.chevron.right")
                .font(theme.font(17, weight: .medium))
                .foregroundStyle(theme.muted)
                .frame(width: PaneKit.avatar, height: PaneKit.avatar)
        }
    }

    private func routineRow(_ project: Project, _ r: Routine) -> some View {
        let isOn = Binding<Bool>(
            get: { model.routines[project.id]?.first(where: { $0.id == r.id })?.enabled ?? r.enabled },
            set: { on in model.setRoutineEnabled(project.id, routineId: r.id, enabled: on) }
        )
        return PaneRow(title: r.name, subtitle: AgentPane.routineStatus(r, now: nowMs()), trailing: AnyView(
            Toggle("", isOn: isOn).labelsHidden().tint(theme.online)
        ))
    }

    /// Model · Reasoning · Voice (Model alone on someone else's agent).
    private func settingsRows(_ project: Project) -> [AnyView] {
        var rows: [AnyView] = [
            AnyView(
                Button { if isMine { showModel = true } } label: {
                    PaneRow(title: "Model", value: project.shortModel, chevron: isMine)
                }
                .buttonStyle(.plain)
                .disabled(!isMine)
            ),
        ]
        guard isMine else { return rows }
        rows.append(AnyView(
            Button { showReasoning = true } label: {
                PaneRow(title: "Reasoning", value: project.effort.label, chevron: true)
            }
            .buttonStyle(.plain)
        ))
        rows.append(AnyView(
            Button { showVoice = true } label: {
                PaneRow(title: "Voice", value: voiceLine(project), chevron: true)
            }
            .buttonStyle(.plain)
        ))
        return rows
    }

    // MARK: Library — everything the agent's chats hold

    private func librarySections(_ project: Project) -> [PaneSection] {
        // One list by day with the long-press preview — the Mac's `profiles/library.rs`
        // ([feedback library-list-ios-2026-10-25]).
        LibraryList.sections(items: items, empty: "Nothing shared in \(project.name)'s chats yet.")
    }

    // MARK: Computer — the agent's box

    private func computerSections(_ project: Project) -> [PaneSection] {
        let report = model.boxReport
        var out: [PaneSection] = []
        for block in AgentPane.computerOrder(mine: isMine, report: report) {
            switch block {
            case .machine:
                out.append(.title("Machine"))
                out.append(.card(machineRows(project, report)))
            case .resources:
                guard let r = report else { continue }
                out.append(.title("Resources"))
                var rows: [AnyView] = []
                if let disk = AgentPane.freeOf(available: r.diskAvailableMb, total: r.diskTotalMb) { rows.append(AnyView(PaneRow(title: "Disk", value: disk))) }
                if let mem = AgentPane.freeOf(available: r.memAvailableMb, total: r.memTotalMb) { rows.append(AnyView(PaneRow(title: "Memory", value: mem))) }
                // CPU share from the box's cgroup (agentd ≥ 0.6.277); the load average is the host's
                // inside Daytona (`loadIsHost`), drawn only when it is really the box's and no CPU exists.
                if let cpu = r.cpuPct {
                    rows.append(AnyView(PaneRow(title: "CPU", value: "\(Int(cpu.rounded())) %")))
                } else if let load = r.load1m, !r.loadIsHost {
                    rows.append(AnyView(PaneRow(title: "Load", value: String(format: "%.2f", load))))
                }
                if let up = r.uptimeS { rows.append(AnyView(PaneRow(title: "Up for", value: AgentPane.durationS(up)))) }
                out.append(.card(rows))
            case .running:
                out.append(.title("Running now"))
                out.append(.card(runningRows(report), separatorInset: PaneKit.separatorInset(.avatar(AnyView(EmptyView())))))
            }
        }
        return out
    }

    /// Status (dot + "Awake · 5 h 12 m"), where it runs, spec, image, spend today — `GET /machine`
    /// plus the report; someone else's agent: status and class from its project.
    private func machineRows(_ project: Project, _ report: BoxReport?) -> [AnyView] {
        var rows: [AnyView] = []
        if isMine {
            let m = model.machine
            var status = m.reported ? m.label : "Checking…"
            if m.state == .awake, let s = report?.awakeForS { status += " · \(AgentPane.durationS(s))" }
            rows.append(AnyView(statusRow(status, mark: machineMark(m.state))))
            rows.append(AnyView(PaneRow(title: "Where it runs", value: m.whereItRuns)))
            // The spec line names the provider first; "Where it runs" already did.
            var spec = m.spec
            let provider = m.provider.prefix(1).uppercased() + m.provider.dropFirst() + " · "
            if !m.provider.isEmpty, spec.hasPrefix(provider) { spec.removeFirst(provider.count) }
            if !spec.isEmpty { rows.append(AnyView(PaneRow(title: "Machine", value: spec))) }
            if let image = report?.image ?? m.image { rows.append(AnyView(PaneRow(title: "Image", value: image))) }
            if m.spendCapUsd > 0 || m.spendTodayUsd > 0 {
                let spend = m.spendCapUsd > 0 ? String(format: "$%.2f of $%.0f", m.spendTodayUsd, m.spendCapUsd) : String(format: "$%.2f", m.spendTodayUsd)
                rows.append(AnyView(PaneRow(title: "Spend today", value: spend)))
            }
        } else {
            let raw = project.machineState ?? ""
            let state: BoxState = project.machineAlwaysOn && raw != "error" ? .awake : (BoxState.fromWire(raw) ?? project.state)
            let shown = Machine(state: state, since: nil, spec: "")
            rows.append(AnyView(statusRow(shown.label, mark: machineMark(state))))
            if let klass = project.machineClass, !klass.isEmpty { rows.append(AnyView(PaneRow(title: "Machine", value: klass))) }
        }
        return rows
    }

    /// The Status row's trailing mark (Jacob, 2026-10-27: never a coloured dot): the
    /// spinner while the machine wakes, the fault glyph on error, nothing at rest.
    private func statusRow(_ status: String, mark: StatusMark?) -> some View {
        PaneRow(title: "Status", value: status, trailing: mark.map { AnyView(StatusMarkView(mark: $0, size: 10)) })
    }

    /// Every workspace with a turn running (spinner, name, elapsed, tool calls), or why
    /// there is nothing to show.
    private func runningRows(_ report: BoxReport?) -> [AnyView] {
        guard let r = report else {
            return [AnyView(PaneRow(title: "Checking the machine…", titleTint: theme.muted))]
        }
        guard r.reachable else {
            return [AnyView(PaneRow(title: "The machine did not answer", subtitle: r.unreachableReason, titleTint: theme.muted))]
        }
        let running = r.running
        guard !running.isEmpty else {
            let n = r.workspaces.count
            return [AnyView(PaneRow(title: "Nothing running", subtitle: "\(n) workspace\(n == 1 ? "" : "s") on this machine", titleTint: theme.muted))]
        }
        return running.map { w in
            let detail = AgentPane.runningDetail(w)
            return AnyView(PaneRow(leading: .avatar(AnyView(activityLeading(true))), title: w.name, subtitle: detail.isEmpty ? nil : detail))
        }
    }

    /// The Voice row's value — the chosen voice's name, or "Default" (the Mac's
    /// `voicepicker::current_label`); the raw id until the catalog arrives.
    private func voiceLine(_ p: Project) -> String {
        guard let id = p.voice else { return "Default" }
        return model.voiceCatalog?.voices.first { $0.id == id }?.name ?? id.capitalized
    }
}

// MARK: - Reasoning picker (the Mac's Reasoning dropdown)

/// The agent's reasoning effort: one row per level, checkmark on the current
/// one; a tap saves (the same write as the Model sheet's Reasoning control).
struct ReasoningPickerView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let project: Project
    private var current: Project { model.project(project.id) ?? project }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    ForEach(Effort.allCases) { e in
                        Button {
                            model.setModel(projectId: project.id, model: current.model, effort: e)
                        } label: {
                            HStack {
                                Text(e.label).foregroundStyle(theme.text)
                                Spacer()
                                if current.effort == e {
                                    Image(systemName: "checkmark").font(.system(size: 15, weight: .semibold)).foregroundStyle(theme.text)
                                }
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .listRowBackground(theme.card)
                    }
                } footer: {
                    Text("How hard \(current.name) thinks before it answers.")
                        .font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                }
            }
            .scrollContentBackground(.hidden)
            .background(theme.bg)
            .navigationTitle("Reasoning")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() } }
            }
        }
        .presentationDetents([.medium])
    }
}


// MARK: - Session log ([feedback sidecar-sessions-ios-2026-10-28])

/// One log line: the time on the left, then the chat (or the session event) over the muted
/// latency / detail. Only an error's sentence is red — no dots, no badges (2026-10-27 rule).
struct SessionLogRow: View {
    @Environment(\.theme) private var theme
    let entry: OutsideLogEntry
    let chatTitle: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: PaneKit.gap) {
            Text(entry.clock)
                .font(theme.font(Theme.small))
                .foregroundStyle(theme.muted)
                .monospacedDigit()
                .frame(width: 66, alignment: .leading)
            VStack(alignment: .leading, spacing: 2) {
                Text(entry.isSession ? entry.sessionTitle : chatTitle)
                    .font(theme.font(Theme.body))
                    .italic(entry.isSession)
                    .foregroundStyle(entry.isSession ? theme.muted : theme.text)
                    .lineLimit(1)
                let detail = entry.detail
                if !detail.isEmpty {
                    Text(detail)
                        .font(theme.font(Theme.small))
                        .italic(entry.isSession)
                        .foregroundStyle(entry.isError ? theme.danger : theme.muted)
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, PaneKit.inset)
        .padding(.vertical, 6)
        .frame(minHeight: PaneKit.rowHeight)
        .accessibilityElement(children: .combine)
    }
}

/// The View all page: every line, newest first, grouped by day, with the pane's tab pill as
/// the filter (All · Replies · Errors · Sessions) — the same labels as the Mac page.
struct SessionLogView: View {
    @Environment(\.theme) private var theme
    @EnvironmentObject private var model: AppModel
    let projectId: Id
    @State private var filter = "All"

    static let filters = ["All", "Replies", "Errors", "Sessions"]

    /// "Last reply 9.8 s · 12 wakes today" for the card's header.
    static func headerLine(summary: OutsideLogSummary?, entries: [OutsideLogEntry]) -> String? {
        var parts: [String] = []
        if let ms = summary?.lastReplyMs { parts.append(String(format: "Last reply %.1f s", Double(ms) / 1000)) }
        let today = entries.filter { $0.kind == "wake" && $0.day == "Today" }.count
        if today > 0 { parts.append("\(today) wake\(today == 1 ? "" : "s") today") }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    /// Which lines a filter keeps.
    static func keeps(_ e: OutsideLogEntry, filter: String) -> Bool {
        switch filter {
        case "Replies": return e.kind == "reply" || e.kind == "post"
        case "Errors": return e.isError
        case "Sessions": return e.isSession
        default: return e.shown
        }
    }

    private var sections: [PaneSection] {
        let entries = (model.outsideLogs[projectId] ?? []).reversed().filter { Self.keeps($0, filter: filter) }
        var out: [PaneSection] = []
        var day = ""
        var rows: [AnyView] = []
        for e in entries {
            if e.day != day {
                if !rows.isEmpty { out.append(.card(rows)); rows = [] }
                out.append(.title(e.day))
                day = e.day
            }
            rows.append(AnyView(SessionLogRow(entry: e, chatTitle: e.chat.map { model.outsideLogChatTitle($0, agent: projectId) } ?? "")))
        }
        if !rows.isEmpty { out.append(.card(rows)) }
        if out.isEmpty {
            switch filter {
            case "Replies": out.append(.empty("No replies yet"))
            case "Errors": out.append(.empty("No errors"))
            case "Sessions": out.append(.empty("No session events yet"))
            default: out.append(.empty("No wakes yet"))
            }
        }
        out.append(.footnote("Written by the sidecar on the agent's machine. Never message text or secrets."))
        return out
    }

    var body: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            ScrollView {
                VStack(spacing: 18) {
                    Color.clear.frame(height: PaneKit.pageTop)
                    ProfilePane(
                        hero: PaneHero(picture: .glyph("list.bullet.rectangle"), name: "Session log",
                                       subtitle: model.project(projectId)?.name ?? ""),
                        tabs: Self.filters,
                        selected: $filter,
                        body: sections
                    )
                    Color.clear.frame(height: 40)
                }
            }
            ProfileTopBar(screen: .agentPage) { EmptyView() }
        }
        .toolbar(.hidden, for: .navigationBar)
        .task(id: projectId) { await model.refreshOutsideLog(projectId) }
    }
}
