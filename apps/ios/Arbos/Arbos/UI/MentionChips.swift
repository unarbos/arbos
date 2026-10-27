import SwiftUI
import UIKit

// MARK: - Mention chips (Jacob, 2026-10-23, after Grok Bot)
//
// `@comms`, `@sam`, `@launch-week`, `@launch-crew` in a bubble draw as a chip —
// the entity's picture, its name in the accent colour, on a soft pill — and a
// tap opens that entity's page (the shared D93 profile pane). Unknown handles
// stay plain text. The Mac twin is `ui/mention.rs` + `select.rs`.
//
// SwiftUI `Text` cannot put a picture inside an attributed run, so a chip is
// three concatenated pieces: a left cap image (the pill's rounded left end
// with the picture in it), the name run (accent on the pill colour, a link
// that `ChatView` routes to `showProfile`), and a right cap image (the pill's
// rounded right end). Text selection copies the name.

/// One mentionable entity.
struct MentionEntry {
    let name: String
    let target: ProfileTarget
    /// What the chip's picture is drawn from.
    let picture: AnyView
}

/// Everyone the app knows right now, keyed by handle (lower-case).
///
/// Build once per directory sync (Mac: `Mentions::from_state` in transcript
/// sync) and pass the same instance into every bubble. Rebuilding inside each
/// `RowView` / `BlockView` body was the main iPhone group-chat lag: every
/// evaluation walked projects, people chats, the directory and servers.
@MainActor
final class MentionTable {
    /// Identity of the table's inputs (the cache's hash); views compare this, not
    /// the instance, so a rebuilt-but-equal table does not redraw every bubble.
    var stamp = 0
    private var byHandle: [String: MentionEntry] = [:]
    /// Display names, longest first, so `@Launch week` beats `@Launch`.
    private var names: [(String, MentionEntry)] = []

    /// The chip's geometry (Grok Bot's mention chip): picture `picture` pt in a
    /// cap `capWidth` wide; the pill rises `padY` over the glyph box.
    static let picture: CGFloat = 16
    static let capWidth: CGFloat = 23
    static let padY: CGFloat = 2
    static let radius: CGFloat = 7

    /// Empty table (no chips) — placeholder until the chat has synced one.
    init() {}

    init(model: AppModel, theme: Theme) {
        for record in model.projects {
            let slug = mentionSlug(record.name)
            guard !slug.isEmpty else { continue }
            // Through the shared resolver, like every other surface (Jacob, 2026-10-26: the
            // chip drew the record's copy while the working row drew the resolver's).
            let p = model.agent(id: record.id, name: record.name)
            add(slug, MentionEntry(
                name: p.name.trimmingCharacters(in: .whitespaces),
                target: .agent(projectId: p.id),
                picture: AnyView(AgentAvatar(project: p, theme: theme, size: Self.picture, still: true))
            ))
        }
        for chat in model.peopleChats {
            for m in chat.members where !m.isAgent {
                add(m.handle, MentionEntry(
                    name: m.name?.isEmpty == false ? m.name! : m.handle,
                    target: .user(ProfilePerson(m)),
                    picture: AnyView(PersonAvatar(url: m.avatarUrl, handle: m.handle, size: Self.picture, name: m.name, id: m.id, colorIndex: m.avatarColorIndex))
                ))
            }
            // Agents other people shared into my chats (#1592): their `@handle` is a chip
            // too, drawn through the same identity the bubble and the presence row use.
            for m in chat.agents {
                let p = model.agent(member: m, chatId: chat.id)
                let slug = mentionSlug(p.name)
                guard !slug.isEmpty else { continue }
                add(slug, MentionEntry(
                    name: p.name,
                    target: .agent(projectId: p.id),
                    picture: AnyView(AgentAvatar(project: p, theme: theme, size: Self.picture, still: true))
                ))
            }
            if chat.isGroup {
                let slug = mentionSlug(chat.title)
                guard !slug.isEmpty else { continue }
                let picture: AnyView
                if chat.isNeutralAvatar {
                    picture = AnyView(NeutralAvatar(name: chat.title, size: Self.picture))
                } else if let leaf = chat.leafPick, LeafAvatarsFlag.shared.on {
                    picture = AnyView(LeafAvatar(pick: leaf, size: Self.picture))
                } else {
                    picture = AnyView(PersonAvatar(url: nil, handle: chat.title, size: Self.picture, name: chat.title, id: chat.id))
                }
                add(slug, MentionEntry(name: chat.title, target: .group(chatId: chat.id, projectId: nil), picture: picture))
            }
        }
        for person in model.directory {
            add(person.handle, MentionEntry(
                name: person.name?.isEmpty == false ? person.name! : person.handle,
                target: .user(ProfilePerson(person)),
                picture: AnyView(PersonAvatar(url: person.avatarUrl, handle: person.handle, size: Self.picture, name: person.name, id: person.id, colorIndex: person.avatarColorIndex))
            ))
        }
        for server in model.servers {
            let slug = mentionSlug(server.name)
            guard !slug.isEmpty else { continue }
            add(slug, MentionEntry(
                name: server.name.trimmingCharacters(in: .whitespaces),
                target: .server(serverId: server.id),
                picture: AnyView(ServerAvatar(server: server, size: Self.picture))
            ))
        }
        names = byHandle.values.map { ($0.name, $0) }
            .sorted { a, b in a.0.count != b.0.count ? a.0.count > b.0.count : a.0 < b.0 }
    }

