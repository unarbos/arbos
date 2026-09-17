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
    /// A reconnect sends `hello` again; the deleted-binary line is said once
    /// per attachment, not once per dropped socket.
    private var saidBinaryGone = false
    private var children: Set<String> = []
    private var childNames: [String: String] = [:]
    private var workers: [String: WorkerStatus] = [:]
    private var workerOrder: [String] = []
    /// Workers the tree has listed: only those read as done when they
    /// leave it. A remote child is never in the tree; its `say` ends it.
    private var inTree: Set<String> = []
    /// Workers this attach saw move (spawned, reported, or running): the
    /// pill and the sheet show these, not every child the tree remembers.
    private var touched: Set<String> = []
    private var history: [ChatItem] = []
    private var replaying = true
    /// A reply is being streamed; the next `assistant` event is its final
    /// text, not a new message.
    private var streamed = false
    /// Worker reports that arrived while a reply was streaming: shown
    /// when the turn ends, so a paragraph is never cut in half.
    private var deferred: [ChatItem] = []
    private var turnStarted: Date?
    /// A `history <agent>` request in flight: its replayed lines and the
    /// continuation waiting for `history_end`.
    private var pendingHistory: (agent: String, items: [ChatItem], done: CheckedContinuation<HistoryPage, Never>)?

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

    func send(text: String, steer: Bool, attachments: [PendingAttachment]) async throws {
        var paths: [String] = []
        for file in attachments {
            let path = "attachments/\(file.storedName)"
            try client.put(path: path, data: file.data)
            if file.isImage { AttachmentCache.store(file.data, as: file.storedName) }
            paths.append(path)
        }
        try client.send(text: text, steer: steer, attachments: paths)
    }

    func interrupt() async throws {
        try client.stop()
    }

    func stop() {
        pump?.cancel()
        client.detach()
        stream?.finish()
        if let pending = pendingHistory {
            pendingHistory = nil
            pending.done.resume(returning: HistoryPage(items: pending.items, from: 0, to: 0, total: 0))
        }
    }

    func history(agent: String) async -> [ChatItem] {
        await page(agent: agent) { try client.history(agent: agent) }?.items ?? []
    }

    func markSeen(through: Int) {
        try? client.seen(through: through)
    }

    func answer(text: String, id: String?) {
        try? client.answer(text, id: id)
    }

    func registerPush(token: String, sandbox: Bool) {
        try? client.registerPush(token: token, sandbox: sandbox)
    }

    func earlier(before seq: Int, limit: Int) async -> HistoryPage? {
        guard seq > 1 else { return nil }
        return await page(agent: focus) { try client.history(agent: focus, before: seq, limit: limit) }
    }

    /// One `history` request: its replayed lines gathered until `history_end`.
    private func page(agent: String, request: () throws -> Void) async -> HistoryPage? {
        guard pendingHistory == nil else { return nil }
        return await withCheckedContinuation { continuation in
            pendingHistory = (agent, [], continuation)
            do {
                try request()
            } catch {
                pendingHistory = nil
                continuation.resume(returning: HistoryPage(items: [], from: 0, to: 0, total: 0))
            }
        }
    }

    // MARK: - Frames → chat

    private func handle(_ frame: KernelFrame) {
        switch frame {
        case .hello(let focus, _, let identity, let store, let build):
            self.focus = focus
            if let identity { stream?.yield(.identity(identity.filled(key: "hello"))) }
            if let store { stream?.yield(.store(store)) }
            // A kernel started from a file that has since been deleted keeps
            // answering and refuses every worker it is asked to start, with
            // an error no one sees (JB-6). Nothing else on the phone shows
            // it, so say it once, plainly, where the refusals will appear.
            if build.binaryGone, !saidBinaryGone {
                saidBinaryGone = true
                stream?.yield(.item(ChatItem(.notice("This project's kernel is running from a file that has been deleted, so it will refuse to start workers. Restarting it on its machine picks up the build that is there now.", failed: false))))
            }
        case .snapshot(let focusPath, let agents):
            focus = focusPath.split(separator: "/").last.map(String.init) ?? focus
            remember(agents)
            stream?.yield(.agents(agents))
        case .tree(let agents):
            remember(agents)
            stream?.yield(.agents(agents))
        case .replayed(let agent, let event):
            if let pending = pendingHistory, pending.agent == agent {
                if let item = item(for: event, worker: agent != focus) { pendingHistory?.items.append(item) }
                return
            }
            guard agent == focus else { return }
            if let item = item(for: event, worker: false) { history.append(item) }
        case .historyEnd(let agent, let total, let from, let to):
            if let pending = pendingHistory, pending.agent == agent {
                pendingHistory = nil
                pending.done.resume(returning: HistoryPage(items: pending.items, from: from, to: to, total: total))
                return
            }
            guard agent == focus else { return }
            replaying = false
            // The kernel replays its last 200 lines; the rest of a long
            // project's history is before them (seqs start at 1).
            let earlier = from > 0 ? from - 1 : max(0, total - history.count)
            stream?.yield(.history(history, earlier: earlier, firstSeq: from))
            history.removeAll()
        case .assistantDelta(let agent, let text, let step):
            guard agent == focus, !text.isEmpty else { return }
            streamed = true
            stream?.yield(.agentDelta(text, step: step))
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
                        flushDeferred()
                        stream?.yield(.item(ChatItem(.worked(seconds: Int(Date().timeIntervalSince(started))))))
                    }
                    stream?.yield(.step(""))
                }
                stream?.yield(.turn(running: running))
            } else if isChild(agent) {
                setWorker(agent, running: running, step: running ? nil : "")
                if !running {
                    stream?.yield(.item(ChatItem(.subagent(name: childNames[agent] ?? agent, status: "done"))))
                }
            }
        case .status(let agent, let step, _):
            if agent == focus {
                stream?.yield(.step(step))
            } else if isChild(agent) {
                setWorker(agent, running: !step.isEmpty ? true : nil, step: step)
            }
        case .working(let agent, let secs):
            if agent == focus { stream?.yield(.step(secs > 3 ? "Thinking · \(secs)s" : "Thinking")) }
        case .file(let path, let text, let error):
            if path == "project.toml", error == nil, let identity = ProjectIdentity.parse(toml: text) {
                stream?.yield(.identity(identity))
            }
        case .ask(let agent, let question, let options, let id):
            if agent == focus {
                stream?.yield(.agentDone)
                stream?.yield(.ask(question: question, options: options, id: id))
            }
        case .notify(let notification):
            stream?.yield(.notify(notification))
        case .seen(let through):
            stream?.yield(.seen(through: through))
        case .pushed(_, let enabled, let reason):
            stream?.yield(.pushed(enabled: enabled, reason: reason))
        case .thinkingDelta:
            break
        case .error(let detail):
            // A hub from before #301 passes `push` to the kernel, which does
            // not know it: that is "no push here", not a line for the chat.
            if detail.contains("unknown frame type \"push\"") {
                stream?.yield(.pushed(enabled: false, reason: "the hub is an older build that does not relay push"))
                return
            }
            // A kernel from before #270 parses `put` as nothing it knows.
            // The words still went; say plainly that the file did not, and
            // why, instead of the kernel's red "unknown frame type".
            if detail.contains("unknown frame type") {
                stream?.yield(.item(ChatItem(.notice("This project's kernel is an older build and can't take photos or files yet — the words were sent, the file was not. Its machine needs a kernel update.", failed: false))))
                return
            }
            // The hub saying the kernel went away is the link going, not a
            // line for the transcript: the store's one calm line covers it.
            // …and "no kernel serving" is the same link, still down, seen from
            // the hub: the calm line covers it too (M-110).
            if detail.contains("went away") || detail.contains("closed") || detail.contains("no kernel serving") {
                stream?.yield(.dropped(detail))
            } else if detail.contains("no machine named") || detail.contains("no project named") || detail.contains("not registered") {
                // The hub knows nothing by that name: retrying will not help.
                stream?.yield(.refused(detail))
            } else {
                stream?.yield(.item(ChatItem(.notice(detail, failed: true))))
            }
        case .written(let path, let error):
            // A file that landed says nothing; one that did not says why.
            if let error {
                let name = path.split(separator: "/").last.map(String.init) ?? path
                stream?.yield(.item(ChatItem(.notice("\(name) did not send: \(error)", failed: true))))
            }
        case .other(let type):
            if type == "closed" { stream?.yield(.dropped("kernel closed")) }
        }
    }

    private func handleLive(_ event: KernelEvent) {
        if case .assistant(let text, let step) = event {
            let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
            if step > 0 {
                // Numbered (#247): the store replaces the step's streamed text,
                // or adds the line when nothing was streamed for it. Never a
                // second copy after "Worked". An empty settled line (the
                // step only called tools, or its words were markup the
                // kernel cut) takes the step's streamed bubble away.
                stream?.yield(.agentReplace(trimmed, step: step))
                return
            }
            guard !trimmed.isEmpty else { return }
            if streamed {
                streamed = false
                stream?.yield(.agentReplace(trimmed, step: 0))
            } else {
                stream?.yield(.agentDelta(trimmed, step: 0))
                stream?.yield(.agentDone)
            }
            return
        }
        if case .tool(let record) = event, record.name == "spawn", let child = record.child ?? (record.args?["name"] as? String) {
            // The record names the machine `host` since the mesh; older
            // kernels said `machine`.
            let machine = (record.args?["host"] as? String) ?? (record.args?["machine"] as? String) ?? ""
            if let error = record.error {
                // The spawn was refused (JB-6: the machine could not start a
                // kernel). No worker exists, so no "Starting" line that
                // never ends; one plain line says who would not start and
                // why, the way a stopped worker is one line on the desktop.
                if workers[child] != nil {
                    workers[child] = nil
                    workerOrder.removeAll { $0 == child }
                    touched.remove(child)
                    publishWorkers()
                }
                let name = (record.args?["brief"] as? String) ?? child
                let place = machine.isEmpty ? "" : " on \(machine)"
                stream?.yield(.agentDone)
                stream?.yield(.item(ChatItem(.notice("Couldn't start \(name)\(place): \(error)", failed: false))))
                return
            }
            children.insert(child)
            childNames[child] = record.args?["brief"] as? String ?? child
            // A worker on another machine sends no steps here — only its
            // report, when it is done. "Starting" forever read as stuck
            // (Jacob, build 1021); say where it runs instead.
            // The spawn record comes twice — when the call starts and again
            // when it ends (`wait: true`). Only the first may say "running":
            // the second arrives after the child's own turn went idle and
            // was flipping a finished worker back to Working (M-100).
            // …and the brief may by then live under the kernel's id (adopt).
            let want = Self.slug(child)
            let known = workers[child] != nil || workerOrder.contains { Self.slug($0) == want || Self.slug(childNames[$0] ?? "") == want }
            if !known {
                setWorker(child, running: true, step: machine.isEmpty ? "Starting" : "Running on \(machine) · reports here when done")
            }
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
        case .user(let text, let channel, let attachments):
            var item = ChatItem(.user(text))
            item.spoken = channel == "voice"
            // The photo shows as a picture, not a filename (Jacob, build
            // 1021): the bytes were cached when this phone sent them.
            item.images = attachments.map { ($0 as NSString).lastPathComponent }.filter { AttachmentCache.has($0) }
            return item
        case .answer:
            // The kernel writes the answer twice — `answer`, then the `user`
            // line it becomes; the user line is the card.
            return nil
        case .assistant(let text, let step):
            // Lines from before #278 may still carry a call written as text.
            let trimmed = ToolMarkup.strip(text)
            return trimmed.isEmpty ? nil : ChatItem(.agent(trimmed, streaming: false), step: step)
        case .tool(let record):
            // Older kernels name the child only in the call's arguments.
            if record.name == "spawn", let child = record.child ?? (record.args?["name"] as? String) {
                let brief = record.args?["brief"] as? String ?? child
                if let error = record.error {
                    // Refused (JB-6): no worker to list; the line says why.
                    let machine = (record.args?["host"] as? String) ?? (record.args?["machine"] as? String) ?? ""
                    return ChatItem(.notice("Couldn't start \(brief)\(machine.isEmpty ? "" : " on \(machine)"): \(error)", failed: false))
                }
                children.insert(child)
                childNames[child] = brief
                // A replayed spawn is a finished worker until the tree or a
                // status frame says otherwise: the phone keeps its archived
                // children in the pill and the sheet, as the desktop does.
                if replaying, !worker, workers[child] == nil { setWorker(child, running: false, step: "") }
                return ChatItem(.subagent(name: brief, status: "spawned"))
            }
            if !worker, Self.hiddenRootTools.contains(record.name) { return nil }
            return ChatItem(.tool(label: record.label, failed: record.error != nil, seconds: record.seconds))
        case .say(let from, let text):
            // A worker's report is its turn's end; a remote child is not in
            // the tree, so this is the only word of its finish.
            adopt(from)
            if !replaying, children.contains(from) { setWorker(from, running: false, step: "") }
            return ChatItem(.subagent(name: childNames[from] ?? from, status: text))
        case .ask(let question):
            return ChatItem(.agent(question, streaming: false))
        case .notice(let text, let failed):
            return ChatItem(.notice(text, failed: failed))
        case .interrupted(let detail):
            // The kernel's detail is the bare reason ("stop"); a person
            // reads "Stopped by you", as the desktop says it.
            return ChatItem(.notice(detail.isEmpty || detail == "stop" ? "Stopped by you" : detail, failed: false))
        case .thinking, .turnComplete, .other:
            return nil
        }
    }

    /// The coordinator's bookkeeping calls: worker lines, the page, the
    /// checklist and the live step stand in for them, never a row.
    private static let hiddenRootTools: Set<String> = [
        "status", "plan", "todo", "say", "subscribe", "remember", "notes", "page", "title", "ask",
    ]

    /// The tree names a worker the way the desktop's panel does; the
    /// spawn record's brief only stands in until the tree arrives.
    private func remember(_ agents: [KernelAgent]) {
        for agent in agents where agent.parent == focus {
            adopt(agent.id)
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

    /// An older kernel's spawn record names the child by its brief ("Write
    /// hello file") while every later frame carries the id the kernel made
    /// from it ("write-hello-file"). Until the two are one entry, the brief's
    /// line stays "Starting" for ever and the id's line says Done (M-100).
    private func adopt(_ id: String) {
        guard workers[id] == nil else { return }
        let want = Self.slug(id)
        guard let old = workerOrder.first(where: { $0 != id && (Self.slug(childNames[$0] ?? $0) == want || Self.slug($0) == want) }) else { return }
        let old_ = workers.removeValue(forKey: old)
        workers[id] = WorkerStatus(id: id, name: old_?.name ?? childNames[old] ?? old, step: old_?.step ?? "", running: old_?.running ?? false)
        if let at = workerOrder.firstIndex(of: old) { workerOrder[at] = id } else { workerOrder.append(id) }
        if touched.remove(old) != nil { touched.insert(id) }
        children.remove(old); children.insert(id)
        if childNames[id] == nil { childNames[id] = childNames[old] ?? old }
        childNames[old] = nil
    }

    /// Adopts a brief-keyed entry first, then asks whether `id` is a child.
    private func isChild(_ id: String) -> Bool {
        adopt(id)
        return children.contains(id)
    }

    private static func slug(_ text: String) -> String {
        let lowered = text.lowercased()
        var out = ""; var dash = false
        for ch in lowered {
            if ch.isLetter || ch.isNumber { out.append(ch); dash = false }
            else if !dash, !out.isEmpty { out.append("-"); dash = true }
        }
        while out.hasSuffix("-") { out.removeLast() }
        return out
    }

    private func setWorker(_ id: String, running: Bool?, step: String?) {
        adopt(id)
        var worker = workers[id] ?? WorkerStatus(id: id, name: childNames[id] ?? id, step: "", running: false)
        if workers[id] == nil { workerOrder.append(id) }
        touched.insert(id)
        if let running { worker.running = running }
        if let step { worker.step = step }
        workers[id] = worker
        publishWorkers()
    }

    private func publishWorkers() {
        for (id, worker) in workers where worker.running { touched.insert(id) }
        stream?.yield(.workers(workerOrder.filter { touched.contains($0) }.compactMap { workers[$0] }))
    }
}
