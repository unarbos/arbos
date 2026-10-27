// Settings → Secrets (D83, spec-secrets.md §8): the kill switch, connected vaults, the
// secrets list, recent activity; a Secret page with per-agent switches; the New secret sheet;
// the agent's Secrets page. D80 structure (SettingsPage / SettingsGroup / SettingsRow) and the
// exact strings of §8.8. No value is ever displayed: the relay never sends one.

import SwiftUI

// MARK: - Building blocks

/// Icon tile · title over a one-line subtitle · trailing (switch, value or chevron). 56 pt.
struct SecretRowLabel<Trailing: View>: View {
    @Environment(\.theme) private var theme
    let icon: String
    let title: String
    var subtitle: String? = nil
    var dim = false
    @ViewBuilder var trailing: () -> Trailing

    var body: some View {
        HStack(spacing: 14) {
            RoundedRectangle(cornerRadius: 7.5, style: .continuous)
                .fill(theme.isDark ? Color(hex: "#5C5C60")! : Color(hex: "#6E6E73")!)
                .frame(width: SettingsRow.tile, height: SettingsRow.tile)
                .overlay(Image(systemName: icon).font(.system(size: 15, weight: .semibold)).foregroundStyle(.white))
                .opacity(dim ? 0.55 : 1)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(theme.font(Theme.body)).foregroundStyle(dim ? theme.muted : theme.text).lineLimit(2)
                if let subtitle, !subtitle.isEmpty {
                    Text(subtitle).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted).lineLimit(2)
                }
            }
            Spacer(minLength: 8)
            trailing()
        }
        .padding(.horizontal, 16)
        .frame(minHeight: 56)
        .contentShape(Rectangle())
    }
}

/// A "＋ Add…" row in the Apple style: tile with a plus, blue-ish label.
struct SecretAddRow: View {
    @Environment(\.theme) private var theme
    let title: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 14) {
                RoundedRectangle(cornerRadius: 7.5, style: .continuous)
                    .fill(theme.field)
                    .frame(width: SettingsRow.tile, height: SettingsRow.tile)
                    .overlay(Image(systemName: "plus").font(.system(size: 15, weight: .semibold)).foregroundStyle(theme.text))
                Text(title).font(theme.font(Theme.body)).foregroundStyle(theme.text)
                Spacer()
            }
            .padding(.horizontal, 16)
            .frame(minHeight: SettingsRow.height)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// A switch row with the tile look.
struct SecretToggleRow: View {
    @Environment(\.theme) private var theme
    let icon: String
    let title: String
    var subtitle: String? = nil
    var tint: Color? = nil
    @Binding var isOn: Bool
    var disabled = false

    var body: some View {
        SecretRowLabel(icon: icon, title: title, subtitle: subtitle) {
            Toggle("", isOn: $isOn)
                .labelsHidden()
                .tint(tint ?? theme.online)
                .disabled(disabled)
                .accessibilityLabel(title)
        }
    }
}

struct SecretField<Content: View>: View {
    @Environment(\.theme) private var theme
    let label: String
    var hint: String? = nil
    @ViewBuilder var content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(label).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
            content()
            if let hint {
                Text(hint).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
            }
        }
    }
}

struct SecretTextField: View {
    @Environment(\.theme) private var theme
    let placeholder: String
    @Binding var text: String
    var secure = false
    var multiline = false
    var mono = false
    var keyboard: UIKeyboardType = .default
    var capitalization: TextInputAutocapitalization = .never

    var body: some View {
        Group {
            if multiline {
                TextEditor(text: $text)
                    .scrollContentBackground(.hidden)
                    .frame(minHeight: 96, maxHeight: 180)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .overlay(alignment: .topLeading) {
                        if text.isEmpty {
                            Text(placeholder).foregroundStyle(theme.placeholder).padding(.horizontal, 14).padding(.vertical, 14).allowsHitTesting(false)
                        }
                    }
            } else if secure {
                SecureField("", text: $text, prompt: Text(placeholder).foregroundStyle(theme.placeholder))
                    .padding(.horizontal, Controls.textInset)
                    .frame(height: Controls.height)
            } else {
                TextField("", text: $text, prompt: Text(placeholder).foregroundStyle(theme.placeholder))
                    .keyboardType(keyboard)
                    .padding(.horizontal, Controls.textInset)
                    .frame(height: Controls.height)
            }
        }
        .font(theme.font(Theme.body, design: mono ? .monospaced : .default))
        .foregroundStyle(theme.text)
        .textInputAutocapitalization(capitalization)
        .autocorrectionDisabled()
        .background(ControlSurface(shape: Capsule()))
    }
}

/// Four small answers under a pending request row: Allow once · Allow for this chat · Always allow · Deny.
/// They wrap onto a second line on narrow phones so Deny is never off-screen.
struct RequestAnswers: View {
    @Environment(\.theme) private var theme
    let agent: String
    /// `@handle` when a person other than the owner asked: "Always allow" then adds them
    /// to the secret's People list (spec §7.7).
    var forWhom: String? = nil
    let decide: (String) -> Void

    var body: some View {
        WrapLayout(spacing: 8) {
            answer("Allow once", "once")
            answer("Allow for this chat", "chat")
            answer("Always allow \(forWhom ?? agent)", "always")
            answer("Deny", "deny", destructive: true)
        }
    }

    private func answer(_ label: String, _ id: String, destructive: Bool = false) -> some View {
        Button { Haptics.pick(); decide(id) } label: {
            Text(label)
                .font(theme.font(Theme.tiny, weight: .medium))
                .foregroundStyle(destructive ? theme.danger : theme.text)
                .padding(.horizontal, 10)
                .frame(height: 28)
                .background(Capsule().fill(theme.field)) // control-size: exception inlineChips
        }
        .buttonStyle(.plain)
    }
}

/// Leading-aligned rows of chips: fills a line, then starts the next.
struct WrapLayout: Layout {
    var spacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        return place(in: width, subviews: subviews).size
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let result = place(in: bounds.width, subviews: subviews)
        for (subview, origin) in zip(subviews, result.origins) {
            subview.place(at: CGPoint(x: bounds.minX + origin.x, y: bounds.minY + origin.y), proposal: .unspecified)
        }
    }

    private func place(in width: CGFloat, subviews: Subviews) -> (size: CGSize, origins: [CGPoint]) {
        var origins: [CGPoint] = []
        var x: CGFloat = 0, y: CGFloat = 0, lineHeight: CGFloat = 0, maxX: CGFloat = 0
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x > 0, x + size.width > width {
                x = 0
                y += lineHeight + spacing
                lineHeight = 0
            }
            origins.append(CGPoint(x: x, y: y))
            x += size.width + spacing
            lineHeight = max(lineHeight, size.height)
            maxX = max(maxX, x - spacing)
        }
        return (CGSize(width: maxX, height: y + lineHeight), origins)
    }
}

/// One pending request row (`WAITING FOR YOU`).
struct SecretRequestRow: View {
    @Environment(\.theme) private var theme
    let request: SecretRequestItem
    let decide: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(requestTitle)
                        .font(theme.font(Theme.body)).foregroundStyle(theme.text)
                    Text([request.projectName, request.askedByName.map { "\($0) asked" }, request.purpose.isEmpty ? nil : request.purpose, SecretsText.ago(request.createdAt)].compactMap { $0 }.joined(separator: " · "))
                        .font(theme.font(Theme.tiny)).foregroundStyle(theme.muted).lineLimit(2)
                }
                Spacer()
            }
            RequestAnswers(agent: request.projectName ?? "this agent", forWhom: request.forWhom, decide: decide)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }

    private var requestTitle: String {
        let who = request.forWhom.map { " for \($0)" } ?? ""
        return request.host.map { "\(request.name)\(who) at \($0)" } ?? "\(request.name)\(who)"
    }
}

/// Activity lines (names, hosts, times; never values).
struct ActivityRows: View {
    @Environment(\.theme) private var theme
    let items: [SecretActivity]

