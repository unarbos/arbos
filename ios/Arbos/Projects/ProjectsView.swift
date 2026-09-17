import PhotosUI
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
        ZStack {
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
                    // An empty "Read" header over nothing reads as a section
                    // somebody collapsed, not as "nothing matched", so it
                    // stands down when a filter empties the list.
                    if !read.isEmpty || projects.entries.isEmpty {
                        section("Read", open: $readOpen, rows: read)
                    }
                    if projects.entries.isEmpty {
                        emptyState
                    } else if visible.isEmpty {
                        nothingMatches
                    }
                    Color.clear.frame(height: 90)
                }
            }
            .refreshable { await projects.refresh() }
            // A bottom inset, as in the chat: the list ends above the
            // composer and the composer rides up with the keyboard
            // (Jacob, build 956: "can't see the chat box").
            .safeAreaInset(edge: .bottom, spacing: 0) { composer }
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
                        projects.remember(opened: entry.target)
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

    /// A search or a filter that matches nothing. Without this the screen
    /// went blank under the search box and said nothing at all, which reads
    /// as a list still loading rather than an answer.
    private var nothingMatches: some View {
        Text(query.isEmpty
             ? "No project is live. Turn the filter off to see the rest."
             : "No project matches “\(query)”.")
            .font(ArbosTheme.callout)
            .foregroundStyle(ArbosTheme.textFaint)
            .padding(.horizontal, ArbosTheme.gutter)
            .padding(.top, 8)
    }

    /// The reference's bottom composer: words typed here go to the
    /// project last open and its chat opens; the mic is the call.
    /// Where a line typed on the list goes: the last project, unless it is
    /// not among the rows on screen — then the first that is.
    ///
    /// The rows on screen, not the whole roster. Search for one project and
    /// the composer went on naming the last one opened, which is not in
    /// front of you and is not what "Message …" beside a filtered list
    /// means. Typing into a list showing one project and having the line go
    /// to another is the same complaint Jacob opened with on build 956.
    private var composerTarget: KernelTarget? {
        let onScreen = visible.map(\.target)
        if onScreen.contains(settings.kernelTarget) { return settings.kernelTarget }
        return onScreen.first
    }

    private var composer: some View {
        // The placeholder names the project, so a line typed here is never a
        // mystery (Jacob, build 956: "where does this chat go?").
        let name = composerTarget.flatMap { t in projects.entries.first { $0.target == t }?.title }
        return ComposerBar(
            text: $draft,
            placeholder: name.map { "Message \($0)…" } ?? "Plan, ask, build…",
            canSend: !draft.trimmingCharacters(in: .whitespaces).isEmpty && composerTarget != nil,
            onSend: {
                guard let target = composerTarget else { return }
                let text = draft
                draft = ""
                projects.remember(opened: target)
                path.append(target)
                Task {
                    await chat.switchTarget(target)
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
        // It answers, so "Off" would be a lie, and "Idle" would hide that
        // every worker it is asked for will be refused.
        if entry.needsRestart { return "Restart needed" }
        // The roster has stopped listing it. "Off" is true but says nothing
        // about which thing is off, and that is the only question worth
        // answering here: his machine, or that project's kernel.
        if !entry.waitingOn.isEmpty { return entry.waitingOn }
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

/// The reference's composer: a floating pill with a `+` at the left (a
/// menu: photo library, files), the field, and the mic at the right — the
/// mic dictates into the field through the speech server; `onMic` is the
/// call, used where the bar has no dictation (the list). Picked files sit
/// above the field as chips until the send.
struct ComposerBar: View {
    @Binding var text: String
    let placeholder: String
    let canSend: Bool
    let onSend: () -> Void
    let onMic: () -> Void
    var micEnabled = true
    var focus: FocusState<Bool>.Binding?
    var attachments: Binding<[PendingAttachment]>?
    var dictation: Dictation?
    /// A turn is running: with nothing typed, the right button is Stop.
    var busy = false
    var onStop: (() -> Void)?
    @State private var photoItems: [PhotosPickerItem] = []
    @State private var showFiles = false
    @State private var showPhotos = false
    @EnvironmentObject private var settings: AppSettings

    private var dictating: Bool { dictation?.active ?? false }

    var body: some View {
        VStack(spacing: 0) {
            if let attachments, !attachments.wrappedValue.isEmpty {
                AttachmentChips(attachments: attachments)
            }
            HStack(alignment: .bottom, spacing: 10) {
                plusButton
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
                if dictating {
                    Button {
                        dictation?.stop()
                    } label: {
                        ZStack {
                            Circle().fill(ArbosTheme.accent.opacity(0.25 + 0.6 * Double(dictation?.level ?? 0)))
                            Image(systemName: "stop.fill")
                                .font(.system(size: 12, weight: .bold))
                                .foregroundStyle(ArbosTheme.text)
                        }
                        .frame(width: 28, height: 28)
                        .padding(.bottom, 1)
                    }
                    .buttonStyle(.plain)
                } else if canSend {
                    Button(action: onSend) {
                        Image(systemName: "arrow.up")
                            .font(.system(size: 14, weight: .bold))
                            .foregroundStyle(Color.black)
                            .frame(width: 28, height: 28)
                            .background(Circle().fill(ArbosTheme.text))
                            .padding(.bottom, 1)
                    }
                    .buttonStyle(.plain)
                } else if busy, let onStop {
                    // The kernel's stall line says "Stop ends the turn"; the
                    // phone had no Stop (M-130). As Cursor's: the send disc
                    // becomes a stop square while the agent works.
                    Button(action: onStop) {
                        Image(systemName: "stop.fill")
                            .font(.system(size: 12, weight: .bold))
                            .foregroundStyle(Color.black)
                            .frame(width: 28, height: 28)
                            .background(Circle().fill(ArbosTheme.text))
                            .padding(.bottom, 1)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Stop")
                } else {
                    Button {
                        if let dictation { dictation.start(settings: settings) } else { onMic() }
                    } label: {
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
        }
        .background(
            RoundedRectangle(cornerRadius: 24, style: .continuous)
                .fill(ArbosTheme.inputBg)
                .overlay(
                    RoundedRectangle(cornerRadius: 24, style: .continuous)
                        .strokeBorder(dictating ? ArbosTheme.accent.opacity(0.6) : ArbosTheme.border, lineWidth: 1)
                )
                .shadow(color: .black.opacity(0.35), radius: 12, y: 4)
        )
        .padding(.horizontal, ArbosTheme.barMargin)
        .padding(.bottom, 10)
        .photosPicker(isPresented: $showPhotos, selection: $photoItems, maxSelectionCount: 4, matching: .images)
        .onChange(of: photoItems) { _, items in
            guard !items.isEmpty, let attachments else { return }
            Task {
                for item in items {
                    if let file = await PendingAttachment.photo(item) { attachments.wrappedValue.append(file) }
                }
                photoItems = []
            }
        }
        .fileImporter(isPresented: $showFiles, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
            guard let attachments, case .success(let urls) = result else { return }
            for url in urls {
                if let file = PendingAttachment.file(url) { attachments.wrappedValue.append(file) }
            }
        }
    }

    /// `+`: the picker menu where the bar takes attachments; dim where not.
    @ViewBuilder
    private var plusButton: some View {
        if attachments != nil {
            Menu {
                Button {
                    showPhotos = true
                } label: {
                    Label("Photo Library", systemImage: "photo.on.rectangle")
                }
                Button {
                    showFiles = true
                } label: {
                    Label("Files", systemImage: "folder")
                }
            } label: {
                Image(systemName: "plus")
                    .font(.system(size: 17, weight: .regular))
                    .foregroundStyle(ArbosTheme.textMuted)
                    .frame(width: 28, height: 28)
                    .background(Circle().fill(ArbosTheme.raisedHover))
                    .padding(.bottom, 1)
            }
        } else {
            Image(systemName: "plus")
                .font(.system(size: 17, weight: .regular))
                .foregroundStyle(ArbosTheme.textDim)
                .frame(width: 28, height: 28)
                .background(Circle().fill(ArbosTheme.raisedHover))
                .padding(.bottom, 1)
        }
    }
}
