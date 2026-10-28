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

    override func tearDown() {
        server.stop()
        server = nil
        model = nil
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
final class LoopbackRelay {
    struct Request {
        var method: String
        var path: String
        var headers: [String: String]
        var body: Data
    }

    private let listener: NWListener
    private let queue = DispatchQueue(label: "loopback-relay")
    private let lock = NSLock()
    private var _requests: [Request] = []
    var requests: [Request] { lock.lock(); defer { lock.unlock() }; return _requests }
    /// The model also probes `/healthz` and the like on a fresh relay; only the chat calls matter here.
    var chatRequests: [Request] { requests.filter { $0.path.hasPrefix("/chats/") } }
    var respond: (Request) -> (Int, Data) = { _ in (404, Data("{}".utf8)) }
    private(set) var port: UInt16 = 0

    init() throws {
        let params = NWParameters.tcp
        params.requiredLocalEndpoint = NWEndpoint.hostPort(host: "127.0.0.1", port: .any)
        listener = try NWListener(using: params)
        let ready = DispatchSemaphore(value: 0)
        listener.stateUpdateHandler = { state in
            if case .ready = state { ready.signal() }
            if case .failed = state { ready.signal() }
        }
        listener.newConnectionHandler = { [weak self] conn in self?.serve(conn) }
        listener.start(queue: queue)
        _ = ready.wait(timeout: .now() + 5)
        guard let p = listener.port?.rawValue, p != 0 else { throw NSError(domain: "LoopbackRelay", code: 1) }
        port = p
    }

    func stop() { listener.cancel() }

    private func serve(_ conn: NWConnection) {
        conn.start(queue: queue)
        var buffer = Data()
        func read() {
            conn.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [weak self] data, _, done, error in
                guard let self, error == nil else { conn.cancel(); return }
                if let data { buffer.append(data) }
                if let req = Self.parse(buffer) {
                    self.lock.lock(); self._requests.append(req); self.lock.unlock()
                    let (status, body) = self.respond(req)
                    let head = "HTTP/1.1 \(status) \(status == 200 ? "OK" : "Error")\r\nContent-Type: application/json\r\nContent-Length: \(body.count)\r\nConnection: close\r\n\r\n"
                    conn.send(content: Data(head.utf8) + body, completion: .contentProcessed { _ in conn.cancel() })
                } else if done {
                    conn.cancel()
                } else {
                    read()
                }
            }
        }
        read()
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