    var body: some View {
        ForEach(items) { a in
            VStack(alignment: .leading, spacing: 2) {
                Text(a.line)
                    .font(theme.font(Theme.small))
                    .foregroundStyle(a.action == "refused" ? theme.danger : theme.text)
                    .lineLimit(2)
                Text(a.whenLine).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 16)
            .padding(.vertical, 9)
        }
    }
}

// MARK: - Secrets page (Settings → Secrets)

struct SecretsPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @State private var showAdd = false
    @State private var showConnect = false
    /// Rig: `ARBOS_SETTINGS_PAGE=secret:<ENV>` pushes that secret's page once the list arrives.
    @State private var rigSecretId: String?
    @State private var showRigSecret = false
    @State private var rigVaultId: String?
    @State private var showRigVault = false

    private var overview: SecretsOverview? { model.secrets }
    private var paused: Bool { overview?.paused ?? false }
    /// Same rule on both platforms: anything waiting shows here and on the agent's page.
    private var pending: [SecretRequestItem] { overview?.requests.filter(\.isPending) ?? [] }

    var body: some View {
        SettingsPage(title: "Secrets") {
            SettingsGroup(footer: paused
                          ? "Secrets are paused. Agents can't use any secret until you turn this off."
                          : "Agents can't use any secret while paused.") {
                SecretToggleRow(icon: "pause.fill", title: "Pause all secrets", tint: theme.danger,
                                // Flips at once (optimistic; a relay error puts it back) — no confirm (Jacob, Oct 21).
                                isOn: Binding(get: { paused }, set: { on in model.setSecretsPaused(on) }),
                                disabled: model.isFixture || overview == nil)
            }
            if !pending.isEmpty {
                SettingsGroup(header: "Waiting for you") {
                    ForEach(pending) { r in
                        SecretRequestRow(request: r) { decision in
                            model.secrets?.requests.removeAll { $0.id == r.id }
                            model.decideSecretRequest(r.id, decision: decision, projectId: r.projectId)
                        }
                    }
                }
            }
            if let o = overview, !o.vaults.isEmpty || model.vaultProvidersAvailable {
                SettingsGroup(header: "Vaults") {
                    ForEach(o.vaults) { v in
                        NavigationLink { VaultPage(vaultId: v.id) } label: {
                            SecretRowLabel(icon: "lock.rectangle.stack.fill", title: "\(v.providerLabel) · \(v.name)", subtitle: vaultSubtitle(v)) {
                                Image(systemName: "chevron.right").font(.system(size: 14, weight: .semibold)).foregroundStyle(theme.placeholder)
                            }
                        }
                    }
                    if model.vaultProvidersAvailable {
                        SecretAddRow(title: "Connect a vault…") { showConnect = true }
                    }
                }
            }
            SettingsGroup(header: "Secrets", footer: (overview?.secrets.isEmpty ?? true) && (overview?.vaults.isEmpty ?? true)
                          ? "Add an API key or password here, pick which agents may use it, and they'll use it without ever seeing it."
                          : "Agents use secrets without seeing them. Values are never shown again after you save them.") {
                ForEach(overview?.secrets ?? []) { s in
                    NavigationLink { SecretPage(secretId: s.id) } label: {
                        SecretRowLabel(icon: s.icon, title: s.env, subtitle: secretSubtitle(s), dim: paused) {
                            Image(systemName: "chevron.right").font(.system(size: 14, weight: .semibold)).foregroundStyle(theme.placeholder)
                        }
                    }
                }
                SecretAddRow(title: "Add a secret…") { showAdd = true }
            }
            if let acts = overview?.activity, !acts.isEmpty {
                SettingsGroup(header: "Recent activity") {
                    ActivityRows(items: Array(acts.prefix(3)))
                    NavigationLink { SecretsActivityPage() } label: {
                        SettingsValueRow("See all activity") {
                            Image(systemName: "chevron.right").font(.system(size: 14, weight: .semibold)).foregroundStyle(theme.placeholder)
                        }
                    }
                }
            }
        }
        .task {
            await model.refreshSecrets()
            if let env = DebugHooks.settingsPage?.stripping(prefix: "secret:"),
               let s = model.secrets?.secrets.first(where: { $0.env == env }) {
                rigSecretId = s.id
                showRigSecret = true
            }
            // Rig: `ARBOS_SETTINGS_PAGE=vault` opens the first connected vault; `connect` the Connect sheet.
            if DebugHooks.settingsPage == "vault", let v = model.secrets?.vaults.first {
                rigVaultId = v.id
                showRigVault = true
            }
            if DebugHooks.settingsPage == "connect" { showConnect = true }
            if DebugHooks.settingsPage == "new-secret" { showAdd = true }
        }
        .navigationDestination(isPresented: $showRigSecret) {
            if let id = rigSecretId { SecretPage(secretId: id) }
        }
        .navigationDestination(isPresented: $showRigVault) {
            if let id = rigVaultId { VaultPage(vaultId: id) }
        }
        .sheet(isPresented: $showAdd) {
            NavigationStack { NewSecretSheet() }
                .environmentObject(model).environment(\.theme, theme)
                .presentationBackground(theme.bg)
                .preferredColorScheme(theme.isDark ? .dark : .light)
        }
        .sheet(isPresented: $showConnect) {
            NavigationStack { ConnectVaultSheet() }
                .environmentObject(model).environment(\.theme, theme)
                .presentationBackground(theme.bg)
                .preferredColorScheme(theme.isDark ? .dark : .light)
        }
    }

    private func secretSubtitle(_ s: SecretItem) -> String {
        let agents = s.agents.filter(\.enabled).map(\.name)
        let who = agents.isEmpty ? "no agents" : agents.joined(separator: ", ")
        // Secret hand-off: "Arbos · given in chat · never used".
        if let given = s.sourceLabel {
            return "\(who) · \(given) · \(SecretsText.usage(lastUsedAt: s.lastUsedAt))"
        }
        return "\(who) · \(SecretsText.usage(lastUsedAt: s.lastUsedAt))"
    }

    private func vaultSubtitle(_ v: ConnectedVault) -> String {
        var parts = ["\(v.itemsCount) item\(v.itemsCount == 1 ? "" : "s")"]
        let agents = v.agents.filter(\.enabled).map(\.name)
        if !agents.isEmpty { parts.append(agents.joined(separator: ", ")) }
        if let t = v.tokenExpiresAt { parts.append("token expires \(SecretsText.shortDate(t))") }
        if v.status != "ok" { parts.append(v.status.replacingOccurrences(of: "_", with: " ")) }
        return parts.joined(separator: " · ")
    }
}

// MARK: - Secret page

