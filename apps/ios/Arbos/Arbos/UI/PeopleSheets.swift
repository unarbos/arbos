// People-chat sheets (D72): New Message (Telegram-style: search, New agent,
// New group, your agents, every Arbos user), the New group / Add members
// picker (chips, search, checkmarks) and project members (invite by handle).

import ArbosFFI
import SwiftUI

/// One row geometry for every card row in these sheets — the New agent / New group /
/// New server rows and the people, agent and server rows under them — so the
/// glyphs, pictures and text share one left edge and the sheet's first paint is
/// its final layout.
enum SheetRow {
    /// The card's inset from the sheet edge.
    static let cardInset: CGFloat = 16
    /// Row content inset inside the card.
    static let paddingX: CGFloat = 14
    /// Glyph circle / picture diameter.
    static let avatar: CGFloat = 44
    /// Picture → text gap.
    static let gap: CGFloat = PaneKit.gap
    /// Where the text column starts inside the card; the row divider starts there too.
    static var textInset: CGFloat { paddingX + avatar + gap }
    /// The text column's distance from the sheet edge.
    static var textLeading: CGFloat { cardInset + textInset }
}

/// "+" on home — Telegram's "New Message" sheet in Grok's dark skin: rises
/// from the bottom, search on top, then New agent / New group, then your
/// agents, then every other Arbos user. Typing filters agents, servers and the
/// directory and also asks `GET /users/search` and `GET /servers/search`, so a
/// GitHub handle not on Arbos yet can be invited and an open server can be joined
/// without an invite link.
struct NewChatSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    @State private var query = ""
    @State private var remote: [Person] = []
    @State private var remoteServers: [Server] = []
    @State private var searching = false
    @State private var opening = false
    @State private var loadingDirectory = false
    /// `ARBOS_SHOW=newagent|newgroup|newserver` lands on that Create screen, `newgroup2` on the
    /// member picker (screenshot rig).
    @State private var showCreate: CreateKind? = {
        switch DebugHooks.show {
        case "newagent": return .agent
        case "newgroup": return .group
        case "newserver": return .server
        default: return nil
        }
    }()
    @State private var showGroupPicker = DebugHooks.show == "newgroup2"
    @FocusState private var focused: Bool

    private var trimmed: String {
        query.trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "@"))
    }

    /// Own + shared agents (home list order by name) through the shared matcher
    /// (name, @slug, description), best first. Empty query hides archived; a
    /// search finds them again, same as the home list.
    private var agents: [Project] {
        let base = model.projects.filter { trimmed.isEmpty ? !model.isArchived($0.id) : true }
            .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
        return PeoplePicker.agents(base, query: trimmed)
    }

    /// Servers I am in that match the query (shared matcher, best first), then
    /// open servers from search I am not in.
    private var servers: [Server] {
        guard !trimmed.isEmpty, model.serversAvailable else { return [] }
        let mine = PeoplePicker.servers(
            model.servers.sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending },
            query: trimmed)
        var seen = Set(mine.map(\.id))
        var out = mine
        for s in remoteServers where !seen.contains(s.id) {
            seen.insert(s.id)
            out.append(s)
        }
        return out
    }

    /// Directory + remote hits through the shared matcher (display name and
    /// @handle, best first), one row per person, never me.
    private var people: [Person] {
        PeoplePicker.people(directory: model.directory, remote: remote, me: model.user?.id, query: trimmed)
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    searchField
                        .padding(.horizontal, SheetSearchField.inset)
                        .padding(.top, 6)
                        .padding(.bottom, 14)

                    if trimmed.isEmpty {
                        card {
                            NavigationLink { CreateEntityView(kind: .agent, onDone: { dismiss() }) } label: {
                                actionRow(icon: "sparkles", title: "New agent", subtitle: "Its own workspace on your machine")
                            }
                            .buttonStyle(.plain)
                            rowDivider
                            NavigationLink { CreateEntityView(kind: .group, onDone: { dismiss() }) } label: {
                                actionRow(icon: "person.2.fill", title: "New group", subtitle: "People and agents in one chat")
                            }
                            .buttonStyle(.plain)
                            if model.serversAvailable {
                                // D88 rule 8: a server of your own, or one you were invited to.
                                rowDivider
                                NavigationLink { CreateEntityView(kind: .server, onDone: { dismiss() }) } label: {
                                    actionRow(icon: "number", title: "New server", subtitle: "Channels for a group of people")
                                }
                                .buttonStyle(.plain)
                                // "Join a server" (paste a link) is gone (Jacob 2026-10-22): invite
                                // links open the app through arbos.life/s/<code>; people are added
                                // by name from the server's Members tab.
                            }
                        }
                        .padding(.horizontal, SheetRow.cardInset)
                        .padding(.bottom, 22)
                    }

                    if !agents.isEmpty {
                        sectionHeader(trimmed.isEmpty ? "Your agents" : "Agents")
                        card {
                            ForEach(Array(agents.enumerated()), id: \.element.id) { i, p in
                                Button { openAgent(p) } label: { AgentPickRow(project: p) }
                                    .buttonStyle(.plain)
                                if i < agents.count - 1 { rowDivider }
                            }
                        }
                        .padding(.horizontal, SheetRow.cardInset)
                        .padding(.bottom, 22)
                    }

                    if !servers.isEmpty {
                        sectionHeader("Servers")
                        card {
                            ForEach(Array(servers.enumerated()), id: \.element.id) { i, s in
                                Button { openServer(s) } label: { ServerPickRow(server: s) }
                                    .buttonStyle(.plain)
                                if i < servers.count - 1 { rowDivider }
                            }
                        }
                        .padding(.horizontal, SheetRow.cardInset)
                        .padding(.bottom, 22)
                    }

                    // Keep the people block when the directory is empty (invite copy),
                    // but skip a redundant "no match" under Agents when a search hit only agents.
                    if !people.isEmpty || (agents.isEmpty && servers.isEmpty) || trimmed.isEmpty {
                        sectionHeader(trimmed.isEmpty ? "Arbos users" : "People")
                        if people.isEmpty {
                            peopleEmpty
                        } else {
                            card {
                                ForEach(Array(people.enumerated()), id: \.element.id) { i, p in
                                    Button { open(p) } label: { PersonRow(person: p) }
                                        .buttonStyle(.plain)
                                    if i < people.count - 1 { rowDivider }
                                }
                            }
                            .padding(.horizontal, SheetRow.cardInset)
                        }
                    }
                    Color.clear.frame(height: 40)
                }
            }
            .scrollIndicators(.visible)
            .scrollDismissesKeyboard(.interactively)
            .background(theme.bg)
            .navigationTitle("New Message")
            .navigationBarTitleDisplayMode(.inline)
            .navigationDestination(isPresented: Binding(get: { showCreate != nil }, set: { if !$0 { showCreate = nil } })) {
                CreateEntityView(kind: showCreate ?? .agent, onDone: { dismiss() })
            }
            .navigationDestination(isPresented: $showGroupPicker) {
                NewGroupView(onDone: { dismiss() }, draft: GroupDraft(title: "Launch week", leaf: LeafArt.pick(seed: "rig")))
            }
            .toolbar {
                ToolbarItem(placement: .topBarLeading) { Button("Cancel") { dismiss() }.foregroundStyle(theme.text) }
            }
            .toolbarBackground(theme.bg, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .task {
                loadingDirectory = model.directory.isEmpty
                await model.loadDirectory()
                loadingDirectory = false
            }
            .onChange(of: query) { _, _ in search() }
        }
        .tint(theme.text)
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        .presentationBackground(theme.bg)
        .presentationCornerRadius(28)
        .preferredColorScheme(theme.isDark ? .dark : .light)
    }

    // MARK: Pieces

    private var searchField: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").font(.system(size: SheetSearchField.iconSize, weight: .medium)).foregroundStyle(theme.muted)
            TextField("", text: $query, prompt: Text("Search people, agents or servers").foregroundStyle(theme.placeholder))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .font(theme.font(Theme.body))
                .foregroundStyle(theme.text)
                .focused($focused)
                .submitLabel(.go)
                .onSubmit {
                    if let first = agents.first { openAgent(first) }
                    else if let first = servers.first { openServer(first) }
                    else if let first = people.first { open(first) }
                    else if !trimmed.isEmpty { openHandle(trimmed) }
                }
            if searching {
                ProgressView().controlSize(.small).tint(theme.muted)
            } else if !query.isEmpty {
                Button { query = ""; remote = []; remoteServers = [] } label: {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(theme.muted)
                }
                .buttonStyle(.plain)
            }
        }
        .sheetSearchFieldChrome(theme)
    }

    private func sectionHeader(_ text: String) -> some View {
        Text(text.uppercased())
            .font(theme.font(12, weight: .medium))
            .kerning(0.4)
            .foregroundStyle(theme.muted)
            .padding(.horizontal, 32)
            .padding(.bottom, 8)
    }

    private func card<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        VStack(spacing: 0) { content() }
            .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(theme.card))
    }

    private var rowDivider: some View {
        Rectangle().fill(theme.separator).frame(height: 0.5).padding(.leading, SheetRow.textInset)
    }

    private func actionRow(icon: String, title: String, subtitle: String) -> some View {
        HStack(spacing: SheetRow.gap) {
            GlassCircle(size: SheetRow.avatar) { // control-size: exception rowIconTiles
                Image(systemName: icon).font(.system(size: 17, weight: .semibold)).foregroundStyle(theme.text)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(theme.font(Theme.body, weight: .medium)).foregroundStyle(theme.text)
                Text(subtitle).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
            }
            Spacer()
            Image(systemName: "chevron.right").font(.system(size: 13, weight: .semibold)).foregroundStyle(theme.muted.opacity(0.7))
        }
        .padding(.horizontal, SheetRow.paddingX)
        .padding(.vertical, 10)
        .contentShape(Rectangle())
    }

    private var peopleEmpty: some View {
        Group {
            if loadingDirectory || searching {
                HStack { Spacer(); ProgressView().tint(theme.muted); Spacer() }.padding(.top, 28)
            } else if trimmed.isEmpty {
                Text(model.credentials == nil
                     ? "Sign in to see who else is on Arbos."
                     : "No one else is on Arbos yet. Search a GitHub username to invite them; the chat opens for both of you when they sign in.")
                    .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: .infinity)
                    .padding(.horizontal, 36).padding(.top, 20)
            } else {
                VStack(spacing: 10) {
                    Text("No one on Arbos or GitHub matches @\(trimmed).")
                        .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                    if trimmed.count >= 2 {
                        Button { openHandle(trimmed) } label: {
                            Text("Invite @\(trimmed)")
                                .font(theme.font(Theme.small, weight: .semibold)).foregroundStyle(theme.onPrimary)
                                .padding(.horizontal, 18).frame(height: 38)
                                .background(Capsule().fill(theme.primary))
                        }
                        .buttonStyle(.plain)
                    }
                }
                .frame(maxWidth: .infinity)
                .padding(.top, 20)
            }
        }
    }

    // MARK: Actions

    private func search() {
        let q = trimmed
        guard q.count >= 2 else { remote = []; remoteServers = []; searching = false; return }
        searching = true
        Task {
            try? await Task.sleep(nanoseconds: 250_000_000)
            guard q == trimmed else { return }
            async let peopleHits = model.searchPeople(q)
            async let serverHits = model.searchOpenServers(q)
            remote = await peopleHits
            remoteServers = await serverHits
            if q == trimmed { searching = false }
        }
    }

    private func open(_ p: Person) { openHandle(p.handle) }

    private func openAgent(_ p: Project) {
        guard !opening else { return }
        opening = true
        model.open(project: p)
        dismiss()
        opening = false
    }

    private func openServer(_ s: Server) {
        guard !opening else { return }
        opening = true
        if s.isMember {
            model.open(server: s)
            dismiss()
            opening = false
            return
        }
        Task {
            if await model.joinOpenServer(s.id) { dismiss() }
            opening = false
        }
    }

    private func openHandle(_ handle: String) {
        guard !opening else { return }
        opening = true
        Task {
            if await model.openDM(handle: handle) != nil { dismiss() }
            opening = false
        }
    }
}

