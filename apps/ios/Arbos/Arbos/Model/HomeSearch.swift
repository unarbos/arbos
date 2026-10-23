// Home search: one ranked match model for every row (feedback
// `search-name-matching-2026-10-23`). Jacob typed "carro" and the person Carro
// (@robertalanm) did not come first while the group "Testing" appeared for no
// visible reason (a message body matched). A row now appears only when the
// query matches something the row can show — display name, @handle, title, a
// member, a message — and a hit beyond the row's own name says why on its second
// line ("Member · Carro @robertalanm"). Case and diacritics fold ("zoe" finds
// "Zoë"), a leading `@` is ignored. Ranking: people, then titles, then members,
// then message bodies. Mirrors crates/arbos-mac `search.rs`.

import Foundation

enum HomeSearch {
    /// What the row matched on, best first.
    enum Kind: Int, Comparable {
        /// A person's display name or @handle (DM rows).
        case person
        /// The row's own title: agent name, group title, server / channel name, a chat title.
        case title
        /// Someone (or an agent) inside a group / shared project.
        case member
        /// A loaded message body.
        case message

        static func < (lhs: Kind, rhs: Kind) -> Bool { lhs.rawValue < rhs.rawValue }
    }

    struct Hit: Equatable {
        var kind: Kind
        /// Shown on the row's second line when the name alone does not explain the hit.
        var reason: String? = nil
        /// The message to open (`message` hits).
        var chatId: Id? = nil
        var messageId: Id? = nil
    }

    /// A group / shared-project member as the matcher sees it.
    struct Member: Equatable {
        var handle: String
        var name: String?
        var isAgent: Bool = false

        var displayName: String { name?.isEmpty == false ? name! : handle }
    }

    /// One message body a row can be found by.
    struct MessageText: Equatable {
        var chatId: Id
        var messageId: Id
        var createdAt: Int64
        var text: String
    }

    /// Lower-case, diacritics stripped, trimmed: the form every haystack and the query are compared in.
    static func fold(_ s: String) -> String {
        s.trimmingCharacters(in: .whitespacesAndNewlines)
            .folding(options: [.caseInsensitive, .diacriticInsensitive, .widthInsensitive], locale: nil)
    }

    /// The typed query as the matcher sees it: folded, a leading `@` dropped so
    /// "@robert" finds the handle too. Empty means "show everything".
    static func normalizeQuery(_ query: String) -> String {
        var q = query.trimmingCharacters(in: .whitespacesAndNewlines)
        while q.hasPrefix("@") { q.removeFirst() }
        return fold(q)
    }

    private static func contains(_ hay: String, _ q: String) -> Bool {
        fold(hay).contains(q)
    }

    /// A DM row: the other person's display name or @handle, else a message.
    static func dmHit(name: String, handle: String, messages: () -> [MessageText], q: String) -> Hit? {
        if q.isEmpty { return Hit(kind: .title) }
        if contains(name, q) || contains(handle, q) { return Hit(kind: .person) }
        return messageHit(messages(), q: q)
    }

    /// A chat under an agent: its id and title.
    struct ChatTitle: Equatable {
        var id: Id
        var title: String
    }

    /// A group, agent or shared-project row: its title, a member, a chat title under it, else a message.
    static func titledHit(title: String, members: [Member], chats: [ChatTitle] = [], messages: () -> [MessageText], q: String) -> Hit? {
        if q.isEmpty { return Hit(kind: .title) }
        if contains(title, q) { return Hit(kind: .title) }
        if let m = members.first(where: { !$0.isAgent && (contains($0.displayName, q) || contains($0.handle, q)) }) {
            let reason = m.displayName == m.handle ? "Member · @\(m.handle)" : "Member · \(m.displayName) @\(m.handle)"
            return Hit(kind: .member, reason: reason)
        }
        if let a = members.first(where: { $0.isAgent && contains($0.displayName, q) }) {
            return Hit(kind: .member, reason: "Agent · \(a.displayName)")
        }
        if let c = chats.first(where: { !$0.title.trimmingCharacters(in: .whitespaces).isEmpty && contains($0.title, q) }) {
            return Hit(kind: .title, reason: "Chat · \(c.title.trimmingCharacters(in: .whitespaces))", chatId: c.id)
        }
        return messageHit(messages(), q: q)
    }

    /// A server row: its name, or one of its channels (named in the reason).
    static func serverHit(name: String, channels: [String], q: String) -> Hit? {
        if q.isEmpty || contains(name, q) { return Hit(kind: .title) }
        if let c = channels.first(where: { contains($0, q) }) { return Hit(kind: .title, reason: "#\(c)") }
        return nil
    }

    /// The newest message whose prose contains `q`, with the snippet as the reason.
    static func messageHit(_ messages: [MessageText], q: String) -> Hit? {
        var best: (MessageText, String)? = nil
        for m in messages {
            guard let s = snippet(m.text, q: q) else { continue }
            if best == nil || m.createdAt >= best!.0.createdAt { best = (m, s) }
        }
        guard let found = best else { return nil }
        return Hit(kind: .message, reason: "Message · \(found.1)", chatId: found.0.chatId, messageId: found.0.messageId)
    }

    /// Up to this many characters of context before the match in a snippet.
    static let snippetBefore = 18
    /// Up to this many characters after the match.
    static let snippetAfter = 60

    /// The part of `text` around the first occurrence of `q` (folded), one line,
    /// with ellipses where it was cut; nil when `q` does not occur.
    static func snippet(_ text: String, q: String) -> String? {
        guard !q.isEmpty else { return nil }
        let folded = text.folding(options: [.caseInsensitive, .diacriticInsensitive, .widthInsensitive], locale: nil)
        // Folding keeps one character per character for the scripts we care
        // about; when it does not, fall back to the plain range search.
        let source: String = folded.count == text.count ? text : folded
        guard let range = folded.range(of: q) else { return nil }
        let start = folded.distance(from: folded.startIndex, to: range.lowerBound)
        let end = folded.distance(from: folded.startIndex, to: range.upperBound)
        let chars = Array(source)
        let from = max(0, start - snippetBefore)
        let to = min(chars.count, end + snippetAfter)
        guard from < to else { return nil }
        var out = String(chars[from..<to].map { $0.isWhitespace || $0.isNewline ? " " : $0 })
        out = out.split(separator: " ", omittingEmptySubsequences: true).joined(separator: " ")
        if from > 0 { out = "…" + out }
        if to < chars.count { out += "…" }
        return out
    }
}
