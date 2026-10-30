import XCTest
@testable import Arbos
import ArbosFFI

/// Fake WebSocket for `SessionRoom`: records outbound frames, delivers inbound
/// frames (or errors) into `receive()`, and fires `onOpen` on demand.
@MainActor
final class FakeSocket: RoomSocket {
    var onOpen: (() -> Void)?
    var httpStatus: Int?
    var closeCode: Int?
    private(set) var sent: [Data] = []
    private var receiveWaiters: [CheckedContinuation<Data, Error>] = []
    private var pending: [Result<Data, Error>] = []

    func resume() {}

    func send(_ data: Data) async throws {
        sent.append(data)
    }

    func receive() async throws -> Data {
        if !pending.isEmpty {
            return try pending.removeFirst().get()
        }
        return try await withCheckedThrowingContinuation { cont in
            receiveWaiters.append(cont)
        }
    }

    func cancel() {
        let waiters = receiveWaiters
        receiveWaiters.removeAll()
        for w in waiters {
            w.resume(throwing: URLError(.cancelled))
        }
    }

    func fireOpen() {
        onOpen?()
    }

    func deliver(_ data: Data) {
        if !receiveWaiters.isEmpty {
            receiveWaiters.removeFirst().resume(returning: data)
        } else {
            pending.append(.success(data))
        }
    }

    func failReceive(_ error: Error = URLError(.networkConnectionLost), httpStatus: Int? = nil) {
        if let httpStatus { self.httpStatus = httpStatus }
        if !receiveWaiters.isEmpty {
            receiveWaiters.removeFirst().resume(throwing: error)
        } else {
            pending.append(.failure(error))
        }
    }
}

@MainActor
final class SessionRoomTests: XCTestCase {
    func testNoHelloBeforeOpen() async throws {
        let sockets = SocketFactory()
        let room = SessionRoom(
            projectId: "proj",
            key: "chat",
            roomChat: "chat",
            doc: CoreChatDoc(key: "chat"),
            roomURL: { _ in URL(string: "wss://example.test/r")! },
            makeSocket: { sockets.make($0) },
            fetchCheckpoint: { _ in (0, Data()) },
            deviceId: { "dev:me" }
        )
        room.start()
        try await waitUntil { sockets.current != nil }
        let sock = try XCTUnwrap(sockets.current)
        XCTAssertTrue(sock.sent.isEmpty, "nothing sent before onOpen")

        sock.fireOpen()
        try await waitUntil { !sock.sent.isEmpty }

        let first = try decodeFrame(bytes: sock.sent[0])
        XCTAssertEqual(first.ty, .hello, "first frame after open must be HELLO")
        room.stop()
    }

    func testStaleCheckpointIgnored() async throws {
        let sockets = SocketFactory()
        let fetches = FetchGate()

        let other = CoreChatDoc(key: "other")
        try other.insertUserMessage(
            id: "m1", text: "x", deviceId: "d", author: .init(sender: "u")
        )
        other.commit()
        let frontier = other.oplogVersion
        let staleBytes = try other.exportSnapshot()

        let doc = CoreChatDoc(key: "chat")
        let room = SessionRoom(
            projectId: "proj",
            key: "chat",
            roomChat: "chat",
            doc: doc,
            roomURL: { _ in URL(string: "wss://example.test/r")! },
            makeSocket: { sockets.make($0) },
            fetchCheckpoint: { _ in try await fetches.next() },
            deviceId: { "dev:me" }
        )
        room.start()
        try await waitUntil { sockets.sockets.count == 1 }
        let sock1 = sockets.sockets[0]
        sock1.fireOpen()
        try await waitUntil { !sock1.sent.isEmpty }

        let state5 = try encodeState(checkpointSeq: 5, frontier: frontier)
        sock1.deliver(state5)
        try await waitUntil { fetches.waiting != nil }
        let firstFetch = try XCTUnwrap(fetches.takeWaiting())

        sock1.failReceive()
        try await waitUntil { sockets.sockets.count == 2 }
        let sock2 = sockets.sockets[1]
        sock2.fireOpen()
        try await waitUntil { !sock2.sent.isEmpty }

        let state9 = try encodeState(checkpointSeq: 9, frontier: Data())
        sock2.deliver(state9)
        try await waitUntil { fetches.waiting != nil }

        firstFetch.resume(returning: (5, staleBytes))
        // Stale resume must not import; give the cancelled task a turn.
        for _ in 0..<20 {
            await Task.yield()
            try await Task.sleep(nanoseconds: 5_000_000)
        }

        XCTAssertTrue(doc.isEmpty, "stale checkpoint must not be imported after reconnect")
        XCTAssertLessThan(room.cursor, Int64(9), "cursor must stay below the newer checkpointSeq")
        room.stop()
    }