/// One server in the New Message search list: disc, name, member count or "Open".
struct ServerPickRow: View {
    @Environment(\.theme) private var theme
    let server: Server

    var body: some View {
        HStack(spacing: SheetRow.gap) {
            ServerAvatar(server: server, size: SheetRow.avatar)
            VStack(alignment: .leading, spacing: 2) {
                Text(server.name).font(theme.font(Theme.body, weight: .medium)).foregroundStyle(theme.text).lineLimit(1)
                Text(server.isMember
                     ? "\(server.memberCount) member\(server.memberCount == 1 ? "" : "s")"
                     : "Open · tap to join")
                    .font(theme.font(Theme.tiny)).foregroundStyle(theme.muted).lineLimit(1)
            }
            Spacer()
            if !server.isMember {
                Text("Join")
                    .font(theme.font(11, weight: .medium))
                    .foregroundStyle(theme.online)
                    .padding(.horizontal, 9).padding(.vertical, 4)
                    .background(Capsule().fill(theme.field)) // control-size: exception inlineChips
            }
        }
        .padding(.horizontal, SheetRow.paddingX)
        .padding(.vertical, 9)
        .contentShape(Rectangle())
    }
}

/// One of my agents in the New Message / New group lists: tree avatar, name, @slug.
struct AgentPickRow: View {
    @Environment(\.theme) private var theme
    let project: Project
    var trailing: AnyView? = nil

