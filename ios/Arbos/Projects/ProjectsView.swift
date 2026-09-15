import SwiftUI

/// The first screen, in the shape of Cursor's agents list on the phone
/// (Jacob's reference, 2026-09-15) with the Arbos palette: round buttons
/// at the top, a large left title, collapsible "Working" and "Read"
/// sections, one row per project with a status glyph, the name, a
/// "Working · folder" line and the age at the right; a floating composer
/// at the bottom that talks to the open project.
struct ProjectsView: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var projects: ProjectStore
    @EnvironmentObject private var chat: ChatStore
    @Binding var path: NavigationPath

    @State private var showSettings = false
    @State private var showCall = false
    @State private var searching = false
    @State private var query = ""
    @State private var liveOnly = false
    @State private var workingOpen = true
    @State private var readOpen = true
    @State private var draft = ""
    @FocusState private var searchFocus: Bool

    var body: some View {
        ZStack(alignment: .bottom) {
            ArbosTheme.bg.ignoresSafeArea()
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    topBar
                    Text("Projects")
                        .font(ArbosTheme.title)
                        .foregroundStyle(ArbosTheme.text)
                        .padding(.horizontal, ArbosTheme.gutter)
                        .padding(.top, 0)
                        .padding(.bottom, 22)
                    if searching { searchField }
                    if !working.isEmpty {
                        section("Working", open: $workingOpen, rows: working)
                    }
                    section("Read", open: $readOpen, rows: read)
                    if projects.entries.isEmpty { emptyState }
                    Color.clear.frame(height: 90)
                }
            }
            .refreshable { await projects.refresh() }
            composer
        }
        .toolbar(.hidden, for: .navigationBar)
        .sheet(isPresented: $showSettings, onDismiss: { Task { await projects.refresh() } }) {
            SettingsView().environmentObject(settings)
        }
        .fullScreenCover(isPresented: $showCall) { CallScreen() }
        .task { await projects.refresh() }
        #if DEBUG
        // `-previewCall 1` on the launch line: straight into the call, for
        // the scripted voice measurements (see `CallView.previewIfAsked`).
        .onAppear { if UserDefaults.standard.bool(forKey: "previewCall") { showCall = true } }
        #endif
    }

    // MARK: - Rows

    private var visible: [ProjectEntry] {
        projects.entries.filter { entry in
            (!liveOnly || entry.live)
                && (query.isEmpty || entry.title.localizedCaseInsensitiveContains(query)
                    || entry.folder.localizedCaseInsensitiveContains(query))
        }
    }

    /// The open project counts as working while any of its agents runs;
    /// the others have no live link and rest in "Read".
    private var working: [ProjectEntry] { visible.filter(isWorking) }
    private var read: [ProjectEntry] { visible.filter { !isWorking($0) } }

    private func isWorking(_ entry: ProjectEntry) -> Bool {
        entry.target == settings.kernelTarget && chat.mode == .live && (chat.busy || chat.running > 0)
    }

    private func section(_ title: String, open: Binding<Bool>, rows: [ProjectEntry]) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.easeInOut(duration: 0.2)) { open.wrappedValue.toggle() }
            } label: {
                HStack(spacing: 6) {
                    Text(title)
                    Image(systemName: "chevron.down")
                        .font(.system(size: 12, weight: .medium))
                        .rotationEffect(.degrees(open.wrappedValue ? 0 : -90))
                }
                .font(ArbosTheme.body)
                .foregroundStyle(ArbosTheme.textFaint)
                .padding(.horizontal, ArbosTheme.gutter)
                .padding(.top, 8)
                .padding(.bottom, 10)
            }
            .buttonStyle(.plain)
            if open.wrappedValue {
                ForEach(rows) { entry in
                    Button {
                        path.append(entry.target)
                    } label: {
                        ProjectRow(entry: entry, working: isWorking(entry), step: step(for: entry), nameShared: isShared(entry))
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .padding(.bottom, 28)
    }

    /// Two projects with one name need the machine to tell them apart.
    private func isShared(_ entry: ProjectEntry) -> Bool {
        projects.entries.filter { $0.title.caseInsensitiveCompare(entry.title) == .orderedSame }.count > 1
    }

    private func step(for entry: ProjectEntry) -> String? {
        guard isWorking(entry) else { return nil }
        if let worker = chat.workers.first(where: { $0.running && !$0.step.isEmpty }) { return worker.step }
        return chat.step.isEmpty ? nil : chat.step
    }

    // MARK: - Chrome

    /// Round buttons on the desktop's raised plate: settings at the left
    /// where the reference has its back chevron; search and filter right.
    private var topBar: some View {
        HStack {
            RoundButton(symbol: "gearshape") { showSettings = true }
            Spacer()
            RoundButton(symbol: "magnifyingglass") {
                withAnimation(.easeInOut(duration: 0.2)) { searching.toggle() }
                searchFocus = searching
                if !searching { query = "" }
            }
            Menu {
                Picker("Show", selection: $liveOnly) {
                    Text("All projects").tag(false)
                    Text("Live only").tag(true)
                }
            } label: {
                RoundButton(symbol: "line.3.horizontal.decrease") {}
                    .allowsHitTesting(false)
            }
        }
        .padding(.horizontal, ArbosTheme.gutter)
        .padding(.top, 4)
        .padding(.bottom, 2)
    }

    private var searchField: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .foregroundStyle(ArbosTheme.textDim)
            TextField("Search projects", text: $query)
                .font(ArbosTheme.body)
                .foregroundStyle(ArbosTheme.text)
                .focused($searchFocus)
                .autocorrectionDisabled()
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(
            RoundedRectangle(cornerRadius: ArbosTheme.promptRadius, style: .continuous)
                .fill(ArbosTheme.inputBg)
        )
        .padding(.horizontal, ArbosTheme.gutter)
        .padding(.bottom, 8)
    }

    private var emptyState: some View {
        Group {
            if projects.loading {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small).tint(ArbosTheme.textFaint)
                    Text("Asking the hub…")
                }
            } else {
                Text(projects.problem ?? "No projects yet.")
            }
        }
        .font(ArbosTheme.callout)
        .foregroundStyle(ArbosTheme.textFaint)
        .padding(.horizontal, ArbosTheme.gutter)
        .padding(.top, 8)
    }

    /// The reference's bottom composer: words typed here go to the
    /// project last open and its chat opens; the mic is the call.
    private var composer: some View {
        ComposerBar(
            text: $draft,
            placeholder: "Plan, ask, build…",
            canSend: !draft.trimmingCharacters(in: .whitespaces).isEmpty && settings.chatEndpoint != nil,
            onSend: {
                let text = draft
                draft = ""
                path.append(settings.kernelTarget)
                Task {
                    await chat.switchTarget(settings.kernelTarget)
                    chat.send(text)
                }
            },
            onMic: { showCall = true },
            micEnabled: settings.isConfigured
        )
    }
}

