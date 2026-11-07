import XCTest
import Network
@testable import Arbos

/// The Add tap on "Add members to <group>" ([feedback iphone-add-members-2026-10-26]):
/// `AppModel.addMembers` posts one batch to `POST /chats/{id}/members {handles, agent_ids}`
/// and answers on screen — the roster is replaced on 200, a refusal becomes the notice the
/// sheet shows, and nothing is silent. The relay here is a loopback HTTP server that
/// answers with the production relay's own JSON for a scratch group (2026-10-27 00:05 UTC:
/// a person and an agent added in one call).
@MainActor
final class AddMembersActionTests: XCTestCase {
    private var server: LoopbackRelay!
    private var model: AppModel!

    override func setUp() async throws {
        server = try LoopbackRelay()
        model = AppModel()
        model.installRelayForTests(url: "http://127.0.0.1:\(server.port)", token: "test-token")
        model.peopleChats = [Self.groupBeforeAdd]
    }

    override func tearDown() async throws {
        defer { server = nil; model = nil }
        try server?.stop()
    }

    static let chatId = "01M5XATTTX957RTR99XNE0W9BD"
    static let agentId = "01M5XATTQM8KRH94R070AA27Y0"

    private func member(_ id: String, _ handle: String, role: String = "member") -> ChatMember {
        ChatMember(id: id, kind: .user, member: "user:\(id)", role: role, handle: handle, avatarUrl: nil, name: nil, status: "member",
                   joinedAt: 0, project: nil, ownerId: nil, ownerHandle: nil, replyPolicy: nil)
    }

    private static var groupBeforeAdd: PeopleChat {
        PeopleChat(id: chatId, kind: "group", createdBy: "test_owner", createdAt: 0,
                   title: "Add-members scratch group with a deliberately very long name", pending: false,
                   members: [], headSeq: 0, readSeq: 0, unread: false, lockState: "people_only", lastActivityAt: 0,
                   myRole: "owner", muted: false, silenced: false)
    }

    // MARK: Tests

    func testOneBatchCallAddsAPersonAndAnAgentAndReplacesTheRoster() async throws {
        server.respond = { req in
            guard req.path.hasPrefix("/chats/") else { return (404, Data("{}".utf8)) }
            XCTAssertEqual(req.method, "POST")
            XCTAssertEqual(req.path, "/chats/\(Self.chatId)/members")
            XCTAssertEqual(req.headers["authorization"], "Bearer test-token")
            let body = try? JSONSerialization.jsonObject(with: req.body) as? [String: Any]
            XCTAssertEqual(body?["handles"] as? [String], ["amb-third-1026"])
            XCTAssertEqual(body?["agent_ids"] as? [String], [Self.agentId])
            return (200, Self.relayChatAfterAdd)
        }
        let ok = await model.addMembers(chatId: Self.chatId, handles: ["@amb-third-1026 "], agentIds: [Self.agentId])
        XCTAssertTrue(ok)
        XCTAssertEqual(server.chatRequests.map(\.path), ["/chats/\(Self.chatId)/members"], "one call for the whole batch, no per-item fallback")
        let chat = try XCTUnwrap(model.peopleChat(Self.chatId))
        XCTAssertTrue(chat.hasAgent(Self.agentId), "the agent is in the roster the relay returned")
        XCTAssertTrue(chat.hasPerson(Person(id: "test_01M5XATTMAKGAKK1Q11CT56MBC", handle: "amb-third-1026", avatarUrl: nil, name: nil, status: "member"), meId: nil))
        XCTAssertEqual(chat.members.count, 4)
        XCTAssertFalse(model.toasts.contains { $0.error?.verb.hasPrefix("add") ?? false }, "a success raises no add failure")
        XCTAssertNil(model.claimedFailure)
    }

