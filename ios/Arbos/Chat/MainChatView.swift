import SwiftUI

/// A project's main chat, in the shape of Cursor's chat page on the
/// phone (Jacob's reference, 2026-09-15) with the Arbos palette: a round
/// back chevron, the project's name centred, a round ⋯ menu; prose with
/// a "Worked Ns" line after each turn; a pill above the composer that
/// opens the workers; the composer with `+`, "Follow up…" and the mic,
/// which is the call.
struct ProjectChatView: View {
    let target: KernelTarget
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var chat: ChatStore
    @EnvironmentObject private var projects: ProjectStore
    @Environment(\.dismiss) private var dismiss

    @State private var draft = ""
    @State private var showCall = false
    @State private var showSettings = false
    @State private var showWorkers = false
    @State private var worker: WorkerStatus?
    @State private var attachments: [PendingAttachment] = []
    @StateObject private var dictation = Dictation()
    @EnvironmentObject private var notifier: Notifier
    /// The row SwiftUI keeps in place while content changes (paging back).
    @State private var heldRow: UUID?
    /// Growth at the bottom pins the view to the tail, until the user pages
    /// back — then the top is theirs until they send again.
    @State private var followGrowth = true
    @FocusState private var composing: Bool

    private var identity: ProjectIdentity {
        chat.identity ?? projects.identity(for: target, remote: true)
    }

    private var entry: ProjectEntry? {
        projects.entries.first { $0.target == target }
    }

    private var title: String {
        identity.label ?? entry?.folder ?? target.label
    }

    var body: some View {
        ZStack {
            ArbosTheme.bg.ignoresSafeArea()
            VStack(spacing: 0) {
                topBar
                transcript
            }
            // The composer is a bottom inset, not an overlay: the transcript
            // ends above it, so the tail is the tail (M-47).
            .safeAreaInset(edge: .bottom, spacing: 0) {
                VStack(spacing: 8) {
                    pills
                    ComposerBar(
                        text: $draft,
                        placeholder: dictation.active ? "Listening…" : (chat.items.isEmpty ? "Plan, ask, build…" : "Follow up…"),
                        canSend: canSend,
                        onSend: send,
                        onMic: { showCall = true },
                        micEnabled: settings.isConfigured,
                        focus: $composing,
                        attachments: $attachments,
                        dictation: dictation
                    )
                }
                // Scrolled text passes under the inset; the pill and the
                // composer sit on the background, not on the words.
                .background(
                    ArbosTheme.bg
                        .padding(.top, -14)
                        .mask(
                            LinearGradient(colors: [.clear, .black, .black], startPoint: .top, endPoint: UnitPoint(x: 0.5, y: 0.12))
                                .padding(.top, -14)
                        )
                        .ignoresSafeArea(edges: .bottom)
                )
            }
        }
        .toolbar(.hidden, for: .navigationBar)
        .fullScreenCover(isPresented: $showCall) { CallScreen() }
        .sheet(isPresented: $showSettings) { SettingsView().environmentObject(settings) }
        .sheet(isPresented: $showWorkers) {
            WorkersSheet(workers: chat.workers) { picked in
                showWorkers = false
                worker = picked
            }
            .presentationDetents([.medium, .large])
            .presentationBackground(ArbosTheme.surface)
        }
        .navigationDestination(item: $worker) { worker in
            WorkerChatView(worker: worker, project: identity)
        }
        .task(id: target) {
            await chat.switchTarget(target)
            notifier.requestIfNeeded()
        }
        .onChange(of: dictation.text) { _, words in
            if dictation.active || !words.isEmpty { draft = words }
        }
        .onChange(of: dictation.active) { _, active in
            if !active, !dictation.text.isEmpty { draft = dictation.consume() }
        }
        .onChange(of: dictation.problem) { _, problem in
            if let problem { chat.notice(problem) }
        }
        .onChange(of: chat.identity) { _, face in
            if let face { projects.remember(face, for: target) }
        }
    }