    var body: some View {
        HStack(spacing: SheetRow.gap) {
            AgentAvatar(project: project, theme: theme, size: SheetRow.avatar, still: true)
            VStack(alignment: .leading, spacing: 2) {
                Text(project.name).font(theme.font(Theme.body, weight: .medium)).foregroundStyle(theme.text).lineLimit(1)
                Text("@\(mentionSlug(project.name))").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted).lineLimit(1)
            }
            Spacer()
            if let trailing { trailing }
        }
        .padding(.horizontal, SheetRow.paddingX)
        .padding(.vertical, 9)
        .contentShape(Rectangle())
    }
}

/// One person: avatar, name, @handle and their Arbos status.
struct PersonRow: View {
    @Environment(\.theme) private var theme
    let person: Person
    var trailing: AnyView? = nil

    var body: some View {
        HStack(spacing: SheetRow.gap) {
            PersonAvatar(url: person.avatarUrl, handle: person.handle, size: SheetRow.avatar, name: person.name, id: person.id, colorIndex: person.avatarColorIndex)
            VStack(alignment: .leading, spacing: 2) {
                Text(person.displayName).font(theme.font(Theme.body, weight: .medium)).foregroundStyle(theme.text).lineLimit(1)
                Text("@\(person.handle)").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted).lineLimit(1)
            }
            Spacer()
            if let trailing {
                trailing
            } else {
                Text(PersonRow.statusLabel(person.status))
                    .font(theme.font(11, weight: .medium))
                    .foregroundStyle(person.status == "member" ? theme.online : theme.muted)
                    .padding(.horizontal, 9).padding(.vertical, 4)
                    .background(Capsule().fill(theme.field)) // control-size: exception inlineChips
            }
        }
        .padding(.horizontal, SheetRow.paddingX)
        .padding(.vertical, 9)
        .contentShape(Rectangle())
    }

    static func statusLabel(_ s: String) -> String {
        switch s {
        case "member": return "On Arbos"
        case "invited": return "Invited"
        default: return "Invite"
        }
    }
}

/// New agent: a name and an avatar colour, then `POST /projects`. The relay
/// sets up its workspace. The preview is the Telegram-style initials circle
/// (D79): Automatic until a swatch is picked.
/// What the Create screen creates (D93: one style, parameterised by entity).
enum CreateKind {
    case agent, group, server

    var title: String {
        switch self {
        case .agent: return "Create New Agent"
        case .group: return "Create New Group"
        case .server: return "Create New Server"
        }
    }

    var placeholder: String {
        switch self {
        case .agent: return "Name your agent"
        case .group: return "Name your group"
        case .server: return "Name your server"
        }
    }

    /// The footer action: groups continue to the member picker.
    var action: String {
        switch self {
        case .agent, .server: return "Create"
        case .group: return "Next"
        }
    }

    var explainer: String? {
        switch self {
        case .server: return "A server has channels and an invite link. You start as its owner, in #general."
        case .agent, .group: return nil
        }
    }
}

