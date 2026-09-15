import Foundation

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

    /// Fires with each finished agent message. The call speaks it when the
    /// server does not.
    var onAgentMessage: ((String) -> Void)?

    private let settings: AppSettings
    private let link: VoiceLink
    private var source: ChatSource?
    private var pump: Task<Void, Never>?
    private var sentAt: Date?

    init(settings: AppSettings, link: VoiceLink) {
        self.settings = settings
        self.link = link
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
        pump?.cancel()
        pump = nil
        source?.stop()
        source = nil
        mode = .offline
        busy = false
    }

    /// Drop the current source and try the kernel again.
    func reconnect() async {
        disconnect()
        items.removeAll()
        agents.removeAll()
        workers.removeAll()
        step = ""
        identity = nil
        lastFirstToken = nil
        await connect()
    }

    /// A worker's transcript, replayed once from the kernel.
    func history(agent: String) async -> [ChatItem] {
        await source?.history(agent: agent) ?? []
    }

    var running: Int { workers.filter(\.running).count }

    /// Open another kernel: the pod, or a machine/project on the hub.
    func switchTarget(_ target: KernelTarget) async {
        guard target != settings.kernelTarget || mode != .live else { return }
        settings.kernelTarget = target
        await reconnect()
    }

    /// What the header calls the chat: the machine/project, then the
    /// root agent's name when the kernel gave it one.
    var title: String {
        let target = settings.kernelTarget.label
        let agent = agentName
        return agent == "main" ? target : "\(target) · \(agent)"
    }

    func send(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let source else { return }
        // A turn already running gets the new words as a steer at its next
        // tool boundary; otherwise this starts one.
        let steer = busy
        busy = true
        sentAt = Date()
        Task {
            do {
                try await source.send(text: trimmed, steer: steer)
            } catch {
                busy = false
                items.append(ChatItem(.notice("kernel offline", failed: true)))
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
        case .history(let seed):
            items = seed
        case .item(let item):
            closeOpenAgentMessage()
            items.append(item)
        case .agentDelta(let delta):
            if let sentAt {
                lastFirstToken = Date().timeIntervalSince(sentAt)
                self.sentAt = nil
            }
            if let index = items.indices.last, case .agent(let text, streaming: true) = items[index].kind {
                items[index].kind = .agent(text + delta, streaming: true)
            } else {
                items.append(ChatItem(.agent(delta, streaming: true)))
            }
        case .agentDone:
            closeOpenAgentMessage()
        case .agentReplace(let text):
            // `turn idle` usually closed (and announced) the streamed
            // message already; only announce here if it is still open.
            if let index = items.lastIndex(where: { if case .agent = $0.kind { return true } else { return false } }) {
                let wasOpen = items[index].isStreamingAgent
                // The kernel's whole text for one step can land after the
                // next step's tokens already started: when the streamed
                // text runs past it, keep streaming instead of cutting.
                if wasOpen, case .agent(let streamedText, _) = items[index].kind,
                   streamedText.trimmingCharacters(in: .whitespacesAndNewlines).hasPrefix(text),
                   streamedText.trimmingCharacters(in: .whitespacesAndNewlines).count > text.count {
                    break
                }
                items[index].kind = .agent(text, streaming: false)
                if wasOpen, !text.isEmpty { onAgentMessage?(text) }
            } else {
                items.append(ChatItem(.agent(text, streaming: false)))
                if !text.isEmpty { onAgentMessage?(text) }
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
