// Driven by ArbosFFI.RoomSession over URLSession (crdt-sync §7.2a).
// The chat doc is the core doc (`CoreChatDoc`); this type only moves bytes and actions.

import Foundation
import ArbosFFI

let protoVersion = 1
/// Row cap (crdt-sync §7.6).
let rowMax = 1024 * 1024 - 4096
let projectDocChatId = "project"

func roomName(_ projectId: String, _ chatId: String) -> String {
    "\(projectId)/\(chatId)"
}

/// Test seam for the WebSocket: production uses `URLSessionWebSocketTask`.
protocol RoomSocket: AnyObject {
    var onOpen: (() -> Void)? { get set }
    /// HTTP status from the upgrade response, set when `receive()` throws.
    var httpStatus: Int? { get }
    /// The WebSocket close code the server sent, set when `receive()` throws after a close frame.
    var closeCode: Int? { get }
    func resume()
    func send(_ data: Data) async throws
    func receive() async throws -> Data
    func cancel()
}

@MainActor
final class SessionRoom {
    let projectId: Id
    let key: Id
    let roomChat: String
    let doc: CoreChatDoc

    var onLink: ((Link) -> Void)?
    /// The header pill's view of this room: syncing while the socket dials and
    /// catches up, live after RowsDone, and on a failure the verdict the error
    /// carries (offline / relay down / reconnecting) — [feedback connection-pill].
    var onLinkState: ((LinkState) -> Void)?
    var onBoxState: ((BoxState, String?) -> Void)?
    var onCaughtUp: (() -> Void)?
    var onAcked: (() -> Void)?
    var onDocPaint: (() -> Void)?
    var onPresence: (([String: Any]) -> Void)?
    /// Relay rejected this room (HTTP 403/404); caller should refresh membership.
    var onUnavailable: (() -> Void)?
    /// A push was refused with `not_available` on this connection; stay connected.
    var onPushRefused: (() -> Void)?
    /// Why the last push was refused ([feedback blocked-state-ios-2026-10-26]): the relay's
    /// `reason` (`blocked_by` / `blocks_member` / `silenced` / `chat_unavailable`) and, on a
    /// block, the other party. Read by `onPushRefused`; nil from a relay that predates it.
    private(set) var lastRefusal: PushRefusalWhy?
    var presenceUser: () -> (id: String, handle: String)? = { nil }
    /// Read receipt (Mac `presence_header` `seen`): the newest message id of this people chat,
    /// sent while it is focused so peers can lift their own bubbles to full opacity.
    var newestMessageId: () -> String? = { nil }
    var inCall: String? {
        didSet {
            guard inCall != oldValue else { return }
            Task { [weak self] in self?.sendPresence() }
        }
    }

    private(set) var cursor: Int64 = 0
    /// How many times the session asked us to reconnect (test surface).
    private(set) var reconnects = 0
    /// How many `state_timeout` session errors we saw (test surface).
    private(set) var stateTimeouts = 0
    /// Last terminal room error shown on About (e.g. "chat: HTTP 403").
    private(set) var lastError: String?
    var onPendingChanged: (([(String, Data)]) -> Void)?
    var onCursorMoved: (() -> Void)?

    var focused = false {
        didSet {
            guard focused != oldValue else { return }
            Task { [weak self] in self?.sendPresence() }
        }
    }

    private let deviceId: () -> String
    private let roomURL: (String) -> URL?
    private let makeSocket: (URL) -> RoomSocket
    private let fetchCheckpoint: (String) async throws -> (Int64, Data)

    private var session: ArbosFFI.RoomSession
    private var urlSession: URLSession?
    private var socket: RoomSocket?
    private var generation = 0
    private var started = false
    private var stopped = false
    private var isDraining = false
    /// Stop the current drain batch after a failed row import (skip following Cursor).
    private var abortDrainBatch = false

