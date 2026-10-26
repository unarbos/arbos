// One agent picker for every Secrets form ([feedback secrets-agent-picker-ios-2026-10-26],
// Jacob 2026-10-26): the agents already granted as removable chips (picture · name · ×) and
// one "Add agent" button that opens a sheet with a search field (type to filter by name or
// @handle) over the list of the agents not yet granted; tapping a row adds that one agent as
// a chip and closes the sheet — one agent per visit, the Mac's one-pick behaviour. Before
// this every agent was a toggle chip, so a user with 16 agents saw a wall of chips. The
// filter is the shared matcher ([feedback shared-search-matcher-ios-2026-10-26]).
//
// Used by New secret, Connect 1Password, "Give access" on a secret and "Give access" on a
// vault (`AgentPickerField` in `SecretsViews.swift`); the Mac has the same shape
// (`ui::agent_picker`). The pure parts (`AgentPickerMatch`) are what `AgentPickerTests` pins.

import SwiftUI

/// The pure half: which agents the sheet offers for a query — through the shared matcher
/// every picker uses (`PeoplePicker.agents` → `EntitySearch`, the core matcher over the
/// FFI: name, @slug and description; exact, prefix, word-start; case, diacritics and a
/// leading `@` folded; best first, registry order in a tie). No filter of its own.
enum AgentPickerMatch {
    /// My own agents (never a shared-in one), not already granted (`picked` — the chips — or
    /// `exclude` — access the page already lists), narrowed and ranked by the matcher.
    static func candidates(_ projects: [Project], picked: Set<Id>, exclude: Set<Id>, query: String) -> [Project] {
        let pool = projects.filter { !$0.shared && !picked.contains($0.id) && !exclude.contains($0.id) }
        return PeoplePicker.agents(pool, query: query)
    }

    /// The chips, in registry order (a chip for an id the registry no longer lists is skipped).
    static func chips(_ projects: [Project], picked: Set<Id>) -> [Project] {
        projects.filter { picked.contains($0.id) }
    }
}