struct SecretPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let secretId: String
    @State private var activity: [SecretActivity] = []
    @State private var showEdit = false
    @State private var showReplace = false
    @State private var showGive = false
    @State private var showAddDirector = false
    @State private var editingAccess: SecretAgentAccess?
    @State private var editingDirector: SecretDirector?
    @State private var confirmDelete = false
    @State private var confirmRevoke = false
    @State private var confirmScrub = false

    private var secret: SecretItem? { model.secrets?.secrets.first { $0.id == secretId } }

    var body: some View {
        SettingsPage(title: secret?.env ?? "Secret") {
            if let s = secret {
                if !s.note.isEmpty {
                    SettingsGroup {
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Note").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                            Text(s.note).font(theme.font(Theme.body)).foregroundStyle(theme.text)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 16).padding(.vertical, 12)
                    }
                }
                SettingsGroup(footer: "Ask every time shows you a card before each use, even for agents that have access.") {
                    SettingsValueRow("Kind", value: s.kindLabel)
                    if s.name != s.env {
                        SettingsValueRow("Name", value: s.name)
                    }
                    // Secret hand-off: where it came from ("In chat with Arbos").
                    if let added = s.addedLabel {
                        SettingsValueRow("Added", value: added)
                    }
                    if let u = s.username, !u.isEmpty {
                        SettingsValueRow("Username", value: u)
                    }
                    SettingsValueRow("Used at", value: s.hosts.isEmpty ? "Decided at first use" : s.hosts.joined(separator: ", "))
                    HStack {
                        Text("Value").font(theme.font(Theme.body)).foregroundStyle(theme.text)
                        Spacer()
                        Text("••••••").font(theme.font(Theme.body)).foregroundStyle(theme.muted)
                        Button("Replace…") { showReplace = true }
                            .font(theme.font(Theme.body)).foregroundStyle(Color(hex: "#1084FE")!)
                            .disabled(model.isFixture)
                    }
                    .padding(.horizontal, 16).frame(minHeight: SettingsRow.height)
                    SettingsValueRow("Expires", value: s.expiresAt.map { SecretsText.shortDate($0) } ?? "Never")
                    HStack {
                        Text("Ask every time").font(theme.font(Theme.body)).foregroundStyle(theme.text)
                        Spacer()
                        Toggle("", isOn: Binding(get: { s.askEveryTime }, set: { model.patchSecret(s.id, ["ask_every_time": $0]) }))
                            .labelsHidden().tint(theme.online).disabled(model.isFixture)
                    }
                    .padding(.horizontal, 16).frame(minHeight: SettingsRow.height)
                }
                SettingsGroup(footer: s.injectEnv
                              ? "On: \(SecretsText.realVarList(s)) is a normal variable in the box of every agent with access. Any program the agent runs can use the key directly — including to spend money or change things on your accounts — without asking you. No card, no host check. Turning access or Pause all off removes it within seconds. Arbos still redacts the value from chat and logs as best it can."
                              : "Off: agents get a placeholder and the relay fills the value in only through egress, for the allowed hosts. Turn this on only for keys you would hand to a CLI or SDK that must read $\(s.env) itself — once on, programs can act with it, including spending, without asking you.") {
                    SecretToggleRow(icon: "terminal.fill", title: "Real value in the environment",
                                    subtitle: s.injectEnv ? "On · agents with access see the value" : "Agents with access get \(SecretsText.realVarList(s)) itself",
                                    isOn: Binding(get: { s.injectEnv }, set: { model.patchSecret(s.id, ["inject_env": $0]) }),
                                    disabled: model.isFixture)
                }
                SettingsGroup(header: "Agents") {
                    ForEach(s.agents) { a in
                        Button { editingAccess = a } label: {
                            agentRow(s, a)
                        }
                        .buttonStyle(.plain)
                    }
                    if s.agents.isEmpty {
                        Text("No agent has access yet.")
                            .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 16).padding(.vertical, 12)
                    }
                    SecretAddRow(title: "Give another agent access…") { showGive = true }
                }
                SettingsGroup(header: "People", footer: "People here can ask any agent that has access to use \(s.env), with no card to you. They never see the value. What they spend is yours.") {
                    Color.clear.frame(height: 0).id("people")
                    ForEach(s.directors) { d in
                        Button { editingDirector = d } label: {
                            personRow(s, d)
                        }
                        .buttonStyle(.plain)
                    }
                    if s.directors.isEmpty {
                        Text("Only you right now.")
                            .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 16).padding(.vertical, 12)
                    }
                    SecretAddRow(title: "Give a person access…") { showAddDirector = true }
                }
                SettingsGroup(footer: s.othersAsk == "card" ? "Ask me: anyone else who asks gets you a card first." : "Refuse: anyone else who asks is told no.") {
                    HStack(spacing: 12) {
                        Text("Others who ask").font(theme.font(Theme.body)).foregroundStyle(theme.text)
                        Spacer(minLength: 8)
                        OthersAskSegment(value: s.othersAsk) { model.patchSecret(s.id, ["others_ask": $0]) }
                            .disabled(model.isFixture)
                    }
                    .padding(.horizontal, 16).frame(minHeight: SettingsRow.height)
                }
                SettingsGroup(header: "Activity") {
                    if activity.isEmpty {
                        Text("No activity in the last 30 days.")
                            .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 16).padding(.vertical, 12)
                    } else {
                        ActivityRows(items: Array(activity.prefix(5)))
                        NavigationLink { SecretsActivityPage(secretId: s.id, title: s.env) } label: {
                            SettingsValueRow("See all") {
                                Image(systemName: "chevron.right").font(.system(size: 14, weight: .semibold)).foregroundStyle(theme.placeholder)
                            }
                        }
                    }
                }
                SettingsGroup {
                    SettingsActionRow(title: "Remove from all agents", disabled: s.agents.allSatisfy { !$0.enabled } || model.isFixture) { confirmRevoke = true }
                }
                SettingsGroup {
                    SettingsActionRow(title: "Scrub from context", disabled: model.isFixture) { confirmScrub = true }
                }
                SettingsGroup {
                    SettingsActionRow(title: "Delete secret", destructive: true, disabled: model.isFixture) { confirmDelete = true }
                }
            } else {
                SettingsGroup { SettingsValueRow("This secret is gone.", value: "") }
            }
        }
        .overlay(alignment: .topTrailing) {
            if secret != nil {
                Button("Edit") { showEdit = true }
                    .font(theme.font(Theme.body, weight: .medium)).foregroundStyle(theme.text)
                    .controlPill()
                    .padding(.trailing, 20).padding(.top, 6)
                    .disabled(model.isFixture)
            }
        }
        .task { await load() }
        .onAppear {
            switch DebugHooks.secretSheet {
            case "people": DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { showAddDirector = true }
            case "person": DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { editingDirector = secret?.directors.first }
            default: break
            }
        }
        .confirmationDialog("Delete \(secret?.env ?? "secret")?", isPresented: $confirmDelete, titleVisibility: .visible) {
            Button("Delete secret", role: .destructive) {
                model.deleteSecret(secretId)
                dismiss()
            }
        } message: { Text("Agents lose it immediately. Activity is kept.") }
        .confirmationDialog("Remove from all agents?", isPresented: $confirmRevoke, titleVisibility: .visible) {
            Button("Remove from all agents", role: .destructive) { model.revokeSecretEverywhere(secretId) }
        } message: { Text("Every agent loses \(secret?.env ?? "it") within seconds. You can turn them back on one by one.") }
        .confirmationDialog("Scrub \(secret?.env ?? "secret") from context?", isPresented: $confirmScrub, titleVisibility: .visible) {
            Button("Scrub from context", role: .destructive) {
                model.scrubSecret(secretId)
                Task { await load() }
            }
        } message: { Text("Arbos looks for this value in chats, agent sessions and logs, and replaces every copy with a redaction.") }
        .sheet(isPresented: $showEdit) {
            if let s = secret {
                NavigationStack { EditSecretSheet(secret: s) }
                    .environmentObject(model).environment(\.theme, theme)
                    .presentationBackground(theme.bg).preferredColorScheme(theme.isDark ? .dark : .light)
            }
        }
        .sheet(isPresented: $showReplace) {
            if let s = secret {
                NavigationStack { ReplaceValueSheet(secret: s) }
                    .environmentObject(model).environment(\.theme, theme)
                    .presentationDetents([.medium, .large])
                    .presentationBackground(theme.bg).preferredColorScheme(theme.isDark ? .dark : .light)
            }
        }
        .sheet(isPresented: $showGive) {
            if let s = secret {
                NavigationStack { GiveAccessSheet(secret: s) }
                    .environmentObject(model).environment(\.theme, theme)
                    .presentationDetents([.medium, .large])
                    .presentationBackground(theme.bg).preferredColorScheme(theme.isDark ? .dark : .light)
            }
        }
        .sheet(isPresented: $showAddDirector) {
            // The standard member picker (New group / Add members), people only (spec §7.7).
            if let s = secret {
                NavigationStack { NewGroupView(onDone: { showAddDirector = false }, addToSecret: s.id) }
                    .tint(theme.text)
                    .environmentObject(model).environment(\.theme, theme)
                    .presentationBackground(theme.bg).preferredColorScheme(theme.isDark ? .dark : .light)
            }
        }
        .sheet(item: $editingAccess) { a in
            if let s = secret {
                NavigationStack { AccessEditSheet(secret: s, access: a) }
                    .environmentObject(model).environment(\.theme, theme)
                    .presentationDetents([.medium, .large])
                    .presentationBackground(theme.bg).preferredColorScheme(theme.isDark ? .dark : .light)
            }
        }
        .sheet(item: $editingDirector) { d in
            if let s = secret {
                NavigationStack { DirectorEditSheet(secret: s, director: d) }
                    .environmentObject(model).environment(\.theme, theme)
                    .presentationDetents([.medium, .large])
                    .presentationBackground(theme.bg).preferredColorScheme(theme.isDark ? .dark : .light)
            }
        }
    }

    /// A People row: the mirror of an agent row — avatar, name, `@handle · note · asked 2 min ago`, switch.
    private func personRow(_ s: SecretItem, _ d: SecretDirector) -> some View {
        HStack(spacing: 14) {
            PersonAvatar(url: d.avatarUrl, handle: d.handle.isEmpty ? d.userId : d.handle, size: SettingsRow.tile, name: d.name, id: d.userId)
            VStack(alignment: .leading, spacing: 2) {
                Text(d.displayName).font(theme.font(Theme.body)).foregroundStyle(theme.text)
                Text(personSubtitle(d)).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted).lineLimit(2)
            }
            Spacer(minLength: 8)
            Toggle("", isOn: Binding(get: { d.enabled }, set: { model.setSecretPerson(secretId: s.id, userId: d.userId, enabled: $0) }))
                .labelsHidden().tint(theme.online).disabled(model.isFixture)
                .accessibilityLabel("\(d.displayName) may ask")
        }
        .padding(.horizontal, 16)
        .frame(minHeight: 56)
        .contentShape(Rectangle())
    }

    private func personSubtitle(_ d: SecretDirector) -> String {
        var parts: [String] = []
        if !d.handle.isEmpty { parts.append("@\(d.handle)") }
        if !d.note.isEmpty { parts.append(d.note) }
        if d.scopeKind == "chat" { parts.append("this chat only") }
        parts.append(SecretsText.asked(lastAskedAt: d.lastAskedAt, disabledAt: d.disabledAt, enabled: d.enabled))
        return parts.joined(separator: " · ")
    }

    private func agentRow(_ s: SecretItem, _ a: SecretAgentAccess) -> some View {
        HStack(spacing: 14) {
            if let p = model.project(a.projectId) {
                AgentAvatar(project: p, theme: theme, size: SettingsRow.tile, still: true)
            } else {
                InitialsAvatar(name: a.name, handle: a.name, id: a.projectId, override: nil, size: SettingsRow.tile)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(a.name).font(theme.font(Theme.body)).foregroundStyle(theme.text)
                Text(accessSubtitle(a)).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted).lineLimit(2)
            }
            Spacer(minLength: 8)
            Toggle("", isOn: Binding(get: { a.enabled }, set: { model.setSecretAccess(secretId: s.id, projectId: a.projectId, enabled: $0) }))
                .labelsHidden().tint(theme.online).disabled(model.isFixture)
                .accessibilityLabel("\(a.name) access")
        }
        .padding(.horizontal, 16)
        .frame(minHeight: 56)
        .contentShape(Rectangle())
    }

    private func accessSubtitle(_ a: SecretAgentAccess) -> String {
        var parts: [String] = []
        if !a.note.isEmpty { parts.append(a.note) }
        if a.scopeKind == "chat" { parts.append("this chat only") }
        parts.append(SecretsText.usage(lastUsedAt: a.lastUsedAt, disabledAt: a.disabledAt, enabled: a.enabled))
        return parts.joined(separator: " · ")
    }

    private func load() async {
        if let (s, acts) = try? await model.relay.secret(secretId) {
            activity = acts
            if let i = model.secrets?.secrets.firstIndex(where: { $0.id == s.id }) {
                model.secrets?.secrets[i] = s
            }
        }
    }
}

