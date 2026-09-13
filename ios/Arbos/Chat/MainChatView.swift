import SwiftUI

/// The project's main agent chat, with a composer. Opens as a sheet over
/// the call screen; typing here talks to the same agent as the voice.
struct MainChatView: View {
    @EnvironmentObject private var chat: ChatStore
    @Environment(\.dismiss) private var dismiss
    /// Open with the keyboard up: the user swiped up to type.
    var focusComposer = false

    @State private var draft = ""
    @FocusState private var composing: Bool

    var body: some View {
        VStack(spacing: 0) {
            header
            transcript
            composer
        }
        .background(Color(red: 0.06, green: 0.06, blue: 0.07).ignoresSafeArea())
        .preferredColorScheme(.dark)
        .onAppear { composing = focusComposer }
        .task { await chat.connect() }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Text(chat.agentName)
                .font(.system(.headline, design: .rounded))
                .foregroundStyle(.white.opacity(0.9))
            Text(chat.mode.tag)
                .font(.caption2.weight(.medium))
                .foregroundStyle(.white.opacity(0.5))
                .padding(.horizontal, 7)
                .padding(.vertical, 3)
                .background(Capsule().fill(.white.opacity(0.08)))
            Spacer()
            if chat.busy {
                ProgressView().tint(.white.opacity(0.5)).controlSize(.small)
            }
        }
        .padding(.horizontal, 20)
        .padding(.top, 18)
        .padding(.bottom, 10)
    }

    private var transcript: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 10) {
                    ForEach(chat.items) { item in
                        ChatRow(item: item)
                            .id(item.id)
                    }
                    if chat.busy, !(chat.items.last?.isStreamingAgent ?? false) {
                        TypingDots().padding(.horizontal, 20).padding(.top, 2)
                    }
                    Color.clear.frame(height: 8).id("tail")
                }
                .padding(.vertical, 8)
            }
            .scrollDismissesKeyboard(.interactively)
            .onChange(of: chat.items) { _, _ in
                withAnimation(.easeOut(duration: 0.15)) { proxy.scrollTo("tail", anchor: .bottom) }
            }
            .onAppear { proxy.scrollTo("tail", anchor: .bottom) }
        }
    }

    private var composer: some View {
        HStack(alignment: .bottom, spacing: 10) {
            TextField("Message \(chat.agentName)", text: $draft, axis: .vertical)
                .lineLimit(1...5)
                .focused($composing)
                .submitLabel(.send)
                .onSubmit(send)
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .background(RoundedRectangle(cornerRadius: 20).fill(.white.opacity(0.08)))
                .foregroundStyle(.white)
            Button(action: send) {
                Image(systemName: "arrow.up")
                    .font(.system(size: 16, weight: .bold))
                    .foregroundStyle(.black)
                    .frame(width: 38, height: 38)
                    .background(Circle().fill(canSend ? Color.white : Color.white.opacity(0.25)))
            }
            .disabled(!canSend)
        }
        .padding(.horizontal, 16)
        .padding(.top, 8)
        .padding(.bottom, 10)
    }

    private var canSend: Bool {
        !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && chat.mode != .offline
    }

    private func send() {
        guard canSend else { return }
        chat.send(draft)
        draft = ""
    }
}

/// One row. Jacob's turns are bright; the agent's are soft; tool and
/// sub-agent lines are thin and dim so they read as texture, not text.
struct ChatRow: View {
    let item: ChatItem

    var body: some View {
        switch item.kind {
        case .user(let text):
            Text(text)
                .font(.body.weight(.medium))
                .foregroundStyle(.white)
                .padding(.horizontal, 20)
                .padding(.top, 8)
                .frame(maxWidth: .infinity, alignment: .leading)
        case .agent(let text, let streaming):
            HStack(alignment: .lastTextBaseline, spacing: 2) {
                Text(text)
                    .font(.body)
                    .foregroundStyle(.white.opacity(0.78))
                if streaming {
                    Cursor()
                }
            }
            .padding(.horizontal, 20)
            .frame(maxWidth: .infinity, alignment: .leading)
        case .tool(let label, let failed, let seconds):
            line(
                symbol: failed ? "xmark" : "chevron.right",
                text: label,
                trailing: seconds.map { $0 >= 1 ? "\($0)s" : "" } ?? "",
                tint: failed ? Color(red: 0.86, green: 0.40, blue: 0.40) : .white.opacity(0.38),
                truncation: .middle
            )
        case .subagent(let name, let status):
            line(
                symbol: "arrow.turn.down.right",
                text: "\(name) · \(status)",
                trailing: "",
                tint: Color(red: 0.55, green: 0.64, blue: 0.98).opacity(0.85)
            )
        case .notice(let text, let failed):
            line(
                symbol: failed ? "exclamationmark.circle" : "info.circle",
                text: text,
                trailing: "",
                tint: failed ? Color(red: 0.86, green: 0.40, blue: 0.40) : .white.opacity(0.38)
            )
        }
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
                .lineLimit(1)
                .truncationMode(truncation)
            Spacer(minLength: 8)
            if !trailing.isEmpty {
                Text(trailing)
            }
        }
        .font(.system(.caption, design: .monospaced))
        .foregroundStyle(tint)
        .padding(.horizontal, 20)
    }
}

/// A blinking caret at the end of a streaming reply.
private struct Cursor: View {
    var body: some View {
        TimelineView(.periodic(from: .now, by: 0.5)) { context in
            let on = Int(context.date.timeIntervalSinceReferenceDate * 2) % 2 == 0
            RoundedRectangle(cornerRadius: 1)
                .fill(.white.opacity(on ? 0.7 : 0.1))
                .frame(width: 2, height: 16)
        }
    }
}

/// Three soft dots while the agent works and has said nothing yet.
private struct TypingDots: View {
    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 20)) { context in
            let t = context.date.timeIntervalSinceReferenceDate
            HStack(spacing: 4) {
                ForEach(0..<3, id: \.self) { i in
                    Circle()
                        .fill(.white.opacity(0.25 + 0.45 * (sin(t * 4 + Double(i) * 0.9) * 0.5 + 0.5)))
                        .frame(width: 6, height: 6)
                }
            }
        }
    }
}
