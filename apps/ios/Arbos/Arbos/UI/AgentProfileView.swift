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
            AgentAvatar(project: project, theme: theme, size: 116, dot: machineDotColor(model.machine.state, theme: theme), working: model.isStreaming(project: project.id), idle: true)
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
                    .padding(.horizontal, 16).frame(height: 46)
                    .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(theme.field))
                    .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(theme.fieldBorder, lineWidth: 1))
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
/// place), then **Details** (Coding → Routines → Settings → Who may message → Profile behind an
/// Edit profile row), **Library** (images · files · voice · links) and **Computer** (Machine ·
/// Resources · Running now from `GET /machine/debug`). No Delete footer: the home row's
/// menu deletes, at once.
struct AgentProfileView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let projectId: Id
    /// For an agent that is not in my project list (someone else's agent in a DM).
    var fallback: Project? = nil
    @State private var tab = "Details"
    /// The Profile card's "Edit profile" row unfolds Name · Handle · Description.
    @State private var editOpen = false
    /// The hero's label (= the description) being edited in place.
    @State private var editingLabel = false
    @State private var labelDraft = ""
    @FocusState private var labelFocused: Bool
    @State private var showRename = false
    @State private var renameDraft = ""
    /// The avatar picker sheet (D92 amendment), from the hero's crayon; the rig's
    /// `ARBOS_SHOW=avatarpicker[-generate|-upload]` opens it on that tab.
    @State private var showAvatarPicker: AvatarPickerTab? = nil
    @State private var showModel = false
    @State private var showReasoning = false
    /// The call-voice picker (#1316's `VoicePickerView`) from the Settings card's Voice row.
    @State private var showVoice = false
    @State private var showMessageGrantPicker = false
    @State private var messageGrantError: String?

    private var project: Project? { model.project(projectId) ?? fallback }
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
            ScrollView {
                if let project {
                    VStack(spacing: 18) {
                        Color.clear.frame(height: 80)
                        ProfilePane(
                            hero: hero(project),
                            tabs: PaneKinds.agent,
                            selected: $tab,
                            body: sections(project)
                        )
                        Color.clear.frame(height: 40)
                    }
                } else {
                    Text("This agent is gone.").foregroundStyle(theme.muted).padding(.top, 120)
                }
            }
            .scrollDismissesKeyboard(.interactively)
            ProfileTopBar {
                if editingLabel {
                    Button("Done") { commitLabel() }
                        .font(theme.font(Theme.body, weight: .semibold))
                        .foregroundStyle(theme.text)
                }
            }
        }
        .task { await model.loadVoices() }
        .task(id: isMine) {
            guard isMine else { return }
            await model.refreshRoutines(projectId)
            await model.refreshBoxReport()
            await model.refreshShellGrants(projectId)
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
            // `ARBOS_SHOW=agentedit` unfolds Edit profile.
            if let t = DebugHooks.panelTab, PaneKinds.agent.contains(t) { tab = t }
            if isMine, DebugHooks.show == "agentvoice" {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { showVoice = true }
            }
            if isMine, DebugHooks.show == "agentlabel" {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { startLabelEdit() }
            }
            if isMine, DebugHooks.show == "agentedit" { editOpen = true }
        }
        .sheet(isPresented: $showReasoning) {
            if let project = model.project(projectId) { ReasoningPickerView(project: project).environmentObject(model).environment(\.theme, theme) }
        }
        .sheet(isPresented: $showVoice) {
            if let project = model.project(projectId) { VoicePickerView(project: project).environmentObject(model).environment(\.theme, theme) }
        }
        .sheet(isPresented: $showMessageGrantPicker) {
            if let project = model.project(projectId) {
                NavigationStack {
                    AddMessageGrantSheet(project: project).environmentObject(model).environment(\.theme, theme)
                }
            }
        }
        .alert("Couldn't change who may message", isPresented: Binding(get: { messageGrantError != nil }, set: { if !$0 { messageGrantError = nil } })) {
            Button("OK", role: .cancel) { }
        } message: { Text(messageGrantError ?? "") }
        .alert("Name", isPresented: $showRename) {
            TextField("Agent name", text: $renameDraft)
            Button("Save") { commitRename() }
            Button("Cancel", role: .cancel) { }
        } message: { Text("What people and their agents call it; the @handle follows the name.") }
    }

    // MARK: Hero — picture, name, the label

    private func hero(_ project: Project) -> PaneHero {
        let hasLabel = !project.description.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        let subtitle = AgentPane.subtitle(description: project.description, mine: isMine, ownerHandle: project.ownerHandle)
        return PaneHero(
            picture: .image(AnyView(heroPicture(project))),
            name: project.name,
            subtitle: subtitle,
            subtitleTint: hasLabel || !isMine ? nil : theme.placeholder,
            subtitleAction: isMine ? { startLabelEdit() } : nil,
            subtitleEditor: editingLabel ? AnyView(labelEditor) : nil
        )
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
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(theme.field))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(theme.fieldBorder, lineWidth: 1))
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
            AgentAvatar(project: project, theme: theme, size: PaneKit.heroAvatar, working: model.isStreaming(project: project.id), idle: true)
        }
    }

    // MARK: Tabs

    private func sections(_ project: Project) -> [PaneSection] {
        switch tab {
        case "Library": return librarySections(project)
        case "Computer": return computerSections(project)
        default: return detailsSections(project)
        }
    }

    // MARK: Details — live content first, the edit fields last

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
            case .access:
                out.append(.title("Who may message \(project.name)"))
                var grants: [AnyView] = project.messageGrants.map { g in AnyView(messageGrantRow(project, g)) }
                grants.append(AnyView(Button { showMessageGrantPicker = true } label: {
                    PaneRow(leading: .glyph("person.badge.plus"), title: "Give a person access…", titleTint: theme.accent)
                }.buttonStyle(.plain)))
                out.append(.card(grants, separatorInset: PaneKit.separatorInset(.glyph(""))))
            case .permissions:
                // Permissions · Shell approvals (#1544): the standing choices made on
                // destructive-shell cards, one switch each; off = revoked, the agent asks again.
                let grants = model.shellGrants[project.id] ?? []
                out.append(.title("Permissions"))
                out.append(.card([AnyView(
                    PaneRow(leading: .tile("terminal"), title: "Shell approvals", value: ShellGrant.summary(grants))
                        .accessibilityIdentifier("shell-approvals-row")
                )]))
                out.append(.footnote("\(project.name) asks before destructive shell commands outside /work/tmp. Standing answers from the cards land here."))
                if !grants.isEmpty {
                    out.append(.title("Shell approvals · \(project.name)"))
                    out.append(.card(grants.map { g in
                        AnyView(
                            PaneRow(
                                leading: .glyph("terminal"),
                                title: g.label,
                                subtitle: g.ruleClass + " · " + g.detail + (g.chatId == nil ? "" : " · this chat only"),
                                trailing: AnyView(
                                    Toggle("", isOn: Binding(get: { true }, set: { on in
                                        if !on { model.revokeShellGrant(projectId: project.id, grantId: g.id) }
                                    }))
                                    .labelsHidden().tint(theme.online)
                                    .accessibilityLabel("\(g.label) allowed")
                                    .accessibilityIdentifier("shell-grant-\(g.id)")
                                )
                            )
                        )
                    }, separatorInset: PaneKit.separatorInset(.glyph(""))))
                    out.append(.footnote("Turn one off and \(project.name) asks again next time. Every allowed run is in Activity."))
                }
            case .profile:
                out.append(.title("Profile"))
                var rows: [AnyView] = []
                if isMine {
                    rows.append(AnyView(Button {
                        withAnimation(.snappy(duration: 0.2)) { editOpen.toggle() }
                    } label: {
                        PaneRow(leading: .tile("pencil"), title: "Edit profile", trailing: AnyView(
                            Image(systemName: editOpen ? "chevron.up" : "chevron.down")
                                .font(theme.font(14, weight: .semibold))
                                .foregroundStyle(theme.placeholder)
                        ))
                    }.buttonStyle(.plain)))
                }
                if !isMine || editOpen { rows += profileRows(project) }
                out.append(.card(rows, separatorInset: isMine ? PaneKit.separatorInset(.tile("")) : PaneKit.inset))
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

    /// Name · Handle · Description — the edit fields, under the Edit profile row.
    private func profileRows(_ project: Project) -> [AnyView] {
        var rows: [AnyView] = []
        if isMine {
            rows.append(AnyView(Button {
                renameDraft = project.name
                showRename = true
            } label: { PaneRow(title: "Name", value: project.name, chevron: true) }.buttonStyle(.plain)))
        } else {
            rows.append(AnyView(PaneRow(title: "Name", value: project.name)))
        }
        rows.append(AnyView(PaneRow(title: "Handle", value: "@\(mentionSlug(project.name))")))
        let description = project.description.trimmingCharacters(in: .whitespacesAndNewlines)
        if isMine {
            rows.append(AnyView(Button { startLabelEdit() } label: {
                PaneRow(title: "Description", value: description.isEmpty ? "Add a description" : description, chevron: true)
            }.buttonStyle(.plain)))
        } else {
            rows.append(AnyView(PaneRow(title: "Description", value: description.isEmpty ? "—" : description)))
        }
        return rows
    }

    // MARK: Library — everything the agent's chats hold

    private func librarySections(_ project: Project) -> [PaneSection] {
        let media = items.filter { $0.kind == .media }
        let files = items.filter { $0.kind == .file }
        let voice = items.filter { $0.kind == .voice }
        let links = items.filter { $0.kind == .link }
        if media.isEmpty && files.isEmpty && voice.isEmpty && links.isEmpty {
            return [.empty("Nothing shared in \(project.name)'s chats yet.")]
        }
        var out: [PaneSection] = []
        if !media.isEmpty { out += [.title("Images · \(media.count)"), .grid(media)] }
        if !files.isEmpty { out += [.title("Files · \(files.count)"), .custom(AnyView(ChatItemsList(items: files, kind: .file, empty: "")))] }
        if !voice.isEmpty { out += [.title("Voice · \(voice.count)"), .custom(AnyView(ChatItemsList(items: voice, kind: .voice, empty: "")))] }
        if !links.isEmpty { out += [.title("Links · \(links.count)"), .custom(AnyView(ChatItemsList(items: links, kind: .link, empty: "")))] }
        return out
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
                if let load = r.load1m { rows.append(AnyView(PaneRow(title: "Load", value: String(format: "%.2f", load)))) }
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
            rows.append(AnyView(statusRow(status, dot: machineDotColor(m.state, theme: theme))))
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
            rows.append(AnyView(statusRow(shown.label, dot: machineDotColor(state, theme: theme))))
            if let klass = project.machineClass, !klass.isEmpty { rows.append(AnyView(PaneRow(title: "Machine", value: klass))) }
        }
        return rows
    }

    private func statusRow(_ status: String, dot: Color) -> some View {
        PaneRow(title: "Status", value: status, trailing: AnyView(Circle().fill(dot).frame(width: 9, height: 9)))
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

    // MARK: Who may message

    /// One grant row: avatar · name / @handle · last message · switch; a long press removes it
    /// (the switch only pauses it).
    private func messageGrantRow(_ project: Project, _ g: MessageGrant) -> some View {
        let subtitle: String = {
            var s = g.handle.map { "@\($0)" } ?? g.kind
            if g.kind == "agent" { s += " · agent" }
            if let at = g.lastAt { s += " · messaged \(SecretsText.ago(at))" } else if !g.note.isEmpty { s += " · \(g.note)" }
            return s
        }()
        let isOn = Binding<Bool>(
            get: { model.project(project.id)?.messageGrants.first(where: { $0.grantee == g.grantee })?.enabled ?? g.enabled },
            set: { on in Task { await setMessageGrant(project.id, grantee: g.grantee, enabled: on) } }
        )
        return PaneRow(
            leading: .avatar(AnyView(
                g.kind == "agent"
                    ? AnyView(InitialsAvatar(name: g.displayName, handle: g.handle ?? g.id, id: g.id, override: nil, size: PaneKit.avatar))
                    : AnyView(PersonAvatar(url: g.avatarUrl, handle: g.handle ?? g.id, size: PaneKit.avatar, name: g.name, id: g.id))
            )),
            title: g.displayName,
            subtitle: subtitle,
            trailing: AnyView(Toggle("", isOn: isOn).labelsHidden().tint(theme.online))
        )
        .contextMenu {
            Button(role: .destructive) {
                Task { await removeMessageGrant(project.id, grantee: g.grantee) }
            } label: { Label("Remove", systemImage: "trash") }
        }
    }

    private func setMessageGrant(_ pid: Id, grantee: String, enabled: Bool) async {
        if let i = model.projects.firstIndex(where: { $0.id == pid }),
           let j = model.projects[i].messageGrants.firstIndex(where: { $0.grantee == grantee }) {
            model.projects[i].messageGrants[j].enabled = enabled
        }
        do {
            let grants = try await model.relay.setMessageGrant(pid, grantee: grantee, enabled: enabled)
            if let i = model.projects.firstIndex(where: { $0.id == pid }) { model.projects[i].messageGrants = grants }
        } catch {
            messageGrantError = error.localizedDescription
            await model.refreshRegistry()
        }
    }

    private func removeMessageGrant(_ pid: Id, grantee: String) async {
        if let i = model.projects.firstIndex(where: { $0.id == pid }) {
            model.projects[i].messageGrants.removeAll { $0.grantee == grantee }
        }
        do {
            let grants = try await model.relay.removeMessageGrant(pid, grantee: grantee)
            if let i = model.projects.firstIndex(where: { $0.id == pid }) { model.projects[i].messageGrants = grants }
        } catch {
            messageGrantError = error.localizedDescription
            await model.refreshRegistry()
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

/// "Give a person access…": pick an Arbos user; their agents may then message this agent
/// (`PUT /projects/{id}/message-access`). The secrets' "Who can direct" picker, same shape.
struct AddMessageGrantSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let project: Project
    @State private var query = ""
    @State private var remote: [Person] = []
    @State private var busy = false
    @State private var error: String?

    private var already: Set<String> { Set(project.messageGrants.map(\.id)) }

    private var candidates: [Person] {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        var out = q.isEmpty
            ? model.directory
            : model.directory.filter { $0.handle.lowercased().contains(q) || ($0.name ?? "").lowercased().contains(q) }
        for p in remote where !out.contains(where: { $0.id == p.id }) { out.append(p) }
        return out.filter { $0.id != model.user?.id && !already.contains($0.id) }
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                Text("The person you add may have their agents message \(project.name). Their messages land in \(project.name)'s chat, and \(project.name) answers them. Your own agents can always message it.")
                    .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                TextField("", text: $query, prompt: Text("Search by name or @handle").foregroundStyle(theme.placeholder))
                    .font(theme.font(Theme.body))
                    .foregroundStyle(theme.text)
                    .padding(.horizontal, 14)
                    .frame(height: Theme.fieldHeight)
                    .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(theme.field))
                    .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(theme.fieldBorder, lineWidth: 1))
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                if candidates.isEmpty {
                    Text(query.isEmpty ? "Nobody else on Arbos yet." : "No one named that.")
                        .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                } else {
                    VStack(spacing: 0) {
                        ForEach(Array(candidates.prefix(12))) { p in
                            Button {
                                Haptics.pick()
                                Task { await grant(p) }
                            } label: { PersonRow(person: p) }
                            .buttonStyle(.plain)
                            .disabled(busy)
                        }
                    }
                }
                if let error {
                    Text(error).font(theme.font(Theme.tiny)).foregroundStyle(theme.danger)
                }
            }
            .padding(.horizontal, 24)
            .padding(.top, 16)
            .padding(.bottom, 32)
        }
        .scrollDismissesKeyboard(.interactively)
        .background(theme.bg)
        .navigationTitle("Who may message \(project.name)")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button("Cancel") { dismiss() }.foregroundStyle(theme.text)
            }
        }
        .task { await model.loadDirectory(force: false) }
        .task(id: query) {
            let q = query.trimmingCharacters(in: .whitespacesAndNewlines)
            guard q.count >= 2 else { remote = []; return }
            try? await Task.sleep(nanoseconds: 250_000_000)
            remote = await model.searchPeople(q)
        }
    }

    private func grant(_ p: Person) async {
        busy = true
        defer { busy = false }
        do {
            let grants = try await model.relay.setMessageGrant(project.id, grantee: "@\(p.handle)", enabled: true)
            if let i = model.projects.firstIndex(where: { $0.id == project.id }) { model.projects[i].messageGrants = grants }
            dismiss()
        } catch {
            self.error = error.localizedDescription
        }
    }
}