/// One project, as the reference draws an agent: its own glyph in its
/// own colour, its name, one status line beneath. The folder shows only
/// when it is not the name already; the machine only when another
/// project shares the name. Nothing is said twice.
struct ProjectRow: View {
    let entry: ProjectEntry
    var working = false
    var step: String?
    var nameShared = false

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            statusGlyph
                .frame(width: 20, height: 22)
            VStack(alignment: .leading, spacing: 6) {
                Text(entry.title)
                    .font(ArbosTheme.body)
                    .foregroundStyle(entry.live ? ArbosTheme.text : ArbosTheme.textMuted)
                    .lineLimit(1)
                HStack(spacing: 0) {
                    Text(stateWord)
                        .foregroundStyle(working ? ArbosTheme.textMuted : ArbosTheme.textFaint)
                    ForEach(details, id: \.self) { detail in
                        Text(" · ").foregroundStyle(ArbosTheme.textDim)
                        Text(detail).foregroundStyle(ArbosTheme.textFaint)
                    }
                }
                .font(ArbosTheme.callout)
                .lineLimit(1)
                .truncationMode(.middle)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, ArbosTheme.gutter)
        .padding(.vertical, 15)
        .contentShape(Rectangle())
        .overlay(alignment: .bottom) {
            Rectangle().fill(ArbosTheme.border).frame(height: 0.5)
                .padding(.leading, ArbosTheme.gutter + 28)
                .padding(.trailing, ArbosTheme.gutter)
        }
    }

    /// What follows the state: the folder if it says something the name
    /// does not, the machine if the name is shared.
    private var details: [String] {
        var out: [String] = []
        let folder = entry.place.isEmpty ? entry.folder : (entry.place as NSString).lastPathComponent
        if folder.caseInsensitiveCompare(entry.title) != .orderedSame { out.append(folder) }
        if nameShared { out.append(entry.machine) }
        return out
    }

    @ViewBuilder
    private var statusGlyph: some View {
        if working {
            BrailleSpinner(tint: entry.identity.tint)
                .font(.system(size: 15, design: .monospaced))
        } else {
            // Its own glyph in its own colour; dimmed when no kernel serves it.
            Image(systemName: entry.identity.symbol)
                .font(.system(size: 14, weight: .medium))
                .foregroundStyle(entry.identity.tint.opacity(entry.live ? 1 : 0.45))
        }
    }

    private var stateWord: String {
        if working { return step.map { "Working · \($0)" } ?? "Working" }
        return entry.live ? "Idle" : "Off"
    }
}