    // MARK: - Chrome

    private var topBar: some View {
        ZStack {
            HStack(spacing: 8) {
                ProjectGlyph(identity: identity, size: 22, working: chat.busy || chat.running > 0)
                Text(title)
                    .font(ArbosTheme.bodySemibold)
                    .foregroundStyle(ArbosTheme.text)
                    .lineLimit(1)
            }
            .frame(maxWidth: 220)
            HStack {
                RoundButton(symbol: "chevron.left") { dismiss() }
                Spacer()
                Menu {
                    Button {
                        showCall = true
                    } label: {
                        Label("Call \(title)", systemImage: "phone")
                    }
                    .disabled(!settings.isConfigured)
                    Button {
                        Task { await chat.reconnect() }
                    } label: {
                        Label("Reconnect", systemImage: "arrow.clockwise")
                    }
                    Button {
                        showSettings = true
                    } label: {
                        Label("Settings", systemImage: "gearshape")
                    }
                } label: {
                    RoundButton(symbol: "ellipsis") {}.allowsHitTesting(false)
                }
            }
        }
        .padding(.horizontal, ArbosTheme.gutter)
        .padding(.top, 4)
        .padding(.bottom, 12)
    }

    /// The reference's pill row above the composer: here, the workers.
    @ViewBuilder
    private var pills: some View {
        if !chat.workers.isEmpty {
            HStack {
                Button {
                    showWorkers = true
                } label: {
                    HStack(spacing: 6) {
                        if chat.running > 0 {
                            BrailleSpinner(tint: ArbosTheme.textMuted)
                                .font(.system(size: 12, design: .monospaced))
                        } else {
                            Image(systemName: "checkmark")
                                .font(.system(size: 11, weight: .semibold))
                        }
                        Text(chat.running > 0 ? "Working \(chat.running)" : "Agents \(chat.workers.count)")
                    }
                    .font(ArbosTheme.body)
                    .foregroundStyle(ArbosTheme.text)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
                    .background(Capsule().fill(ArbosTheme.raised))
                    .overlay(Capsule().strokeBorder(ArbosTheme.border, lineWidth: 1))
                    .shadow(color: .black.opacity(0.3), radius: 10, y: 3)
                }
                .buttonStyle(.plain)
                Spacer()
            }
            .padding(.horizontal, ArbosTheme.barMargin)
            .padding(.bottom, 4)
        }
    }

    // MARK: - Transcript

