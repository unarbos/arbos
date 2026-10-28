// Home search: one ranked match model for every row (feedback
// `search-name-matching-2026-10-23`). Jacob typed "carro" and the person Carro
// (@robertalanm) did not come first while the group "Testing" appeared for no
// visible reason (a message body matched). A row now appears only when the
// query matches something the row can show — display name, @handle, title, a
// member, a message — and a hit beyond the row's own name says why on its second
// line ("Member · Carro @robertalanm"). Case and diacritics fold ("zoe" finds
// "Zoë"), a leading `@` is ignored. Ranking: people, then titles, then members,
// then message bodies. Mirrors crates/arbos-mac `search.rs`.
//
// Names, handles and titles go through the one shared matcher in the Rust core
// (`arbos_core::search`, reached through ArbosFFI as `EntitySearch`): exact >
// prefix > word-start, display name before handle, diacritics folded — the same
// rule the Mac and every picker on both platforms use ([feedback
// shared-search-matcher-ios-2026-10-26]). Message bodies are not entities and
// keep the substring search here.

import ArbosFFI
import Foundation

/// The shared search matcher (`arbos_core::search`) for every search box that
/// lists people, agents, groups, servers or channels: the New message sheet,
/// New group / Add members / Give a person access, the Secrets agent picker,
/// @mention completion, the home search and the Forward sheet. Jacob, Mac
/// v0.6.268, 2026-10-26: "aff" in "Give a person access" listed only "Andreas F
/// @aff" because that sheet matched handles alone; this is the one rule now.
enum EntitySearch {
    /// Lower-case, diacritics stripped, trimmed (the core's fold).
    static func fold(_ s: String) -> String { searchFold(text: s) }

    /// The typed query as the matcher sees it: leading `@` dropped, folded,
    /// whitespace collapsed. Empty means "show everything". Idempotent.
    static func normalize(_ query: String) -> String { searchNormalizeQuery(query: query) }

    /// How an entity matches the typed `query` — the best of its display name,
    /// @handle and title (exact > prefix > word-start, name before handle before
    /// title; `SearchMatch.order` is that tuple as one number, 0 best); nil when
    /// none matches. An empty query matches everything.
    static func match(name: String, handle: String, title: String? = nil, query: String) -> SearchMatch? {
        searchMatch(name: name, handle: handle, title: title, query: query)
    }

    /// Does the entity match at all?
    static func matches(name: String, handle: String, title: String? = nil, query: String) -> Bool {
        match(name: name, handle: handle, title: title, query: query) != nil
    }

    /// How one text (a group title, a channel name) matches the typed `query`.
    static func matchText(_ text: String, query: String) -> SearchMatchKind? {
        searchMatchText(text: text, query: query)
    }

    /// The items that match `query`, best first (exact > prefix > word-start,
    /// name before handle before title), input order inside a tie; an empty
    /// query keeps every item in order.
    static func rank<T>(_ items: [T], query: String, fields: (T) -> SearchCandidate) -> [(item: T, match: SearchMatch)] {
        searchRank(candidates: items.map(fields), query: query).map { (item: items[Int($0.index)], match: $0.matched) }
    }

    /// `rank` without the verdicts.
    static func filter<T>(_ items: [T], query: String, fields: (T) -> SearchCandidate) -> [T] {
        rank(items, query: query, fields: fields).map(\.item)
    }
}

/// The rows of the people / agent / server pickers (New message, New group,
/// Add members, Add server members, Give a person access, the Secrets agent
/// picker) for what was typed, through `EntitySearch`. Pure, so the tests pin
/// them; every picker calls these instead of filtering on its own.
enum PeoplePicker {
    /// Directory first, then remote hits not already listed, never me; then the
    /// shared matcher on display name and @handle, best first. An empty query
    /// keeps everyone in that order.
    static func people(directory: [Person], remote: [Person], me: String?, query: String) -> [Person] {
        var seen: Set<String> = []
        var pool: [Person] = []
        for p in directory + remote where p.id != me {
            if seen.insert(p.id).inserted { pool.append(p) }
        }
        return EntitySearch.filter(pool, query: query) { SearchCandidate(name: $0.displayName, handle: $0.handle, title: nil) }
    }

    /// Agents by name, @slug and description, best first, list order inside a tie.
    static func agents(_ agents: [Project], query: String) -> [Project] {
        EntitySearch.filter(agents, query: query) {
            SearchCandidate(name: $0.name, handle: mentionSlug($0.name), title: $0.description.isEmpty ? nil : $0.description)
        }
    }

    /// Servers by name, best first, list order inside a tie.
    static func servers(_ servers: [Server], query: String) -> [Server] {
        EntitySearch.filter(servers, query: query) { SearchCandidate(name: $0.name, handle: "", title: nil) }
    }

    /// Any titled rows (the Secrets agent picker's agents, a vault's items) by title.
    static func titled<T>(_ items: [T], query: String, title: (T) -> String) -> [T] {
        EntitySearch.filter(items, query: query) { SearchCandidate(name: title($0), handle: "", title: nil) }
    }
}

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

    /// A DM row: the other person's display name or @handle (the shared matcher), else a message.
    static func dmHit(name: String, handle: String, messages: () -> [MessageText], q: String) -> Hit? {
        if q.isEmpty { return Hit(kind: .title) }
        if EntitySearch.matches(name: name, handle: handle, query: q) { return Hit(kind: .person) }
        return messageHit(messages(), q: q)
    }

    /// A chat under an agent: its id and title.
    struct ChatTitle: Equatable {
        var id: Id
        var title: String
    }

    /// A group, agent or shared-project row: its title (or an agent's `description`,
    /// the matcher's title field), a member, a chat title under it, else a message.
    static func titledHit(title: String, description: String? = nil, members: [Member], chats: [ChatTitle] = [], messages: () -> [MessageText], q: String) -> Hit? {
        if q.isEmpty { return Hit(kind: .title) }
        if EntitySearch.matches(name: title, handle: "", title: description, query: q) { return Hit(kind: .title) }
        if let m = members.first(where: { !$0.isAgent && EntitySearch.matches(name: $0.displayName, handle: $0.handle, query: q) }) {
            let reason = m.displayName == m.handle ? "Member · @\(m.handle)" : "Member · \(m.displayName) @\(m.handle)"
            return Hit(kind: .member, reason: reason)
        }
        if let a = members.first(where: { $0.isAgent && EntitySearch.matches(name: $0.displayName, handle: $0.handle, query: q) }) {
            return Hit(kind: .member, reason: "Agent · \(a.displayName)")
        }
        if let c = chats.first(where: { !$0.title.trimmingCharacters(in: .whitespaces).isEmpty && EntitySearch.matchText($0.title, query: q) != nil }) {
            return Hit(kind: .title, reason: "Chat · \(c.title.trimmingCharacters(in: .whitespaces))", chatId: c.id)
        }
        return messageHit(messages(), q: q)
    }

    /// A server row: its name, or one of its channels (named in the reason).
    static func serverHit(name: String, channels: [String], q: String) -> Hit? {
        if q.isEmpty || EntitySearch.matchText(name, query: q) != nil { return Hit(kind: .title) }
        if let c = channels.first(where: { EntitySearch.matchText($0, query: q) != nil }) { return Hit(kind: .title, reason: "#\(c)") }
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
