import Foundation

/// One row of the main chat, as the screen draws it.
struct ChatItem: Identifiable, Equatable {
    enum Kind: Equatable {
        /// `pending`: typed here, not yet echoed by the kernel — shown at
        /// once so nothing typed ever vanishes; resent if the link drops.
        case user(String, pending: Bool = false)
        /// `streaming` while text is still arriving.
        case agent(String, streaming: Bool)
        /// A tool call, folded to one dim line.
        case tool(label: String, failed: Bool, seconds: Int?)
        /// A sub-agent's status: spawned, what it said, done.
        case subagent(name: String, status: String)
        case notice(String, failed: Bool)
        /// The desktop's turn headline once a live turn ends: "Worked 12s".
        case worked(seconds: Int)
    }

    let id: UUID
    var kind: Kind
    /// The model step this agent text belongs to (#247); 0 when unknown.
    /// The settled `assistant` line of a step replaces the text streamed
    /// for the same step, never a neighbour's.
    var step: Int = 0

    init(id: UUID = UUID(), _ kind: Kind, step: Int = 0) {
        self.id = id
        self.kind = kind
        self.step = step
    }

    var isStreamingAgent: Bool {
        if case .agent(_, streaming: true) = kind { return true }
        return false
    }

    var isAgent: Bool {
        if case .agent = kind { return true }
        return false
    }
}

/// One of the root's workers, as the desktop's worker line shows it:
/// "1 Working · <step>" while it runs, "Done <name>" after.
struct WorkerStatus: Identifiable, Hashable {
    let id: String
    var name: String
    var step: String
    var running: Bool
}

/// What a chat source tells the store, in order.
enum ChatUpdate {
    /// Replace everything shown (mock seed; later, a transcript replay).
    /// `earlier` is how many transcript lines lie before the first shown;
    /// `firstSeq` is the first shown line's seq, for paging back.
    case history([ChatItem], earlier: Int, firstSeq: Int)
    case item(ChatItem)
    /// Append to the open agent message of this step, opening one if there is none.
    case agentDelta(String, step: Int)
    /// Close the open agent message.
    case agentDone
    /// The kernel's settled text for one step. Replaces what the deltas
    /// built for that step, so the same words never show twice; a step
    /// nothing was streamed for becomes a new message.
    case agentReplace(String, step: Int)
    case turn(running: Bool)
    case agents([KernelAgent])
    /// The root's workers and what each is doing now.
    case workers([WorkerStatus])
    /// The focused agent's live step ("Reading notes.md"); empty when idle.
    case step(String)
    /// `.arbos/project.toml` read off the kernel.
    case identity(ProjectIdentity)
    /// The link to the kernel went; the store reconnects on its own.
    case dropped(String)
}

/// Where the main chat comes from: the live kernel, or a scripted stand-in
/// while no kernel is reachable.
@MainActor
protocol ChatSource: AnyObject {
    var updates: AsyncStream<ChatUpdate> { get }
    func start() async throws
    func send(text: String, steer: Bool, attachments: [PendingAttachment]) async throws
    func stop()
    /// A worker's transcript, replayed once. Sources without workers
    /// return nothing.
    func history(agent: String) async -> [ChatItem]
    /// The lines before `seq` of the focused transcript, oldest first;
    /// nil where there is no paging.
    func earlier(before seq: Int, limit: Int) async -> HistoryPage?
}

extension ChatSource {
    func history(agent: String) async -> [ChatItem] { [] }
    func earlier(before seq: Int, limit: Int) async -> HistoryPage? { nil }
}

/// One page of a transcript: the lines and the seq range they cover.
struct HistoryPage {
    var items: [ChatItem]
    var from: Int
    var to: Int
    var total: Int
}
