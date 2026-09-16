import Foundation
import Network

/// The project's main agent chat, shared by the call screen (voice) and
/// the chat sheet (text). One agent, two ways in.
@MainActor
final class ChatStore: ObservableObject {
    enum Mode: Equatable {
        case offline
        case connecting
        /// Through the speech server: text channel + kernel mirror.
        case server
        /// Straight to the kernel's attach port.
        case live
        /// Nothing reachable; a scripted stand-in is answering.
        case mock

        var tag: String {
            switch self {
            case .offline: return "offline"
            case .connecting: return "connecting"
            case .server, .live: return "live"
            case .mock: return "demo"
            }
        }
    }

    @Published private(set) var mode: Mode = .offline
    @Published private(set) var items: [ChatItem] = []
    @Published private(set) var busy = false
    @Published private(set) var agents: [KernelAgent] = []
    /// The root's workers, for the worker lines and the Working pill.
    @Published private(set) var workers: [WorkerStatus] = []
    /// The root's live step while a turn runs ("Reading notes.md").
    @Published private(set) var step = ""
    /// The project's face from `.arbos/project.toml`, once read.
    @Published private(set) var identity: ProjectIdentity?
    /// Send → first token of the last typed turn.
    @Published private(set) var lastFirstToken: TimeInterval?
    /// Transcript lines before the first one shown (the kernel replays its
    /// last 200 on attach); a long project's earlier history.
    @Published private(set) var earlierLines = 0
    /// Seconds until the next reconnect try, while the link is down.
    @Published private(set) var reconnectIn: Int?

    /// Fires with each finished agent message. The call speaks it when the
    /// server does not.
    var onAgentMessage: ((String) -> Void)?

    private let settings: AppSettings
    private let link: VoiceLink
    private var source: ChatSource?
    private var pump: Task<Void, Never>?
    private var sentAt: Date?
    private var reconnectTask: Task<Void, Never>?
    private var reconnectAttempt = 0
    /// Typed lines the kernel has not echoed yet, oldest first.
    private var pendingSends: [(id: UUID, text: String, steer: Bool)] = []
    private let pathMonitor = NWPathMonitor()
    private var pathWasSatisfied = true

    init(settings: AppSettings, link: VoiceLink) {
        self.settings = settings
        self.link = link
        // The network came back (Wi-Fi to cellular, a tunnel, a dead spot):
        // reconnect now instead of waiting out the backoff.
        pathMonitor.pathUpdateHandler = { [weak self] path in
            let satisfied = path.status == .satisfied
            Task { @MainActor in
                guard let self else { return }
                if satisfied, !self.pathWasSatisfied || self.reconnectIn != nil { self.resumeIfNeeded() }
                self.pathWasSatisfied = satisfied
            }
        }
        pathMonitor.start(queue: DispatchQueue(label: "arbos.path"))
    }

    var agentName: String {
        let root = agents.first { $0.parent == nil }
        guard let name = root?.name, name != "root" else { return "main" }
        return name
    }

    /// The kernel itself first (it replays history and streams tokens),
    /// then the speech server's mirror, then the scripted chat so the
    /// screen is still real. If the configured kernel host is down, the
    /// published endpoint file is consulted once. Safe to call again.
    func connect() async {
        guard mode == .offline else { return }
        mode = .connecting
        if let endpoint = settings.chatEndpoint {
            if await attachKernel(endpoint) { return }
            if let directory = await EndpointDirectory.fetch() {
                if let fresh = directory.kernelURL, fresh != settings.kernelURL { settings.kernelURL = fresh }
                if let hub = directory.hubURL, hub != settings.hubURL { settings.hubURL = hub }
                if let moved = settings.chatEndpoint, moved != endpoint, await attachKernel(moved) { return }
            }
        } else if case .hub = settings.kernelTarget {
            // The hub target is gone or unconfigured: fall back to the pod.
            settings.kernelTarget = .pod
            if let endpoint = settings.chatEndpoint, await attachKernel(endpoint) { return }
        }
        if settings.provider == .selfHosted, settings.isConfigured {
            let server = VoiceServerChat(link: link)
            if (try? await server.start()) != nil {
                adopt(server, mode: .server)
                return
            }
            server.stop()
        }
        // A configured kernel that is not answering is an outage, not a
        // reason to show scripted answers: stay offline and retry.
        if settings.chatEndpoint != nil {
            mode = .offline
            return
        }
        let mock = MockKernelChat()
        try? await mock.start()
        adopt(mock, mode: .mock)
    }

