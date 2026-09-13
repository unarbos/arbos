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

    /// Speech server first (it carries the kernel's chat and the text
    /// channel), then a direct kernel, then the scripted chat so the screen
    /// is still real. Safe to call again.
    func connect() async {
        guard mode == .offline else { return }
        mode = .connecting
        if settings.provider == .selfHosted, settings.isConfigured {
            let server = VoiceServerChat(link: link)
            if (try? await server.start()) != nil {
                adopt(server, mode: .server)
                return
            }
            server.stop()
        }
        if let endpoint = settings.kernelEndpoint {
            let live = LiveKernelChat(endpoint: endpoint)
            if (try? await live.start()) != nil {
                adopt(live, mode: .live)
                return
            }
        }
        let mock = MockKernelChat()
        try? await mock.start()
        adopt(mock, mode: .mock)
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
        await connect()
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
        case .turn(let running):
            busy = running
            if !running { closeOpenAgentMessage() }
        case .agents(let list):
            agents = list
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