    /// One shared table for the open chat — rebuilt only when the mention
    /// sources change (projects, people, directory, servers, appearance).
    @MainActor
    final class Cache {
        static let shared = Cache()
        private var table = MentionTable()
        private var stamp = 0

        func table(model: AppModel, theme: Theme) -> MentionTable {
            let next = Self.stamp(model)
            if next != stamp {
                stamp = next
                table = MentionTable(model: model, theme: theme)
                table.stamp = next
            }
            return table
        }

        private static func stamp(_ model: AppModel) -> Int {
            var h = Hasher()
            // Appearance / surface drive Theme; chips embed AgentAvatar with that theme.
            h.combine(model.appearanceSetting)
            h.combine(model.surfaceSetting)
            for p in model.projects {
                h.combine(p.id)
                h.combine(p.name)
                h.combine(p.avatarColorIndex)
                h.combine(p.avatarLeaf)
                h.combine(p.avatarLeafColor)
                h.combine(p.imageKey)
                h.combine(p.avatarVersion)
            }
            // Sorted: a chat moving to the top on activity must not change the table.
            for chat in model.peopleChats.sorted(by: { $0.id < $1.id }) {
                h.combine(chat.id)
                h.combine(chat.title)
                h.combine(chat.avatarLeaf)
                h.combine(chat.avatarLeafColor)
                for m in chat.members {
                    h.combine(m.id)
                    h.combine(m.handle)
                    h.combine(m.name)
                    h.combine(m.avatarUrl)
                    h.combine(m.project?.name)
                    h.combine(m.project?.avatarLeaf)
                    h.combine(m.project?.avatarLeafColor)
                    h.combine(m.project?.imageKey)
                    h.combine(m.project?.avatarVersion)
                    h.combine(m.isAgent)
                }
            }
            for person in model.directory {
                h.combine(person.id)
                h.combine(person.handle)
                h.combine(person.name)
                h.combine(person.avatarUrl)
            }
            for server in model.servers {
                h.combine(server.id)
                h.combine(server.name)
            }
            return h.finalize()
        }
    }

    private func add(_ handle: String, _ entry: MentionEntry) {
        let key = handle.lowercased()
        if byHandle[key] == nil { byHandle[key] = entry }
    }

    var isEmpty: Bool { byHandle.isEmpty }

    /// `@handle` → `@Display Name` for every handle we know; anything else is left
    /// exactly as typed.
    func display(_ text: String) -> String {
        guard !byHandle.isEmpty, text.contains("@") else { return text }
        var out = ""
        var i = text.startIndex
        while let at = text[i...].firstIndex(of: "@") {
            out += text[i..<at]
            let before = at > text.startIndex ? text[text.index(before: at)] : nil
            let startsToken = before.map { !$0.isLetter && !$0.isNumber && $0 != "@" } ?? true
            let handleEnd = Self.handleEnd(text, from: text.index(after: at))
            if startsToken, handleEnd > text.index(after: at),
               let entry = byHandle[String(text[text.index(after: at)..<handleEnd]).lowercased()] {
                out += "@" + entry.name
                i = handleEnd
                continue
            }
            out += "@"
            i = text.index(after: at)
        }
        out += text[i...]
        return out
    }