/// Chips of the granted agents + "Add agent". `picked` is the form's grant set; `exclude`
/// hides agents the page already shows with access (nothing on the forms that create).
struct AgentPickerField: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Binding var picked: Set<Id>
    var exclude: Set<Id> = []
    @State private var adding = false

    static let chipHeight: CGFloat = 34
    static let picture: CGFloat = 20

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            WrapLayout(spacing: 8) {
                ForEach(AgentPickerMatch.chips(model.projects, picked: picked)) { p in
                    chip(p)
                }
                addButton
            }
            .padding(.horizontal, 1)
            if picked.isEmpty {
                Text("No agent can use it yet.").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
            }
        }
        .sheet(isPresented: $adding) {
            NavigationStack {
                AddAgentSheet(picked: picked, exclude: exclude) { id in
                    Haptics.pick()
                    picked.insert(id)
                }
            }
        }
        .onAppear(perform: applyRig)
    }

    /// Rig (shots): `ARBOS_RIG_AGENT_PICKED` names start as chips; `ARBOS_RIG_AGENT_PICKER`
    /// opens the sheet (its query is typed by the sheet).
    private func applyRig() {
        let names = DebugHooks.agentPickerPicked
        if !names.isEmpty {
            for p in model.projects where names.contains(where: { $0.caseInsensitiveCompare(p.name) == .orderedSame }) {
                picked.insert(p.id)
            }
        }
        if DebugHooks.agentPickerQuery != nil, !adding {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { adding = true }
        }
    }

    /// The agent's current picture through the shared resolver (the newest copy the model
    /// holds — my record or a roster's), never the leaf of a stale copy.
    private func resolved(_ p: Project) -> Project {
        model.agentProject(model.agentIdentity(sender: p.id, handle: mentionSlug(p.name), name: p.name, chatId: nil), chatId: nil)
    }

    private func chip(_ p: Project) -> some View {
        HStack(spacing: 7) {
            AgentAvatar(project: resolved(p), theme: theme, size: Self.picture, still: true)
            Text(p.name).font(theme.font(Theme.small, weight: .medium)).lineLimit(1)
            Button {
                Haptics.pick()
                picked.remove(p.id)
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(theme.muted)
                    .frame(width: 22, height: 22)
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Remove \(p.name)")
        }
        .foregroundStyle(theme.text)
        .padding(.leading, 6)
        .padding(.trailing, 4)
        .frame(height: Self.chipHeight)
        .background(Capsule().fill(theme.field)) // control-size: exception inlineChips
        .overlay(Capsule().strokeBorder(theme.fieldBorder, lineWidth: 1)) // control-size: exception inlineChips
        .accessibilityIdentifier("agent-picker-chip-\(p.id)")
    }

    private var addButton: some View {
        Button {
            adding = true
        } label: {
            HStack(spacing: 7) {
                Image(systemName: "plus").font(.system(size: 14, weight: .semibold))
                Text("Add agent").font(theme.font(Theme.small, weight: .semibold))
            }
            .foregroundStyle(theme.text)
            .padding(.leading, 11)
            .padding(.trailing, 13)
            .frame(height: Self.chipHeight)
            .background(Capsule().fill(theme.bubbleAssistant)) // control-size: exception inlineChips
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("agent-picker-add")
    }
}

/// The "Add agent" sheet: a search field of the control family (`.controlField()`, the
/// composer's 48 pt — the iPhone control-size token) focused with the keyboard up, then
/// the agents not yet granted. A tap adds one and closes.
struct AddAgentSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let picked: Set<Id>
    let exclude: Set<Id>
    let add: (Id) -> Void
    @State private var query = ""
    @FocusState private var searching: Bool

    private var rows: [Project] {
        AgentPickerMatch.candidates(model.projects, picked: picked, exclude: exclude, query: query)
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                searchField
                if rows.isEmpty {
                    Text(query.trimmingCharacters(in: .whitespaces).isEmpty
                         ? "Every agent has access."
                         : "No agent matches “\(query.trimmingCharacters(in: .whitespaces))”.")
                        .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                        .padding(.horizontal, 4)
                } else {
                    PaneCard(header: "Your agents", separatorInset: PaneKit.separatorInset(.avatar(AnyView(EmptyView())))) {
                        ForEach(rows) { p in
                            Button {
                                add(p.id)
                                dismiss()
                            } label: {
                                PaneRow(
                                    leading: .avatar(AnyView(AgentAvatar(project: resolved(p), theme: theme, size: PaneKit.avatar, still: true))),
                                    title: p.name,
                                    subtitle: "@\(mentionSlug(p.name))",
                                    trailing: AnyView(
                                        Image(systemName: "plus")
                                            .font(.system(size: 13, weight: .semibold))
                                            .foregroundStyle(theme.text)
                                            .frame(width: 28, height: 28)
                                            .background(Circle().fill(theme.bubbleUser.opacity(0.35))) // control-size: exception inlineChips
                                    )
                                )
                            }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("agent-picker-row-\(p.id)")
                        }
                    }
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 12)
            .padding(.bottom, 32)
        }
        .scrollDismissesKeyboard(.interactively)
        .background(theme.bg)
        .navigationTitle("Add agent")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button("Cancel") { dismiss() }.foregroundStyle(theme.text)
            }
        }
        .onAppear {
            if let q = DebugHooks.agentPickerQuery, !q.isEmpty, query.isEmpty { query = q }
            searching = true
        }
    }

    private func resolved(_ p: Project) -> Project {
        model.agentProject(model.agentIdentity(sender: p.id, handle: mentionSlug(p.name), name: p.name, chatId: nil), chatId: nil)
    }

    private var searchField: some View {
        HStack(spacing: 9) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 17, weight: .medium))
                .foregroundStyle(theme.muted)
            TextField("", text: $query, prompt: Text("Search agents").foregroundStyle(theme.placeholder))
                .font(theme.font(Theme.body))
                .foregroundStyle(theme.text)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.search)
                .focused($searching)
                .onSubmit {
                    // ↩ on the keyboard adds the first match, as the Mac's ↩ adds the highlighted row.
                    if let first = rows.first {
                        add(first.id)
                        dismiss()
                    }
                }
            if !query.isEmpty {
                Button {
                    query = ""
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 17))
                        .foregroundStyle(theme.muted)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
            }
        }
        // The iPhone control family's field (`Controls.height` 48, the composer's), as Jacob
        // asked for this new screen (2026-10-26) — not the secrets forms' old 42.
        .controlField()
        .accessibilityIdentifier("agent-picker-search")
    }
}