/// The one Create screen (D93) for a new agent, group or server — Grok Bot's "Create New Bot"
/// layout: title + close ×, the preview, a centred name field, the picture options, Cancel + the
/// action pinned above the home indicator. Grok's sizes in points (measured on the 2x reference)
/// inside the shipped bottom-sheet frame; the 44 pt footer pills are the one deliberate deviation
/// (touch minimum). Agents create here; a group goes on to the member picker (`NewGroupView`) as
/// step two with the name + picture as its draft; a server keeps its one-line explainer under
/// the field. Pushed from New Message with its own header; × and Cancel close the whole sheet.
///
/// **Picture (Jacob, 2026-10-25, [feedback create-sheet-avatar-upload-generate-ios-2026-10-25]):
/// the sixteen-leaf grid and the ten pastel swatches are gone. Under the name field sit the
/// agent avatar picker's own two options — `Upload | Generate` (`AvatarTabPill` +
/// `AvatarSourceBody` on an `AvatarSourceState`) — and the preview above the field shows the
/// chosen photo, the generated picture, or, when neither was picked, the fallback every new
/// agent / group / server gets: the name's first letter on a neutral grey disc (`NeutralAvatar`).
/// The entity is stored with the neutral pair (`LeafArt.neutral`), never an automatic leaf;
/// existing agents keep theirs.**
struct CreateEntityView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let kind: CreateKind
    /// Closes the whole sheet (a pushed view's own dismiss only pops).
    let onDone: () -> Void
    @State private var name = DebugHooks.newAgentName ?? ""
    @State private var option: AvatarPickerTab = CreateEntityView.initialOption
    @StateObject private var source = AvatarSourceState()
    @State private var creating = false
    @State private var localError: UserError?
    @State private var next = false
    @FocusState private var focused: Bool

    private var clean: String { name.trimmingCharacters(in: .whitespaces) }
    private var canGo: Bool { !clean.isEmpty && !creating && !source.generating }

    /// The two options every Create sheet offers, in order — the draft picker's tabs.
    static let options: [AvatarPickerTab] = [.upload, .generate]
    /// The leaf pair a new entity is stored with: the neutral one, whatever else was picked, so
    /// no leaf is ever auto-assigned (a later Reset lands on the initial, not on a leaf).
    static var storedLeaf: LeafPick { LeafArt.neutralPick }
    /// Rig: `ARBOS_CREATE_PICTURE=generate[:<prompt>]` opens the sheet on Generate.
    private static var initialOption: AvatarPickerTab {
        (DebugHooks.createPicture ?? "").hasPrefix("generate") ? .generate : .upload
    }

    // Grok's palette, approved as is.
    private let sheetBg = AvatarSheetPalette.sheetBg
    private let line = AvatarSheetPalette.line
    private let placeholder = AvatarSheetPalette.placeholder
    private let cancelBg = AvatarSheetPalette.pillBg
    private let createBg = AvatarSheetPalette.disabledBg
    private let createText = AvatarSheetPalette.disabledText

    var body: some View {
        VStack(spacing: 0) {
            header
            // The sheet's note (Jacob 2026-10-26): under the header, before the hero — a refused
            // create, or a failed generation (`source.error`); the body below moves down.
            if let error = source.error ?? localError {
                ErrorNote(id: "create-entity-error", error: error,
                          detailsOpen: Binding(get: { source.detailsOpen }, set: { source.detailsOpen = $0 }),
                          onTryAgain: source.error != nil ? generate : nil)
                    .padding(.horizontal, 16.5)
                    .padding(.top, 12)
            } else {
                SheetErrorNote(id: "create-entity-error", onTryAgain: go)
                    .padding(.horizontal, 16.5)
                    .padding(.top, 12)
            }
            // The middle scrolls when the keyboard is up (the footer stays above the keyboard,
            // so the action is always reachable); at rest it is Grok's fixed layout.
            ScrollView(showsIndicators: false) {
                ZStack(alignment: .top) {
                    preview
                        .padding(.top, 62 - 54.5)
                        .animation(.snappy, value: source.generatedKey)
                    // Grok's vertical rhythm from the sheet top (header 54.5): field 183…218; the
                    // option capsules start where Grok's first icon row began (246); the chosen
                    // option's body 16 under them. The server explainer adds 40 under the field.
                    VStack(spacing: 0) {
                        field.padding(.top, 183 - 54.5)
                        if let explainer = kind.explainer {
                            Text(explainer)
                                .font(.system(size: 13))
                                .foregroundStyle(placeholder)
                                .multilineTextAlignment(.center)
                                .lineLimit(2)
                                .lineSpacing(3)
                                .fixedSize(horizontal: false, vertical: true)
                                .frame(maxWidth: .infinity)
                                .padding(.horizontal, 16.5)
                                .padding(.top, 8)
                                .frame(height: 40, alignment: .top)
                        }
                        AvatarTabPill(tabs: Self.options, tab: $option) { source.error = nil }
                            .padding(.top, 246 - 218)
                            .accessibilityIdentifier("create-entity-options")
                        AvatarSourceBody(state: source, tab: option, keepsLine: "Create keeps this one.", uploadsLine: "Create uploads it.", generate: generate) {
                            EmptyView()
                        }
                        .padding(.top, 16)
                    }
                }
                .padding(.bottom, 24)
            }
            .scrollDismissesKeyboard(.interactively)
            footer
        }
        .claimFailures("create-entity", ["create the server", "create the agent", "create the group"])
        .background(sheetBg.ignoresSafeArea())
        .toolbar(.hidden, for: .navigationBar)
        .navigationDestination(isPresented: $next) {
            NewGroupView(onDone: onDone, draft: GroupDraft(title: clean, leaf: Self.storedLeaf, picture: source.draftPicture(for: option)))
        }
        // The rig shoots the sheet without the keyboard (`ARBOS_SHOW=new…`); users get the caret.
        .onAppear {
            if !(DebugHooks.show ?? "").hasPrefix("new") { focused = true }
            if let spec = DebugHooks.createPicture, let colon = spec.firstIndex(of: ":") {
                source.prompt = String(spec[spec.index(after: colon)...])
            }
        }
    }

    private var header: some View {
        HStack {
            Text(kind.title)
                .font(.system(size: 14.2, weight: .semibold))
                .foregroundStyle(Color(white: 0xF5 / 255))
                .padding(.leading, 17.5)
            Spacer()
            Button(action: onDone) {
                Image(systemName: "xmark")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(Color(white: 0xD2 / 255))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .padding(.trailing, 5.5)
            .accessibilityLabel("Close")
        }
        .frame(height: 54.5)
        .overlay(alignment: .bottom) { Rectangle().fill(line).frame(height: 1) }
    }

    /// The 114 pt preview: the chosen photo, the generated picture (fetched from my own slot),
    /// else the neutral initial of the typed name — an empty disc until something is typed.
    @ViewBuilder
    private var preview: some View {
        if option == .upload, let image = source.uploadPreview {
            Image(uiImage: image).resizable().scaledToFill()
                .frame(width: 114, height: 114).clipShape(Circle())
        } else if option == .generate, let key = source.generatedKey, let url = model.draftPictureURL(key: key) {
            AsyncImage(url: url) { phase in
                if let image = phase.image {
                    image.resizable().scaledToFill()
                } else {
                    ZStack {
                        NeutralAvatar(name: clean, size: 114)
                        if phase.error == nil { ProgressView().tint(placeholder) }
                    }
                }
            }
            .frame(width: 114, height: 114)
            .clipShape(Circle())
        } else {
            NeutralAvatar(name: clean, size: 114)
        }
    }

    private var field: some View {
        TextField("", text: $name, prompt: Text(kind.placeholder).foregroundStyle(placeholder))
            .font(.system(size: 15.5))
            .foregroundStyle(Color(white: 0xF5 / 255))
            .multilineTextAlignment(.center)
            .autocorrectionDisabled()
            .focused($focused)
            .submitLabel(kind == .group ? .next : .done)
            .onSubmit(go)
            .padding(.horizontal, 12)
            .frame(height: 35)
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(line, lineWidth: 1))
            .padding(.horizontal, 16.5)
    }

    private var footer: some View {
        HStack(spacing: 10) {
            Spacer()
            Button(action: onDone) {
                Text("Cancel").font(.system(size: 15, weight: .semibold)).foregroundStyle(Color(white: 0xF5 / 255))
                    .frame(width: 84, height: 44).background(Capsule().fill(cancelBg))
            }
            .buttonStyle(.plain)
            Button(action: go) {
                Text(creating ? "Creating…" : kind.action).font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(canGo ? theme.onPrimary : createText)
                    .frame(width: 92, height: 44).background(Capsule().fill(canGo ? theme.primary : createBg))
            }
            .buttonStyle(.plain)
            .disabled(!canGo)
        }
        .padding(.trailing, 16)
        .padding(.top, 25)
        .padding(.bottom, 35)
        .overlay(alignment: .top) { Rectangle().fill(line).frame(height: 1) }
    }

    /// Generate into my own slot (`POST /me/avatar/generate`): the entity does not exist yet.
    private func generate() {
        let text = source.prompt.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty, !source.generating else { return }
        let model = model
        source.run { try await model.generateDraftAvatar(prompt: text) }
    }

    private func go() {
        guard canGo else { return }
        let picture = source.draftPicture(for: option)
        switch kind {
        case .agent:
            model.createProject(name: clean, leaf: Self.storedLeaf, picture: picture)
            onDone()
        case .group:
            next = true
        case .server:
            creating = true
            localError = nil
            Task {
                if await model.createServer(name: clean, leaf: Self.storedLeaf, picture: picture) {
                    onDone()
                }
                // A refusal is the claimed failure drawn by the note above the hero.
                creating = false
            }
        }
    }
}