    func testARefusalIsReportedAndNothingIsClaimed() async throws {
        server.respond = { req in
            guard req.path.hasPrefix("/chats/") else { return (404, Data("{}".utf8)) }
            return (403, #"{"error":{"type":"own_project_only","message":"you can only add or change your own Project's agent"}}"#.data(using: .utf8)!)
        }
        // No sheet up: the failure is an error toast (ErrorNote, #1955).
        let ok = await model.addMembers(chatId: Self.chatId, handles: [], agentIds: [Self.agentId])
        XCTAssertFalse(ok)
        let toast = try XCTUnwrap(model.toasts.last, "the refusal is reported")
        let error = try XCTUnwrap(toast.error, "as a failure, not an info line")
        XCTAssertEqual(error.verb, "add members")
        XCTAssertFalse(error.sentence.isEmpty)
        XCTAssertFalse(model.peopleChat(Self.chatId)!.hasAgent(Self.agentId), "the roster is untouched")
        XCTAssertFalse(model.messages[Self.chatId, default: []].contains { $0.id.hasPrefix(OptimisticNotice.idPrefix) },
                       "the optimistic “added” line is withdrawn")

        // The Add members sheet up (it claims these verbs): the same refusal draws in the sheet's
        // own note under the search field, not as a toast.
        model.failureClaim = FailureClaim(surface: "people-sheet", verbs: ["add members", "add @", "add the agent", "add them", "create the group"])
        let toastsBefore = model.toasts.count
        let okWithSheet = await model.addMembers(chatId: Self.chatId, handles: [], agentIds: [Self.agentId])
        XCTAssertFalse(okWithSheet)
        let claimed = try XCTUnwrap(model.claimedFailure, "the sheet shows it inline")
        XCTAssertEqual(claimed.verb, "add members")
        XCTAssertEqual(model.toasts.count, toastsBefore, "no toast while the sheet claims the verb")
    }

    func testAnOldRelayWithoutTheBatchRouteGetsThePerItemCalls() async throws {
        server.respond = { req in
            guard req.path.hasPrefix("/chats/") else { return (404, Data("{}".utf8)) }
            let body = (try? JSONSerialization.jsonObject(with: req.body) as? [String: Any]) ?? [:]
            if body["handles"] != nil { return (400, #"{"error":{"type":"bad_request","message":"handle required"}}"#.data(using: .utf8)!) }
            if req.path.hasSuffix("/agents") { return (200, Self.relayChatAfterAdd) }
            return (200, Self.relayChatAfterAdd)
        }
        let ok = await model.addMembers(chatId: Self.chatId, handles: ["amb-third-1026"], agentIds: [Self.agentId])
        XCTAssertTrue(ok)
        XCTAssertEqual(server.chatRequests.filter { $0.method == "POST" }.map(\.path),
                       ["/chats/\(Self.chatId)/members", "/chats/\(Self.chatId)/members", "/chats/\(Self.chatId)/agents"])
        XCTAssertTrue(model.peopleChat(Self.chatId)!.hasAgent(Self.agentId))
    }

    /// `GET /chats/01M5XATTTX957RTR99XNE0W9BD` on relay.arbos.life after the batch add (scratch
    /// users amb-owner-1026 / amb-guest-1026 / amb-third-1026 and the agent "Scratch Agent").
    static let relayChatAfterAdd: Data = #"""
    {"archived_at":null,"avatar_leaf":null,"avatar_leaf_color":null,"created_at":1793059547997,"created_by":"test_01M5XATTDM46M7BDXANR0ECHVQ","head_seq":3,"id":"01M5XATTTX957RTR99XNE0W9BD","image_key":null,"kind":"group","last_activity_at":1793059563925,"lock_state":"visible_to_agents","me":{"archived_at":null,"muted":false,"read_receipts":true,"role":"owner","silenced":false},"members":[{"added_by":"test_01M5XATTDM46M7BDXANR0ECHVQ","avatar_color_index_effective":6,"avatar_url":null,"handle":"amb-owner-1026","joined_at":1793059547997,"last_seen_at":1793071800000,"member":"user:test_01M5XATTDM46M7BDXANR0ECHVQ","name":"AMB Owner","online":true,"read_receipts":true,"role":"owner","silenced":false,"status":"member","user_id":"test_01M5XATTDM46M7BDXANR0ECHVQ"},{"added_by":"test_01M5XATTDM46M7BDXANR0ECHVQ","avatar_color_index_effective":0,"avatar_url":null,"handle":"amb-guest-1026","joined_at":1793059547998,"last_seen_at":1793071800000,"member":"user:test_01M5XATTGZKXFJ23CCYFFNT8EF","name":"AMB Guest","online":true,"read_receipts":true,"role":"member","silenced":false,"status":"member","user_id":"test_01M5XATTGZKXFJ23CCYFFNT8EF"},{"added_by":"test_01M5XATTDM46M7BDXANR0ECHVQ","avatar":"olive","avatar_color":"slate","avatar_color_index":null,"avatar_color_index_effective":4,"avatar_leaf":null,"avatar_leaf_color":null,"avatar_leaf_color_effective":2,"avatar_leaf_effective":2,"description":"","handle":"scratch-agent","joined_at":1793059557140,"member":"agent:01M5XATTQM8KRH94R070AA27Y0","owner":"test_01M5XATTDM46M7BDXANR0ECHVQ","owner_handle":"amb-owner-1026","project":{"avatar":"olive","avatar_color":"slate","avatar_color_index":null,"avatar_color_index_effective":4,"avatar_leaf":null,"avatar_leaf_color":null,"avatar_leaf_color_effective":2,"avatar_leaf_effective":2,"avatar_version":1793059547892,"color":"#8b7cf6","description":"","icon":"Sa","id":"01M5XATTQM8KRH94R070AA27Y0","image_key":null,"model":"anthropic/claude-haiku-5.5","name":"Scratch Agent","owner":{"avatar":null,"avatar_color":null,"avatar_color_index":null,"avatar_color_index_effective":6,"avatar_url":null,"handle":"amb-owner-1026","id":"test_01M5XATTDM46M7BDXANR0ECHVQ","kind":"user","last_seen_at":1793071800000,"name":"AMB Owner","online":true}},"role":"agent","settings":{"reply_policy":"mention","restrict_guests":false}},{"added_by":"test_01M5XATTDM46M7BDXANR0ECHVQ","avatar_color_index_effective":1,"avatar_url":null,"handle":"amb-third-1026","joined_at":1793059557140,"last_seen_at":1793071800000,"member":"user:test_01M5XATTMAKGAKK1Q11CT56MBC","name":"AMB Third","online":true,"read_receipts":true,"role":"member","silenced":false,"status":"member","user_id":"test_01M5XATTMAKGAKK1Q11CT56MBC"}],"pending":false,"read_seq":0,"room":"/room/people/01M5XATTTX957RTR99XNE0W9BD","title":"Add-members scratch group with a deliberately very long name","unread":false}
    """#.data(using: .utf8)!
}

// MARK: - Loopback HTTP/1.1 relay

/// One-request-per-connection HTTP server on 127.0.0.1: reads a request, hands it to
/// `respond`, writes `HTTP/1.1 <status>` with a JSON body and closes.
///
/// Every wait in here is bounded and every bound is answered. That is not decoration: the
/// version of this harness before 2026-11-07 waited for the listener to become ready and
/// then asserted that the port was non-zero, which is a different proposition — a listener
/// that had failed reached the same guard. It also left its connections running on `queue`
/// after the owning test was gone, and closed each response from inside `.contentProcessed`,
/// which is the transport having accepted the bytes rather than the client having read them.
final class LoopbackRelay {
    struct Request {
        var method: String
        var path: String
        var headers: [String: String]
        var body: Data
    }

    /// Why the listener never came up. A listener that failed says so, instead of arriving at
    /// a port assertion that reports the wrong thing about the wrong object.
    enum StartFailure: Error, CustomStringConvertible {
        case listenerFailed(NWError?)
        case neverSettled(TimeInterval)
        case readyWithoutPort

        var description: String {
            switch self {
            case .listenerFailed(let error):
                return "loopback listener failed to start: \(error.map { "\($0)" } ?? "no reason given")"
            case .neverSettled(let seconds):
                return "loopback listener was neither ready nor failed after \(seconds)s"
            case .readyWithoutPort:
                return "loopback listener reported ready without a port"
            }
        }
    }

    /// Teardown could not account for every connection it opened. Worth failing the test over:
    /// a connection still running on `queue` belongs to a relay whose test has ended.
    struct StopFailure: Error, CustomStringConvertible {
        let stranded: Int
        let after: TimeInterval
        var description: String { "\(stranded) loopback connection(s) still running \(after)s after cancel" }
    }

    /// Both ends of a half-sent request wait for each other — the relay for the rest of a body,
    /// the client for a response — so the relay has to give up first to be the one that reports.
    ///
    /// Sized against measurement, because a bound close to the healthy tail is itself a flake
    /// source. Across twenty green CI runs these three tests have a median of 0.030–0.118 s
    /// but a tail that reaches **4.344 s** while still passing, so a five-second bound would
    /// have sat 0.7 s above the worst healthy run observed. `connectionDeadline` must also stay
    /// under `RelayAPI`'s own 30 s request timeout: inside that band the client was going to
    /// fail the call anyway, so this bound changes which side reports a stall, never whether
    /// one is reported.
    private static let startDeadline: TimeInterval = 10
    private static let connectionDeadline: TimeInterval = 20
    private static let drainDeadline: TimeInterval = 20
    /// How often the drain re-reads connection state, so it never depends on a state callback
    /// arriving to notice that a connection has already gone down.
    private static let drainPoll: TimeInterval = 0.05

    private let listener: NWListener
    private let queue = DispatchQueue(label: "loopback-relay")
    private let lock = NSCondition()
    private var _requests: [Request] = []
    private var _respond: (Request) -> (Int, Data) = { _ in (404, Data("{}".utf8)) }
    private var _live: [ObjectIdentifier: NWConnection] = [:]

    var requests: [Request] { lock.lock(); defer { lock.unlock() }; return _requests }
    /// The model also probes `/healthz` and the like on a fresh relay; only the chat calls matter here.
    var chatRequests: [Request] { requests.filter { $0.path.hasPrefix("/chats/") } }
    /// Written by the test on the main actor, read on `queue` once per connection, so it takes
    /// the same lock as the `_requests` beside it rather than being the one unguarded property.
    var respond: (Request) -> (Int, Data) {
        get { lock.lock(); defer { lock.unlock() }; return _respond }
        set { lock.lock(); defer { lock.unlock() }; _respond = newValue }
    }
    private(set) var port: UInt16 = 0

    init() throws {
        let params = NWParameters.tcp
        params.requiredLocalEndpoint = NWEndpoint.hostPort(host: "127.0.0.1", port: .any)
        listener = try NWListener(using: params)

        // Ready, failed and never-settled are three outcomes and get three answers. The
        // semaphore only reports that the handler fired, so what it fired with is recorded
        // alongside it.
        let settled = DispatchSemaphore(value: 0)
        let outcome = StartOutcome()
        listener.stateUpdateHandler = { state in
            switch state {
            case .ready: outcome.record(.ready); settled.signal()
            case .failed(let error): outcome.record(.failed(error)); settled.signal()
            case .cancelled: outcome.record(.failed(nil)); settled.signal()
            default: break
            }
        }
        listener.newConnectionHandler = { [weak self] conn in self?.serve(conn) }
        listener.start(queue: queue)

        if settled.wait(timeout: .now() + Self.startDeadline) == .timedOut {
            listener.cancel()
            throw StartFailure.neverSettled(Self.startDeadline)
        }
        if case .failed(let error) = outcome.value {
            listener.cancel()
            throw StartFailure.listenerFailed(error)
        }
        guard let p = listener.port?.rawValue, p != 0 else {
            listener.cancel()
            throw StartFailure.readyWithoutPort
        }
        port = p
    }

    /// Any terminal state counts as down. Waiting specifically for `.cancelled` to be
    /// *delivered* would make the drain depend on a callback that nothing in this harness has
    /// ever exercised, and the cost of being wrong about it is the full bound on every
    /// teardown rather than a slow report.
    private static func isDown(_ state: NWConnection.State) -> Bool {
        switch state {
        case .cancelled, .failed: return true
        default: return false
        }
    }

    /// Cancels the listener *and* every connection it opened, and does not return until each
    /// one is down. A connection that outlives its test keeps a finished relay alive on a
    /// shared queue, and a callback delivered into that can take the test host out without
    /// writing anything anybody can read afterwards.
    ///
    /// Because the connections the harness itself finished have already left `_live`, and
    /// because anything already terminal is pruned by state below, this only ever blocks on a
    /// connection that is genuinely still live.
    func stop() throws {
        listener.cancel()
        let deadline = Date().addingTimeInterval(Self.drainDeadline)
        var cancelled: Set<ObjectIdentifier> = []
        lock.lock()
        defer { lock.unlock() }
        while true {
            for (id, conn) in _live where Self.isDown(conn.state) { _live[id] = nil }
            if _live.isEmpty { return }
            if Date() >= deadline { throw StopFailure(stranded: _live.count, after: Self.drainDeadline) }

            // Re-checked each pass: a connection accepted after the previous one would
            // otherwise never be cancelled by anyone, and the drain would time out blaming it.
            let pending = _live.filter { !cancelled.contains($0.key) }
            if !pending.isEmpty {
                cancelled.formUnion(pending.keys)
                lock.unlock()
                for conn in pending.values { conn.cancel() }
                lock.lock()
                continue
            }
            // Unlike the listener wait in `init`, where the result was the only signal there
            // was, nothing here needs to tell a broadcast from the poll interval expiring: the
            // top of the loop re-reads every connection's state and the deadline itself.
            _ = lock.wait(until: min(deadline, Date().addingTimeInterval(Self.drainPoll)))
        }
    }

    private func serve(_ conn: NWConnection) {
        lock.lock(); _live[ObjectIdentifier(conn)] = conn; lock.unlock()
        conn.stateUpdateHandler = { [weak self] state in
            switch state {
            case .cancelled, .failed: self?.forget(conn)
            default: break
            }
        }
        conn.start(queue: queue)

        let deadline = Date().addingTimeInterval(Self.connectionDeadline)
        var buffer = Data()
        func read() {
            // Deliberately not `finish` here: that would capture `self` into this local
            // function and retain-cycle it through the connection. The drain prunes by state,
            // so a connection cancelled on this path still leaves `_live` promptly.
            guard Date() < deadline else { conn.cancel(); return }
            conn.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [weak self] data, _, done, error in
                guard let self else { conn.cancel(); return }
                guard error == nil else { self.finish(conn); return }
                if let data { buffer.append(data) }
                if let req = Self.parse(buffer) {
                    self.lock.lock(); self._requests.append(req); self.lock.unlock()
                    let responder = self.respond
                    let (status, body) = responder(req)
                    let head = "HTTP/1.1 \(status) \(status == 200 ? "OK" : "Error")\r\nContent-Type: application/json\r\nContent-Length: \(body.count)\r\nConnection: close\r\n\r\n"
                    // `isComplete: true` is the default; it is spelled out because the FIN
                    // ordering behind the bytes is load-bearing for `Connection: close`. What
                    // changed is below it: wait for the client's own close instead of
                    // cancelling from `.contentProcessed`, which reports that the transport
                    // took the bytes and not that the client received them.
                    conn.send(content: Data(head.utf8) + body, isComplete: true,
                              completion: .contentProcessed { [weak self] _ in
                                  self?.awaitClose(conn, by: deadline)
                              })
                } else if done {
                    self.finish(conn)
                } else {
                    read()
                }
            }
        }
        read()
    }

    /// Holds the connection open until the client closes it, so the response is never cut
    /// short, and gives up at the same deadline as everything else here.
    private func awaitClose(_ conn: NWConnection, by deadline: Date) {
        guard Date() < deadline else { finish(conn); return }
        conn.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [weak self] _, _, done, error in
            guard let self else { conn.cancel(); return }
            guard error == nil, !done else { self.finish(conn); return }
            self.awaitClose(conn, by: deadline)
        }
    }

    /// The harness is done with this connection: cancel it and stop counting it, rather than
    /// cancelling and then waiting to be told. Every path the harness drives goes through
    /// here, which is what leaves the drain with only genuinely live connections to block on.
    private func finish(_ conn: NWConnection) {
        conn.cancel()
        forget(conn)
    }

    private func forget(_ conn: NWConnection) {
        lock.lock()
        _live[ObjectIdentifier(conn)] = nil
        lock.broadcast()
        lock.unlock()
    }

    /// The listener's terminal start state, written on `queue` and read by `init`.
    private final class StartOutcome {
        enum Value {
            case pending
            case ready
            case failed(NWError?)
        }

        private let lock = NSLock()
        private var stored: Value = .pending

        var value: Value { lock.lock(); defer { lock.unlock() }; return stored }

        func record(_ next: Value) {
            lock.lock()
            defer { lock.unlock() }
            if case .pending = stored { stored = next }
        }
    }

    /// nil until the head and the whole `Content-Length` body have arrived.
    private static func parse(_ data: Data) -> Request? {
        guard let split = data.range(of: Data("\r\n\r\n".utf8)) else { return nil }
        let head = String(decoding: data[..<split.lowerBound], as: UTF8.self)
        var lines = head.components(separatedBy: "\r\n")
        let first = lines.removeFirst().split(separator: " ")
        guard first.count >= 2 else { return nil }
        var headers: [String: String] = [:]
        for line in lines {
            guard let colon = line.firstIndex(of: ":") else { continue }
            headers[line[..<colon].lowercased()] = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
        }
        let length = Int(headers["content-length"] ?? "0") ?? 0
        let body = data[split.upperBound...]
        guard body.count >= length else { return nil }
        return Request(method: String(first[0]), path: String(first[1]), headers: headers, body: Data(body.prefix(length)))
    }
}
