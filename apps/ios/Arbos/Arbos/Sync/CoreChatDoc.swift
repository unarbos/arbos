// Chat doc on arbos-core via ArbosFFI.ChatDoc.
// AppModel / SessionRoom use this as the live chat replica.

import Foundation
import ArbosFFI

/// What a root-level diff touched, mapped to message indexes.
struct Touched {
    /// The `messages` list itself changed (insert/delete): re-read everything.
    var listChanged = false
    var messages = Set<Int>()
    /// Anything outside `messages` (project doc maps, `commands`).
    var other = false
}

/// Loro chat replica backed by `ArbosFFI.ChatDoc` (shared core writers/readers).
final class CoreChatDoc {
    let key: Id
    private let doc: ArbosFFI.ChatDoc
    /// Retain UniFFI foreign listeners for the lifetime of this wrapper.
    private var changeListener: DocChangeListener?
    private var localUpdateListener: LocalUpdateListener?

    init(key: Id) {
        self.key = key
        self.doc = ArbosFFI.ChatDoc.open()
    }

    /// A replica loaded from a full snapshot (the perf rig's exported chat).
    init(key: Id, snapshot: Data) throws {
        self.key = key
        self.doc = try ArbosFFI.ChatDoc.openFromSnapshot(snapshot: snapshot)
    }

    var count: Int { Int(doc.messageCount()) }

    /// Nothing imported or written yet (a fresh replica is not worth saving).
    var isEmpty: Bool { doc.isEmpty() }

    /// Encoded oplog version vector (checkpoint frontier bytes).
    var oplogVersion: Data { doc.oplogVersion() }

    func commit() { doc.commit() }

    func importUpdate(_ bytes: Data) throws {
        try doc.apply(bytes: bytes)
    }

    /// Import an update; `true` when Loro parked it as pending (dependencies missing).
    func importUpdateReportingPending(_ bytes: Data) throws -> Bool {
        try doc.applyUpdate(bytes: bytes)
    }

    /// Does the replica already contain this checkpoint frontier?
    func contains(frontier: Data) -> Bool {
        doc.containsFrontier(frontier: frontier)
    }

    /// Ops this replica holds beyond an encoded version vector (empty when none).
    func updatesBeyond(_ vv: Data) -> Data {
        doc.updatesBeyond(theirsVv: vv)
    }

    /// Bytes for the on-device store.
    func exportSnapshot() throws -> Data {
        doc.snapshot()
    }

    // MARK: Reading