    private var transcript: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: ArbosTheme.itemGap) {
                    if chat.earlierLines > 0 {
                        Button {
                            Task { await chat.loadEarlier() }
                        } label: {
                            HStack(spacing: 6) {
                                if chat.loadingEarlier {
                                    ProgressView().controlSize(.mini).tint(ArbosTheme.textDim)
                                }
                                Text(chat.loadingEarlier ? "Loading earlier lines…" : "Show \(min(chat.earlierLines, 200)) earlier lines")
                            }
                            .font(ArbosTheme.caption)
                            .foregroundStyle(ArbosTheme.textMuted)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 6)
                        }
                        .buttonStyle(.plain)
                        .disabled(chat.loadingEarlier)
                    }
                    if chat.items.isEmpty, chat.mode != .connecting {
                        Text(emptyLine)
                            .font(ArbosTheme.body)
                            .foregroundStyle(ArbosTheme.textFaint)
                            .padding(.top, 8)
                    }
                    ForEach(chat.items) { item in
                        ChatRow(item: item, waitingOn: title).id(item.id)
                    }
                    if !chat.unseen.isEmpty {
                        AwayCard(notifications: chat.unseen) { chat.markSeen() }
                            .id("away")
                    }
                    if chat.busy {
                        WorkingLine(step: chat.step)
                    }
                    workerLines
                    if let notice = modeNotice {
                        Text(notice)
                            .font(ArbosTheme.caption)
                            .foregroundStyle(ArbosTheme.textDim)
                            .padding(.top, 4)
                    }
                    Color.clear.frame(height: 8).id("tail")
                }
                .padding(.horizontal, ArbosTheme.gutter)
                .padding(.top, 4)
            }
            .modifier(ChatScrollAnchor(followGrowth: followGrowth))
            .scrollPosition(id: $heldRow, anchor: .top)
            .scrollDismissesKeyboard(.interactively)
            // A tap on the words puts the keyboard away (Jacob, build 956),
            // and the tail comes back into view as the keyboard moves.
            .onTapGesture { composing = false }
            .onChange(of: composing) { _, _ in
                Task { @MainActor in
                    try? await Task.sleep(for: .milliseconds(350))
                    withAnimation(.easeOut(duration: 0.15)) { proxy.scrollTo("tail", anchor: .bottom) }
                }
            }
            .onChange(of: chat.items) { _, _ in
                if let anchor = chat.anchorAfterPrepend {
                    // Older lines came in above: hold the row that was at the top.
                    chat.anchorAfterPrepend = nil
                    followGrowth = false
                    heldRow = anchor
                    proxy.scrollTo(anchor, anchor: .top)
                    Task { @MainActor in
                        try? await Task.sleep(for: .milliseconds(120))
                        proxy.scrollTo(anchor, anchor: .top)
                    }
                } else {
                    withAnimation(.easeOut(duration: 0.15)) { proxy.scrollTo("tail", anchor: .bottom) }
                }
            }
            .onChange(of: chat.mode) { _, _ in
                // The one line under the transcript changed; keep it in view.
                withAnimation(.easeOut(duration: 0.15)) { proxy.scrollTo("tail", anchor: .bottom) }
            }
            .onChange(of: chat.earlierLines) { old, new in
                // A long replay lands in one go; the lazy layout settles over
                // a few frames, so the tail is asked for twice.
                guard new > old || old == 0 else { return }
                Task { @MainActor in
                    try? await Task.sleep(for: .milliseconds(250))
                    proxy.scrollTo("tail", anchor: .bottom)
                    try? await Task.sleep(for: .milliseconds(600))
                    proxy.scrollTo("tail", anchor: .bottom)
                    try? await Task.sleep(for: .milliseconds(900))
                    proxy.scrollTo("tail", anchor: .bottom)
                }
            }
            .onChange(of: chat.workers) { _, _ in
                withAnimation(.easeOut(duration: 0.15)) { proxy.scrollTo("tail", anchor: .bottom) }
            }
            .onAppear { proxy.scrollTo("tail", anchor: .bottom) }
        }
    }

    private var emptyLine: String {
        var line = "\(title) is ready."
        if let entry, !entry.place.isEmpty { line += " \(entry.machine) · \(entry.place)." }
        return line
    }

    /// One row per running worker, as the desktop draws them: the first
    /// carries the count, each names its worker and its step. Tap opens
    /// the worker's chat.
    @ViewBuilder
    private var workerLines: some View {
        let running = chat.workers.filter(\.running)
        if !running.isEmpty {
            VStack(alignment: .leading, spacing: ArbosTheme.rowGap) {
                ForEach(Array(running.enumerated()), id: \.element.id) { index, status in
                    Button {
                        worker = status
                    } label: {
                        WorkerLine(status: status, count: index == 0 ? running.count : nil)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.top, 2)
        }
    }

    private var modeNotice: String? {
        switch chat.mode {
        case .mock: return "No kernel reachable — a scripted chat is answering."
        case .offline:
            if let refusal = chat.refusal { return refusal }
            if let seconds = chat.reconnectIn { return "Link lost — reconnecting in \(seconds)s" }
            return "Kernel offline."
        case .connecting: return "Reconnecting…"
        case .server, .live: return nil
        }
    }

    // MARK: - Composer

    private var canSend: Bool {
        (!draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !attachments.isEmpty) && !dictation.active
    }

    private func send() {
        guard canSend else { return }
        followGrowth = true
        heldRow = nil
        chat.send(draft, attachments: attachments)
        draft = ""
        attachments = []
    }
}

/// The workers behind the pill: one row each, spinner or check, the
/// step; a row opens the worker's chat.
struct WorkersSheet: View {
    let workers: [WorkerStatus]
    let open: (WorkerStatus) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Agents")
                .font(.system(size: 20, weight: .semibold))
                .foregroundStyle(ArbosTheme.text)
                .padding(.horizontal, ArbosTheme.gutter)
                .padding(.top, 22)
                .padding(.bottom, 12)
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(workers) { worker in
                        Button {
                            open(worker)
                        } label: {
                            HStack(spacing: 12) {
                                if worker.running {
                                    BrailleSpinner(tint: ArbosTheme.textMuted)
                                        .font(.system(size: 13, design: .monospaced))
                                        .frame(width: 18)
                                } else {
                                    Image(systemName: "checkmark")
                                        .font(.system(size: 12, weight: .semibold))
                                        .foregroundStyle(ArbosTheme.textFaint)
                                        .frame(width: 18)
                                }
                                VStack(alignment: .leading, spacing: 3) {
                                    Text(worker.name)
                                        .font(ArbosTheme.body)
                                        .foregroundStyle(ArbosTheme.text)
                                        .lineLimit(1)
                                    Text(worker.running ? (worker.step.isEmpty ? "Working" : worker.step) : "Done")
                                        .font(ArbosTheme.callout)
                                        .foregroundStyle(ArbosTheme.textFaint)
                                        .lineLimit(1)
                                }
                                Spacer()
                                Image(systemName: "chevron.right")
                                    .font(.system(size: 11, weight: .semibold))
                                    .foregroundStyle(ArbosTheme.textDim)
                            }
                            .padding(.horizontal, ArbosTheme.gutter)
                            .padding(.vertical, 14)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .overlay(alignment: .bottom) {
                            Rectangle().fill(ArbosTheme.border).frame(height: 0.5)
                                .padding(.leading, ArbosTheme.gutter + 30)
                        }
                    }
                }
            }
        }
        .background(ArbosTheme.surface)
    }
}

