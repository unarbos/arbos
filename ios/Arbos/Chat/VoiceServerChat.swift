import Foundation

/// The main chat through the speech server: typed turns on its text
/// channel (`text.input` → `text.delta`* → `text.done`) and the kernel's
/// transcript mirrored as `agent.*` events. Same `ChatUpdate` stream as
/// the direct kernel source.
@MainActor
final class VoiceServerChat: ChatSource {
    private let link: VoiceLink
    private var stream: AsyncStream<ChatUpdate>.Continuation?
    private var subscription: UUID?
    private var textBusy = false
    private var kernelBusy = false
    private var rootID = "root"
    private var names: [String: String] = [:]

    let updates: AsyncStream<ChatUpdate>

    init(link: VoiceLink) {
        self.link = link
        var held: AsyncStream<ChatUpdate>.Continuation!
        updates = AsyncStream { held = $0 }
        stream = held
    }

    func start() async throws {
        subscription = link.subscribe { [weak self] event in self?.handle(event) }
        try await link.connect()
    }

    func answer(text: String, id: String?) { Task { try? await send(text: text, steer: false, attachments: []) } }

    func send(text: String, steer: Bool, attachments: [PendingAttachment]) async throws {
        try await link.connect()
        stream?.yield(.item(ChatItem(.user(text))))
        textBusy = true
        publishBusy()
        link.sendText(text)
    }

    func stop() {
        link.unsubscribe(subscription)
        subscription = nil
        stream?.finish()
    }

    // MARK: - Events → chat

    private func handle(_ event: VoiceEvent) {
        switch event {
        case .textDelta(let delta):
            stream?.yield(.agentDelta(delta, step: 0))
        case .textDone(_, let cancelled):
            stream?.yield(.agentDone)
            if cancelled { stream?.yield(.item(ChatItem(.notice("stopped", failed: false)))) }
            textBusy = false
            publishBusy()
        case .toolCall(let name, let summary):
            stream?.yield(.item(ChatItem(.tool(
                label: summary.isEmpty ? name : "\(name) · \(summary)", failed: false, seconds: nil
            ))))
        case .toolResult(let name, let output):
            let short = output.replacingOccurrences(of: "\n", with: " ")
            stream?.yield(.item(ChatItem(.tool(
                label: short.isEmpty ? name : "\(name) → \(short)", failed: false, seconds: nil
            ))))
        case .agentDone(let agent, let text):
            stream?.yield(.item(ChatItem(.subagent(name: name(of: agent), status: text))))
        case .agentTree(let agents):
            if let root = agents.first(where: { $0.parent == nil }) { rootID = root.id }
            for agent in agents { names[agent.id] = agent.name }
            stream?.yield(.agents(agents))
        case .agentTurn(let agent, let running):
            if agent == rootID {
                kernelBusy = running
                publishBusy()
            } else {
                stream?.yield(.item(ChatItem(.subagent(
                    name: name(of: agent), status: running ? "working" : "done"
                ))))
            }
        case .agentEvent(let agent, let kind, let text, let from):
            guard agent == rootID else { return }
            let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
            switch kind {
            case "user":
                stream?.yield(.item(ChatItem(.user(trimmed))))
            case "assistant":
                guard !trimmed.isEmpty else { return }
                stream?.yield(.agentDelta(trimmed, step: 0))
                stream?.yield(.agentDone)
            case "say":
                stream?.yield(.item(ChatItem(.subagent(name: name(of: from ?? agent), status: trimmed))))
            case "tool":
                stream?.yield(.item(ChatItem(.tool(label: trimmed.isEmpty ? "tool" : trimmed, failed: false, seconds: nil))))
            case "notice":
                stream?.yield(.item(ChatItem(.notice(trimmed, failed: false))))
            default:
                break
            }
        case .closed:
            textBusy = false
            publishBusy()
        default:
            break
        }
    }

    private func publishBusy() {
        stream?.yield(.turn(running: textBusy || kernelBusy))
    }

    private func name(of id: String) -> String {
        let name = names[id] ?? id
        return name.count > 32 ? String(name.prefix(31)) + "…" : name
    }
}
