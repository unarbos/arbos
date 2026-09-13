import Foundation

/// The main chat read off a live `arbos-kernel` through `ArbosKernelClient`.
///
/// The kernel broadcasts transcript lines as they are appended, from the
/// moment we attach. It does not replay history over the attach socket
/// (the desktop reads `transcript.jsonl` from disk or over ssh), so a
/// fresh attach starts empty and fills as the agent works. Text arrives
/// one `assistant` event per model step, not token by token; each step
/// lands as one delta.
@MainActor
final class LiveKernelChat: ChatSource {
    private let client: ArbosKernelClient
    private let endpoint: ArbosKernelClient.Endpoint
    private var stream: AsyncStream<ChatUpdate>.Continuation?
    private var pump: Task<Void, Never>?
    private var focus = "root"
    private var children: Set<String> = []
    private var childNames: [String: String] = [:]

    let updates: AsyncStream<ChatUpdate>

    init(endpoint: ArbosKernelClient.Endpoint) {
        self.endpoint = endpoint
        client = ArbosKernelClient()
        var held: AsyncStream<ChatUpdate>.Continuation!
        updates = AsyncStream { held = $0 }
        stream = held
    }

    func start() async throws {
        try await client.attach(endpoint)
        pump = Task { [weak self, client] in
            for await frame in client.frames {
                guard let self, !Task.isCancelled else { return }
                self.handle(frame)
            }
            self?.stream?.yield(.dropped("kernel closed"))
        }
    }

    func send(text: String, steer: Bool) async throws {
        try await client.send(text: text, steer: steer)
    }

    func stop() {
        pump?.cancel()
        Task { await client.detach() }
        stream?.finish()
    }

    // MARK: - Frames → chat

    private func handle(_ frame: KernelFrame) {
        switch frame {
        case .snapshot(let focusPath, let agents):
            focus = focusPath.split(separator: "/").last.map(String.init) ?? "root"
            remember(agents)
            stream?.yield(.agents(agents))
        case .tree(let agents):
            remember(agents)
            stream?.yield(.agents(agents))
        case .event(let agent, let event):
            if agent == focus {
                handleFocused(event)
            }
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
        case .other:
            break
        }
    }

    private func handleFocused(_ event: KernelEvent) {
        switch event {
        case .user(let text), .answer(let text):
            stream?.yield(.agentDone)
            stream?.yield(.item(ChatItem(.user(text))))
        case .assistant(let text):
            // One event is one finished model step. Close it at once so
            // the call can speak it before the step's tools finish.
            let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty else { return }
            stream?.yield(.agentDelta(trimmed))
            stream?.yield(.agentDone)
        case .tool(let record):
            stream?.yield(.agentDone)
            if record.name == "spawn", let child = record.child {
                let brief = record.args?["brief"] as? String ?? child
                children.insert(child)
                childNames[child] = brief
                stream?.yield(.item(ChatItem(.subagent(name: brief, status: "spawned"))))
            } else {
                stream?.yield(.item(ChatItem(.tool(
                    label: record.label, failed: record.error != nil, seconds: record.seconds
                ))))
            }
        case .say(let from, let text):
            stream?.yield(.agentDone)
            stream?.yield(.item(ChatItem(.subagent(name: childNames[from] ?? from, status: text))))
        case .ask(let question):
            stream?.yield(.agentDone)
            stream?.yield(.item(ChatItem(.agent(question, streaming: false))))
        case .notice(let text, let failed):
            stream?.yield(.item(ChatItem(.notice(text, failed: failed))))
        case .turnComplete:
            stream?.yield(.agentDone)
        case .interrupted(let detail):
            stream?.yield(.agentDone)
            stream?.yield(.item(ChatItem(.notice(detail.isEmpty ? "stopped" : detail, failed: false))))
        case .thinking, .other:
            break
        }
    }

    private func remember(_ agents: [KernelAgent]) {
        for agent in agents where agent.parent == focus {
            children.insert(agent.id)
            if childNames[agent.id] == nil { childNames[agent.id] = agent.name }
        }
    }
}