// MARK: - Sheets

/// Cancel · title · Save bar on a sheet.
private struct SheetFrame<Content: View>: View {
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let title: String
    let action: String
    let canSave: Bool
    let busy: Bool
    let save: () -> Void
    @ViewBuilder var content: () -> Content

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) { content() }
                .padding(.horizontal, 24)
                .padding(.top, 16)
                .padding(.bottom, 32)
        }
        .scrollDismissesKeyboard(.interactively)
        .background(theme.bg)
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button("Cancel") { dismiss() }.foregroundStyle(theme.text)
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button(busy ? "Saving…" : action) { save() }
                    .font(theme.font(Theme.body, weight: .semibold))
                    .foregroundStyle(theme.text)
                    .disabled(!canSave || busy)
                    .opacity(canSave && !busy ? 1 : 0.45)
            }
        }
    }
}

struct NewSecretSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    @State private var kind = "key"
    @State private var name = ""
    @State private var value = ""
    @State private var url = ""
    @State private var username = ""
    @State private var password = ""
    @State private var note = ""
    @State private var hosts = ""
    @State private var picked: Set<Id> = []
    @State private var injectEnv = false
    @State private var busy = false
    @State private var error: String?

    private var env: String { SecretsText.envName(name) }
    private var canSave: Bool {
        guard SecretsText.validName(name) else { return false }
        switch kind {
        case "login": return !password.isEmpty
        default: return !value.isEmpty
        }
    }

    var body: some View {
        SheetFrame(title: "New secret", action: "Save", canSave: canSave, busy: busy, save: { Task { await save() } }) {
            Picker("Kind", selection: $kind) {
                Text("Key").tag("key")
                Text("Login").tag("login")
                Text("Text").tag("text")
            }
            .pickerStyle(.segmented)
            SecretField(label: "Name", hint: "Letters, numbers and _. Agents see it as $\(env.isEmpty ? "NAME" : env).") {
                SecretTextField(placeholder: kind == "login" ? "GITHUB_LOGIN" : "STRIPE_KEY", text: $name, mono: true, capitalization: .characters)
            }
            switch kind {
            case "login":
                SecretField(label: "Sign-in URL") {
                    SecretTextField(placeholder: "https://github.com/login", text: $url, keyboard: .URL)
                }
                SecretField(label: "Username") {
                    SecretTextField(placeholder: "name@example.com", text: $username, keyboard: .emailAddress)
                }
                SecretField(label: "Password", hint: "Sent once and encrypted. You won't see it again.") {
                    SecretTextField(placeholder: "••••••••", text: $password, secure: true)
                }
            case "text":
                SecretField(label: "Value", hint: "Sent once and encrypted. You won't see it again.") {
                    SecretTextField(placeholder: "Paste a certificate, JSON key, or any text", text: $value, multiline: true, mono: true)
                }
            default:
                SecretField(label: "Value", hint: "Sent once and encrypted. You won't see it again.") {
                    SecretTextField(placeholder: "sk_live_…", text: $value, secure: true)
                }
            }
            SecretField(label: "Note", hint: SecretsText.noteHint) {
                SecretTextField(placeholder: SecretsText.notePlaceholder, text: $note, multiline: true, capitalization: .sentences)
            }
            SecretField(label: "Agents") {
                AgentPickerField(picked: $picked)
            }
            SecretField(label: "Where it may be used (optional)", hint: "Leave empty to decide the first time an agent uses it.") {
                SecretTextField(placeholder: "api.stripe.com", text: $hosts, keyboard: .URL)
            }
            SecretField(label: "In the agent's environment", hint: injectEnv
                        ? "On: agents with access get $\(env.isEmpty ? "NAME" : env) itself. Any program they run can use it directly — including to spend money — without asking you. Only for keys you would hand to a CLI."
                        : "Off: agents get a placeholder; the relay fills the value in only through egress, for the allowed hosts. Turn on only for keys a CLI or SDK must read itself.") {
                Toggle(isOn: $injectEnv) {
                    Text("Real value in the agent's environment").font(theme.font(Theme.body)).foregroundStyle(theme.text)
                }
                .tint(theme.online)
            }
            if let error {
                Text(error).font(theme.font(Theme.small)).foregroundStyle(theme.danger)
            }
        }
    }

    private func save() async {
        guard canSave, !busy else { return }
        busy = true
        error = nil
        defer { busy = false }
        var body: [String: Any] = [
            "name": name.trimmingCharacters(in: .whitespaces),
            "kind": kind,
            "note": note.trimmingCharacters(in: .whitespacesAndNewlines),
            "hosts": hosts.split(whereSeparator: { $0 == "," || $0 == " " || $0 == "\n" }).map(String.init),
            "agents": picked.map { ["project_id": $0] },
            "inject_env": injectEnv,
        ]
        switch kind {
        case "login":
            body["fields"] = ["url": url, "username": username, "password": password]
        default:
            body["value"] = value
        }
        do {
            _ = try await model.relay.createSecret(body)
            value = ""; password = ""
            await model.refreshSecrets()
            dismiss()
        } catch let e as RelayError {
            error = e.message
        } catch {
            self.error = error.localizedDescription
        }
    }
}

