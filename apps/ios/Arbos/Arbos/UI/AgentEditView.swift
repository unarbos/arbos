// Agent Edit page — Grok Bot's Edit Bot hierarchy ([feedback
// iphone-entity-profile-edit-2026-11-10]): glass back, white Done pill, centred
// avatar with a pencil badge that pushes the avatar picker, then sectioned
// cards for Name, Appearance, Model / Reasoning / Voice (and the label). Every
// sub-page is a further push; nothing here rises as a sheet.

import SwiftUI

struct AgentEditView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let projectId: Id

    @State private var name = ""
    @State private var label = ""
    @FocusState private var nameFocused: Bool
    @State private var showAvatar = false
    @State private var showModel = false
    @State private var showReasoning = false
    @State private var showVoice = false
    @State private var avatarTab: AvatarPickerTab = .bot

    private var project: Project? { model.project(projectId) }
    private var isMine: Bool { project?.shared != true }

    var body: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            VStack(spacing: 0) {
                EntityEditTopBar(title: "Edit Agent", doneTitle: "Done", onDone: saveAndPop)
                ScrollView(showsIndicators: false) {
                    if let project {
                        VStack(alignment: .leading, spacing: 22) {
                            EntityEditHero(onAvatar: isMine ? { showAvatar = true } : nil) {
                                AgentAvatar(project: project, theme: theme, size: PaneKit.heroAvatar)
                            }
                            .padding(.top, 12)
                            .id(project.effectiveAvatarColorIndex.map(String.init) ?? "auto")

                            VStack(alignment: .leading, spacing: 8) {
                                EntitySectionHeader(title: "Name")
                                PaneCard {
                                    EntityNameField(text: $name, placeholder: "Agent name", focused: $nameFocused,
                                                    onSubmit: commitName)
                                }
                                .padding(.horizontal, PaneKit.inset)
                                .disabled(!isMine)
                            }

                            VStack(alignment: .leading, spacing: 8) {
                                EntitySectionHeader(title: "Label")
                                PaneCard {
                                    TextField("", text: $label,
                                              prompt: Text(AgentPane.labelPlaceholder).foregroundStyle(theme.placeholder))
                                        .font(theme.font(Theme.body))
                                        .foregroundStyle(theme.text)
                                        .padding(.horizontal, PaneKit.inset)
                                        .frame(minHeight: PaneKit.rowHeight)
                                        .onChange(of: label) { _, v in
                                            if v.count > 500 { label = String(v.prefix(500)) }
                                        }
                                        .accessibilityIdentifier("agent-label-field")
                                }
                                .padding(.horizontal, PaneKit.inset)
                                .disabled(!isMine)
                            }

                            VStack(alignment: .leading, spacing: 8) {
                                EntitySectionHeader(title: "Appearance")
                                PaneCard {
                                    Button { if isMine { showAvatar = true } } label: {
                                        PaneRow(title: "Avatar", chevron: isMine)
                                    }
                                    .buttonStyle(.plain)
                                    .disabled(!isMine)
                                    .accessibilityIdentifier("edit-avatar-row")
                                }
                                .padding(.horizontal, PaneKit.inset)
                            }

                            if isMine {
                                VStack(alignment: .leading, spacing: 8) {
                                    EntitySectionHeader(title: "Voice Calls")
                                    PaneCard {
                                        Button { showModel = true } label: {
                                            EntityPickerRow(title: "Model", value: project.shortModel)
                                        }
                                        .buttonStyle(.plain)
                                        .accessibilityIdentifier("edit-model-row")
                                        Button { showReasoning = true } label: {
                                            EntityPickerRow(title: "Reasoning", value: project.effort.label)
                                        }
                                        .buttonStyle(.plain)
                                        .accessibilityIdentifier("edit-reasoning-row")
                                        Button { showVoice = true } label: {
                                            EntityPickerRow(title: "Voice", value: voiceLine(project))
                                        }
                                        .buttonStyle(.plain)
                                        .accessibilityIdentifier("edit-voice-row")
                                    }
                                    .padding(.horizontal, PaneKit.inset)
                                }
                            } else {
                                VStack(alignment: .leading, spacing: 8) {
                                    EntitySectionHeader(title: "Settings")
                                    PaneCard {
                                        PaneRow(title: "Model", value: project.shortModel)
                                    }
                                    .padding(.horizontal, PaneKit.inset)
                                }
                            }

                            Color.clear.frame(height: 40)
                        }
                    } else {
                        Text("This agent is gone.").foregroundStyle(theme.muted).padding(.top, 120)
                    }
                }
                .scrollDismissesKeyboard(.interactively)
            }
        }
        .toolbar(.hidden, for: .navigationBar)
        .backSwipeToPop()
        .accessibilityIdentifier("agent-edit")
        .onAppear {
            name = project?.name ?? ""
            label = project?.description ?? ""
        }
        .task { await model.loadVoices() }
        .navigationDestination(isPresented: $showAvatar) {
            AvatarPickerPage(target: .agent(projectId), initialTab: avatarTab)
                .backSwipeToPop()
        }
        .navigationDestination(isPresented: $showModel) {
            if let project = model.project(projectId) {
                ModelPickerPage(project: project).environmentObject(model).environment(\.theme, theme).backSwipeToPop()
            }
        }
        .navigationDestination(isPresented: $showReasoning) {
            if let project = model.project(projectId) {
                ReasoningPickerPage(project: project).environmentObject(model).environment(\.theme, theme).backSwipeToPop()
            }
        }
        .navigationDestination(isPresented: $showVoice) {
            if let project = model.project(projectId) {
                VoicePickerPage(project: project).environmentObject(model).environment(\.theme, theme).backSwipeToPop()
            }
        }
        .onAppear {
            if let show = DebugHooks.show, show.hasPrefix("avatarpicker") {
                avatarTab = show.hasSuffix("generate") ? .generate : (show.hasSuffix("upload") ? .upload : .bot)
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { showAvatar = true }
            }
            if DebugHooks.show == "agentvoice" {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { showVoice = true }
            }
            if let setting = model.pendingAgentSetting {
                model.pendingAgentSetting = nil
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                    switch setting {
                    case .model: showModel = true
                    case .reasoning: showReasoning = true
                    case .voice: showVoice = true
                    }
                }
            }
        }
    }

    private func voiceLine(_ p: Project) -> String {
        guard let id = p.voice else { return "Default" }
        return model.voiceCatalog?.voices.first { $0.id == id }?.name ?? id.capitalized
    }

    private func commitName() {
        let clean = name.trimmingCharacters(in: .whitespaces)
        guard let project, isMine, !clean.isEmpty, clean != project.name else { return }
        model.updateProject(projectId, name: clean)
    }

    private func saveAndPop() {
        commitName()
        let cleanLabel = label.trimmingCharacters(in: .whitespacesAndNewlines)
        if isMine, let project, cleanLabel != project.description {
            model.updateProject(projectId, description: cleanLabel)
        }
        dismiss()
    }
}