    func readMessage(_ ix: Int) -> Message? {
        guard ix >= 0, ix < count,
              let raw = doc.readMessageJson(index: UInt64(ix)),
              let data = raw.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data)
        else { return nil }
        return DocJSON.message(json, chatId: key)
    }

    /// The whole transcript in one FFI call and one JSON parse. Reading row by
    /// row crossed the FFI `count` times; a long group chat paid for that on every
    /// snapshot (2026-10-23, long-chat-perf).
    func readAll() -> [Message] {
        let raw = doc.readRowsJson()
        guard let data = raw.data(using: .utf8),
              let rows = (try? JSONSerialization.jsonObject(with: data)) as? [Any]
        else { return [] }
        return rows.compactMap { DocJSON.message($0, chatId: key) }
    }

    func mapValue(_ name: String) -> [String: Any] {
        let raw = doc.readMapJson(name: name)
        guard let data = raw.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data),
              let dict = json as? [String: Any]
        else { return [:] }
        return dict
    }

    // MARK: Writing

    struct Author {
        var sender: String
        var handle: String?
        var name: String?
        var avatar: String?
    }

    /// `forwardedFrom` = the original sender's name on a forwarded message (the bubble's
    /// "Forwarded from <name>" header; written as `forwardedFrom` on the row).
    /// `replyRef` = a cross-chat reply (Reply in DM, crdt-sync §6.2 `replyRef`): the quoted
    /// message's address and snapshot, written through core as one value map.
    func insertUserMessage(
        id: Id, text: String, deviceId: String, attachments: [Attachment] = [],
        author: Author? = nil, mentions: [String] = [], replyTo: String? = nil,
        forwardedFrom: String? = nil, replyRef: ReplyRef? = nil
    ) throws {
        let opts = UserMessageOptsRecord(
            attachments: attachments.map {
                UserAttachmentRecord(name: $0.name, mime: $0.mime, kind: $0.kind, blobRef: $0.blobRef)
            },
            mentions: mentions,
            replyTo: replyTo,
            sender: author?.sender,
            senderHandle: author?.handle,
            senderName: author?.name,
            senderAvatar: author?.avatar,
            forwardedFrom: forwardedFrom,
            replyRef: replyRef.map {
                ReplyRefRecord(chatId: $0.chatId, projectId: $0.projectId, messageId: $0.messageId,
                               chatTitle: $0.chatTitle, sender: $0.sender, senderName: $0.senderName,
                               senderHandle: $0.senderHandle, text: $0.text)
            }
        )
        try doc.insertUserMessage(id: id, text: text, deviceId: deviceId, opts: opts)
    }

    /// Optional `callId` is passed to the FFI as `linkId` (core's `callId`).
    func insertSystemLine(id: Id, text: String, deviceId: String, callId: String? = nil) throws {
        try doc.insertSystemLine(id: id, text: text, deviceId: deviceId, linkId: callId)
    }

    func insertAssistantGreeting(
        id: Id, text: String, deviceId: String, projectId: Id, agentName: String
    ) throws {
        try doc.insertAssistantGreeting(
            id: id, text: text, deviceId: deviceId, projectId: projectId, agentName: agentName
        )
    }

    func toggleReaction(messageId: Id, emoji: String, sender: String) throws {
        try doc.toggleReaction(messageId: messageId, emoji: emoji, sender: sender)
    }

    /// Remove a message for everyone. Does not commit. Do not announce.
    func deleteMessage(messageId: Id) throws {
        try doc.deleteMessage(messageId: messageId)
    }

    /// Replace a message's text (edit). Does not commit. Sets `editedAt`.
    func editMessage(messageId: Id, text: String) throws {
        try doc.editMessage(messageId: messageId, text: text)
    }

    /// Append one part to a message from its JSON object (`kind` + fields; a string `id`
    /// is kept). Forward writes a chart part and its fallback picture with it
    /// ([feedback forward-attachments-ios-2026-10-27]). Returns the part id.
    @discardableResult
    func insertPart(messageId: Id, part: [String: Any]) throws -> String {
        let data = try JSONSerialization.data(withJSONObject: part)
        let json = String(data: data, encoding: .utf8) ?? "{}"
        return try doc.addPartJson(messageId: messageId, partJson: json)
    }

    /// Append a `commands` row (`send`, `stop`, `answer`, `retry`, `retitle`). Does not commit.
    func pushCommand(_ kind: String, payload: [String: Any], device: String) throws {
        let data = try JSONSerialization.data(withJSONObject: payload)
        let json = String(data: data, encoding: .utf8) ?? "{}"
        _ = try doc.addCommand(kind: kind, payloadJson: json, deviceId: device)
    }

    func setModelSettings(model: String, effort: Effort) throws {
        try doc.setModelSettings(model: model, effort: effort.rawValue)
    }

    // MARK: Subscriptions

    func onChange(_ handler: @escaping (Touched) -> Void) {
        let bridge = CoreChatDocChangeListener(handler)
        changeListener = bridge
        doc.onChange(listener: bridge)
    }

    func onLocalUpdate(_ handler: @escaping (Data) -> Void) {
        let bridge = CoreChatDocLocalUpdateListener(handler)
        localUpdateListener = bridge
        doc.onLocalUpdate(listener: bridge)
    }
}

/// Maps `ArbosFFI.Touched` onto the app's `Touched`.
private final class CoreChatDocChangeListener: DocChangeListener, @unchecked Sendable {
    private let handler: (Touched) -> Void

    init(_ handler: @escaping (Touched) -> Void) {
        self.handler = handler
    }

    func onChange(touched: ArbosFFI.Touched) {
        var out = Touched()
        out.listChanged = touched.listChanged
        out.messages = Set(touched.messages.map { Int($0) })
        out.other = touched.other
        handler(out)
    }
}

private final class CoreChatDocLocalUpdateListener: LocalUpdateListener, @unchecked Sendable {
    private let handler: (Data) -> Void

    init(_ handler: @escaping (Data) -> Void) {
        self.handler = handler
    }

    func onLocalUpdate(update: Data) {
        handler(update)
    }
}
