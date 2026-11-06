import XCTest
@testable import Arbos

/// `Row` keeps its fields in one immutable shared box, and `==` short-cuts on the same box.
/// Every check here holds the boxed answers to the plain field-by-field compare `Row ==` was
/// before the box (`Plain`), so a shared box can never make two different rows look equal.
final class RowStorageTests: XCTestCase {
    private struct Plain: Equatable {
        let id: String
        let messageId: Id
        let role: Role
        let kind: RowKind
        let reactions: [MessageReaction]
        let replyCount: Int
        let createdAt: Int64
        let gapBefore: CGFloat
        let runStart: Bool
        let showsAvatar: Bool

        init(_ r: Row) {
            id = r.id; messageId = r.messageId; role = r.role; kind = r.kind; reactions = r.reactions
            replyCount = r.replyCount; createdAt = r.createdAt; gapBefore = r.gapBefore
            runStart = r.runStart; showsAvatar = r.showsAvatar
        }
    }

    private let slot = "people:fx-group-launch"
    private let me = "user:fx-me"

    private func build(_ list: [Message]) -> [Row] {
        Transcript.rows(list, projectId: slot, me: me, people: true, multiParty: true)
    }

    /// What `ChatSession.refreshRows` does for a token on the last message: keep the rows above
    /// it and rebuild only the last two messages' rows.
    private func spliceTail(_ old: [Row], _ list: [Message]) -> [Row] {
        let last = list[list.count - 1]
        let rebuilt = Transcript.rows([list[list.count - 2], last], byId: Transcript.index(list), projectId: slot,
                                      me: me, people: true, multiParty: true)
        func start(_ rows: [Row]) -> Int {
            guard let ix = rows.firstIndex(where: { $0.messageId == last.id }) else { return rows.count }
            return ix > 0 && rows[ix - 1].id == "\(last.id)#date" ? ix - 1 : ix
        }
        return Array(old.prefix(start(old))) + rebuilt[start(rebuilt)...]
    }

    private func message(_ id: String, role: Role, sender: String, status: MessageStatus = .done, at: Int64,
                         parts: [Part], handle: String, name: String) -> Message {
        Message(id: id, chatId: "fx-group-launch", role: role, sender: sender, status: status, turn: role == .assistant ? .reply : nil,
                decision: nil, subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: at,
                completedAt: status == .done || status == .error ? at + 1_000 : nil, pending: false, parts: parts, hiddenCount: 0,
                senderHandle: handle, senderName: name)
    }

    private func answer(_ id: String, at: Int64, text: String?, status: MessageStatus = .done) -> Message {
        message(id, role: .assistant, sender: "agent:fx-comms", status: status, at: at,
                parts: text.map { [.text(id: "\(id)-t", text: $0)] } ?? [], handle: "comms", name: "Comms")
    }

    /// The rich long group (pictures, files, reply quotes, markdown, code cards, charts,
    /// mentions, reactions, membership lines, a two-hour gap) plus named links (link icons), a
    /// sticker and an answer that failed.
    private func denseMix() -> [Message] {
        var list = AppModel.longGroupTranscript(rows: 360, before: 1_790_000_000_000, rich: true)
        var at = (list.last?.createdAt ?? 0) + 40_000
        func next() -> Int64 { at += 40_000; return at }
        list.append(answer("mix-links", at: next(), text: "Reading list:\n\n- [Launch post](https://arbos.life/blog/launch)\n- [Relay notes](https://relay.arbos.life/notes)\n- [Press kit](https://press.example/kit)"))
        list.append(message("mix-sticker", role: .user, sender: "user:fx-u4", at: next(),
                            parts: [.attachment(id: "mix-sticker-a", attachment: Attachment(name: "sticker.png", mime: "image/png", blobRef: "fx-sticker", kind: "sticker"))],
                            handle: "sam", name: "Sam Okafor"))
        list.append(answer("mix-failed", at: next(), text: nil, status: .error))
        var reply = message("mix-reply", role: .user, sender: me, at: next(), parts: [.text(id: "mix-reply-t", text: "Picking this up.")],
                            handle: "jacob", name: "Jacob Steeves")
        reply.replyTo = "mix-links"
        list.append(reply)
        list.append(answer("mix-stream", at: next(), text: "Looking", status: .streaming))
        return list
    }