    func testHttp403DoesNotReconnect() async throws {
        let sockets = SocketFactory()
        let room = SessionRoom(
            projectId: "people",
            key: "chat-gone",
            roomChat: "chat-gone",
            doc: CoreChatDoc(key: "chat-gone"),
            roomURL: { _ in URL(string: "wss://example.test/r")! },
            makeSocket: { sockets.make($0) },
            fetchCheckpoint: { _ in (0, Data()) },
            deviceId: { "dev:me" }
        )
        room.start()
        try await waitUntil { sockets.sockets.count == 1 }
        let sock = try XCTUnwrap(sockets.current)
        sock.failReceive(httpStatus: 403)
        try await waitUntil { room.lastError != nil }
        // Brief settle: must not dial again after the terminal 403.
        for _ in 0..<20 {
            await Task.yield()
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        XCTAssertEqual(sockets.sockets.count, 1, "HTTP 403 must not open a second socket")
        let err = try XCTUnwrap(room.lastError)
        XCTAssertTrue(err.contains("HTTP 403"), "lastError=\(err)")
        room.stop()
    }

    func testPlainReceiveErrorStillReconnects() async throws {
        let sockets = SocketFactory()
        let room = SessionRoom(
            projectId: "proj",
            key: "chat",
            roomChat: "chat",
            doc: CoreChatDoc(key: "chat"),
            roomURL: { _ in URL(string: "wss://example.test/r")! },
            makeSocket: { sockets.make($0) },
            fetchCheckpoint: { _ in (0, Data()) },
            deviceId: { "dev:me" }
        )
        room.start()
        try await waitUntil { sockets.sockets.count == 1 }
        let sock = try XCTUnwrap(sockets.current)
        sock.fireOpen()
        try await waitUntil { !sock.sent.isEmpty }
        sock.failReceive()
        try await waitUntil { sockets.sockets.count == 2 }
        room.stop()
    }

    func testFailedRowImportLeavesCursorAndReconnects() async throws {
        let sockets = SocketFactory()
        let room = SessionRoom(
            projectId: "proj",
            key: "chat",
            roomChat: "chat",
            doc: CoreChatDoc(key: "chat"),
            roomURL: { _ in URL(string: "wss://example.test/r")! },
            makeSocket: { sockets.make($0) },
            fetchCheckpoint: { _ in (0, Data()) },
            cursor: 20,
            deviceId: { "dev:me" }
        )
        room.start()
        try await waitUntil { sockets.sockets.count == 1 }
        let sock1 = try XCTUnwrap(sockets.current)
        sock1.fireOpen()
        try await waitUntil { !sock1.sent.isEmpty }

        // STATE with no checkpoint → Live + RowsReq.
        sock1.deliver(try encodeState(checkpointSeq: 0, frontier: Data()))
        try await waitUntil {
            sock1.sent.contains { data in
                (try? decodeFrame(bytes: data))?.ty == .rowsReq
            }
        }
        XCTAssertEqual(room.cursor, 20)

        // ROW whose payload is not a Loro update — import fails.
        let header = try JSONSerialization.data(
            withJSONObject: [
                "seq": Int64(21),
                "device": "dev:other",
                "batchId": "batch-bad",
            ] as [String: Any]
        )
        let headerJson = try XCTUnwrap(String(data: header, encoding: .utf8))
        let badPayload = Data("not-a-loro-update".utf8)
        sock1.deliver(try encodeFrame(ty: .row, headerJson: headerJson, payload: badPayload))

        try await waitUntil { sockets.sockets.count == 2 }
        XCTAssertEqual(room.cursor, 20, "failed import must not advance the room cursor")
        room.stop()
    }

    func testNotAvailableDoesNotCallOnUnavailableOrOpenANewSocket() async throws {
        let sockets = SocketFactory()
        let room = SessionRoom(
            projectId: "people",
            key: "chat",
            roomChat: "chat",
            doc: CoreChatDoc(key: "chat"),
            roomURL: { _ in URL(string: "wss://example.test/r")! },
            makeSocket: { sockets.make($0) },
            fetchCheckpoint: { _ in (0, Data()) },
            pending: [("batch-1", Data("pending".utf8))],
            deviceId: { "dev:me" }
        )
        var unavailableCalls = 0
        var pushRefusedCalls = 0
        room.onUnavailable = { unavailableCalls += 1 }
        room.onPushRefused = { pushRefusedCalls += 1 }
        room.start()
        try await waitUntil { sockets.sockets.count == 1 }
        let sock = try XCTUnwrap(sockets.current)
        sock.fireOpen()
        try await waitUntil { !sock.sent.isEmpty }

        sock.deliver(try encodeState(checkpointSeq: 0, frontier: Data()))
        try await waitUntil {
            sock.sent.contains { data in
                (try? decodeFrame(bytes: data))?.ty == .rowsReq
            }
        }

        let rowsDone = try encodeFrame(
            ty: .rowsDone,
            headerJson: #"{"headSeq":0}"#,
            payload: Data()
        )
        sock.deliver(rowsDone)
        try await waitUntil {
            sock.sent.contains { data in
                (try? decodeFrame(bytes: data))?.ty == .push
            }
        }

        let errHeader = try JSONSerialization.data(
            withJSONObject: [
                "code": "not_available",
                "message": "this chat is not available",
                "batchId": "batch-1",
            ] as [String: Any]
        )
        let errJson = try XCTUnwrap(String(data: errHeader, encoding: .utf8))
        sock.deliver(try encodeFrame(ty: .error, headerJson: errJson, payload: Data()))

        try await waitUntil { pushRefusedCalls > 0 || room.lastError != nil }

        // Brief settle: must not dial again after not_available.
        for _ in 0..<30 {
            await Task.yield()
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        XCTAssertEqual(sockets.sockets.count, 1, "not_available must not open a second socket")
        XCTAssertEqual(room.reconnects, 0)
        XCTAssertEqual(unavailableCalls, 0, "not_available must not call onUnavailable")
        XCTAssertEqual(pushRefusedCalls, 1)
        let err = try XCTUnwrap(room.lastError)
        XCTAssertTrue(err.contains("not_available"), "lastError=\(err)")
        room.stop()
    }

    /// [feedback 2005]: a refused room resends its pending batch — same batch id, same socket,
    /// no redial — when the host says the condition may have changed, lifts the refusal, and
    /// the relay's ACK then clears the pending and the notice.
    func testRetryRefusedResendsPendingWithTheSameBatchIdOnTheSameSocket() async throws {
        let sockets = SocketFactory()
        let room = SessionRoom(
            projectId: "people",
            key: "chat",
            roomChat: "chat",
            doc: CoreChatDoc(key: "chat"),
            roomURL: { _ in URL(string: "wss://example.test/r")! },
            makeSocket: { sockets.make($0) },
            fetchCheckpoint: { _ in (0, Data()) },
            pending: [("batch-1", Data("pending".utf8))],
            deviceId: { "dev:me" }
        )
        var pushRefusedCalls = 0
        var acks = 0
        room.onPushRefused = { pushRefusedCalls += 1 }
        room.onAcked = { acks += 1 }
        room.start()
        try await waitUntil { sockets.sockets.count == 1 }
        let sock = try XCTUnwrap(sockets.current)
        sock.fireOpen()
        try await waitUntil { !sock.sent.isEmpty }
        sock.deliver(try encodeState(checkpointSeq: 0, frontier: Data()))
        try await waitUntil {
            sock.sent.contains { (try? decodeFrame(bytes: $0))?.ty == .rowsReq }
        }
        sock.deliver(try encodeFrame(ty: .rowsDone, headerJson: #"{"headSeq":0}"#, payload: Data()))
        try await waitUntil { self.pushBatchIds(sock.sent) == ["batch-1"] }

        // Blocked by the other person of the DM: refused, standing (no tight loop).
        let errHeader = try JSONSerialization.data(
            withJSONObject: [
                "code": "not_available",
                "message": "someone in this chat has blocked you",
                "batchId": "batch-1",
                "reason": "blocked_by",
                "byHandle": "sami",
                "byName": "Sami Kassab",
            ] as [String: Any]
        )
        let errJson = try XCTUnwrap(String(data: errHeader, encoding: .utf8))
        sock.deliver(try encodeFrame(ty: .error, headerJson: errJson, payload: Data()))
        try await waitUntil { pushRefusedCalls == 1 }
        XCTAssertTrue(room.isPushRefused)
        XCTAssertEqual(room.lastRefusal?.reason, "blocked_by")
        XCTAssertEqual(room.lastRefusal?.byName, "Sami Kassab")

        // Nothing changed yet: the nudge resends once with the original id and lifts the flag.
        XCTAssertTrue(room.retryRefused())
        try await waitUntil { self.pushBatchIds(sock.sent) == ["batch-1", "batch-1"] }
        XCTAssertFalse(room.isPushRefused)
        XCTAssertEqual(sockets.sockets.count, 1, "a retry never redials")
        XCTAssertEqual(room.reconnects, 0)
        // A second nudge right away collapses; one on a room that is not refused sends nothing.
        XCTAssertFalse(room.retryRefused())
        XCTAssertEqual(pushBatchIds(sock.sent), ["batch-1", "batch-1"])

        // The relay already held it (dup ACK): pending empties, the host hears the ACK.
        sock.deliver(try encodeFrame(
            ty: .ack, headerJson: #"{"batchId":"batch-1","seq":4,"dup":true}"#, payload: Data()
        ))
        try await waitUntil { acks == 1 }
        XCTAssertTrue(room.pendingBatches.isEmpty)
        XCTAssertFalse(room.isPushRefused)
        room.stop()
    }

    /// [feedback 2005]: only pushes that change who may post into a chat nudge its room.
    func testChatToRetryNamesOnlyMembershipAndSettingsChanges() {
        XCTAssertEqual(
            RefusedSendRetry.chatToRetry(.peopleChatUpdated(PeopleChatUpdatedPayloadDto(chat_id: "dm-1"))),
            "dm-1"
        )
        XCTAssertEqual(
            RefusedSendRetry.chatToRetry(.peopleMemberAdded(
                PeopleMemberAddedPayloadDto(chat_id: "g-2", user_id: "user:sam", handle: "sam", by_handle: "me")
            )),
            "g-2"
        )
        XCTAssertEqual(
            RefusedSendRetry.chatToRetry(.peopleMemberRemoved(
                PeopleMemberRemovedPayloadDto(chat_id: "g-3", user_id: "user:sam", handle: "sam", by_handle: "me", banned: nil)
            )),
            "g-3"
        )
        XCTAssertNil(RefusedSendRetry.chatToRetry(.peopleChatUpdated(PeopleChatUpdatedPayloadDto(chat_id: ""))))
        XCTAssertNil(RefusedSendRetry.chatToRetry(.peopleChatDeleted(PeopleChatDeletedPayloadDto(chat_id: "dm-1", by_handle: "sam"))))
        XCTAssertNil(RefusedSendRetry.chatToRetry(.chatArchived(ChatArchivedPayloadDto(chat_id: "dm-1", project_id: "dm-1", archived_at: 5))))
    }

    private func pushBatchIds(_ sent: [Data]) -> [String] {
        sent.compactMap { data -> String? in
            guard let frame = try? decodeFrame(bytes: data), frame.ty == .push,
                  let obj = try? JSONSerialization.jsonObject(with: Data(frame.headerJson.utf8)) as? [String: Any]
            else { return nil }
            return obj["batchId"] as? String
        }
    }

    func testRateLimitedDoesNotOpenANewSocket() async throws {
        let sockets = SocketFactory()
        let room = SessionRoom(
            projectId: "people",
            key: "chat",
            roomChat: "chat",
            doc: CoreChatDoc(key: "chat"),
            roomURL: { _ in URL(string: "wss://example.test/r")! },
            makeSocket: { sockets.make($0) },
            fetchCheckpoint: { _ in (0, Data()) },
            pending: [("batch-1", Data("pending".utf8))],
            deviceId: { "dev:me" }
        )
        room.start()
        try await waitUntil { sockets.sockets.count == 1 }
        let sock = try XCTUnwrap(sockets.current)
        sock.fireOpen()
        try await waitUntil { !sock.sent.isEmpty }

        sock.deliver(try encodeState(checkpointSeq: 0, frontier: Data()))
        try await waitUntil {
            sock.sent.contains { data in
                (try? decodeFrame(bytes: data))?.ty == .rowsReq
            }
        }

        let rowsDone = try encodeFrame(
            ty: .rowsDone,
            headerJson: #"{"headSeq":0}"#,
            payload: Data()
        )
        sock.deliver(rowsDone)
        try await waitUntil {
            sock.sent.contains { data in
                (try? decodeFrame(bytes: data))?.ty == .push
            }
        }

        let errHeader = try JSONSerialization.data(
            withJSONObject: [
                "code": "rate_limited",
                "message": "slow down",
                "batchId": "batch-1",
                "retryS": Int64(5),
            ] as [String: Any]
        )
        let errJson = try XCTUnwrap(String(data: errHeader, encoding: .utf8))
        sock.deliver(try encodeFrame(ty: .error, headerJson: errJson, payload: Data()))

        // Brief settle: must not dial again after rate_limited.
        for _ in 0..<30 {
            await Task.yield()
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        XCTAssertEqual(sockets.sockets.count, 1, "rate_limited must not open a second socket")
        XCTAssertEqual(room.reconnects, 0)
        room.stop()
    }

    // MARK: - Helpers

    private func encodeState(checkpointSeq: Int64, frontier: Data) throws -> Data {
        let header = try JSONSerialization.data(
            withJSONObject: [
                "headSeq": Int64(10),
                "seqFloor": Int64(0),
                "checkpointSeq": checkpointSeq,
                "checkpointSize": Int64(frontier.count),
                "rowCount": Int64(0),
                "rowBytes": Int64(0),
                "box": "awake",
            ] as [String: Any]
        )
        let headerJson = try XCTUnwrap(String(data: header, encoding: .utf8))
        return try encodeFrame(ty: .state, headerJson: headerJson, payload: frontier)
    }

    private func waitUntil(
        timeout: TimeInterval = 3,
        file: StaticString = #filePath,
        line: UInt = #line,
        _ predicate: @escaping @MainActor () -> Bool
    ) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while !predicate() {
            if Date() > deadline {
                XCTFail("timed out waiting", file: file, line: line)
                throw WaitTimeout()
            }
            await Task.yield()
            try await Task.sleep(nanoseconds: 10_000_000)
        }
    }
}

private struct WaitTimeout: Error {}

@MainActor
private final class SocketFactory {
    private(set) var sockets: [FakeSocket] = []
    var current: FakeSocket? { sockets.last }

    func make(_ url: URL) -> RoomSocket {
        let s = FakeSocket()
        sockets.append(s)
        return s
    }
}

@MainActor
private final class FetchGate {
    private(set) var waiting: CheckedContinuation<(Int64, Data), Error>?

    func next() async throws -> (Int64, Data) {
        try await withCheckedThrowingContinuation { cont in
            waiting = cont
        }
    }

    func takeWaiting() -> CheckedContinuation<(Int64, Data), Error>? {
        let c = waiting
        waiting = nil
        return c
    }
}
