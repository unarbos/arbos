import Foundation

/// The project's main agent chat, shared by the call screen (voice) and
/// the chat sheet (text). One agent, two ways in.
@MainActor
final class ChatStore: ObservableObject {
    enum Mode: Equatable {
        case offline
        case connecting
        case live
        /// No kernel reachable; a scripted stand-in is answering.
        case mock

        var tag: String {
            switch self {
            case .offline: return "offline"
            case .connecting: return "connecting"
            case .live: return "live"
            case .mock: return "demo"
            }
        }
    }

    @Published private(set) var mode: Mode = .offline
    @Published private(set) var items: [ChatItem] = []
    @Published private(set) var busy = false
    @Published private(set) var agents: [KernelAgent] = []

    /// Fires with each finished agent message. The call speaks it.
    var onAgentMessage: ((String) -> Void)?

    private let settings: AppSettings
    private var source: ChatSource?
    private var pump: Task<Void, Never>?

    init(settings: AppSettings) {
        self.settings = settings
    }

    var agentName: String {
        agents.first { $0.parent == nil }?.name ?? "main"
    }

    /// Attach to the kernel if Settings names one and it answers; else run
    /// the scripted chat so the screen is still real. Safe to call again.
    func connect() async {
        guard mode == .offline else { return }
        mode = .connecting
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