/// A 44 pt round button on the raised plate — the reference's back,
/// search, filter and menu buttons.
struct RoundButton: View {
    let symbol: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 17, weight: .semibold))
                .foregroundStyle(ArbosTheme.text)
                .frame(width: 44, height: 44)
                .background(Circle().fill(ArbosTheme.raised))
                .overlay(Circle().strokeBorder(ArbosTheme.border, lineWidth: 1))
        }
        .buttonStyle(.plain)
    }
}

/// The reference's composer: a floating pill with a `+` disc at the left,
/// the field, and the mic at the right. `+` is for attachments (a later
/// cycle) and sits dim until then; the mic opens the call.
struct ComposerBar: View {
    @Binding var text: String
    let placeholder: String
    let canSend: Bool
    let onSend: () -> Void
    let onMic: () -> Void
    var micEnabled = true
    var focus: FocusState<Bool>.Binding?

    var body: some View {
        HStack(alignment: .bottom, spacing: 10) {
            Image(systemName: "plus")
                .font(.system(size: 17, weight: .regular))
                .foregroundStyle(ArbosTheme.textMuted)
                .frame(width: 28, height: 28)
                .background(Circle().fill(ArbosTheme.raisedHover))
                .padding(.bottom, 1)
            Group {
                if let focus {
                    TextField(placeholder, text: $text, axis: .vertical).focused(focus)
                } else {
                    TextField(placeholder, text: $text, axis: .vertical)
                }
            }
            .font(ArbosTheme.body)
            .lineLimit(1...6)
            .foregroundStyle(ArbosTheme.text)
            .tint(ArbosTheme.accent)
            .padding(.vertical, 4)
            .submitLabel(.send)
            .onSubmit { if canSend { onSend() } }
            if canSend {
                Button(action: onSend) {
                    Image(systemName: "arrow.up")
                        .font(.system(size: 14, weight: .bold))
                        .foregroundStyle(Color.black)
                        .frame(width: 28, height: 28)
                        .background(Circle().fill(ArbosTheme.text))
                        .padding(.bottom, 1)
                }
                .buttonStyle(.plain)
            } else {
                Button(action: onMic) {
                    Image(systemName: "mic.fill")
                        .font(.system(size: 18, weight: .medium))
                        .foregroundStyle(micEnabled ? ArbosTheme.text : ArbosTheme.textDim)
                        .frame(width: 28, height: 28)
                        .padding(.bottom, 1)
                }
                .buttonStyle(.plain)
                .disabled(!micEnabled)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 9)
        .background(
            RoundedRectangle(cornerRadius: 24, style: .continuous)
                .fill(ArbosTheme.inputBg)
                .overlay(
                    RoundedRectangle(cornerRadius: 24, style: .continuous)
                        .strokeBorder(ArbosTheme.border, lineWidth: 1)
                )
                .shadow(color: .black.opacity(0.35), radius: 12, y: 4)
        )
        .padding(.horizontal, ArbosTheme.barMargin)
        .padding(.bottom, 10)
    }
}