struct EditSecretSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let secret: SecretItem
    @State private var name = ""
    @State private var note = ""
    @State private var hosts = ""
    @State private var expires: Date?
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        SheetFrame(title: secret.env, action: "Save", canSave: SecretsText.validName(name), busy: busy, save: { Task { await save() } }) {
            SecretField(label: "Name", hint: "Agents see it as $\(SecretsText.envName(name)).") {
                SecretTextField(placeholder: "STRIPE_KEY", text: $name, mono: true, capitalization: .characters)
            }
            SecretField(label: "Note", hint: SecretsText.noteHint) {
                SecretTextField(placeholder: SecretsText.notePlaceholder, text: $note, multiline: true, capitalization: .sentences)
            }
            SecretField(label: "Where it may be used (optional)", hint: "Leave empty to decide the first time an agent uses it.") {
                SecretTextField(placeholder: "api.stripe.com", text: $hosts, keyboard: .URL)
            }
            SecretField(label: "Expires") {
                HStack {
                    Toggle("Expires", isOn: Binding(get: { expires != nil }, set: { on in
                        expires = on ? (expires ?? Calendar.current.date(byAdding: .day, value: 30, to: Date())) : nil
                    }))
                    .labelsHidden().tint(theme.online)
                    if let e = expires {
                        DatePicker("", selection: Binding(get: { e }, set: { expires = $0 }), in: Date()..., displayedComponents: .date)
                            .labelsHidden()
                    } else {
                        Text("Never").font(theme.font(Theme.body)).foregroundStyle(theme.muted)
                    }
                    Spacer()
                }
            }
            if let error { Text(error).font(theme.font(Theme.small)).foregroundStyle(theme.danger) }
        }
        .onAppear {
            name = secret.name
            note = secret.note
            hosts = secret.hosts.joined(separator: ", ")
            expires = secret.expiresAt.map { Date(timeIntervalSince1970: Double($0) / 1000) }
        }
    }

    private func save() async {
        busy = true
        defer { busy = false }
        var body: [String: Any] = [
            "name": name.trimmingCharacters(in: .whitespaces),
            "note": note.trimmingCharacters(in: .whitespacesAndNewlines),
            "hosts": hosts.split(whereSeparator: { $0 == "," || $0 == " " || $0 == "\n" }).map(String.init),
        ]
        body["expires_at"] = expires.map { Int64($0.timeIntervalSince1970 * 1000) } ?? NSNull()
        do {
            _ = try await model.relay.patchSecret(secret.id, body)
            await model.refreshSecrets()
            dismiss()
        } catch let e as RelayError { error = e.message } catch { self.error = error.localizedDescription }
    }
}

struct ReplaceValueSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let secret: SecretItem
    @State private var value = ""
    @State private var username = ""
    @State private var password = ""
    @State private var url = ""
    @State private var busy = false
    @State private var error: String?

    private var canSave: Bool { secret.kind == "login" ? !password.isEmpty : !value.isEmpty }

    var body: some View {
        SheetFrame(title: "Replace value", action: "Replace", canSave: canSave, busy: busy, save: { Task { await save() } }) {
            Text("The old value keeps working for 24 hours while agents pick up the new one.")
                .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
            if secret.kind == "login" {
                SecretField(label: "Sign-in URL") { SecretTextField(placeholder: "https://…", text: $url, keyboard: .URL) }
                SecretField(label: "Username") { SecretTextField(placeholder: secret.username ?? "name@example.com", text: $username, keyboard: .emailAddress) }
                SecretField(label: "Password", hint: "Sent once and encrypted. You won't see it again.") {
                    SecretTextField(placeholder: "••••••••", text: $password, secure: true)
                }
            } else {
                SecretField(label: "Value", hint: "Sent once and encrypted. You won't see it again.") {
                    SecretTextField(placeholder: secret.kind == "text" ? "Paste the new text" : "sk_live_…", text: $value, secure: secret.kind != "text", multiline: secret.kind == "text", mono: true)
                }
            }
            if let error { Text(error).font(theme.font(Theme.small)).foregroundStyle(theme.danger) }
        }
        .onAppear { username = secret.username ?? "" }
    }

    private func save() async {
        busy = true
        defer { busy = false }
        let body: [String: Any] = secret.kind == "login"
            ? ["fields": ["url": url, "username": username, "password": password]]
            : ["value": value]
        do {
            _ = try await model.relay.replaceSecretValue(secret.id, body)
            value = ""; password = ""
            await model.refreshSecrets()
            dismiss()
        } catch let e as RelayError { error = e.message } catch { self.error = error.localizedDescription }
    }
}

/// Two-segment control: Refuse | Ask me (what a non-listed person's ask does).
struct OthersAskSegment: View {
    @Environment(\.theme) private var theme
    let value: String
    let set: (String) -> Void

    var body: some View {
        HStack(spacing: 2) {
            segment("Refuse", "refuse")
            segment("Ask me", "card")
        }
        .padding(2)
        .background(Capsule().fill(theme.field)) // control-size: exception inlineChips
        .fixedSize()
    }

    private func segment(_ label: String, _ id: String) -> some View {
        let on = value == id || (id == "refuse" && value != "card")
        return Button { if !on { Haptics.pick(); set(id) } } label: {
            Text(label)
                .font(theme.font(Theme.small, weight: on ? .semibold : .regular))
                .lineLimit(1)
                .foregroundStyle(on ? theme.onPrimary : theme.muted)
                .padding(.horizontal, 12)
                .frame(height: 28)
                .background(Capsule().fill(on ? theme.primary : Color.clear))
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Others who ask: \(label)")
    }
}

/// Tap a People row (not its switch): edit the note or take them off the list.
struct DirectorEditSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let secret: SecretItem
    let director: SecretDirector
    @State private var note = ""
    @State private var busy = false

    var body: some View {
        SheetFrame(title: director.displayName, action: "Save", canSave: true, busy: busy, save: { Task { await save() } }) {
            HStack(spacing: 12) {
                PersonAvatar(url: director.avatarUrl, handle: director.handle.isEmpty ? director.userId : director.handle, size: 44, name: director.name, id: director.userId)
                VStack(alignment: .leading, spacing: 2) {
                    Text(director.displayName).font(theme.font(Theme.body)).foregroundStyle(theme.text)
                    if !director.handle.isEmpty {
                        Text("@\(director.handle)").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                    }
                }
                Spacer()
            }
            SecretField(label: "Note for the agent", hint: "Shown whenever the agent sees this secret, when \(director.displayName) asks. Allowed actions and spend limits, e.g. reports only · may spend up to $5 a day.") {
                SecretTextField(placeholder: "only 1k usd of credit usage per day", text: $note, capitalization: .sentences)
            }
            Button(role: .destructive) {
                model.removeSecretDirector(secretId: secret.id, userId: director.userId)
                dismiss()
            } label: {
                Text("Remove from the list")
                    .font(theme.font(Theme.body, weight: .semibold))
                    .foregroundStyle(theme.danger)
                    .frame(maxWidth: .infinity).frame(height: Controls.height)
                    .background(ControlSurface(shape: Capsule()))
            }
            .buttonStyle(.plain)
            .disabled(model.isFixture)
        }
        .onAppear { note = director.note }
    }

