import Foundation

/// The main chat read straight off a phone-reachable `arbos-kernel` through
/// `ArbosKernelClient`.
///
/// On attach the kernel replays the focused agent's recent transcript
/// (`replayed` … `history_end`), which becomes the chat's history in one
/// go. Live replies stream token by token as `assistant_delta`; the kernel
/// then sends `turn idle` and the whole `assistant` event, which replaces
/// the streamed text rather than adding to it.
@MainActor
final class LiveKernelChat: ChatSource {
    private let client: ArbosKernelClient
    private let endpoint: ArbosKernelClient.Endpoint
    private var stream: AsyncStream<ChatUpdate>.Continuation?
    private var pump: Task<Void, Never>?
    private var focus = "root"
    private var children: Set<String> = []
    private var childNames: [String: String] = [:]
    private var history: [ChatItem] = []
    private var replaying = true
    /// A reply is being streamed; the next `assistant` event is its final
    /// text, not a new message.
    private var streamed = false

    let updates: AsyncStream<ChatUpdate>

    init(endpoint: ArbosKernelClient.Endpoint) {
        self.endpoint = endpoint
        client = ArbosKernelClient()
        var held: AsyncStream<ChatUpdate>.Continuation!
        updates = AsyncStream { held = $0 }
        stream = held
    }

    func start() async throws {
        pump = Task { [weak self, client] in
            for await frame in client.frames {
                guard let self, !Task.isCancelled else { return }
                self.handle(frame)
            }
        }
        try await client.attach(endpoint)
    }

    func send(text: String, steer: Bool) async throws {
        try client.send(text: text, steer: steer)
    }

    func stop() {
        pump?.cancel()
        client.detach()
        stream?.finish()
    }

    // MARK: - Frames → chat

    private func handle(_ frame: KernelFrame) {
        switch frame {
        case .hello(let focus, _):
            self.focus = focus
        case .snapshot(let focusPath, let agents):
            focus = focusPath.split(separator: "/").last.map(String.init) ?? focus
            remember(agents)
            stream?.yield(.agents(agents))
        case .tree(let agents):
            remember(agents)
            stream?.yield(.agents(agents))
        case .replayed(let agent, let event):
            guard agent == focus else { return }
            if let item = item(for: event) { history.append(item) }
        case .historyEnd(let agent):
            guard agent == focus else { return }
            replaying = false
            stream?.yield(.history(history))
            history.removeAll()
        case .assistantDelta(let agent, let text):
            guard agent == focus, !text.isEmpty else { return }
            streamed = true
            stream?.yield(.agentDelta(text))
        case .event(let agent, let event):
            if agent == focus { handleLive(event) }
        case .turn(let agent, let state):
            if agent == focus {
                stream?.yield(.turn(running: state == "running"))
            } else if children.contains(agent) {
                stream?.yield(.item(ChatItem(.subagent(
                    name: childNames[agent] ?? agent,
                    status: state == "running" ? "working" : "done"
                ))))
            }
        case .ask(let agent, let question, _):
            if agent == focus {
                stream?.yield(.agentDone)
                stream?.yield(.item(ChatItem(.agent(question, streaming: false))))
            }
        case .other(let type):
            if type == "closed" { stream?.yield(.dropped("kernel closed")) }
        }
    }

    private func handleLive(_ event: KernelEvent) {
        if case .assistant(let text) = event {
            let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty else { return }
            if streamed {
                streamed = false
                stream?.yield(.agentReplace(trimmed))
            } else {
                stream?.yield(.agentDelta(trimmed))
                stream?.yield(.agentDone)
            }
            return
        }
        if case .tool(let record) = event, record.name == "spawn", let child = record.child {
            children.insert(child)
            childNames[child] = record.args?["brief"] as? String ?? child
        }
        guard let item = item(for: event) else { return }
        stream?.yield(.agentDone)
        stream?.yield(.item(item))
    }

    /// One transcript line as a chat row; nil for lines the chat does not
    /// draw (wakes, thinking, turn boundaries).
    private func item(for event: KernelEvent) -> ChatItem? {
        switch event {
        case .user(let text), .answer(let text):
            return ChatItem(.user(text))
        case .assistant(let text):
            let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
            return trimmed.isEmpty ? nil : ChatItem(.agent(trimmed, streaming: false))
        case .tool(let record):
            if record.name == "spawn", let child = record.child {
                let brief = record.args?["brief"] as? String ?? child
                children.insert(child)
                childNames[child] = brief
                return ChatItem(.subagent(name: brief, status: "spawned"))
            }
            return ChatItem(.tool(label: record.label, failed: record.error != nil, seconds: record.seconds))
        case .say(let from, let text):
            return ChatItem(.subagent(name: childNames[from] ?? from, status: text))
        case .ask(let question):
            return ChatItem(.agent(question, streaming: false))
        case .notice(let text, let failed):
            return ChatItem(.notice(text, failed: failed))
        case .interrupted(let detail):
            return ChatItem(.notice(detail.isEmpty ? "stopped" : detail, failed: false))
        case .thinking, .turnComplete, .other:
            return nil
        }
    }

    private func remember(_ agents: [KernelAgent]) {
        for agent in agents where agent.parent == focus {
            children.insert(agent.id)
            if childNames[agent.id] == nil { childNames[agent.id] = agent.name }
        }
    }
}