    private func attachKernel(_ endpoint: ArbosKernelClient.Endpoint) async -> Bool {
        let live = LiveKernelChat(endpoint: endpoint)
        // Frames start arriving during attach; the pump must be running
        // before the history replay lands.
        adopt(live, mode: .connecting)
        if (try? await live.start()) != nil {
            mode = .live
            return true
        }
        live.stop()
        pump?.cancel()
        source = nil
        return false
    }

    func disconnect() {
        reconnectTask?.cancel()
        reconnectTask = nil
        reconnectIn = nil
        pump?.cancel()
        pump = nil
        source?.stop()
        source = nil
        mode = .offline
        busy = false
    }

    /// The app came back to the front (the phone woke, the user returned):
    /// a link iOS cut while the app slept is reopened at once.
    func resumeIfNeeded() {
        guard mode == .offline || mode == .mock, settings.chatEndpoint != nil else { return }
        reconnectTask?.cancel()
        reconnectIn = nil
        reconnectAttempt = 0
        Task { await reconnect() }
    }

    /// The link went: try again after 2 s, then 4, 8, capped at 15, until
    /// it holds (the path monitor cuts the wait short when the network
    /// returns). `reconnectIn` counts down for the chat's notice.
    private func scheduleReconnect() {
        reconnectTask?.cancel()
        let delay = min(15, 2 << min(reconnectAttempt, 3))
        reconnectAttempt += 1
        reconnectIn = delay
        reconnectTask = Task { [weak self] in
            for remaining in stride(from: delay, to: 0, by: -1) {
                guard let self, !Task.isCancelled else { return }
                self.reconnectIn = remaining
                try? await Task.sleep(for: .seconds(1))
            }
            guard let self, !Task.isCancelled else { return }
            self.reconnectIn = nil
            await self.reconnect()
        }
    }

    /// Drop the current source and try the kernel again. The transcript
    /// stays on screen until the replay replaces it, so a reconnect never
    /// shows an empty chat.
    func reconnect() async {
        let attempt = reconnectAttempt
        disconnect()
        reconnectAttempt = attempt
        agents.removeAll()
        workers.removeAll()
        step = ""
        lastFirstToken = nil
        await connect()
        if mode == .live || mode == .server {
            reconnectAttempt = 0
            flushPending()
        } else if settings.chatEndpoint != nil, mode != .connecting {
            scheduleReconnect()
        }
    }

    /// A worker's transcript, replayed once from the kernel.
    func history(agent: String) async -> [ChatItem] {
        await source?.history(agent: agent) ?? []
    }

    var running: Int { workers.filter(\.running).count }

    /// A line the app itself has to say (a picker or dictation problem).
    func notice(_ text: String) {
        items.append(ChatItem(.notice(text, failed: true)))
    }

    /// Open another kernel: the pod, or a machine/project on the hub.
    func switchTarget(_ target: KernelTarget) async {
        guard target != settings.kernelTarget || mode != .live else { return }
        if target != settings.kernelTarget {
            items.removeAll()
            earlierLines = 0
            identity = nil
        }
        settings.kernelTarget = target
        reconnectAttempt = 0
        await reconnect()
    }

    /// What the header calls the chat: the machine/project, then the
    /// root agent's name when the kernel gave it one.
    var title: String {
        let target = settings.kernelTarget.label
        let agent = agentName
        return agent == "main" ? target : "\(target) · \(agent)"
    }