    private var receiveTask: Task<Void, Never>?
    private var tickTask: Task<Void, Never>?
    private var presenceTask: Task<Void, Never>?
    private var reconnectTask: Task<Void, Never>?
    /// The current socket reached `didOpen` (so a later failure is a drop, not a failed dial).
    private var sawOpen = false
    /// The verdict for the next `scheduleReconnect`, set where the error is known.
    private var pendingFault: LinkState?
    /// The relay closed the socket on purpose (1012 Service Restart / 1001 Going Away): the
    /// next reconnect redials soon with a fresh backoff instead of the doubled one (#1340).
    private var cleanClose = false
    /// How long after a clean server close the room redials.
    static let restartRedialMs: UInt64 = 1_500
    /// Match `arbos_client::HELLO_DEADLINE` / Mac dial timeout: abandon a hung
    /// WebSocket upgrade or a STATE that never arrives.
    static let helloDeadlineNs: UInt64 = 15_000_000_000
    /// Host clock tick while dialing / waiting for HELLO (Mac uses 1 s).
    static let tickEveryNs: UInt64 = 1_000_000_000
    private var checkpointTask: Task<Void, Never>?
    private var sendChain: Task<Void, Never>?
    /// Wall clock when the current `connect()` began (dial watchdog).
    private var connectStartedAt: Date?

    /// Production path: wiring `RelayAPI` into the three closures the
    /// designated init uses for tests.
    convenience init(
        projectId: Id, key: Id, roomChat: String, doc: CoreChatDoc, api: RelayAPI,
        cursor: Int64 = 0, pending: [(String, Data)] = [], deviceId: @escaping () -> String
    ) {
        let openDelegate = RoomSocketOpenDelegate()
        let config = URLSessionConfiguration.default
        config.waitsForConnectivity = true
        let urlSession = URLSession(configuration: config, delegate: openDelegate, delegateQueue: nil)
        self.init(
            projectId: projectId,
            key: key,
            roomChat: roomChat,
            doc: doc,
            roomURL: { api.roomURL(room: $0) },
            makeSocket: { url in
                let task = urlSession.webSocketTask(with: url)
                task.maximumMessageSize = rowMax + 8192
                let socket = URLSessionRoomSocket(task: task)
                openDelegate.socket = socket
                return socket
            },
            fetchCheckpoint: { try await api.checkpoint(room: $0) },
            cursor: cursor,
            pending: pending,
            deviceId: deviceId
        )
        self.urlSession = urlSession
    }

    /// Designated init: inject URL / socket / checkpoint for tests.
    init(
        projectId: Id, key: Id, roomChat: String, doc: CoreChatDoc,
        roomURL: @escaping (String) -> URL?,
        makeSocket: @escaping (URL) -> RoomSocket,
        fetchCheckpoint: @escaping (String) async throws -> (Int64, Data),
        cursor: Int64 = 0, pending: [(String, Data)] = [],
        deviceId: @escaping () -> String
    ) {
        self.projectId = projectId
        self.key = key
        self.roomChat = roomChat
        self.doc = doc
        self.roomURL = roomURL
        self.makeSocket = makeSocket
        self.fetchCheckpoint = fetchCheckpoint
        self.cursor = cursor
        self.deviceId = deviceId
        let pushes = pending.map { ArbosFFI.PendingPush(batchId: $0.0, bytes: $0.1) }
        self.session = ArbosFFI.RoomSession(
            deviceId: deviceId(),
            room: roomName(projectId, roomChat),
            cursor: cursor,
            pending: pushes
        )
        doc.onLocalUpdate { [weak self] bytes in
            Task { @MainActor in
                #if DEBUG
                if let self, SendTrace.traces(self.key) {
                    SendTrace.mark("push-queued", detail: "bytes=\(bytes.count) live=\(self.session.isLive())")
                }
                #endif
                self?.session.onLocalCommit(bytes: bytes)
                self?.drain()
            }
        }
    }

    func start() {
        guard !started else { return }
        started = true
        connect()
    }

    func stop() {
        stopped = true
        generation += 1
        cancelTasks()
        socket?.cancel()
        socket = nil
        urlSession?.invalidateAndCancel()
        urlSession = nil
    }

    deinit {
        urlSession?.invalidateAndCancel()
    }

    func wake(reason: String) {
        session.wake(reason: reason)
        drain()
    }

    /// This connection stands refused (`not_available`): sends queue until an ACK, a reconnect
    /// or `retryRefused()` clears it.
    var isPushRefused: Bool { session.pushRefused() }

    /// The unACKed batches this room still has to deliver (test surface).
    var pendingBatches: [(String, Data)] { session.pending().map { ($0.batchId, $0.bytes) } }