    /// Boxed `==` gives the plain answer for the two arrays and for every row pair matched by id.
    private func assertSameAnswers(_ a: [Row], _ b: [Row], _ what: String, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(a == b, a.map(Plain.init) == b.map(Plain.init), "\(what): arrays", file: file, line: line)
        let byId = Dictionary(b.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        for row in a {
            guard let other = byId[row.id] else { continue }
            XCTAssertEqual(row == other, Plain(row) == Plain(other), "\(what): \(row.id)", file: file, line: line)
        }
    }

    func testTheDenseMixHasEveryKindOfRow() {
        let rows = build(denseMix())
        func has(_ test: (RowKind) -> Bool) -> Bool { rows.contains { test($0.kind) } }
        XCTAssertTrue(rows.contains { $0.id.hasSuffix("#date") }, "date lines")
        XCTAssertTrue(rows.contains { !$0.reactions.isEmpty }, "reactions")
        XCTAssertTrue(rows.contains { $0.messageId == "mix-sticker" }, "a sticker")
        XCTAssertTrue(rows.contains { $0.id == "mix-failed#failed" }, "a failed answer")
        XCTAssertTrue(has { if case .other(_, _, _, _, let q, _) = $0 { return q != nil } else { return false } }, "reply quotes")
        XCTAssertTrue(has { if case .member(let blocks, _, _, _, _, _, _, _) = $0 { return blocks.count > 1 } else { return false } },
                      "markdown, code and chart answers in blocks")
        XCTAssertGreaterThan(Set(rows.map(\.messageId)).count, 300)
    }

    /// Streaming: the tail splice keeps the rows above the last message (same boxes), and
    /// equals the full rebuild (new boxes, full compare) on every token.
    func testAStreamedTokenSplicesToTheFullRebuild() {
        var list = denseMix()
        var shown = build(list)
        var text = "Looking"
        for token in ["at", "the", "relay", "logs.\n\nFound", "it:", "`drain`", "skips", "rooms.\n\n```rust\nlet n = 1;\n```"] {
            text += " " + token
            list[list.count - 1] = answer("mix-stream", at: list[list.count - 1].createdAt, text: text, status: .streaming)
            let spliced = spliceTail(shown, list)
            let full = build(list)
            XCTAssertEqual(spliced, full, "token \(token)")
            XCTAssertEqual(spliced.map(Plain.init), full.map(Plain.init), "token \(token)")
            let kept = spliced.prefix { $0.messageId != "mix-stream" }
            XCTAssertGreaterThan(kept.count, 300)
            for (a, b) in zip(kept, shown) { XCTAssertTrue(a.sharesStorage(with: b), "kept row \(a.id) is the same box") }
            for (a, b) in zip(spliced, full) { XCTAssertFalse(a.sharesStorage(with: b)) }
            assertSameAnswers(shown, spliced, "token \(token)")
            shown = spliced
        }
        list[list.count - 1] = answer("mix-stream", at: list[list.count - 1].createdAt, text: text)
        XCTAssertEqual(spliceTail(shown, list), build(list), "the finished answer")
    }

    func testEditsThatAreNotAppendsGiveThePlainAnswer() {
        let base = denseMix()
        let before = build(base)
        let mid = base.firstIndex { $0.id == "long-182" }!
        XCTAssertEqual(base[mid].role, .user)

        var edited = base
        edited[mid].parts = [.text(id: "long-182-t", text: "Edited: the call moved to 11.")]
        var deleted = base
        deleted.remove(at: mid)
        var reacted = base
        reacted[mid].reactions.append(MessageReaction(emoji: "🎉", senders: ["user:fx-u3"]))
        var retried = base
        let failed = retried.firstIndex { $0.id == "mix-failed" }!
        retried[failed] = answer("mix-failed", at: retried[failed].createdAt, text: "Trying again", status: .streaming)
        let older = AppModel.longGroupTranscript(rows: 400, before: 1_790_000_000_000, rich: true)
        let page = older.prefix(40).map { m -> Message in
            var m = m
            m.id = "page-" + m.id
            m.replyTo = m.replyTo.map { "page-" + $0 }
            return m
        }
        XCTAssertLessThan(page.last!.createdAt, base.first!.createdAt)
        let paged = page + base

        for (what, list) in [("edit", edited), ("delete", deleted), ("reaction", reacted), ("retry", retried), ("history page", paged)] {
            let after = build(list)
            XCTAssertNotEqual(before, after, what)
            XCTAssertNotEqual(before.map(Plain.init), after.map(Plain.init), what)
            assertSameAnswers(before, after, what)
            assertSameAnswers(after, before, what)
            XCTAssertEqual(after, build(list), "\(what): a second build is equal")
        }
    }

    func testChangingACopiedRowLeavesTheOriginalAlone() {
        let original = build(denseMix()).first { !$0.reactions.isEmpty }!
        let plain = Plain(original)
        let changes: [(String, (inout Row) -> Void)] = [
            ("id", { $0.id += "-x" }),
            ("messageId", { $0.messageId += "-x" }),
            ("role", { $0.role = $0.role == .system ? .user : .system }),
            ("kind", { $0.kind = .system(icon: nil, text: "changed", avatarProject: nil) }),
            ("reactions", { $0.reactions.append(MessageReaction(emoji: "🎉", senders: ["user:fx-u3"])) }),
            ("replyCount", { $0.replyCount += 1 }),
            ("createdAt", { $0.createdAt += 1 }),
            ("gapBefore", { $0.gapBefore += 1 }),
            ("runStart", { $0.runStart.toggle() }),
            ("showsAvatar", { $0.showsAvatar.toggle() }),
        ]
        for (field, change) in changes {
            var copy = original
            XCTAssertTrue(copy.sharesStorage(with: original), field)
            change(&copy)
            XCTAssertEqual(Plain(original), plain, "\(field): the original is unchanged")
            XCTAssertFalse(copy.sharesStorage(with: original), field)
            XCTAssertNotEqual(copy, original, field)
            XCTAssertNotEqual(Plain(copy), plain, field)
        }

        let rows = build(denseMix())
        var copies = rows
        copies[5].replyCount += 1
        copies[6].kind = .system(icon: nil, text: "changed", avatarProject: nil)
        XCTAssertEqual(rows.map(Plain.init), build(denseMix()).map(Plain.init), "the array the copies came from is unchanged")
        XCTAssertNotEqual(rows, copies)
    }

    func testRowsBuiltApartCompareEqualThroughTheFullCompare() {
        let list = denseMix()
        let a = build(list), b = build(list)
        XCTAssertEqual(a.count, b.count)
        for (x, y) in zip(a, b) {
            XCTAssertFalse(x.sharesStorage(with: y), x.id)
            XCTAssertEqual(x, y, x.id)
        }
        let one = Row(id: "r", messageId: "m", role: .user, kind: .system(icon: nil, text: "same", avatarProject: nil), createdAt: 7)
        let two = Row(id: "r", messageId: "m", role: .user, kind: .system(icon: nil, text: "same", avatarProject: nil), createdAt: 7)
        let three = Row(id: "r", messageId: "m", role: .user, kind: .system(icon: nil, text: "other", avatarProject: nil), createdAt: 7)
        XCTAssertEqual(one, two)
        XCTAssertNotEqual(one, three)
    }
}