/// What the Create group screen hands to the member picker (step 2).
struct GroupDraft: Equatable {
    var title: String
    var leaf: LeafPick
    /// The Create screen's uploaded / generated picture, applied once the group exists.
    var picture: DraftPicture? = nil
}

/// New group: pick people (and your own agents), then create. With `addTo`
/// (the group pane's Add members) the same picker is titled for that group,
/// shows its members as already in, and Add puts the picks into it.
struct NewGroupView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let onDone: () -> Void
    /// People-chat id to add to; nil creates a new group.
    var addTo: String? = nil
    /// Server id to add to (D88 Members › Add members): people only, members greyed.
    var addToServer: String? = nil
    /// Secret id whose People list the picks join (Secret page › "Give a person access…",
    /// spec §7.7): people only, listed people greyed.
    var addToSecret: String? = nil
    /// D93 step 1's name + leaf picture (nil when opened as Add members).
    var draft: GroupDraft? = nil
    @State private var query = ""
    @State private var remote: [Person] = []
    @State private var picked: [Person] = []
    @State private var pickedAgents: [Project] = []
    @State private var creating = false
    /// Shown on this screen — the root notice alert sits under the sheet and looks like a silent fail.
    @State private var localError: UserError?
    /// The old string form of that state, kept so a sibling branch written against it
    /// (`iphone-add-members-2026-10-26`) still compiles in the same integration tree; drawn as a refusal.
    private var errorText: String? {
        get { localError?.sentence }
        nonmutating set { localError = newValue.map { UserError.refused("add members", $0) } }
    }

    private var trimmed: String { query.trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "@")) }

    /// Own agents only (same as Members → Add agent); shared ones cannot be re-shared here.
    /// Empty query hides archived agents; a search finds them again — archived agents still
    /// exist and can join groups unless deleted (home archive is hide-only).
    private var agentCandidates: [Project] {
        if addToServer != nil || addToSecret != nil { return [] }
        let base = model.projects.filter { !$0.shared && (trimmed.isEmpty ? !model.isArchived($0.id) : true) }
            .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
        return PeoplePicker.agents(base, query: trimmed)
    }

    /// Directory + remote hits through the shared matcher (display name and @handle):
    /// "aff" lists Affine and AffineTeam next to Andreas F @aff, not @aff alone
    /// ([feedback shared-search-matcher-ios-2026-10-26]).
    private var candidates: [Person] {
        PeoplePicker.people(directory: model.directory, remote: remote, me: model.user?.id, query: trimmed)
    }

    private var addToChat: PeopleChat? { addTo.flatMap { model.peopleChat($0) } }
    private var server: Server? { addToServer.flatMap { model.server($0) } }
    private var secret: SecretItem? { addToSecret.flatMap { id in model.secrets?.secrets.first { $0.id == id } } }
    private var adding: Bool { addTo != nil || addToServer != nil || addToSecret != nil }
    private var title: String {
        if secret != nil { return "Give a person access" }
        if let s = server { return "Add members to \(s.name)" }
        return addToChat.map { "Add members to \($0.title)" } ?? (adding ? "Add members" : "New group")
    }
    /// Create needs a person (a group of one is a DM); Add takes agents alone too.
    /// People, agents, or both — a group of your agents is a group (Create
    /// greyed out with only agents picked read as broken, 2026-10-22).
    private var canSubmit: Bool { !(picked.isEmpty && pickedAgents.isEmpty) }

    private func isIn(_ p: Person) -> Bool {
        if let secret {
            return p.id == model.user?.id || secret.directors.contains { $0.userId == p.id || $0.handle.caseInsensitiveCompare(p.handle) == .orderedSame }
        }
        if let s = server {
            return p.id == model.user?.id || s.members.contains { $0.id == p.id || $0.handle.caseInsensitiveCompare(p.handle) == .orderedSame }
        }
        return addToChat?.hasPerson(p, meId: model.user?.id) ?? false
    }
    private func isIn(_ p: Project) -> Bool { addToChat?.hasAgent(p.id) ?? false }

    private func pickMark(selected: Bool, already: Bool) -> AnyView {
        AnyView(
            Image(systemName: selected || already ? "checkmark.circle.fill" : "circle")
                .font(theme.font(22))
                .foregroundStyle(already ? theme.muted : (selected ? theme.text : theme.muted.opacity(0.5)))
        )
    }

    var body: some View {
        VStack(spacing: 0) {
            if !picked.isEmpty || !pickedAgents.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(pickedAgents) { p in
                            MemberChip(label: "@\(mentionSlug(p.name))", onRemove: { toggleAgent(p) }) {
                                AgentAvatar(project: p, theme: theme, size: memberChipAvatar, still: true)
                            }
                        }
                        ForEach(picked) { p in
                            MemberChip(label: "@\(p.handle)", onRemove: { toggle(p) }) {
                                PersonAvatar(url: p.avatarUrl, handle: p.handle, size: memberChipAvatar, name: p.name, id: p.id, colorIndex: p.avatarColorIndex)
                            }
                        }
                    }
                    .padding(.horizontal, 16)
                }
                .frame(height: 44)
            }
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").font(.system(size: SheetSearchField.iconSize, weight: .medium)).foregroundStyle(theme.muted)
                TextField("", text: $query, prompt: Text(addToSecret != nil ? "Search people" : "Add people or agents").foregroundStyle(theme.placeholder))
                    .textInputAutocapitalization(.never).autocorrectionDisabled()
                    .font(theme.font(Theme.body)).foregroundStyle(theme.text)
            }
            .sheetSearchFieldChrome(theme)
            .padding(.horizontal, SheetSearchField.inset).padding(.vertical, 10)
            .onChange(of: query) { _, _ in search() }

            // The sheet's note: under the search field, before the list (a refused add, nobody
            // picked, the relay's reason) — the shared element.
            SheetErrorNote(id: "people-sheet-error", local: localError)
                .padding(.horizontal, 16)
                .padding(.bottom, 8)

            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    if !agentCandidates.isEmpty {
                        Text("YOUR AGENTS")
                            .font(theme.font(12, weight: .medium))
                            .kerning(0.4)
                            .foregroundStyle(theme.muted)
                            .padding(.horizontal, 16)
                            .padding(.bottom, 8)
                        VStack(spacing: 0) {
                            ForEach(Array(agentCandidates.enumerated()), id: \.element.id) { i, p in
                                let already = isIn(p)
                                Button { toggleAgent(p) } label: {
                                    AgentPickRow(project: p, trailing: pickMark(selected: pickedAgents.contains(where: { $0.id == p.id }), already: already))
                                }
                                .buttonStyle(.plain)
                                .disabled(already)
                                .opacity(already ? 0.5 : 1)
                                if i < agentCandidates.count - 1 { Rectangle().fill(theme.separator).frame(height: 0.5).padding(.leading, SheetRow.textInset) }
                            }
                        }
                        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(theme.card))
                        .padding(.horizontal, 16)
                        .padding(.bottom, 18)
                    }

                    if !candidates.isEmpty || agentCandidates.isEmpty {
                        if !agentCandidates.isEmpty {
                            Text("PEOPLE")
                                .font(theme.font(12, weight: .medium))
                                .kerning(0.4)
                                .foregroundStyle(theme.muted)
                                .padding(.horizontal, 16)
                                .padding(.bottom, 8)
                        }
                        VStack(spacing: 0) {
                            ForEach(Array(candidates.enumerated()), id: \.element.id) { i, p in
                                let already = isIn(p)
                                Button { toggle(p) } label: {
                                    PersonRow(person: p, trailing: pickMark(selected: picked.contains(p), already: already))
                                }
                                .buttonStyle(.plain)
                                .disabled(already)
                                .opacity(already ? 0.5 : 1)
                                if i < candidates.count - 1 { Rectangle().fill(theme.separator).frame(height: 0.5).padding(.leading, SheetRow.textInset) }
                            }
                            if candidates.isEmpty {
                                Text(trimmed.isEmpty ? "No one else on Arbos yet. Search a GitHub username." : "No one matches @\(trimmed).")
                                    .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                                    .multilineTextAlignment(.center).padding(.horizontal, 36).padding(.vertical, 24)
                            }
                        }
                        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(theme.card))
                        .padding(.horizontal, 16)
                    }
                    Color.clear.frame(height: 40)
                }
            }
        }
        .background(theme.bg)
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(theme.bg, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .toolbar {
            if adding {
                ToolbarItem(placement: .topBarLeading) { Button("Cancel") { onDone() }.foregroundStyle(theme.text) }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button { if adding { add() } else { create() } } label: {
                    Text(adding ? (creating ? "Adding…" : "Add") : (creating ? "Creating…" : "Create"))
                        .font(theme.font(Theme.small, weight: .semibold))
                        .foregroundStyle(theme.onPrimary)
                        .padding(.horizontal, 12)
                        .frame(height: 32)
                        .background(Capsule().fill(theme.primary))
                }
                .buttonStyle(.plain)
                .disabled(!canSubmit || creating)
                .opacity(!canSubmit || creating ? 0.45 : 1)
            }
        }
        .task { await model.loadDirectory() }
    }

    private func toggle(_ p: Person) {
        guard !isIn(p) else { return }
        if let i = picked.firstIndex(of: p) { picked.remove(at: i) } else { picked.append(p) }
        localError = nil
    }

    private func toggleAgent(_ p: Project) {
        guard !isIn(p) else { return }
        if let i = pickedAgents.firstIndex(where: { $0.id == p.id }) { pickedAgents.remove(at: i) } else { pickedAgents.append(p) }
        localError = nil
    }

    /// Add members: every pick goes to the group — or server, or secret — this picker was opened for.
    private func add() {
        if let secret {
            let new = picked.filter { !isIn($0) }
            guard !new.isEmpty else { localError = UserError.refused("add them", "Pick someone who is not on the list yet."); return }
            creating = true
            localError = nil
            Task {
                if await model.addSecretPeople(secretId: secret.id, handles: new.map(\.handle)) {
                    onDone()
                }
                creating = false
            }
            return
        }
        if let s = server {
            let new = picked.filter { !isIn($0) }
            guard !new.isEmpty else { localError = UserError.refused("add members", "Pick someone who is not in the server yet."); return }
            creating = true
            localError = nil
            Task {
                if await model.addServerMembers(serverId: s.id, userIds: new.map(\.id)) {
                    onDone()
                }
                creating = false
            }
            return
        }
        guard let chat = addToChat, canSubmit, !creating else { return }
        let plan = AddMembersPlan.make(chat: chat, meId: model.user?.id, picked: picked, pickedAgents: pickedAgents)
        guard !plan.isEmpty else { localError = UserError.refused("add members", "Pick someone who is not in the group yet."); return }
        creating = true
        localError = nil
        Task {
            if await model.addMembers(chatId: plan.chatId, handles: plan.handles, agentIds: plan.agentIds) {
                onDone()
            }
            creating = false
        }
    }

    private func search() {
        let q = trimmed
        guard q.count >= 2 else { remote = []; return }
        Task {
            try? await Task.sleep(nanoseconds: 250_000_000)
            guard q == trimmed else { return }
            remote = await model.searchPeople(q)
        }
    }

    private func create() {
        guard canSubmit, !creating else { return }
        creating = true
        localError = nil
        let agentIds = pickedAgents.map(\.id)
        Task {
            if await model.createGroup(handles: picked.map(\.handle), agentIds: agentIds, title: draft?.title, leaf: draft?.leaf, picture: draft?.picture) {
                onDone()
            }
            creating = false
        }
    }
}