    /// The condition behind a refusal may have changed — a people-chat event for this room
    /// (members, settings, a lifted mute) or the user unblocked someone. The session resends
    /// every pending batch now with its original batch id (the relay answers `dup` for what
    /// it already holds) and drops the refusal; the next ACK clears the notice. Nothing is sent
    /// while the room is not refused, and nudges inside two seconds collapse ([feedback 2005]).
    @discardableResult
    func retryRefused() -> Bool {
        guard !stopped else { return false }
        let sent = session.retryRefused(nowMs: sessionNowMs())
        drain()
        return sent
    }

    /// Scene returned from `.background` (lock unlock). iOS often leaves
    /// URLSession WebSockets half-dead: a dial stays on Syncing… forever, or a
    /// "live" socket never errors and stops delivering rows. `recreateListedRooms`
    /// only creates missing rooms — always drop the current attempt and dial now.
    func resumeAfterForeground() {
        guard started, !stopped else { return }
        redialNow(fault: .reconnecting)
    }

    // MARK: - Connect

    private func connect() {
        guard !stopped else { return }
        generation += 1
        let gen = generation
        cancelTasks()
        socket?.cancel()
        socket = nil
        sawOpen = false
        connectStartedAt = Date()

        let room = roomName(projectId, roomChat)
        guard let url = roomURL(room) else {
            session.onClose(nowMs: sessionNowMs())
            drain()
            return
        }

        let sock = makeSocket(url)
        socket = sock
        onLinkState?(.syncing)
        sock.onOpen = { [weak self] in
            Task { @MainActor [weak self] in
                guard let self, self.generation == gen else { return }
                self.handleOpen(gen: gen)
            }
        }
        sock.resume()
        startReceive(gen: gen, socket: sock)
        startTick(gen: gen)
    }