/// The desktop's live headline while a turn runs: "Working <step>", the
/// words breathing so the eye knows the agent is going.
struct WorkingLine: View {
    let step: String

    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 30)) { context in
            let t = context.date.timeIntervalSinceReferenceDate
            let pulse = 0.55 + 0.45 * (sin(t * 2.2) * 0.5 + 0.5)
            Text(step.isEmpty || step == "Starting" ? (step.isEmpty ? "Working" : "Starting") : "Working \(step)")
                .font(ArbosTheme.body)
                .foregroundStyle(ArbosTheme.textMuted.opacity(pulse))
                .lineLimit(1)
        }
    }
}

/// "2 Working agent-B · Editing sets.md" / "Working agent-A · Reading notes.md".
struct WorkerLine: View {
    let status: WorkerStatus
    var count: Int?

    var body: some View {
        HStack(spacing: 8) {
            BrailleSpinner(tint: ArbosTheme.textMuted)
                .font(.system(size: 14, design: .monospaced))
                .frame(width: 14)
            Text(text)
                .font(ArbosTheme.body)
                .foregroundStyle(ArbosTheme.textMuted)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 0)
            Image(systemName: "chevron.right")
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(ArbosTheme.textDim)
        }
        .contentShape(Rectangle())
    }

    private var text: String {
        let lead = count.map { "\($0) Working" } ?? "Working"
        let step = status.step.isEmpty ? "" : " · \(status.step)"
        return "\(lead) \(status.name)\(step)"
    }
}