    /// The `@Name` runs in a displayed text (the `@` included).
    func find(in text: String) -> [(Range<String.Index>, MentionEntry)] {
        var out: [(Range<String.Index>, MentionEntry)] = []
        guard !names.isEmpty else { return out }
        var i = text.startIndex
        while let at = text[i...].firstIndex(of: "@") {
            let before = at > text.startIndex ? text[text.index(before: at)] : nil
            let startsToken = before.map { !$0.isLetter && !$0.isNumber && $0 != "@" } ?? true
            let after = text[text.index(after: at)...]
            var hit: (String, MentionEntry)?
            if startsToken {
                hit = names.first { name, _ in
                    guard after.hasPrefix(name) else { return false }
                    let end = after.index(after.startIndex, offsetBy: name.count)
                    return end == after.endIndex || !(after[end].isLetter || after[end].isNumber)
                }
            }
            if let (name, entry) = hit {
                let end = text.index(at, offsetBy: 1 + name.count)
                out.append((at..<end, entry))
                i = end
            } else {
                i = text.index(after: at)
            }
        }
        return out
    }

    /// `[A-Za-z0-9_.-]+` with trailing `.`/`-` left out.
    private static func handleEnd(_ text: String, from: String.Index) -> String.Index {
        var end = from
        var i = from
        while i < text.endIndex, text[i].isLetter || text[i].isNumber || text[i] == "_" || text[i] == "." || text[i] == "-" {
            i = text.index(after: i)
            end = i
        }
        while end > from, text[text.index(before: end)] == "." || text[text.index(before: end)] == "-" {
            end = text.index(before: end)
        }
        return end
    }

    // MARK: Routing

    nonisolated static func url(for target: ProfileTarget) -> URL? {
        switch target {
        case .agent(let projectId): return URL(string: "arbos://mention/agent/\(projectId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? projectId)")
        case .user(let p): return URL(string: "arbos://mention/user/\(p.handle.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? p.handle)")
        case .group(let chatId, _): return URL(string: "arbos://mention/group/\(chatId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? chatId)")
        case .server(let serverId): return URL(string: "arbos://mention/server/\(serverId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? serverId)")
        case .channel(let channelId): return URL(string: "arbos://mention/channel/\(channelId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? channelId)")
        }
    }

    /// The target behind an `arbos://mention/<kind>/<id>` link, resolved against the model.
    static func target(from url: URL, model: AppModel) -> ProfileTarget? {
        guard url.scheme == "arbos", url.host == "mention" else { return nil }
        let parts = url.pathComponents.filter { $0 != "/" }
        guard parts.count == 2, let id = parts[1].removingPercentEncoding else { return nil }
        switch parts[0] {
        case "agent": return .agent(projectId: id)
        case "group": return .group(chatId: id, projectId: nil)
        case "server": return .server(serverId: id)
        case "channel": return .channel(channelId: id)
        case "user":
            for c in model.peopleChats {
                if let m = c.members.first(where: { !$0.isAgent && $0.handle.caseInsensitiveCompare(id) == .orderedSame }) { return .user(ProfilePerson(m)) }
            }
            if let p = model.directory.first(where: { $0.handle.caseInsensitiveCompare(id) == .orderedSame }) { return .user(ProfilePerson(p)) }
            return .user(ProfilePerson(id: id, handle: id, name: nil, avatarUrl: nil))
        default: return nil
        }
    }

    // MARK: Chip caps

    private static var capCache: [String: UIImage] = [:]

    /// The pill's left end with the picture in it, rendered once per entity / pill colour.
    func leftCap(for entry: MentionEntry, pill: Color, lineHeight: CGFloat, key: String) -> UIImage? {
        let cacheKey = "L|\(key)|\(pill.description)|\(lineHeight)"
        if let hit = Self.capCache[cacheKey] { return hit }
        let h = lineHeight + Self.padY * 2
        let view = ZStack(alignment: .leading) {
            UnevenRoundedRectangle(topLeadingRadius: Self.radius, bottomLeadingRadius: Self.radius, bottomTrailingRadius: 0, topTrailingRadius: 0, style: .continuous)
                .fill(pill)
            entry.picture.padding(.leading, 4)
        }
        .frame(width: Self.capWidth, height: h)
        let renderer = ImageRenderer(content: view)
        renderer.scale = UIScreen.main.scale
        guard let img = renderer.uiImage else { return nil }
        Self.capCache[cacheKey] = img
        return img
    }