    private func handleOpen(gen: Int) {
        sawOpen = true
        session.onOpen(nowMs: sessionNowMs())
        // Presence right after onOpen — core holds it until Live.
        sendPresence()
        presenceTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 15_000_000_000)
                guard let self, self.generation == gen, !Task.isCancelled else { return }
                self.sendPresence()
            }
        }
    }

    /// 1 s host ticks while this generation's socket is up: dial timeout before
    /// `didOpen`, HELLO deadline after (same as Mac `run_session`).
    private func startTick(gen: Int) {
        tickTask?.cancel()
        tickTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: SessionRoom.tickEveryNs)
                guard let self, self.generation == gen, !Task.isCancelled else { return }
                if !self.sawOpen {
                    let deadlineSec = Double(SessionRoom.helloDeadlineNs) / 1_000_000_000
                    if let started = self.connectStartedAt,
                       Date().timeIntervalSince(started) >= deadlineSec {
                        self.pendingFault = LinkState.fault(URLError(.timedOut))
                        self.session.onClose(nowMs: self.sessionNowMs())
                        self.drain()
                        return
                    }
                    continue
                }
                self.session.onTick(nowMs: self.sessionNowMs())
                self.drain()
            }
        }
    }

    /// Tear down the current attempt (or a sleeping reconnect) and dial at once.
    private func redialNow(fault: LinkState) {
        guard !stopped else { return }
        reconnects += 1
        onLink?(.reconnecting)
        onLinkState?(fault)
        pendingFault = nil
        sawOpen = false
        cleanClose = false
        connectStartedAt = nil
        // Fresh session keeps cursor + pending and resets backoff (same as a
        // clean relay restart), so a long hung dial does not keep doubling.
        session = ArbosFFI.RoomSession(
            deviceId: deviceId(),
            room: session.room(),
            cursor: session.cursor(),
            pending: session.pending()
        )
        cancelTasks()
        socket?.cancel()
        socket = nil
        connect()
    }

    private func startReceive(gen: Int, socket: RoomSocket) {
        receiveTask = Task { [weak self] in
            while !Task.isCancelled {
                do {
                    let data = try await socket.receive()
                    guard let self, self.generation == gen else { return }
                    self.session.onFrame(bytes: data)
                    self.drain()
                } catch {
                    guard let self, self.generation == gen else { return }
                    if let status = socket.httpStatus, status == 403 || status == 404 {
                        // Chat unknown or not a member: do not reconnect.
                        self.stopped = true
                        self.cancelTasks()
                        self.socket?.cancel()
                        self.socket = nil
                        NSLog("relay room \(self.roomChat): HTTP \(status), not retrying")
                        self.lastError = "\(self.roomChat): HTTP \(status)"
                        self.onLinkState?(.live)
                        self.onUnavailable?()
                        return
                    }
                    // A socket that never opened failed to reach the relay: say
                    // why (offline / relay down). One that was up just dropped.
                    if let code = socket.closeCode, SessionRoom.isRestartClose(code) {
                        self.cleanClose = true
                    }
                    if self.session.isLive() || self.sawOpen {
                        self.pendingFault = .reconnecting
                    } else if let status = socket.httpStatus, (500..<600).contains(status) {
                        self.pendingFault = .relayDown
                    } else {
                        self.pendingFault = LinkState.fault(error)
                    }
                    self.session.onClose(nowMs: self.sessionNowMs())
                    self.drain()
                    return
                }
            }
        }
    }

    // MARK: - Drain

    private func drain() {
        guard !isDraining else { return }
        isDraining = true
        defer { isDraining = false }
        while true {
            let actions = session.drainActions()
            if actions.isEmpty { return }
            for action in actions {
                apply(action)
                if abortDrainBatch {
                    abortDrainBatch = false
                    break
                }
            }
        }
    }

    private func apply(_ action: ArbosFFI.RoomAction) {
        switch action {
        case .sendFrame(let bytes):
            enqueueSend(bytes)
        case .fetchCheckpoint(let room, let frontier, _):
            applyFetchCheckpoint(room: room, frontier: frontier)
        case .applyBytes(let bytes):
            do {
                // Parked (dependencies missing): the session keeps the persisted cursor
                // below this row so a relaunch re-requests it (#1540 follow-up).
                #if DEBUG
                let t0 = Date()
                defer { PerfCounters.time("import.room", ms: Date().timeIntervalSince(t0) * 1000) }
                #endif
                if try doc.importUpdateReportingPending(bytes) {
                    session.applyPending()
                }
            } catch {
                NSLog("relay room \(roomChat): import failed: \(error.localizedDescription)")
                // Skip the Cursor that follows in this drain; reconnect from the
                // last good cursor via applyFailed.
                session.applyFailed()
                abortDrainBatch = true
            }
        case .cursor(let seq):
            cursor = seq
        case .pending(let batches):
            onPendingChanged?(batches.map { ($0.batchId, $0.bytes) })
        case .boxState(let state, let detail, _):
            onBoxState?(mapBoxState(state), detail)
        case .linkLive:
            onLink?(.live)
            onLinkState?(.live)
            onCaughtUp?()
            onCursorMoved?()
        case .healCheck(let relayVv):
            // The relay told us what it holds; if this replica has more (a lost checkpoint,
            // #1518), push the difference back once so the room heals from this device.
            // Always answer (even empty) so the session can release the held pending resend.
            let missing = doc.updatesBeyond(relayVv)
            if !missing.isEmpty {
                NSLog("relay room \(roomChat): history heal, pushing \(missing.count) bytes the relay lacked")
            }
            session.onHeal(bytes: missing)
        case .scheduleReconnect(let afterMs):
            applyScheduleReconnect(afterMs: afterMs)
        case .sessionError(let code, let message):
            if code == "state_timeout" { stateTimeouts += 1 }
            // The relay restarts in a few seconds (#1340): the pill says so; the clean close that follows redials.
            if code == "restart_soon" { onLinkState?(.restartSoon); return }
            // Push refused for this connection — stay connected; `pushRefused` carries the notice.
            if code == "not_available" {
                lastError = "\(roomChat): not_available"
                NSLog("relay room \(roomChat): error \(code): \(message)")
                return
            }
            NSLog("relay room \(roomChat): error \(code): \(message)")
        case .pushRefused(_, let code, let reason, let byHandle, let byName):
            lastError = "\(roomChat): \(code)" + (reason.map { " (\($0))" } ?? "")
            lastRefusal = PushRefusalWhy(code: code, reason: reason, byHandle: byHandle, byName: byName)
            onPushRefused?()
        case .presence(_, _, let typing, _, let mine, let headerJson):
            guard !mine else { return }
            if let data = headerJson.data(using: .utf8),
               var obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
                // RoomAction carries a real Bool; JSONSerialization would leave
                // `typing` as NSNumber and `as? Bool` in noteTyping would miss it.
                if let typing { obj["typing"] = typing }
                onPresence?(obj)
            }
        case .droppedOversize(let bytes):
            NSLog("relay room \(roomChat): local update \(bytes) bytes exceeds row cap; dropped")
        case .ack(_, let seq):
            #if DEBUG
            if SendTrace.traces(key) { SendTrace.mark("relay-ack", detail: "seq=\(seq)") }
            #endif
            onAcked?()
            onCursorMoved?()
        }
    }

    private func applyFetchCheckpoint(room: String, frontier: Data) {
        if doc.contains(frontier: frontier) {
            session.onCheckpointSkipped()
            if !doc.isEmpty { onDocPaint?() }
            return
        }
        let gen = generation
        checkpointTask?.cancel()
        checkpointTask = Task { [weak self] in
            guard let self else { return }
            do {
                let (seq, bytes) = try await self.fetchCheckpoint(room)
                guard self.generation == gen, !Task.isCancelled else { return }
                try self.doc.importUpdate(bytes)
                guard self.generation == gen else { return }
                self.session.onCheckpoint(seq: seq)
                self.onDocPaint?()
                self.drain()
            } catch {
                guard self.generation == gen, !Task.isCancelled else { return }
                NSLog("relay room \(room): checkpoint failed: \(error.localizedDescription)")
                self.session.onCheckpointFailed()
                self.drain()
            }
        }
    }

    /// Close codes a relay sends when it is going down on purpose.
    static func isRestartClose(_ code: Int) -> Bool { code == 1012 || code == 1001 }

    private func applyScheduleReconnect(afterMs: UInt64) {
        reconnects += 1
        onLink?(.reconnecting)
        // A HELLO deadline (`state_timeout`) or a host-side close with no error: a plain reconnect.
        onLinkState?(pendingFault ?? .reconnecting)
        pendingFault = nil
        sawOpen = false
        // A deploy restart is not a fault: a fresh session (cursor and pending
        // kept) forgets the backoff, and the redial comes soon. A run of real
        // failures keeps doubling as before.
        var afterMs = afterMs
        if cleanClose {
            cleanClose = false
            session = ArbosFFI.RoomSession(deviceId: deviceId(), room: session.room(),
                                           cursor: session.cursor(), pending: session.pending())
            afterMs = Self.restartRedialMs
        }
        checkpointTask?.cancel()
        checkpointTask = nil
        tickTask?.cancel()
        tickTask = nil
        presenceTask?.cancel()
        presenceTask = nil
        receiveTask?.cancel()
        receiveTask = nil
        sendChain?.cancel()
        sendChain = nil
        socket?.cancel()
        socket = nil
        reconnectTask?.cancel()
        reconnectTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: afterMs * 1_000_000)
            guard let self, !self.stopped, !Task.isCancelled else { return }
            self.session.prepareReconnect()
            self.connect()
        }
    }

    // MARK: - Send / presence

    private func enqueueSend(_ bytes: Data) {
        let gen = generation
        let previous = sendChain
        sendChain = Task { [weak self] in
            await previous?.value
            guard let self, self.generation == gen, !Task.isCancelled else { return }
            do {
                try await self.socket?.send(bytes)
            } catch {
                guard self.generation == gen else { return }
                self.session.onClose(nowMs: self.sessionNowMs())
                self.drain()
            }
        }
    }

    /// Presence header JSON shared by the 15 s tick and typing pings (Mac `presence_header`).
    private func presenceHeaderJSON() -> String? {
        var header: [String: Any] = ["device": deviceId(), "at": nowMs()]
        if let u = presenceUser() { header["user"] = u.id; header["handle"] = u.handle }
        if focused {
            header["focused_chat"] = roomChat
            if let seen = newestMessageId(), !seen.isEmpty { header["seen"] = seen }
        }
        if let inCall { header["in_call"] = inCall }
        guard let data = try? JSONSerialization.data(withJSONObject: header),
              let json = String(data: data, encoding: .utf8) else { return nil }
        return json
    }

    private func sendPresence() {
        guard let json = presenceHeaderJSON() else { return }
        session.presence(presenceHeader: json)
        drain()
    }

    /// Composer keystroke in a people chat: set / clear the `typing` field on a
    /// presence frame so peers show "<name> is typing…" ([feedback
    /// app-01M5N7WSA1VCYHE9BE3CX4V0R6], Mac `RoomCmd::Typing`).
    func setTyping(_ active: Bool) {
        guard let json = presenceHeaderJSON() else { return }
        session.typing(presenceHeader: json, active: active)
        drain()
    }

    private func mapBoxState(_ state: ArbosFFI.BoxState) -> BoxState {
        switch state {
        case .awake: return .awake
        case .waking: return .waking
        case .asleep: return .hibernatedLight
        case .error: return .error
        }
    }

    private func sessionNowMs() -> UInt64 {
        UInt64(Date().timeIntervalSince1970 * 1000)
    }

    private func cancelTasks() {
        reconnectTask?.cancel()
        reconnectTask = nil
        checkpointTask?.cancel()
        checkpointTask = nil
        tickTask?.cancel()
        tickTask = nil
        presenceTask?.cancel()
        presenceTask = nil
        receiveTask?.cancel()
        receiveTask = nil
        sendChain?.cancel()
        sendChain = nil
    }
}

