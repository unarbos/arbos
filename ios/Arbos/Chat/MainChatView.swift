import SwiftUI

/// A project's main chat — the desktop's Project chat, on a phone. The
/// header block names the project the way its tab does; prompts are
/// right-aligned cards; the agent's words are plain prose; a "Worked Ns"
/// line closes each turn; workers show as the desktop's worker lines and
/// open their own chat. The handset opens the call.
struct ProjectChatView: View {
    let target: KernelTarget
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var chat: ChatStore
    @EnvironmentObject private var projects: ProjectStore

    @State private var draft = ""
    @State private var showCall = false
    @State private var worker: WorkerStatus?
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
        VStack(spacing: 0) {
            transcript
            composer
        }
        .background(ArbosTheme.bg.ignoresSafeArea())
        .toolbar {
            ToolbarItem(placement: .principal) {
                HStack(spacing: 8) {
                    ProjectGlyph(identity: identity, size: 22, working: chat.busy || chat.running > 0)
                    Text(title)
                        .font(ArbosTheme.bodyMedium)
                        .foregroundStyle(ArbosTheme.text)
                        .lineLimit(1)
                }
            }
            ToolbarItem(placement: .primaryAction) {
                Button {
                    showCall = true
                } label: {
                    Image(systemName: "phone")
                        .foregroundStyle(ArbosTheme.text)
                }
                .disabled(!settings.isConfigured)
            }
        }
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(ArbosTheme.bg, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .fullScreenCover(isPresented: $showCall) {
            CallScreen()
        }
        .navigationDestination(item: $worker) { worker in
            WorkerChatView(worker: worker, project: identity)
        }
        .task(id: target) { await chat.switchTarget(target) }
        .onChange(of: chat.identity) { _, face in
            if let face { projects.remember(face, for: target) }
        }
    }

    // MARK: - Transcript

    private var transcript: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: ArbosTheme.itemGap) {
                    header
                    ForEach(chat.items) { item in
                        ChatRow(item: item).id(item.id)
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
                .padding(.vertical, 8)
            }
            .scrollDismissesKeyboard(.interactively)
            .onChange(of: chat.items) { _, _ in
                withAnimation(.easeOut(duration: 0.15)) { proxy.scrollTo("tail", anchor: .bottom) }
            }
            .onChange(of: chat.workers) { _, _ in
                withAnimation(.easeOut(duration: 0.15)) { proxy.scrollTo("tail", anchor: .bottom) }
            }
            .onAppear { proxy.scrollTo("tail", anchor: .bottom) }
        }
    }

    /// The desktop's header block: glyph, name, one line about the place.
    private var header: some View {
        VStack(alignment: .leading, spacing: 10) {
            ProjectGlyph(identity: identity, size: 44)
            Text(title)
                .font(.system(size: 22, weight: .semibold))
                .foregroundStyle(ArbosTheme.text)
            Text(headerLine)
                .font(ArbosTheme.callout)
                .foregroundStyle(ArbosTheme.textFaint)
        }
        .padding(.top, 18)
        .padding(.bottom, 14)
    }

    private var headerLine: String {
        var parts: [String] = []
        if let entry {
            parts.append(entry.machine)
            if !entry.place.isEmpty { parts.append(entry.place) }
        } else {
            parts.append(target.label)
        }
        switch chat.mode {
        case .connecting: parts.append("connecting…")
        case .offline: parts.append("offline")
        case .mock: parts.append("demo")
        case .server, .live: break
        }
        return parts.joined(separator: " · ")
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
        case .offline: return "Kernel offline."
        case .connecting, .server, .live: return nil
        }
    }

    // MARK: - Composer

    /// The desktop's composer pill: radius 14, a hairline, the field, the
    /// send disc at the trailing edge. Stop while a turn runs.
    private var composer: some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField(placeholder, text: $draft, axis: .vertical)
                .font(ArbosTheme.body)
                .lineLimit(1...6)
                .focused($composing)
                .foregroundStyle(ArbosTheme.text)
                .tint(ArbosTheme.accent)
                .padding(.leading, 6)
                .padding(.vertical, 6)
            Button(action: send) {
                Image(systemName: "arrow.up")
                    .font(.system(size: 13, weight: .bold))
                    .foregroundStyle(canSend ? Color.black : ArbosTheme.textDim)
                    .frame(width: 28, height: 28)
                    .background(Circle().fill(canSend ? ArbosTheme.text : ArbosTheme.raisedHover))
            }
            .disabled(!canSend)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(
            RoundedRectangle(cornerRadius: ArbosTheme.composerRadius, style: .continuous)
                .fill(ArbosTheme.inputBg)
                .overlay(
                    RoundedRectangle(cornerRadius: ArbosTheme.composerRadius, style: .continuous)
                        .strokeBorder(ArbosTheme.border, lineWidth: 1)
                )
        )
        .padding(.horizontal, ArbosTheme.gutter)
        .padding(.top, 6)
        .padding(.bottom, 8)
    }

    private var placeholder: String {
        chat.items.isEmpty ? "What are you working on?" : "Send follow-up"
    }

    private var canSend: Bool {
        !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && chat.mode != .offline && chat.mode != .connecting
    }

    private func send() {
        guard canSend else { return }
        chat.send(draft)
        draft = ""
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
            Text(step.isEmpty ? "Working" : "Working \(step)")
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
                .font(.system(size: 12, design: .monospaced))
                .frame(width: 12)
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
/// (`#212121`, radius 10, 70 % max width). The agent's words are prose.
/// Tool lines, worker reports and notices are one dim line each.
struct ChatRow: View {
    let item: ChatItem

    var body: some View {
        switch item.kind {
        case .user(let text):
            HStack {
                Spacer(minLength: 0)
                Text(text)
                    .font(ArbosTheme.body)
                    .foregroundStyle(ArbosTheme.text)
                    .padding(.horizontal, ArbosTheme.promptPadX)
                    .padding(.vertical, ArbosTheme.promptPadY)
                    .background(
                        RoundedRectangle(cornerRadius: ArbosTheme.promptRadius, style: .continuous)
                            .fill(ArbosTheme.card)
                    )
                    .frame(maxWidth: UIScreen.main.bounds.width * 0.78, alignment: .trailing)
            }
            .padding(.top, 6)
        case .agent(let text, let streaming):
            HStack(alignment: .lastTextBaseline, spacing: 2) {
                Text(text)
                    .font(ArbosTheme.body)
                    .lineSpacing(5)
                    .foregroundStyle(ArbosTheme.text)
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

    private func line(
        symbol: String, text: String, trailing: String, tint: Color,
        truncation: Text.TruncationMode = .tail
    ) -> some View {
        HStack(spacing: 8) {
            Image(systemName: symbol)
                .font(.system(size: 9, weight: .semibold))
                .frame(width: 10)
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