    /// The pill's right end.
    func rightCap(pill: Color, lineHeight: CGFloat) -> UIImage? {
        let cacheKey = "R|\(pill.description)|\(lineHeight)"
        if let hit = Self.capCache[cacheKey] { return hit }
        let h = lineHeight + Self.padY * 2
        let view = UnevenRoundedRectangle(topLeadingRadius: 0, bottomLeadingRadius: 0, bottomTrailingRadius: Self.radius, topTrailingRadius: Self.radius, style: .continuous)
            .fill(pill)
            .frame(width: Self.radius, height: h)
        let renderer = ImageRenderer(content: view)
        renderer.scale = UIScreen.main.scale
        guard let img = renderer.uiImage else { return nil }
        Self.capCache[cacheKey] = img
        return img
    }

    private static func key(for target: ProfileTarget) -> String {
        switch target {
        case .agent(let id): return "agent:\(id)"
        case .user(let p): return "user:\(p.id)"
        case .group(let id, _): return "group:\(id)"
        case .server(let id): return "server:\(id)"
        case .channel(let id): return "channel:\(id)"
        }
    }

    /// `inline` prose with every known `@Name` run drawn as a chip.
    func text(_ attr: AttributedString, link: Color, pill: Color, lineHeight: CGFloat) -> Text {
        let plain = String(attr.characters)
        let chips = find(in: plain)
        // The prose between chips still gets its link icons (LinkIcons.text).
        let icons = LinkIcons.shared
        guard !chips.isEmpty else { return icons.text(attr, lineHeight: lineHeight) }
        var out = Text("")
        var cursor = attr.startIndex
        for (range, entry) in chips {
            // Map the plain-string range onto the attributed string by character offset.
            let loOff = plain.distance(from: plain.startIndex, to: range.lowerBound)
            let hiOff = plain.distance(from: plain.startIndex, to: range.upperBound)
            let lo = attr.characters.index(attr.startIndex, offsetBy: loOff)
            let hi = attr.characters.index(attr.startIndex, offsetBy: hiOff)
            out = out + icons.text(AttributedString(attr[cursor..<lo]), lineHeight: lineHeight)
            // The name run: accent on the pill, a link `ChatView` routes to the pane.
            var name = AttributedString(attr[attr.index(afterCharacter: lo)..<hi])
            name.swiftUI.foregroundColor = link
            name.swiftUI.backgroundColor = pill
            name.underlineStyle = nil
            name.link = Self.url(for: entry.target)
            name.swiftUI.font = .system(size: Theme.body, weight: .medium)
            let dy = -Self.padY - 1
            if let cap = leftCap(for: entry, pill: pill, lineHeight: lineHeight, key: Self.key(for: entry.target)) {
                out = out + Text(Image(uiImage: cap)).baselineOffset(dy)
            }
            out = out + Text(name)
            if let cap = rightCap(pill: pill, lineHeight: lineHeight) {
                out = out + Text(Image(uiImage: cap)).baselineOffset(dy)
            }
            cursor = hi
        }
        out = out + icons.text(AttributedString(attr[cursor..<attr.endIndex]), lineHeight: lineHeight)
        return out
    }
}

extension Markdown {
    /// `inline(...)` as a `Text`, with mention chips when `mentions` knows the handles and the
    /// site icon before every named link (`LinkIcons`).
    @MainActor
    static func inlineText(_ text: String, color: Color, link: Color, codeChip: Color, mentions: MentionTable?) -> Text {
        guard let mentions, !mentions.isEmpty else {
            return LinkIcons.shared.text(inline(text, color: color, link: link, codeChip: codeChip), lineHeight: Theme.body * 1.2)
        }
        let attr = inline(mentions.display(text), color: color, link: link, codeChip: codeChip)
        return mentions.text(attr, link: link, pill: link.opacity(0.14), lineHeight: Theme.body * 1.2)
    }
}