// MARK: - Push wrappers for pickers (sheets stay for chat chrome; Edit pushes)

/// Avatar picker as a full pushed page — same body as the sheet, without detents.
struct AvatarPickerPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let target: AvatarTarget
    var initialTab: AvatarPickerTab = .bot

    var body: some View {
        AvatarPickerSheet(target: target, initialTab: initialTab)
            .environmentObject(model)
            .environment(\.theme, theme)
            .toolbar(.hidden, for: .navigationBar)
            // presentationDetents is a no-op when not presented as a sheet.
            .accessibilityIdentifier("avatar-picker-page")
    }
}

/// Model picker pushed from Edit — strips the sheet NavigationStack chrome.
struct ModelPickerPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let project: Project

    var body: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            VStack(spacing: 0) {
                EntityEditTopBar(title: "Model", doneTitle: "Done", onDone: { dismiss() })
                ModelPickerView(project: project, pushed: true)
                    .environmentObject(model)
                    .environment(\.theme, theme)
            }
        }
        .toolbar(.hidden, for: .navigationBar)
        .accessibilityIdentifier("model-picker-page")
    }
}

struct ReasoningPickerPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let project: Project

    var body: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            VStack(spacing: 0) {
                EntityEditTopBar(title: "Reasoning", doneTitle: "Done", onDone: { dismiss() })
                ReasoningPickerView(project: project, pushed: true)
                    .environmentObject(model)
                    .environment(\.theme, theme)
            }
        }
        .toolbar(.hidden, for: .navigationBar)
        .accessibilityIdentifier("reasoning-picker-page")
    }
}

struct VoicePickerPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let project: Project

    var body: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            VStack(spacing: 0) {
                EntityEditTopBar(title: "Voice", doneTitle: "Done", onDone: { dismiss() })
                VoicePickerView(project: project, pushed: true)
                    .environmentObject(model)
                    .environment(\.theme, theme)
            }
        }
        .toolbar(.hidden, for: .navigationBar)
        .accessibilityIdentifier("voice-picker-page")
    }
}
