import Foundation

/// Talks to a phone-reachable `arbos-kernel serve --bind` over WebSocket.
///
/// The wire is the kernel's attach protocol (`Frame` in
/// `crates/arbos-core/src/wire.rs`): one JSON object per message, tagged
/// by `type` in snake_case. Auth is `Authorization: Bearer <token>`. On
/// attach the 0.2 kernel sends `hello`, a `snapshot`, one `plan` per
/// agent, the focused agent's recent transcript as `replayed` frames, then
/// `history_end`. Live turns stream `assistant_delta` tokens, then
/// `turn idle`, then the whole `assistant` event.
///
/// A text turn is `{"type":"user","agent":"root","text":"…"}`.
@MainActor
final class ArbosKernelClient {
    enum State: Equatable {
        case detached
        case attaching
        case attached
        case failed(String)
    }

    struct Endpoint: Equatable {
        var url: URL
        var token: String
    }

    private(set) var state: State = .detached
    private(set) var agents: [KernelAgent] = []
    private(set) var focus: String = "root"
    private(set) var kernelVersion: String = ""

    let frames: AsyncStream<KernelFrame>
    private let frameSink: AsyncStream<KernelFrame>.Continuation
    private var socket: OrderedWebSocket?

    init() {
        var held: AsyncStream<KernelFrame>.Continuation!
        frames = AsyncStream { held = $0 }
        frameSink = held
    }

    /// Open the socket and return once the kernel said `hello`.
    func attach(_ endpoint: Endpoint) async throws {
        detach()
        state = .attaching
        var request = URLRequest(url: endpoint.url)
        request.setValue("Bearer \(endpoint.token)", forHTTPHeaderField: "Authorization")
        let socket = OrderedWebSocket(request: request)
        self.socket = socket
        socket.open(
            onMessage: { [weak self] message in
                Task { @MainActor in self?.receive(message) }
            },
            onFailure: { [weak self] error in
                Task { @MainActor in self?.dropped(error.localizedDescription) }
            }
        )
        let deadline = Date().addingTimeInterval(8)
        while Date() < deadline {
            switch state {
            case .attached: return
            case .failed(let reason): throw KernelClientError.failed(reason)
            case .detached: throw KernelClientError.failed("closed")
            case .attaching: try await Task.sleep(for: .milliseconds(50))
            }
        }
        dropped("kernel did not answer")
        throw KernelClientError.failed("kernel did not answer")
    }

    func detach() {
        socket?.close()
        socket = nil
        if state != .detached { state = .detached }
    }

    /// Send one text turn. `steer: true` slips it into a running turn at
    /// its next tool boundary instead of queueing a new one.
    func send(text: String, agent: String? = nil, steer: Bool = false, attachments: [String] = []) throws {
        var frame: [String: Any] = [
            "type": "user", "agent": agent ?? focus, "text": text, "steer": steer,
            "channel": "text", "device": "phone",
        ]
        if !attachments.isEmpty { frame["attachments"] = attachments }
        try write(frame)
    }

    /// This phone's APNs token, for the hub to push the project's
    /// notifications to (#301). Sent on every attach; tokens rotate.
    func registerPush(token: String, sandbox: Bool) throws {
        try write(["type": "push", "platform": "apns", "token": token, "sandbox": sandbox])
    }

    /// The user has seen every notification up to `through`; the kernel
    /// tells every other client, so nothing shouts twice.
    func seen(through id: Int) throws {
        try write(["type": "seen", "through": id])
    }

    /// A file from the phone into the kernel's store (`.arbos/<path>`), so a
    /// `user` frame can name it in `attachments`. A kernel without this frame
    /// answers with an error the chat shows.
    func put(path: String, data: Data) throws {
        try write(["type": "put", "path": path, "data": data.base64EncodedString()])
    }

    func stop(agent: String? = nil) throws {
        try write(["type": "stop", "agent": agent ?? focus])
    }

    /// Reply to an `ask` frame.
    func answer(_ text: String, agent: String? = nil, id: String? = nil) throws {
        var frame: [String: Any] = ["type": "answer", "agent": agent ?? focus, "text": text]
        if let id { frame["id"] = id }
        try write(frame)
    }

    /// One file under `.arbos/` (`project.toml`, `notes.md`); answered
    /// with a `file` frame.
    func read(path: String) throws {
        try write(["type": "read", "path": path])
    }

    /// Replay `agent`'s transcript: `replayed` frames, then `history_end`.
    /// How a worker's chat is opened without refocusing the kernel.
    func history(agent: String, limit: Int = 200) throws {
        try write(["type": "history", "agent": agent, "since": 0, "limit": limit])
    }

    /// The `limit` lines before `seq`, oldest first (kernel #272).
    func history(agent: String, before seq: Int, limit: Int = 200) throws {
        try write(["type": "history", "agent": agent, "before": seq, "limit": limit])
    }

    // MARK: - Private

    private func write(_ object: [String: Any]) throws {
        guard let socket, state == .attached else { throw KernelClientError.notAttached }
        socket.send(json: object)
    }

    private func receive(_ message: URLSessionWebSocketTask.Message) {
        guard let (_, object) = decodeTypedJSON(message), let frame = KernelFrame(json: object) else { return }
        #if DEBUG
        if let type = object["type"] as? String, ["turn", "status", "tree", "snapshot", "ask", "working"].contains(type) {
            let agent = object["agent"] as? String ?? ""
            let extra = (object["state"] as? String) ?? (object["step"] as? String) ?? ""
            print("frame \(type) \(agent) \(extra.prefix(60))")
        }
        #endif
        switch frame {
        case .hello(let focus, let version, _, _):
            self.focus = focus
            kernelVersion = version
            if state == .attaching { state = .attached }
        case .snapshot(let focusPath, let agents):
            focus = focusPath.split(separator: "/").last.map(String.init) ?? focus
            self.agents = agents
            if state == .attaching { state = .attached }
        case .tree(let agents):
            self.agents = agents
        default:
            break
        }
        frameSink.yield(frame)
    }

    private func dropped(_ reason: String) {
        guard state == .attached || state == .attaching else { return }
        state = .failed(reason)
        socket = nil
        frameSink.yield(.other(type: "closed"))
    }
}

enum KernelClientError: LocalizedError {
    case failed(String)
    case notAttached

    var errorDescription: String? {
        switch self {
        case .failed(let reason): return reason
        case .notAttached: return "Not attached to a kernel."
        }
    }
}