    private func save() async {
        busy = true
        defer { busy = false }
        let text = note.trimmingCharacters(in: .whitespacesAndNewlines)
        _ = await model.secretsAction("Save") {
            let s = try await model.relay.setSecretDirector(secret.id, userId: director.userId, note: text)
            if let i = model.secrets?.secrets.firstIndex(where: { $0.id == s.id }) {
                model.secrets?.secrets[i] = s
            }
        }
        dismiss()
    }
}

/// `Give another agent access…`: pick agents, add a note, switch them on.
struct GiveAccessSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let secret: SecretItem
    @State private var picked: Set<Id> = []
    @State private var note = ""
    @State private var busy = false

    var body: some View {
        SheetFrame(title: "Give access", action: "Save", canSave: !picked.isEmpty, busy: busy, save: { Task { await save() } }) {
            Text("\(secret.env) will be usable by the agents you pick, without them ever seeing it.")
                .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
            SecretField(label: "Agents") { AgentPickerField(picked: $picked) }
            SecretField(label: "Note for the agent", hint: SecretsText.accessNoteHint) { SecretTextField(placeholder: "reports only · may spend up to $5 a day", text: $note, capitalization: .sentences) }
        }
        .onAppear {
            picked = Set(secret.agents.filter(\.enabled).map(\.projectId))
        }
    }

    private func save() async {
        busy = true
        defer { busy = false }
        let before = Set(secret.agents.filter(\.enabled).map(\.projectId))
        for pid in picked.subtracting(before) {
            _ = try? await model.relay.setSecretAccess(secret.id, projectId: pid, enabled: true, note: note.isEmpty ? nil : note)
        }
        for pid in before.subtracting(picked) {
            _ = try? await model.relay.setSecretAccess(secret.id, projectId: pid, enabled: false)
        }
        await model.refreshSecrets()
        dismiss()
    }
}

/// Tap an agent row (not its switch): note, hosts, scope, Turn off.
struct AccessEditSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let secret: SecretItem
    let access: SecretAgentAccess
    @State private var note = ""
    @State private var hosts = ""
    @State private var everywhere = true
    @State private var busy = false

    var body: some View {
        SheetFrame(title: access.name, action: "Save", canSave: true, busy: busy, save: { Task { await save() } }) {
            SecretField(label: "Note for the agent", hint: SecretsText.accessNoteHint) { SecretTextField(placeholder: "reports only · may spend up to $5 a day", text: $note, capitalization: .sentences) }
            SecretField(label: "Where it may be used (optional)", hint: secret.hosts.isEmpty ? "Leave empty to decide the first time \(access.name) uses it." : "Also allowed: \(secret.hosts.joined(separator: ", ")) (from the secret).") {
                SecretTextField(placeholder: "api.stripe.com", text: $hosts, keyboard: .URL)
            }
            SecretField(label: "Scope") {
                Picker("Scope", selection: $everywhere) {
                    Text("Everywhere").tag(true)
                    Text("This chat only").tag(false)
                }
                .pickerStyle(.segmented)
                .disabled(access.scopeChatId == nil)
                if access.scopeChatId == nil && !everywhere {
                    Text("Chat-only access is made from the card in a chat (Allow for this chat).").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                }
            }
            Button(role: .destructive) {
                model.setSecretAccess(secretId: secret.id, projectId: access.projectId, enabled: !access.enabled)
                dismiss()
            } label: {
                Text(access.enabled ? "Turn off" : "Turn on")
                    .font(theme.font(Theme.body, weight: .semibold))
                    .foregroundStyle(access.enabled ? theme.danger : theme.text)
                    .frame(maxWidth: .infinity).frame(height: Controls.height)
                    .background(ControlSurface(shape: Capsule()))
            }
            .buttonStyle(.plain)
        }
        .onAppear {
            note = access.note
            hosts = access.hosts.joined(separator: ", ")
            everywhere = access.scopeKind != "chat"
        }
    }

    private func save() async {
        busy = true
        defer { busy = false }
        var scope: [String: Any] = ["kind": "all"]
        if !everywhere, let c = access.scopeChatId { scope = ["kind": "chat", "chat_id": c] }
        _ = try? await model.relay.setSecretAccess(secret.id, projectId: access.projectId, enabled: access.enabled,
                                                   note: note, hosts: hosts.split(whereSeparator: { $0 == "," || $0 == " " }).map(String.init), scope: scope)
        await model.refreshSecrets()
        dismiss()
    }
}

// MARK: - Activity

struct SecretsActivityPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    var secretId: String? = nil
    var projectId: String? = nil
    var vaultId: String? = nil
    var title: String = "Activity"
    @State private var items: [SecretActivity] = []
    @State private var nextBefore: Int64?
    @State private var loading = false

    var body: some View {
        SettingsPage(title: title) {
            SettingsGroup(footer: items.isEmpty && !loading ? "No activity in the last 30 days." : nil) {
                if items.isEmpty && loading {
                    ProgressView().frame(maxWidth: .infinity).padding(.vertical, 20)
                }
                ActivityRows(items: items)
                if nextBefore != nil {
                    SettingsActionRow(title: loading ? "Loading…" : "Load more", disabled: loading) { Task { await load(more: true) } }
                }
            }
        }
        .task { await load(more: false) }
    }

    private func load(more: Bool) async {
        loading = true
        defer { loading = false }
        if let r = try? await model.relay.secretsActivity(secretId: secretId, projectId: projectId, vaultId: vaultId, before: more ? nextBefore : nil, limit: 100) {
            items = more ? items + r.items : r.items
            nextBefore = r.nextBefore
        }
    }
}

// MARK: - Agent → Secrets