// MARK: - URLSession socket

/// Separate NSObject so `didOpenWithProtocol` can fire `onOpen` without the
/// socket retaining the session. Held weakly from here.
private final class RoomSocketOpenDelegate: NSObject, URLSessionWebSocketDelegate {
    weak var socket: URLSessionRoomSocket?

    func urlSession(
        _ session: URLSession,
        webSocketTask: URLSessionWebSocketTask,
        didOpenWithProtocol protocol: String?
    ) {
        // Only the open for the current socket — an older cancelled task must
        // not start the session for a newer one.
        guard webSocketTask === socket?.task else { return }
        socket?.handleDidOpen()
    }
}

private final class URLSessionRoomSocket: RoomSocket {
    var onOpen: (() -> Void)?
    private(set) var httpStatus: Int?
    private(set) var closeCode: Int?
    fileprivate let task: URLSessionWebSocketTask

    init(task: URLSessionWebSocketTask) {
        self.task = task
    }

    fileprivate func handleDidOpen() {
        onOpen?()
    }

    func resume() {
        task.resume()
    }

    func send(_ data: Data) async throws {
        try await task.send(.data(data))
    }

    func receive() async throws -> Data {
        while true {
            do {
                let message = try await task.receive()
                switch message {
                case .data(let data):
                    return data
                case .string(let s):
                    if let data = Data(base64Encoded: s) { return data }
                @unknown default:
                    continue
                }
            } catch {
                httpStatus = (task.response as? HTTPURLResponse)?.statusCode
                if task.closeCode != .invalid { closeCode = task.closeCode.rawValue }
                throw error
            }
        }
    }

    func cancel() {
        task.cancel(with: .goingAway, reason: nil)
    }
}


/// The relay's reason for a refused push, carried on the `not_available` error frame since
/// [feedback blocked-state-relay-2026-10-26]; the code itself never changes.
struct PushRefusalWhy: Equatable {
    var code: String
    var reason: String?
    var byHandle: String?
    var byName: String?

    /// The plain sentence the chat shows.
    var sentence: String { BlockedState.refusalSentence(reason: reason, name: byName, handle: byHandle) }
}

/// Which pushes nudge a refused room to resend ([feedback 2005]): the ones that change who may
/// post into the chat (its settings, a lifted mute, a member who left or joined). A plain
/// message carries no such change, and nothing else names a chat the sender can post into.
enum RefusedSendRetry {
    static func chatToRetry(_ event: PushEventDto) -> String? {
        let id: String?
        switch event {
        case .peopleChatUpdated(let p): id = p.chat_id
        case .peopleMemberAdded(let p): id = p.chat_id
        case .peopleMemberRemoved(let p): id = p.chat_id
        default: id = nil
        }
        return id.flatMap { $0.isEmpty ? nil : $0 }
    }
}
