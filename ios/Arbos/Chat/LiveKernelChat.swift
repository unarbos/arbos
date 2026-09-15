import Foundation

/// The main chat read straight off a phone-reachable `arbos-kernel` through
/// `ArbosKernelClient`.
///
/// On attach the kernel replays the focused agent's recent transcript
/// (`replayed` … `history_end`), which becomes the chat's history in one
/// go. Live replies stream token by token as `assistant_delta`; the kernel
/// then sends `turn idle` and the whole `assistant` event, which replaces
/// the streamed text rather than adding to it. `status` frames drive the
/// worker lines; `read project.toml` fetches the project's face.
@MainActor
final class LiveKernelChat: ChatSource {
    private let client: ArbosKernelClient
    private let endpoint: ArbosKernelClient.Endpoint
    private var stream: AsyncStream<ChatUpdate>.Continuation?
    private var pump: Task<Void, Never>?
    private var focus = "root"
    private var children: Set<String> = []
    private var childNames: [String: String] = [:]
    private var workers: [String: WorkerStatus] = [:]
    private var workerOrder: [String] = []
    /// Workers the tree has listed: only those read as done when they
    /// leave it. A remote child is never in the tree; its `say` ends it.
    private var inTree: Set<String> = []
    private var history: [ChatItem] = []
    private var replaying = true
    /// A reply is being streamed; the next `assistant` event is its final
    /// text, not a new message.
    private var streamed = false
    /// Worker reports that arrived while a reply was streaming: shown
    /// after the reply closes, so a paragraph is never cut in half.
    private var deferred: [ChatItem] = []
    private var turnStarted: Date?
    /// A `history <agent>` request in flight: its replayed lines and the
    /// continuation waiting for `history_end`.
    private var pendingHistory: (agent: String, items: [ChatItem], done: CheckedContinuation<[ChatItem], Never>)?

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
        try? client.read(path: "project.toml")
    }

    func send(text: String, steer: Bool) async throws {
        try client.send(text: text, steer: steer)
    }

    func stop() {
        pump?.cancel()
        client.detach()
        stream?.finish()
        if let pending = pendingHistory {
            pendingHistory = nil
            pending.done.resume(returning: pending.items)
        }
    }

    func history(agent: String) async -> [ChatItem] {
        guard pendingHistory == nil else { return [] }
        return await withCheckedContinuation { continuation in
            pendingHistory = (agent, [], continuation)
            do {
                try client.history(agent: agent)
            } catch {
                pendingHistory = nil
                continuation.resume(returning: [])
            }
        }
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
            if let pending = pendingHistory, pending.agent == agent {
                if let item = item(for: event, worker: true) { pendingHistory?.items.append(item) }
                return
            }
            guard agent == focus else { return }
            if let item = item(for: event, worker: false) { history.append(item) }
        case .historyEnd(let agent):
            if let pending = pendingHistory, pending.agent == agent {
                pendingHistory = nil
                pending.done.resume(returning: pending.items)
                return
            }
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
            let running = state == "running"
            if agent == focus {
                if running {
                    if turnStarted == nil { turnStarted = Date() }
                } else {
                    if let started = turnStarted {
                        turnStarted = nil
                        stream?.yield(.agentDone)
                        streamed = false
                        flushDeferred()
                        stream?.yield(.item(ChatItem(.worked(seconds: Int(Date().timeIntervalSince(started))))))
                    }
                    stream?.yield(.step(""))
                }
                stream?.yield(.turn(running: running))
            } else if children.contains(agent) {
                setWorker(agent, running: running, step: running ? nil : "")
                if !running {
                    stream?.yield(.item(ChatItem(.subagent(name: childNames[agent] ?? agent, status: "done"))))
                }
            }
        case .status(let agent, let step, _):
            if agent == focus {
                stream?.yield(.step(step))
            } else if children.contains(agent) {
                setWorker(agent, running: !step.isEmpty ? true : nil, step: step)
            }
        case .working(let agent, let secs):
            if agent == focus { stream?.yield(.step(secs > 3 ? "Thinking · \(secs)s" : "Thinking")) }
        case .file(let path, let text, let error):
            if path == "project.toml", error == nil, let identity = ProjectIdentity.parse(toml: text) {
                stream?.yield(.identity(identity))
            }
        case .ask(let agent, let question, _):
            if agent == focus {
                stream?.yield(.agentDone)
                stream?.yield(.item(ChatItem(.agent(question, streaming: false))))
            }
        case .thinkingDelta:
            break
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
            flushDeferred()
            return
        }
        if case .tool(let record) = event, record.name == "spawn", let child = record.child {
            children.insert(child)
            childNames[child] = record.args?["brief"] as? String ?? child
            setWorker(child, running: true, step: "Starting")
        }
        guard let item = item(for: event, worker: false) else { return }
        if case .say = event, streamed {
            deferred.append(item)
            return
        }
        stream?.yield(.agentDone)
        stream?.yield(.item(item))
    }

    private func flushDeferred() {
        for item in deferred { stream?.yield(.item(item)) }
        deferred.removeAll()
    }

    /// One transcript line as a chat row; nil for lines the chat does not
    /// draw (wakes, thinking, turn boundaries). The root's own tool calls
    /// are hidden the way the desktop's Project chat hides them; a
    /// worker's chat shows every tool line.
    private func item(for event: KernelEvent, worker: Bool) -> ChatItem? {
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
            if !worker, Self.hiddenRootTools.contains(record.name) { return nil }
            return ChatItem(.tool(label: record.label, failed: record.error != nil, seconds: record.seconds))
        case .say(let from, let text):
            // A worker's report is its turn's end; a remote child is not in
            // the tree, so this is the only word of its finish.
            if children.contains(from) { setWorker(from, running: false, step: "") }
            return ChatItem(.subagent(name: childNames[from] ?? from, status: text))
        case .ask(let question):
            return ChatItem(.agent(question, streaming: false))
        case .notice(let text, let failed):
            return ChatItem(.notice(text, failed: failed))
        case .interrupted(let detail):
            return ChatItem(.notice(detail.isEmpty ? "Stopped by you" : detail, failed: false))
        case .thinking, .turnComplete, .other:
            return nil
        }
    }

    /// The coordinator's bookkeeping calls: worker lines, the page, the
    /// checklist and the live step stand in for them, never a row.
    private static let hiddenRootTools: Set<String> = [
        "status", "plan", "todo", "say", "subscribe", "remember", "notes", "page", "title",
    ]

    /// The tree names a worker the way the desktop's panel does; the
    /// spawn record's brief only stands in until the tree arrives.
    private func remember(_ agents: [KernelAgent]) {
        for agent in agents where agent.parent == focus {
            children.insert(agent.id)
            childNames[agent.id] = agent.name
            inTree.insert(agent.id)
            // The tree carries a local agent's live step. A remote child's
            // status lives on its machine, so no step here says nothing:
            // its spawn record and its report set running on and off.
            if workers[agent.id] == nil {
                workers[agent.id] = WorkerStatus(id: agent.id, name: agent.name, step: agent.step ?? "", running: agent.step != nil)
                workerOrder.append(agent.id)
            } else {
                workers[agent.id]?.name = agent.name
                if let step = agent.step {
                    workers[agent.id]?.running = true
                    workers[agent.id]?.step = step
                }
            }
        }
        // A worker gone from the tree is archived: no longer running.
        for id in workerOrder where inTree.contains(id) && !agents.contains(where: { $0.id == id }) {
            workers[id]?.running = false
        }
        publishWorkers()
    }

    private func setWorker(_ id: String, running: Bool?, step: String?) {
        var worker = workers[id] ?? WorkerStatus(id: id, name: childNames[id] ?? id, step: "", running: false)
        if workers[id] == nil { workerOrder.append(id) }
        if let running { worker.running = running }
        if let step { worker.step = step }
        workers[id] = worker
        publishWorkers()
    }

    private func publishWorkers() {
        stream?.yield(.workers(workerOrder.compactMap { workers[$0] }))
    }
}