struct AgentSecretsPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let projectId: Id
    @State private var showGive = false

    private var project: Project? { model.project(projectId) }
    private var data: ProjectSecrets? { model.projectSecrets[projectId] }
    private var name: String { project?.name ?? "Agent" }

    var body: some View {
        SettingsPage(title: "\(name) · Secrets") {
            SettingsGroup(footer: agentFooter) {
                if let d = data, d.paused {
                    Text("Secrets are paused. Agents can't use any secret until you turn this off.")
                        .font(theme.font(Theme.small)).foregroundStyle(theme.danger)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 16).padding(.vertical, 12)
                }
                ForEach(data?.items ?? []) { item in
                    SecretRowLabel(icon: item.secret?.injectEnv == true ? "terminal.fill" : item.icon, title: item.title, subtitle: itemSubtitle(item), dim: data?.paused ?? false) {
                        Toggle("", isOn: Binding(get: { item.enabled }, set: { on in
                            if let s = item.secret {
                                model.setSecretAccess(secretId: s.id, projectId: projectId, enabled: on)
                            } else if let v = item.vault {
                                model.setVaultAccess(vaultId: v.id, projectId: projectId, enabled: on)
                            }
                        }))
                        .labelsHidden().tint(theme.online)
                        .disabled(model.isFixture || !(data?.owner ?? false))
                        .accessibilityLabel("\(item.title) access")
                    }
                }
                if data?.owner ?? false {
                    SecretAddRow(title: "Give access to a secret…") { showGive = true }
                }
            }
            if let reqs = data?.requests.filter(\.isPending), !reqs.isEmpty {
                SettingsGroup(header: "Waiting for you") {
                    ForEach(reqs) { r in
                        SecretRequestRow(request: r) { decision in
                            model.projectSecrets[projectId]?.requests.removeAll { $0.id == r.id }
                            model.decideSecretRequest(r.id, decision: decision, projectId: projectId)
                        }
                    }
                }
            }
            SettingsGroup(header: "Activity") {
                if let acts = data?.activity, !acts.isEmpty {
                    ActivityRows(items: Array(acts.prefix(5)))
                    NavigationLink { SecretsActivityPage(projectId: projectId, title: "\(name) · Activity") } label: {
                        SettingsValueRow("See all") {
                            Image(systemName: "chevron.right").font(.system(size: 14, weight: .semibold)).foregroundStyle(theme.placeholder)
                        }
                    }
                } else {
                    Text("No activity in the last 30 days.")
                        .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 16).padding(.vertical, 12)
                }
            }
        }
        .task {
            await model.refreshProjectSecrets(projectId)
            if model.secrets == nil { await model.refreshSecrets() }
        }
        .sheet(isPresented: $showGive) {
            NavigationStack { PickSecretsSheet(projectId: projectId) }
                .environmentObject(model).environment(\.theme, theme)
                .presentationDetents([.medium, .large])
                .presentationBackground(theme.bg).preferredColorScheme(theme.isDark ? .dark : .light)
        }
    }

    /// "Comms uses these without seeing them" — except the ones whose real value it holds.
    private var agentFooter: String {
        guard let items = data?.items, !items.isEmpty else { return "\(name) has no secrets yet." }
        let real = items.compactMap { $0.secret }.filter(\.injectEnv).map(\.env)
        let except = real.isEmpty ? "" : ", except \(real.joined(separator: ", ")) — a real value in its environment"
        return "\(name) uses these without seeing them\(except). Turn one off and it stops within seconds."
    }

    private func itemSubtitle(_ item: ProjectSecretAccess) -> String {
        if let v = item.vault {
            var s = "everything in this vault · \(v.itemsCount) item\(v.itemsCount == 1 ? "" : "s")"
            if v.envItems > 0 { s += " · \(v.envItems) as real value\(v.envItems == 1 ? "" : "s")" }
            return s
        }
        var parts: [String] = []
        if let s = item.secret, s.injectEnv { parts.append("Real value in its environment") }
        if !item.note.isEmpty { parts.append(item.note) }
        parts.append(SecretsText.usage(lastUsedAt: item.lastUsedAt, disabledAt: item.disabledAt, enabled: item.enabled))
        return parts.joined(separator: " · ")
    }
}

/// `Give access to a secret…` from the agent page: pick among the user's secrets.
struct PickSecretsSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let projectId: Id
    @State private var picked: Set<String> = []
    @State private var note = ""
    @State private var busy = false

    private var secrets: [SecretItem] { model.secrets?.secrets ?? [] }
    private var already: Set<String> {
        Set(model.projectSecrets[projectId]?.items.filter(\.enabled).compactMap { $0.secret?.id } ?? [])
    }

    var body: some View {
        SheetFrame(title: "Give access", action: "Save", canSave: !picked.isEmpty, busy: busy, save: { Task { await save() } }) {
            if secrets.isEmpty {
                Text("Add an API key or password in Settings → Secrets first.").font(theme.font(Theme.small)).foregroundStyle(theme.muted)
            }
            VStack(spacing: 0) {
                ForEach(secrets) { s in
                    let on = picked.contains(s.id) || already.contains(s.id)
                    Button {
                        guard !already.contains(s.id) else { return }
                        Haptics.pick()
                        if picked.contains(s.id) { picked.remove(s.id) } else { picked.insert(s.id) }
                    } label: {
                        SecretRowLabel(icon: s.icon, title: s.env, subtitle: s.note.isEmpty ? nil : s.note, dim: already.contains(s.id)) {
                            Image(systemName: on ? "checkmark.circle.fill" : "circle")
                                .font(theme.font(20)).foregroundStyle(on ? theme.online : theme.placeholder)
                        }
                    }
                    .buttonStyle(.plain)
                    if s.id != secrets.last?.id { Rectangle().fill(theme.separator).frame(height: 1 / UIScreen.main.scale).padding(.leading, SettingsRow.separatorInset) }
                }
            }
            .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(theme.bubbleAssistant))
            SecretField(label: "Note for the agent", hint: SecretsText.accessNoteHint) { SecretTextField(placeholder: "reports only · may spend up to $5 a day", text: $note, capitalization: .sentences) }
        }
        .task { if model.secrets == nil { await model.refreshSecrets() } }
    }

    private func save() async {
        busy = true
        defer { busy = false }
        for id in picked {
            _ = try? await model.relay.setSecretAccess(id, projectId: projectId, enabled: true, note: note.isEmpty ? nil : note)
        }
        await model.refreshProjectSecrets(projectId)
        await model.refreshSecrets()
        dismiss()
    }
}

// MARK: - Connected vaults (1Password, Phase 2)

struct ConnectVaultSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    @State private var token = ""
    @State private var note = ""
    @State private var picked: Set<Id> = []
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        SheetFrame(title: "Connect 1Password", action: "Connect", canSave: token.trimmingCharacters(in: .whitespaces).count > 20, busy: busy, save: { Task { await connect() } }) {
            step(1, "In 1Password, create a service account with read access to one vault, e.g. \"Arbos — Research\".") {
                SwiftUI.Link("Open 1password.com", destination: URL(string: "https://my.1password.com/developer-tools/infrastructure-secrets/serviceaccount/")!)
                    .font(theme.font(Theme.small, weight: .medium)).foregroundStyle(Color(hex: "#1084FE")!)
            }
            step(2, "Paste its token") {
                SecretTextField(placeholder: "ops_••••••••••••••••••••", text: $token, secure: true, mono: true)
                Text("Stored encrypted. Arbos never shows it again.").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
            }
            SecretField(label: "Agents that may use this vault") { AgentPickerField(picked: $picked) }
            SecretField(label: "Note", hint: SecretsText.vaultNoteHint) { SecretTextField(placeholder: "read-only · confirm before charging", text: $note, capitalization: .sentences) }
            if let error { Text(error).font(theme.font(Theme.small)).foregroundStyle(theme.danger) }
        }
    }

    private func step<C: View>(_ n: Int, _ text: String, @ViewBuilder _ content: () -> C) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Text("\(n)").font(theme.font(14, weight: .semibold)).foregroundStyle(theme.onPrimary)
                .frame(width: 24, height: 24).background(Circle().fill(theme.primary))
            VStack(alignment: .leading, spacing: 8) {
                Text(text).font(theme.font(Theme.body)).foregroundStyle(theme.text)
                content()
            }
        }
    }

    private func connect() async {
        busy = true
        error = nil
        defer { busy = false }
        do {
            _ = try await model.relay.connectVault(provider: "onepassword", token: token.trimmingCharacters(in: .whitespacesAndNewlines), name: nil,
                                                   agents: picked.map { ["project_id": $0, "note": note] })
            token = ""
            await model.refreshSecrets()
            dismiss()
        } catch let e as RelayError {
            switch e.type {
            case "bad_token": error = "That token didn't work. Check it was copied whole."
            case "no_vaults": error = "This token can read no vaults."
            case "rate_limited": error = "1Password rate limit reached — try again later."
            default: error = e.message
            }
        } catch { self.error = error.localizedDescription }
    }
}