/// Project members (D72 step 3): invite by @handle, list, remove (owner).
struct ProjectMembersSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let project: Project
    @State private var members: [ProjectMember] = []
    @State private var handle = ""
    @State private var busy = false
    @State private var remote: [Person] = []
    @State private var searching = false

    private var trimmed: String {
        handle.trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "@"))
    }

    private var memberIds: Set<String> { Set(members.map(\.id)) }

    /// Directory + remote matches for what was typed (shared matcher on display
    /// name and @handle), current members and me left out.
    private var suggestions: [Person] {
        guard !trimmed.isEmpty else { return [] }
        return PeoplePicker.people(directory: model.directory, remote: remote, me: model.user?.id, query: trimmed)
            .filter { !memberIds.contains($0.id) }
    }

    var body: some View {
        NavigationStack {
            List {
                if !project.shared {
                    Section {
                        HStack(spacing: 8) {
                            Text("@").foregroundStyle(theme.muted)
                            TextField("", text: $handle, prompt: Text("GitHub username").foregroundStyle(theme.placeholder))
                                .textInputAutocapitalization(.never).autocorrectionDisabled()
                                .onSubmit { invite() }
                            if searching {
                                ProgressView().controlSize(.small).tint(theme.muted)
                            }
                            Button(busy ? "…" : "Invite") { invite() }
                                .font(theme.font(Theme.small, weight: .semibold))
                                .disabled(busy || trimmed.isEmpty)
                        }
                        if !trimmed.isEmpty {
                            ForEach(suggestions) { p in
                                Button { invite(p.handle) } label: {
                                    HStack(spacing: PaneKit.gap) {
                                        PersonAvatar(url: p.avatarUrl, handle: p.handle, size: 40, name: p.name, id: p.id, colorIndex: p.avatarColorIndex)
                                        VStack(alignment: .leading, spacing: 2) {
                                            Text(p.displayName).foregroundStyle(theme.text)
                                            Text("@\(p.handle)").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                                        }
                                        Spacer()
                                        Text(PersonRow.statusLabel(p.status))
                                            .font(theme.font(11, weight: .medium))
                                            .foregroundStyle(p.status == "member" ? theme.online : theme.muted)
                                    }
                                }
                                .buttonStyle(.plain)
                                .disabled(busy)
                            }
                            if suggestions.isEmpty && !searching && trimmed.count >= 2 {
                                Text("No one matches @\(trimmed).")
                                    .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                            }
                        }
                    } footer: {
                        Text("A member drives this agent with you, sees its chats and files, and spends your credits. You can remove them any time.")
                    }
                    .listRowBackground(theme.card)
                }
                Section("Members") {
                    ForEach(members) { m in
                        HStack(spacing: PaneKit.gap) {
                            Button { model.showProfile(.user(ProfilePerson(m))) } label: { PersonAvatar(url: m.avatarUrl, handle: m.handle, size: 40, name: m.name, id: m.id, colorIndex: m.avatarColorIndex) }.buttonStyle(.plain)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(m.name?.isEmpty == false ? m.name! : m.handle).foregroundStyle(theme.text)
                                Text("@\(m.handle) · \(m.role)\(m.status == "invited" ? " · invited" : "")").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                            }
                        }
                        .swipeActions {
                            if !project.shared && m.role != "owner" {
                                Button("Remove", role: .destructive) {
                                    Task { await model.removeProjectMember(project.id, userId: m.id); await load() }
                                }
                            }
                        }
                    }
                    if members.isEmpty { Text("Just you so far.").foregroundStyle(theme.muted) }
                }
                .listRowBackground(theme.card)
            }
            .scrollContentBackground(.hidden)
            .background(theme.bg)
            .navigationTitle("Share \(project.name)")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() } } }
            .task {
                await load()
                await model.loadDirectory()
            }
            .onChange(of: handle) { _, _ in search() }
        }
        .presentationDetents([.medium, .large])
    }

    private func load() async { members = await model.projectMembers(project.id) }

    private func search() {
        let q = trimmed
        guard q.count >= 2 else { remote = []; searching = false; return }
        searching = true
        Task {
            try? await Task.sleep(nanoseconds: 250_000_000)
            guard q == trimmed else { return }
            remote = await model.searchPeople(q)
            if q == trimmed { searching = false }
        }
    }

    private func invite(_ raw: String? = nil) {
        let h = (raw ?? trimmed)
            .trimmingCharacters(in: .whitespaces)
            .trimmingCharacters(in: CharacterSet(charactersIn: "@"))
        guard !h.isEmpty, !busy else { return }
        busy = true
        Task {
            if let list = await model.inviteToProject(project.id, handle: h) {
                members = list
                handle = ""
                remote = []
            }
            busy = false
        }
    }
}


