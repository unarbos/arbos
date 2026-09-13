import Foundation

/// One row of the main chat, as the screen draws it.
struct ChatItem: Identifiable, Equatable {
    enum Kind: Equatable {
        case user(String)
        /// `streaming` while text is still arriving.
        case agent(String, streaming: Bool)
        /// A tool call, folded to one dim line.
        case tool(label: String, failed: Bool, seconds: Int?)
        /// A sub-agent's status: spawned, what it said, done.
        case subagent(name: String, status: String)
        case notice(String, failed: Bool)
    }

    let id: UUID
    var kind: Kind

    init(id: UUID = UUID(), _ kind: Kind) {
        self.id = id
        self.kind = kind
    }

    var isStreamingAgent: Bool {
        if case .agent(_, streaming: true) = kind { return true }
        return false
    }
}

/// What a chat source tells the store, in order.
enum ChatUpdate {
    /// Replace everything shown (mock seed; later, a transcript replay).
    case history([ChatItem])
    case item(ChatItem)
    /// Append to the open agent message, opening one if there is none.
    case agentDelta(String)
    /// Close the open agent message.
    case agentDone
    case turn(running: Bool)
    case agents([KernelAgent])
    case dropped(String)
}

/// Where the main chat comes from: the live kernel, or a scripted stand-in
/// while no kernel is reachable.
protocol ChatSource: AnyObject {
    var updates: AsyncStream<ChatUpdate> { get }
    func start() async throws
    func send(text: String, steer: Bool) async throws
    func stop()
}
