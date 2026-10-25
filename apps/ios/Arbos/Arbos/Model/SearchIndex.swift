// The phone's local search index (the Mac's `search_index.rs` twin, [feedback
// iphone-bottom-search-bar-2026-10-25]): every loaded chat's message text,
// attachments (name) and `http(s)` links, folded once per chat version and
// scanned in memory on each keystroke — never a relay round trip. The index
// is lazy: `sync` re-reads a chat only when its `messagesVersion` moved, so
// typing costs a substring scan and nothing else.

import Foundation

struct SearchIndex: Equatable {
    /// One message's searchable text.
    struct Text: Equatable {
        var chatId: Id
        var projectId: Id
        var messageId: Id
        var createdAt: Int64
        /// Who wrote it, as the row's subtitle names them ("Sam Rivera", "You", the agent).
        var who: String
        var text: String
        var folded: String
    }

    /// One attachment or link shared in a chat.
    struct Content: Equatable {
        enum Kind: Equatable { case media, file, link }
        var chatId: Id
        var projectId: Id
        var messageId: Id
        /// The Library item's id (`<message>#<part>`), or `<message>#<part>#<offset>` for a link.
        var itemId: String
        var createdAt: Int64
        var who: String
        var kind: Kind
        /// File name, or the link's URL.
        var name: String
        var folded: String
        var mime: String
        var bytes: Int64?
        var url: URL?
    }

    private struct ChatEntries: Equatable {
        var version: Int
        var texts: [Text]
        var contents: [Content]
    }

    private var chats: [Id: ChatEntries] = [:]

    private static let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)

    /// Chats in the index.
    var chatCount: Int { chats.count }
    /// Messages with text in the index.
    var textCount: Int { chats.values.reduce(0) { $0 + $1.texts.count } }
    /// Attachments and links in the index.
    var contentCount: Int { chats.values.reduce(0) { $0 + $1.contents.count } }

    /// The indexed version of a chat, nil when it is not in the index.
    func version(of chatId: Id) -> Int? { chats[chatId]?.version }

    /// Bring one chat up to `version`: re-read its messages only when the version moved.
    /// `who` names a message's sender for the row subtitles. Returns true when it re-indexed.
    @discardableResult
    mutating func sync(chatId: Id, projectId: Id, version: Int, messages: () -> [Message], who: (Message) -> String) -> Bool {
        if let have = chats[chatId], have.version == version { return false }
        var texts: [Text] = []
        var contents: [Content] = []
        for m in messages() where m.role != .system && !m.hidden {
            let sender = who(m)
            let text = m.plainText
            if !text.isEmpty {
                texts.append(Text(chatId: chatId, projectId: projectId, messageId: m.id, createdAt: m.createdAt,
                                  who: sender, text: text, folded: HomeSearch.fold(text)))
            }
            for part in m.parts {
                switch part {
                case .attachment(let partId, let a):
                    guard !m.voice, !(a.mime.hasPrefix("audio/") || a.kind == "audio") else { continue }
                    let kind: Content.Kind = (a.isImage || a.mime.hasPrefix("video/") || a.kind == "video") ? .media : .file
                    contents.append(Content(chatId: chatId, projectId: projectId, messageId: m.id, itemId: "\(m.id)#\(partId)",
                                            createdAt: m.createdAt, who: sender, kind: kind, name: a.name,
                                            folded: HomeSearch.fold(a.name), mime: a.mime, bytes: a.bytes, url: nil))
                case .text(let partId, let body):
                    guard let detector = Self.detector else { continue }
                    let ns = body as NSString
                    for match in detector.matches(in: body, range: NSRange(location: 0, length: ns.length)) {
                        guard let url = match.url, url.scheme?.hasPrefix("http") == true else { continue }
                        contents.append(Content(chatId: chatId, projectId: projectId, messageId: m.id,
                                                itemId: "\(m.id)#\(partId)#\(match.range.location)", createdAt: m.createdAt,
                                                who: sender, kind: .link, name: url.absoluteString,
                                                folded: HomeSearch.fold(url.absoluteString), mime: "text/uri-list", bytes: nil, url: url))
                    }
                default:
                    continue
                }
            }
        }
        chats[chatId] = ChatEntries(version: version, texts: texts, contents: contents)
        return true
    }

    /// Forget a chat (deleted, left, evicted).
    mutating func remove(chatId: Id) { chats[chatId] = nil }

    /// Keep only these chats (the registry moved on).
    mutating func retain(chatIds: Set<Id>) {
        for id in chats.keys where !chatIds.contains(id) { chats[id] = nil }
    }

    /// A message-text hit with its snippet.
    struct TextHit: Equatable {
        var text: Text
        var snippet: String
    }

    /// Every message whose text contains the folded query, newest first.
    func textHits(q: String) -> [TextHit] {
        guard !q.isEmpty else { return [] }
        var out: [TextHit] = []
        for chat in chats.values {
            for t in chat.texts where t.folded.contains(q) {
                guard let s = HomeSearch.snippet(t.text, q: q) else { continue }
                out.append(TextHit(text: t, snippet: s))
            }
        }
        return out.sorted { $0.text.createdAt > $1.text.createdAt }
    }

    /// Every attachment / link whose name or URL contains the folded query, newest first.
    func contentHits(q: String) -> [Content] {
        guard !q.isEmpty else { return [] }
        return chats.values.flatMap { $0.contents.filter { $0.folded.contains(q) } }
            .sorted { $0.createdAt > $1.createdAt }
    }
}