struct VaultPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let vaultId: String
    @State private var items: [[String: Any]] = []
    @State private var activity: [SecretActivity] = []
    @State private var search = ""
    @State private var showToken = false
    @State private var token = ""
    @State private var confirmDisconnect = false
    @State private var showGive = false
    @State private var busy = false

    private var vault: ConnectedVault? { model.secrets?.vaults.first { $0.id == vaultId } }
    private var filtered: [[String: Any]] {
        PeoplePicker.titled(items, query: search) { ($0["title"] as? String) ?? "" }
    }

    var body: some View {
        SettingsPage(title: vault.map { "\($0.providerLabel) · \($0.name)" } ?? "Vault") {
            if let v = vault {
                SettingsGroup {
                    SettingsValueRow("Items", value: "\(v.itemsCount)")
                    SettingsValueRow("Token expires", value: v.tokenExpiresAt.map { SecretsText.shortDate($0) } ?? "Never")
                    if let used = v.readsUsedHour, let cap = v.readsPerHour {
                        SettingsValueRow("Reads this hour", value: "\(used) / \(cap)")
                    }
                    if v.status != "ok" {
                        SettingsValueRow("Status", value: v.status.replacingOccurrences(of: "_", with: " "))
                    }
                }
                SettingsGroup(header: "Agents", footer: "An agent with access may use any item in this vault by reference; the first use at a new host asks you.") {
                    ForEach(v.agents) { a in
                        // The agent's own D79 avatar, as on the Secret page.
                        HStack(spacing: 14) {
                            if let p = model.project(a.projectId) {
                                AgentAvatar(project: p, theme: theme, size: SettingsRow.tile, still: true)
                            } else {
                                InitialsAvatar(name: a.name, handle: a.name, id: a.projectId, override: nil, size: SettingsRow.tile)
                            }
                            VStack(alignment: .leading, spacing: 2) {
                                Text(a.name).font(theme.font(Theme.body)).foregroundStyle(theme.text)
                                Text(a.note.isEmpty ? SecretsText.usage(lastUsedAt: a.lastUsedAt, disabledAt: a.disabledAt, enabled: a.enabled) : a.note)
                                    .font(theme.font(Theme.tiny)).foregroundStyle(theme.muted).lineLimit(2)
                            }
                            Spacer(minLength: 8)
                            Toggle("", isOn: Binding(get: { a.enabled }, set: { model.setVaultAccess(vaultId: v.id, projectId: a.projectId, enabled: $0) }))
                                .labelsHidden().tint(theme.online).disabled(model.isFixture)
                                .accessibilityLabel("\(a.name) access")
                        }
                        .padding(.horizontal, 16)
                        .frame(minHeight: 56)
                        .contentShape(Rectangle())
                    }
                    SecretAddRow(title: "Give another agent access…") { showGive = true }
                }
                SettingsGroup(header: "Items") {
                    TextField("", text: $search, prompt: Text("Search items").foregroundStyle(theme.placeholder))
                        .font(theme.font(Theme.body)).foregroundStyle(theme.text)
                        .padding(.horizontal, 16).frame(minHeight: SettingsRow.height)
                    ForEach(Array(filtered.prefix(50).enumerated()), id: \.offset) { _, it in
                        let on = (it["inject_env"] as? Bool) ?? false
                        let env = (it["env"] as? String) ?? ""
                        HStack(spacing: 12) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text((it["title"] as? String) ?? "Item").font(theme.font(Theme.body)).foregroundStyle(theme.text)
                                Text(on ? "Real value as $\(env) in the agents' environment" : ((it["fields"] as? [String]) ?? []).joined(separator: " · "))
                                    .font(theme.font(Theme.tiny)).foregroundStyle(theme.muted).lineLimit(2)
                            }
                            Spacer(minLength: 8)
                            Toggle("", isOn: Binding(get: { on }, set: { setItemEnv(it, on: $0) }))
                                .labelsHidden().tint(theme.online).disabled(model.isFixture || busy)
                                .accessibilityLabel("Real value of \((it["title"] as? String) ?? "item") in the environment")
                        }
                        .padding(.horizontal, 16).padding(.vertical, 9)
                    }
                    if items.isEmpty {
                        Text(busy ? "Loading…" : "No items listed yet.").font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                            .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 16).padding(.vertical, 12)
                    }
                }
                SettingsGroup(header: "Activity") {
                    if activity.isEmpty {
                        Text("No activity in the last 30 days.").font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                            .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 16).padding(.vertical, 12)
                    } else {
                        ActivityRows(items: Array(activity.prefix(5)))
                        NavigationLink { SecretsActivityPage(vaultId: v.id, title: v.name) } label: {
                            SettingsValueRow("See all") { Image(systemName: "chevron.right").font(.system(size: 14, weight: .semibold)).foregroundStyle(theme.placeholder) }
                        }
                    }
                }
                SettingsGroup {
                    SettingsActionRow(title: "Replace token…") { showToken = true }
                }
                SettingsGroup(footer: "Also revoke the service account in 1Password.") {
                    SettingsActionRow(title: "Disconnect", destructive: true) { confirmDisconnect = true }
                }
            }
        }
        .task { await load() }
        .confirmationDialog("Disconnect this vault?", isPresented: $confirmDisconnect, titleVisibility: .visible) {
            Button("Disconnect", role: .destructive) { model.deleteVault(vaultId); dismiss() }
        } message: {
            Text("\(agentsLine) lose every item in it. Also revoke the service account in 1Password.")
        }
        .alert("Replace token", isPresented: $showToken) {
            SecureField("ops_…", text: $token)
            Button("Replace") {
                let t = token; token = ""
                Task { await model.secretsAction("Replace token") { try await model.relay.replaceVaultToken(vaultId, token: t) }; await load() }
            }
            Button("Cancel", role: .cancel) { token = "" }
        } message: { Text("Stored encrypted. Arbos never shows it again.") }
        .sheet(isPresented: $showGive) {
            NavigationStack { GiveVaultAccessSheet(vaultId: vaultId) }
                .environmentObject(model).environment(\.theme, theme)
                .presentationDetents([.medium, .large])
                .presentationBackground(theme.bg).preferredColorScheme(theme.isDark ? .dark : .light)
        }
    }

    private var agentsLine: String {
        let names = vault?.agents.filter(\.enabled).map(\.name) ?? []
        return names.isEmpty ? "Agents" : names.joined(separator: " and ")
    }

    private func load() async {
        if model.isFixture {
            items = model.fixtureVaultItems
            activity = model.secrets?.activity ?? []
            return
        }
        busy = true
        defer { busy = false }
        if let (v, list, acts) = try? await model.relay.vault(vaultId) {
            items = list
            activity = acts
            if let i = model.secrets?.vaults.firstIndex(where: { $0.id == v.id }) { model.secrets?.vaults[i] = v }
        }
    }

    /// One item's real value into (or out of) the environment of every agent with access.
    private func setItemEnv(_ it: [String: Any], on: Bool) {
        guard let ref = it["ref"] as? String else { return }
        Task {
            busy = true
            defer { busy = false }
            do {
                items = try await model.relay.setVaultItemEnv(vaultId, ref: ref, on: on)
                await model.refreshSecrets()
            } catch let e as RelayError {
                model.notice = Notice(text: "Real value: \(e.message)")
            } catch {
                model.notice = Notice(text: "Real value: \(error.localizedDescription)")
            }
        }
    }
}

struct GiveVaultAccessSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let vaultId: String
    @State private var picked: Set<Id> = []
    @State private var note = ""
    @State private var busy = false

    var body: some View {
        SheetFrame(title: "Give access", action: "Save", canSave: !picked.isEmpty, busy: busy, save: { Task { await save() } }) {
            SecretField(label: "Agents") { AgentPickerField(picked: $picked) }
            SecretField(label: "Note", hint: SecretsText.vaultNoteHint) { SecretTextField(placeholder: "read-only · confirm before charging", text: $note, capitalization: .sentences) }
        }
        .onAppear { picked = Set(model.secrets?.vaults.first { $0.id == vaultId }?.agents.filter(\.enabled).map(\.projectId) ?? []) }
    }

    private func save() async {
        busy = true
        defer { busy = false }
        let before = Set(model.secrets?.vaults.first { $0.id == vaultId }?.agents.filter(\.enabled).map(\.projectId) ?? [])
        for pid in picked.subtracting(before) {
            _ = try? await model.relay.setVaultAccess(vaultId, projectId: pid, enabled: true, note: note.isEmpty ? nil : note)
        }
        // A chip removed with × takes the agent's access away, as on the secret's sheet.
        for pid in before.subtracting(picked) {
            _ = try? await model.relay.setVaultAccess(vaultId, projectId: pid, enabled: false, note: nil)
        }
        await model.refreshSecrets()
        dismiss()
    }
}
