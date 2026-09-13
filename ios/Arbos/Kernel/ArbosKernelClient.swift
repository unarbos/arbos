import Foundation
import Network

/// Talks to a running `arbos-kernel serve` the way the desktop does.
///
/// The kernel binds a loopback TCP port and writes it to
/// `<place>/.arbos/kernel.json` as `{"url":"tcp://127.0.0.1:PORT","pid":N}`.
/// The protocol is newline-delimited JSON, `Frame` in
/// `crates/arbos-core/src/wire.rs`. On attach the kernel sends a `snapshot`
/// and one `plan` per agent, then streams `event` / `turn` / `tree` frames.
/// A text turn is `{"type":"user","agent":"root","text":"…"}`.
///
/// Reaching the kernel from a phone: the desktop opens `ssh -N -L` to the
/// host and dials loopback. iOS cannot run ssh. The plan is a tailnet
/// (Tailscale / WireGuard) so the kernel host has a stable private address,
/// plus a one-line change to the kernel to bind `0.0.0.0` or the tailnet IP
/// instead of `127.0.0.1`. Until then this client dials whatever host:port
/// Settings holds, which is enough for a Mac on the same Wi-Fi with the
/// port forwarded.
///
/// The voice loop does not go through here yet: the speech provider
/// answers directly. Next step is routing the user's final transcript in
/// as a `user` frame and reading the `assistant` event back for the model
/// to speak, so the phone talks to the same agent as the desktop.
actor ArbosKernelClient {
    enum State: Equatable {
        case detached
        case attaching
        case attached
        case failed(String)
    }

    struct Endpoint: Equatable {
        var host: String
        var port: UInt16
    }

    private(set) var state: State = .detached
    private(set) var agents: [KernelAgent] = []
    private(set) var focus: String = "root"

    let frames: AsyncStream<KernelFrame>
    private let frameSink: AsyncStream<KernelFrame>.Continuation
    private var connection: NWConnection?
    private var pending = Data()

    init() {
        var held: AsyncStream<KernelFrame>.Continuation!
        frames = AsyncStream { held = $0 }
        frameSink = held
    }

    func attach(_ endpoint: Endpoint) async throws {
        guard let port = NWEndpoint.Port(rawValue: endpoint.port) else {
            throw KernelClientError.badEndpoint
        }
        detach()
        state = .attaching
        let connection = NWConnection(host: NWEndpoint.Host(endpoint.host), port: port, using: .tcp)
        self.connection = connection
        do {
            try await withCheckedThrowingContinuation { (resume: CheckedContinuation<Void, Error>) in
                let settle = OnceGate()
                connection.stateUpdateHandler = { [weak self] change in
                    switch change {
                    case .ready:
                        if settle.claim() { resume.resume() }
                    case .failed(let error), .waiting(let error):
                        if settle.claim() {
                            resume.resume(throwing: error)
                        } else {
                            Task { await self?.dropped(error.localizedDescription) }
                        }
                    case .cancelled:
                        if !settle.claim() { Task { await self?.dropped("closed") } }
                    default:
                        break
                    }
                }
                connection.start(queue: .global(qos: .userInitiated))
            }
        } catch {
            state = .failed(error.localizedDescription)
            self.connection = nil
            throw error
        }
        state = .attached
        receive(on: connection)
    }

    func detach() {
        connection?.cancel()
        connection = nil
        pending.removeAll()
        if state != .detached { state = .detached }
    }

    /// Send one text turn. `steer: true` slips it into a running turn at
    /// its next tool boundary instead of queueing a new one.
    func send(text: String, agent: String? = nil, steer: Bool = false) throws {
        try write(["type": "user", "agent": agent ?? focus, "text": text, "steer": steer])
    }

    func stop(agent: String? = nil) throws {
        try write(["type": "stop", "agent": agent ?? focus])
    }

    /// Reply to an `ask` frame.
    func answer(_ text: String, agent: String? = nil) throws {
        try write(["type": "answer", "agent": agent ?? focus, "text": text])
    }

    // MARK: - Private

    private func write(_ object: [String: Any]) throws {
        guard let connection, state == .attached else { throw KernelClientError.notAttached }
        var line = try JSONSerialization.data(withJSONObject: object)
        line.append(0x0A)
        connection.send(content: line, completion: .contentProcessed { _ in })
    }

    private func receive(on connection: NWConnection) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 1 << 16) { [weak self] data, _, complete, error in
            Task {
                guard let self else { return }
                if let data { await self.ingest(data) }
                if complete || error != nil {
                    await self.dropped(error?.localizedDescription ?? "closed")
                } else {
                    await self.receive(on: connection)
                }
            }
        }
    }

    private func ingest(_ data: Data) {
        pending.append(data)
        while let newline = pending.firstIndex(of: 0x0A) {
            let line = pending[pending.startIndex..<newline]
            pending.removeSubrange(pending.startIndex...newline)
            guard !line.isEmpty,
                  let object = try? JSONSerialization.jsonObject(with: line) as? [String: Any],
                  let frame = KernelFrame(json: object) else { continue }
            switch frame {
            case .snapshot(let focus, let agents):
                self.focus = focus.split(separator: "/").last.map(String.init) ?? "root"
                self.agents = agents
            case .tree(let agents):
                self.agents = agents
            case .event, .turn, .ask, .other:
                break
            }
            frameSink.yield(frame)
        }
    }

    private func dropped(_ reason: String) {
        guard state == .attached || state == .attaching else { return }
        state = .failed(reason)
        connection = nil
    }
}

/// A continuation may only resume once; the connection reports state many
/// times. First caller wins.
private final class OnceGate: @unchecked Sendable {
    private let lock = NSLock()
    private var claimed = false

    func claim() -> Bool {
        lock.lock(); defer { lock.unlock() }
        guard !claimed else { return false }
        claimed = true
        return true
    }
}

enum KernelClientError: LocalizedError {
    case badEndpoint
    case notAttached

    var errorDescription: String? {
        switch self {
        case .badEndpoint: return "Bad kernel host or port."
        case .notAttached: return "Not attached to a kernel."
        }
    }
}