    func send(_ text: String, attachments: [PendingAttachment] = []) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty || !attachments.isEmpty else { return }
        // Shown at once, as pending; the kernel's echo of the same words
        // makes it real. A turn already running gets the new words as a
        // steer at its next tool boundary; otherwise this starts one.
        let steer = busy
        let shown = attachments.isEmpty ? trimmed : (trimmed.isEmpty ? "" : trimmed + "\n") + attachments.map { "📎 \($0.name)" }.joined(separator: "\n")
        let card = ChatItem(.user(shown, pending: true))
        closeOpenAgentMessage()
        items.append(card)
        pendingSends.append((card.id, trimmed, steer))
        guard let source, mode == .live || mode == .server || mode == .mock else { return }
        busy = true
        sentAt = Date()
        Task {
            do {
                try await source.send(text: trimmed, steer: steer, attachments: attachments)
            } catch {
                busy = false
            }
        }
    }

    /// After a reconnect: whatever was typed while the link was down goes
    /// out now, oldest first, in one turn each.
    private func flushPending() {
        guard let source, !pendingSends.isEmpty else { return }
        let queue = pendingSends
        busy = true
        Task {
            for entry in queue {
                try? await source.send(text: entry.text, steer: false, attachments: [])
                try? await Task.sleep(for: .milliseconds(200))
            }
        }
    }

    // MARK: - Private

    private func adopt(_ source: ChatSource, mode: Mode) {
        pump?.cancel()
        self.source = source
        self.mode = mode
        pump = Task { [weak self] in
            for await update in source.updates {
                guard let self, !Task.isCancelled else { return }
                self.apply(update)
            }
        }
    }

    private func apply(_ update: ChatUpdate) {
        switch update {
        case .history(let seed, let earlier):
            let stillPending = items.filter { if case .user(_, pending: true) = $0.kind { return true } else { return false } }
            items = seed + stillPending
            earlierLines = earlier
        case .item(let item):
            if case .user(let text, _) = item.kind, let index = pendingSends.firstIndex(where: { $0.text == text || ($0.text.isEmpty && text.isEmpty) }) {
                // The kernel echoed a line typed here: the pending card is real now.
                let pending = pendingSends.remove(at: index)
                if let row = items.firstIndex(where: { $0.id == pending.id }), case .user(let shown, _) = items[row].kind {
                    items[row].kind = .user(shown)
                    return
                }
            }
            closeOpenAgentMessage()
            items.append(item)
        case .agentDelta(let delta, let step):
            if let sentAt {
                lastFirstToken = Date().timeIntervalSince(sentAt)
                self.sentAt = nil
            }
            if let index = items.indices.last, case .agent(let text, streaming: true) = items[index].kind,
               step == 0 || items[index].step == 0 || items[index].step == step {
                items[index].kind = .agent(text + delta, streaming: true)
                if items[index].step == 0 { items[index].step = step }
            } else {
                closeOpenAgentMessage()
                items.append(ChatItem(.agent(delta, streaming: true), step: step))
            }
        case .agentDone:
            closeOpenAgentMessage()
        case .agentReplace(let text, let step):
            // The step's own item when the kernel numbers steps; the last
            // agent item when it does not (older kernels).
            let index = step > 0
                ? items.lastIndex(where: { $0.isAgent && $0.step == step })
                : items.lastIndex(where: \.isAgent)
            if let index {
                let wasOpen = items[index].isStreamingAgent
                // The kernel's whole text for one step can land after the
                // next step's tokens already started (step 0 only): when the
                // streamed text runs past it, keep streaming instead of cutting.
                if wasOpen, step == 0, case .agent(let streamedText, _) = items[index].kind,
                   streamedText.trimmingCharacters(in: .whitespacesAndNewlines).hasPrefix(text),
                   streamedText.trimmingCharacters(in: .whitespacesAndNewlines).count > text.count {
                    break
                }
                items[index].kind = .agent(text, streaming: false)
                if wasOpen, !text.isEmpty { onAgentMessage?(text) }
            } else if !text.isEmpty {
                // Nothing streamed for this step (attached mid-turn): a new message.
                closeOpenAgentMessage()
                items.append(ChatItem(.agent(text, streaming: false), step: step))
                onAgentMessage?(text)
            }
        case .turn(let running):
            busy = running
            if !running { closeOpenAgentMessage() }
        case .agents(let list):
            agents = list
        case .workers(let list):
            workers = list
        case .step(let text):
            step = text
        case .identity(let face):
            identity = face
        case .dropped(let reason):
            items.append(ChatItem(.notice(reason, failed: true)))
            mode = .offline
            busy = false
            if settings.chatEndpoint != nil { scheduleReconnect() }
        }
        if items.count > 200 { items.removeFirst(items.count - 200) }
    }

    private func closeOpenAgentMessage() {
        guard let index = items.indices.last,
              case .agent(let text, streaming: true) = items[index].kind else { return }
        let final = text.trimmingCharacters(in: .whitespacesAndNewlines)
        items[index].kind = .agent(final, streaming: false)
        if !final.isEmpty { onAgentMessage?(final) }
    }
}