// MARK: - Shared sheet pieces

/// The search field of the New Message / New group / Add person sheets: the
/// control family's field ([feedback iphone-header-composer-sizing-2026-10-26]; was
/// Grok's 40 pt sheet field), side edges lined up with the row content below it
/// (card inset 16 + row padding 14).
enum SheetSearchField {
    static let height: CGFloat = Controls.height
    static let inset: CGFloat = 30
    static let iconSize: CGFloat = Controls.glyph
}

private struct SheetSearchFieldChrome: ViewModifier {
    let theme: Theme
    func body(content: Content) -> some View {
        content.controlField()
    }
}

extension View {
    func sheetSearchFieldChrome(_ theme: Theme) -> some View { modifier(SheetSearchFieldChrome(theme: theme)) }
}

/// Group-chat call picker: call everyone, or tick specific members (people and
/// agents) and Call. `onCall(nil)` = whole group; `onCall([ids])` = those ids
/// (bare user / project ids, same as `ChatMember.id`).
struct GroupCallPickerSheet: View {
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let people: PeopleChat
    let meId: String?
    let onCall: (_ ring: [String]?) -> Void
    @State private var picked: Set<String> = []

    /// Everyone else in the group who can join a call (agents without a live project omitted).
    private var others: [ChatMember] {
        people.members.filter { $0.id != meId && (!$0.isAgent || $0.project != nil) }
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                Button {
                    onCall(nil)
                    dismiss()
                } label: {
                    HStack(spacing: SheetRow.gap) {
                        GlassCircle(size: SheetRow.avatar) { // control-size: exception rowIconTiles
                            Image(systemName: "person.3.fill")
                                .font(.system(size: 16, weight: .semibold))
                                .foregroundStyle(theme.text)
                        }
                        VStack(alignment: .leading, spacing: 2) {
                            Text("Everyone")
                                .font(theme.font(Theme.body, weight: .medium))
                                .foregroundStyle(theme.text)
                            Text("Call the whole group")
                                .font(theme.font(Theme.tiny))
                                .foregroundStyle(theme.muted)
                        }
                        Spacer()
                        Image(systemName: "phone.fill")
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundStyle(theme.muted)
                    }
                    .padding(.horizontal, SheetRow.paddingX)
                    .padding(.vertical, 12)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(theme.card))
                .padding(.horizontal, 16)
                .padding(.top, 8)
                .padding(.bottom, 18)
                .accessibilityLabel("Call everyone")

                if !others.isEmpty {
                    Text("OR CHOOSE")
                        .font(theme.font(12, weight: .medium))
                        .kerning(0.4)
                        .foregroundStyle(theme.muted)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 16)
                        .padding(.bottom, 8)

                    ScrollView {
                        VStack(spacing: 0) {
                            ForEach(Array(others.enumerated()), id: \.element.id) { i, m in
                                Button { toggle(m.id) } label: {
                                    HStack(spacing: SheetRow.gap) {
                                        if m.isAgent, let p = m.project {
                                            AgentAvatar(project: p, theme: theme, size: SheetRow.avatar, still: true)
                                        } else {
                                            PersonAvatar(url: m.avatarUrl, handle: m.handle, size: SheetRow.avatar,
                                                         name: m.name, id: m.id, colorIndex: m.avatarColorIndex)
                                        }
                                        Text(m.displayName)
                                            .font(theme.font(Theme.body, weight: .medium))
                                            .foregroundStyle(theme.text)
                                            .lineLimit(1)
                                        Spacer()
                                        Image(systemName: picked.contains(m.id) ? "checkmark.circle.fill" : "circle")
                                            .font(.system(size: 22))
                                            .foregroundStyle(picked.contains(m.id) ? theme.primary : theme.muted)
                                    }
                                    .padding(.horizontal, SheetRow.paddingX)
                                    .padding(.vertical, 9)
                                    .contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)
                                .accessibilityLabel(m.displayName)
                                .accessibilityAddTraits(picked.contains(m.id) ? .isSelected : [])
                                if i < others.count - 1 {
                                    Rectangle().fill(theme.separator).frame(height: 0.5).padding(.leading, SheetRow.textInset)
                                }
                            }
                        }
                        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(theme.card))
                        .padding(.horizontal, 16)
                        Color.clear.frame(height: 24)
                    }
                }

                Spacer(minLength: 0)

                Button {
                    onCall(Array(picked))
                    dismiss()
                } label: {
                    Text(picked.isEmpty ? "Call" : (picked.count == 1 ? "Call 1 person" : "Call \(picked.count) people"))
                        .font(theme.font(Theme.body, weight: .semibold))
                        .foregroundStyle(theme.onPrimary)
                        .frame(maxWidth: .infinity)
                        .frame(height: 48)
                        .background(Capsule().fill(theme.primary))
                }
                .buttonStyle(.plain)
                .disabled(picked.isEmpty)
                .opacity(picked.isEmpty ? 0.45 : 1)
                .padding(.horizontal, 16)
                .padding(.bottom, 12)
                .accessibilityLabel("Call selected")
            }
            .background(theme.bg)
            .navigationTitle("Start a call")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(theme.bg, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") { dismiss() }.foregroundStyle(theme.text)
                }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .presentationBackground(theme.bg)
    }

    private func toggle(_ id: String) {
        if picked.contains(id) { picked.remove(id) } else { picked.insert(id) }
    }
}

/// One selected member in a picker (New group, Add members): the member's
/// picture, `@handle`, and an × that removes it. People and agents share it,
/// so an agent chip carries the agent's avatar exactly like a person chip.
/// Avatar size inside a selected-member chip (a free constant: a static on the
/// generic chip cannot be named without its type parameter).
let memberChipAvatar: CGFloat = 22

struct MemberChip<Picture: View>: View {
    @Environment(\.theme) private var theme
    let label: String
    let onRemove: () -> Void
    let picture: () -> Picture

    init(label: String, onRemove: @escaping () -> Void, @ViewBuilder picture: @escaping () -> Picture) {
        self.label = label
        self.onRemove = onRemove
        self.picture = picture
    }

    var body: some View {
        HStack(spacing: 6) {
            picture()
            Text(label).font(theme.font(Theme.tiny, weight: .medium)).foregroundStyle(theme.text)
            Button(action: onRemove) { Image(systemName: "xmark").font(.system(size: 10, weight: .bold)).foregroundStyle(theme.muted) }
                .buttonStyle(.plain)
        }
        .padding(.leading, 4).padding(.trailing, 10).frame(height: 32)
        .background(Capsule().fill(theme.field)) // control-size: exception inlineChips
    }
}