/// One row, in the desktop's shapes. A prompt is a right-aligned card
/// (`#212121`, radius 10, 78 % max width). The agent's words are prose
/// with links in the accent. Tool lines, worker reports and notices are
/// one dim line each; "Worked Ns" closes a turn.
struct ChatRow: View {
    let item: ChatItem
    /// The project's name, so a pending card can say who is not answering.
    /// A card still pending after ten seconds has had no echo from the
    /// kernel — whether the socket knows the link is down yet or not.
    var waitingOn: String? = nil

    var body: some View {
        switch item.kind {
        case .user(let text, let pending):
            VStack(alignment: .trailing, spacing: 4) {
                Text(text)
                    .font(ArbosTheme.body)
                    .lineSpacing(ArbosTheme.lineSpacing)
                    .foregroundStyle(pending ? ArbosTheme.textMuted : ArbosTheme.text)
                    .padding(.horizontal, ArbosTheme.promptPadX)
                    .padding(.vertical, ArbosTheme.promptPadY)
                    .background(
                        RoundedRectangle(cornerRadius: ArbosTheme.promptRadius, style: .continuous)
                            .fill(ArbosTheme.card)
                    )
                    .frame(maxWidth: UIScreen.main.bounds.width * 0.78, alignment: .trailing)
                if pending {
                    // Silence reads as broken; a calm sentence reads as
                    // working. After ten seconds without the kernel's echo,
                    // the card says who is not answering (Jacob, build 956).
                    TimelineView(.periodic(from: item.createdAt, by: 1)) { context in
                        let waited = context.date.timeIntervalSince(item.createdAt)
                        if let project = waitingOn, waited >= 10 {
                            Text("\(project) is not answering — waiting")
                                .font(ArbosTheme.caption)
                                .foregroundStyle(ArbosTheme.textMuted)
                        } else {
                            Text("Sending…")
                                .font(ArbosTheme.caption)
                                .foregroundStyle(ArbosTheme.textDim)
                        }
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.top, 6)
        case .agent(let text, let streaming):
            // A call written as text streams in raw; the prose before it is
            // shown, the markup is not (it goes for good when the bubble closes).
            let shown = streaming ? ToolMarkup.visible(text) : text
            HStack(alignment: .lastTextBaseline, spacing: 2) {
                Text(Self.prose(shown))
                    .font(ArbosTheme.body)
                    .lineSpacing(ArbosTheme.lineSpacing)
                    .foregroundStyle(ArbosTheme.text)
                    .tint(ArbosTheme.accent)
                    .textSelection(.enabled)
                if streaming { Caret() }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        case .worked(let seconds):
            Text("Worked \(Self.duration(seconds))")
                .font(ArbosTheme.body)
                .foregroundStyle(ArbosTheme.textFaint)
        case .tool(let label, let failed, let seconds):
            line(
                symbol: failed ? "xmark" : "chevron.right",
                text: label,
                trailing: seconds.map { $0 >= 1 ? "\($0)s" : "" } ?? "",
                tint: failed ? ArbosTheme.danger : ArbosTheme.textFaint,
                truncation: .middle
            )
        case .subagent(let name, let status):
            switch status {
            case "done":
                Text("Done \(name)")
                    .font(ArbosTheme.body)
                    .foregroundStyle(ArbosTheme.textFaint)
                    .lineLimit(1)
            case "spawned":
                EmptyView()
            default:
                line(symbol: "arrow.turn.down.right", text: "\(name) · \(status)", trailing: "", tint: ArbosTheme.textFaint)
            }
        case .notice(let text, let failed):
            line(
                symbol: failed ? "exclamationmark.circle" : "info.circle",
                text: text,
                trailing: "",
                tint: failed ? ArbosTheme.danger : ArbosTheme.textFaint
            )
        }
    }

    static func duration(_ seconds: Int) -> String {
        seconds < 60 ? "\(seconds)s" : "\(seconds / 60)m \(seconds % 60)s"
    }

    /// Markdown as the model writes it — links, `code`, **bold** — so PR
    /// numbers and paths read as on the desktop. Plain text if it does
    /// not parse.
    static func prose(_ text: String) -> AttributedString {
        (try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(text)
    }

    private func line(
        symbol: String, text: String, trailing: String, tint: Color,
        truncation: Text.TruncationMode = .tail
    ) -> some View {
        HStack(spacing: 8) {
            Image(systemName: symbol)
                .font(.system(size: 10, weight: .semibold))
                .frame(width: 12)
            Text(text)
                .lineLimit(2)
                .truncationMode(truncation)
            Spacer(minLength: 8)
            if !trailing.isEmpty {
                Text(trailing)
            }
        }
        .font(ArbosTheme.callout)
        .foregroundStyle(tint)
    }
}

/// A blinking caret at the end of a streaming reply.
private struct Caret: View {
    var body: some View {
        TimelineView(.periodic(from: .now, by: 0.5)) { context in
            let on = Int(context.date.timeIntervalSinceReferenceDate * 2) % 2 == 0
            RoundedRectangle(cornerRadius: 1)
                .fill(ArbosTheme.text.opacity(on ? 0.7 : 0.1))
                .frame(width: 2, height: 16)
        }
    }
}


/// Open at the bottom and keep short content there, but do not re-pin to
/// the bottom when the content grows: growth is followed by hand (the tail
/// scroll on new items), and older lines prepended at the top must not
/// drag the view to the end. iOS 17 has only the all-roles anchor.
private struct ChatScrollAnchor: ViewModifier {
    let followGrowth: Bool

    func body(content: Content) -> some View {
        if #available(iOS 18, *) {
            content
                .defaultScrollAnchor(.bottom, for: .initialOffset)
                .defaultScrollAnchor(.bottom, for: .alignment)
                .defaultScrollAnchor(followGrowth ? .bottom : nil, for: .sizeChanges)
        } else {
            content.defaultScrollAnchor(.bottom)
        }
    }
}


/// What happened while the user was away: the kernel's unseen `notify`s,
/// oldest first, on one raised card under the transcript. "Got it" tells
/// the kernel, which tells every other client. An ask leads; a failure is
/// marked; the rest read as one line each.
struct AwayCard: View {
    let notifications: [KernelNotification]
    let seen: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(notifications.count == 1 ? "While you were away" : "While you were away · \(notifications.count)")
                    .font(ArbosTheme.caption)
                    .foregroundStyle(ArbosTheme.textMuted)
                Spacer()
                Button("Got it", action: seen)
                    .font(ArbosTheme.caption)
                    .foregroundStyle(ArbosTheme.accent)
                    .buttonStyle(.plain)
            }
            ForEach(notifications.suffix(6)) { note in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Image(systemName: note.isAsk ? "questionmark.circle" : (note.failed ? "exclamationmark.circle" : "circle.fill"))
                        .font(.system(size: note.isAsk || note.failed ? 13 : 6))
                        .foregroundStyle(note.failed ? ArbosTheme.danger : (note.isAsk ? ArbosTheme.accent : ArbosTheme.textDim))
                        .frame(width: 14)
                    VStack(alignment: .leading, spacing: 2) {
                        if !note.title.isEmpty {
                            Text(note.title)
                                .font(ArbosTheme.caption.weight(.semibold))
                                .foregroundStyle(ArbosTheme.textMuted)
                        }
                        Text(note.body)
                            .font(ArbosTheme.body)
                            .foregroundStyle(ArbosTheme.text)
                            .lineLimit(3)
                    }
                }
            }
        }
        .padding(12)
        .background(
            RoundedRectangle(cornerRadius: ArbosTheme.cardRadius, style: .continuous)
                .fill(ArbosTheme.raised)
        )
        .padding(.top, 8)
    }
}
