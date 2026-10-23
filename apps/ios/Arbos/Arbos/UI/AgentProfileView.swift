// Agent pages. `AgentProfileView` is the Telegram-style profile (avatar,
// name, status, action tiles, info card, Media/Files/Links) opened from
// anywhere an agent shows; its Edit button opens `AgentEditSheet` — the
// original editor: tap-to-rename, colour, character picker, model, chats,
// sharing. Name/colour/avatar/description save with `PATCH /projects/{id}`.

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


// MARK: - Telegram-style agent profile

struct AgentProfileView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let projectId: Id
    /// For an agent that is not in my project list (someone else's agent in a DM).
    var fallback: Project? = nil
    /// Jacob's rule (2026-10-22): one tab set per entity, identical on Mac
    /// and iPhone — Profile | Settings | Library (`PaneKinds.agent`, the Mac's
    /// `agent.rs`). Kinds share the structure, not a tab count (22:32 UTC).
    @State private var tab = "Profile"
    @State private var showEdit = false
    /// The avatar picker sheet (D92 amendment), from the hero's crayon; the rig's
    /// `ARBOS_SHOW=avatarpicker[-generate|-upload]` opens it on that tab.
    @State private var showAvatarPicker: AvatarPickerTab? = nil
    @State private var showModel = false
    @State private var showDescription = false
    @State private var descriptionDraft = ""
    @State private var showReasoning = false
    @State private var confirmDelete = false
    @State private var showMessageGrantPicker = false
    @State private var messageGrantError: String?

    private var project: Project? { model.project(projectId) ?? fallback }
    private var isMine: Bool { model.project(projectId) != nil && project?.shared != true }
    private var items: [ChatItem] {
        let msgs = model.chats(of: projectId).flatMap { model.messages(of: $0.id) }
        return ChatMedia.items(messages: msgs, slot: projectId, model: model)
    }

    var body: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            ScrollView {
                if let project {
                    VStack(spacing: 18) {
                        Color.clear.frame(height: 80)
                        // The shared pane (D93): leaf / photo with the owner's crayon,
                        // name, "working" or who runs it; Profile | Settings | Library
                        // (the Mac's set); Delete agent for the owner.
                        ProfilePane(
                            hero: PaneHero(picture: .image(AnyView(heroPicture(project))), name: project.name, subtitle: subtitleLine(project)),
                            tabs: PaneKinds.agent,
                            selected: $tab,
                            body: sections(project),
                            footer: isMine ? [.card([AnyView(ProfileLinkRow(title: "Delete agent", destructive: true) { confirmDelete = true })])] : []
                        )
                        Color.clear.frame(height: 40)
                    }
                } else {
                    Text("This agent is gone.").foregroundStyle(theme.muted).padding(.top, 120)
                }
            }
            ProfileTopBar { EmptyView() }
        }
        .confirmationDialog("Delete \(project?.name ?? "this agent")?", isPresented: $confirmDelete, titleVisibility: .visible) {
            Button("Delete", role: .destructive) {
                model.deleteProject(projectId)
                dismiss()
            }
            Button("Cancel", role: .cancel) { }
        } message: { Text("Removes this agent and its chats from your list.") }
        .task { await model.loadVoices() }
        .sheet(isPresented: $showModel) {
            if let project = model.project(projectId) { ModelPickerView(project: project).environmentObject(model).environment(\.theme, theme) }
        }
        .sheet(isPresented: $showEdit) {
            AgentEditSheet(projectId: projectId).environmentObject(model).environment(\.theme, theme)
        }
        .sheet(item: $showAvatarPicker) { initial in
            AvatarPickerSheet(target: .agent(projectId), initialTab: initial).environmentObject(model).environment(\.theme, theme)
        }
        .onAppear {
            if isMine, let show = DebugHooks.show, show.hasPrefix("avatarpicker") {
                let tab: AvatarPickerTab = show.hasSuffix("generate") ? .generate : (show.hasSuffix("upload") ? .upload : .bot)
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { showAvatarPicker = tab }
            }
        }
        .sheet(isPresented: $showReasoning) {
            if let project = model.project(projectId) { ReasoningPickerView(project: project).environmentObject(model).environment(\.theme, theme) }
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
        .alert("Description", isPresented: $showDescription) {
            TextField("One paragraph about this agent", text: $descriptionDraft)
            Button("Save") { model.updateProject(projectId, description: descriptionDraft.trimmingCharacters(in: .whitespacesAndNewlines)) }
            Button("Cancel", role: .cancel) { }
        } message: { Text("Shown to everyone who can see this agent (up to 500 characters).") }
    }

    /// One grant row: avatar · name / @handle · last message · switch; swipe-free, a long
    /// press removes it (the switch only pauses it).
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

    /// The hero picture: the leaf / photo, with the crayon → edit sheet for my own agent.
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

    /// "working" while a turn runs, else who runs the agent (the Mac's subtitle).
    private func subtitleLine(_ p: Project) -> String {
        if model.isStreaming(project: p.id) { return "working" }
        if isMine { return "Agent · run by you" }
        if let o = p.ownerHandle, !o.isEmpty { return "Agent · run by @\(o)" }
        return "Agent"
    }

    /// The selected tab's sections. Voice, Share and "Add to this chat" left the
    /// pane on 2026-10-22 (Jacob: simple, consistent, powerful).
    private func sections(_ project: Project) -> [PaneSection] {
        switch tab {
        case "Settings":
            // Identical to the Mac: Model · Reasoning. Per-agent secrets are reached
            // like the Mac's — Settings → Secrets → the secret's Agents; camera vision
            // is the call screen's "Let <agent> see" switch.
            let rows: [AnyView] = [
                AnyView(
                    Button { if isMine { showModel = true } } label: {
                        PaneRow(title: "Model", value: project.shortModel, chevron: isMine)
                    }
                    .buttonStyle(.plain)
                    .disabled(!isMine)
                ),
                AnyView(
                    Button { if isMine { showReasoning = true } } label: {
                        PaneRow(title: "Reasoning", value: project.effort.label, chevron: isMine)
                    }
                    .buttonStyle(.plain)
                    .disabled(!isMine)
                ),
            ]
            guard isMine else { return [.card(rows)] }
            // Agent-to-agent messaging: who else's agents may write into this agent's chat.
            // My own agents always may; this card lists other people (the Mac's card, D83's
            // People shape: avatar · name/@handle · last message · switch).
            var grants: [AnyView] = project.messageGrants.map { g in AnyView(messageGrantRow(project, g)) }
            grants.append(AnyView(Button { showMessageGrantPicker = true } label: {
                PaneRow(leading: .glyph("person.badge.plus"), title: "Give a person access…", titleTint: theme.accent)
            }.buttonStyle(.plain)))
            return [
                .card(rows),
                .title("Who may message \(project.name)"),
                .card(grants, separatorInset: PaneKit.separatorInset(.glyph(""))),
                .footnote("Your own agents can always message \(project.name). People listed here may have their agents message it too; their messages land in this chat and \(project.name) answers them."),
            ]
        case "Library":
            // Everything the agent's chats hold — images, files, voice memos, links
            // — in sections (the group pane's Library, plus links).
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
        default:
            // Profile: name, handle, description (the owner edits by tapping).
            var rows: [AnyView] = [
                AnyView(PaneRow(title: "Name", value: project.name)),
                AnyView(PaneRow(title: "Handle", value: "@\(mentionSlug(project.name))")),
            ]
            if project.description.isEmpty {
                rows.append(isMine
                    ? AnyView(ProfileLinkRow(title: "Add a description") { descriptionDraft = ""; showDescription = true })
                    : AnyView(PaneRow(title: "Description", value: "—")))
            } else {
                rows.append(AnyView(
                    Button { if isMine { descriptionDraft = project.description; showDescription = true } } label: {
                        PaneRow(title: "Description", value: project.description, chevron: isMine)
                    }
                    .buttonStyle(.plain)
                    .disabled(!isMine)
                ))
            }
            return [
                .card(rows),
                .footnote(isMine
                    ? "The name, handle, picture and description are what people and their agents see, here and on the Mac."
                    : "Someone else's agent: what it shares with you shows in your chats."),
            ]
        }
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
