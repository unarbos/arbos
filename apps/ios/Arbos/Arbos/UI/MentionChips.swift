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
// three concatenated pieces: a lead image (the picture, with the pill's inset
// before it and the gap to the name after it), the name run (accent, a link
// that `ChatView` routes to `showProfile`) and a trail image (the inset after
// the name). One pill goes behind all three: `MentionPillRenderer` draws it
// per line, centred on the name's glyph box, before the glyphs
// ([feedback mention-chip-highlight-2026-10-28]: the name's own text background
// stacked a second, offset box inside the caps' pill). iOS 17 has no
// `TextRenderer`, so there the caps still carry the pill's ends.

/// One mentionable entity.
struct MentionEntry {
    let name: String
    let target: ProfileTarget
    /// What the chip's picture is drawn from.
    let picture: AnyView
    /// The picture's URL when the entity has one: the cap is rendered through `ImageRenderer`,
    /// which draws only what `AvatarImageCache` already holds, so the cap's cache key carries
    /// whether the picture had landed — a cap drawn before the download is redrawn after it.
    var pictureURL: String? = nil
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
    /// `byHandle`'s entries in the order they were added: two that share a display name
    /// resolve the same way on every rebuild.
    private var order: [MentionEntry] = []
    /// Display names, longest first, so `@Launch week` beats `@Launch`.
    private var names: [(String, MentionEntry)] = []

    /// The chip's geometry (Grok Bot's mention chip): `padX` inside both ends of
    /// the pill, the `picture` pt disc, `gap` before the name (#1533); the pill
    /// rises `padY` over the name's glyph box and the disc sits on its centre.
    nonisolated static let picture: CGFloat = 16
    nonisolated static let padX: CGFloat = 4
    nonisolated static let gap: CGFloat = 3
    nonisolated static let capWidth: CGFloat = padX + picture + gap
    nonisolated static let padY: CGFloat = 2
    nonisolated static let radius: CGFloat = 7
    /// The name's face: the pill's height and centre come from it.
    nonisolated static let nameFont = UIFont.systemFont(ofSize: Theme.body, weight: .medium)
    /// The glyph box's centre above the baseline.
    nonisolated static var glyphCentre: CGFloat { (nameFont.ascender + nameFont.descender) / 2 }
    nonisolated static var pillHeight: CGFloat { nameFont.ascender - nameFont.descender + padY * 2 }
    /// Lifts the disc (an inline image sits on the baseline) onto the glyph box's centre.
    nonisolated static var pictureBaselineOffset: CGFloat { glyphCentre - picture / 2 }

    /// The pill for one line's share of a chip: `minX…maxX` across, centred on
    /// the name's glyph box over `baseline`.
    nonisolated static func pillRect(minX: CGFloat, maxX: CGFloat, baseline: CGFloat) -> CGRect {
        CGRect(x: minX, y: baseline - glyphCentre - pillHeight / 2, width: maxX - minX, height: pillHeight)
    }

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
                // The group's picture through the resolver (the chip drew initials even for
                // a group with a picture); the stamp below carries the image key.
                let url = GroupAvatar.pictureURL(chat, model: model)
                if let url { AvatarImageCache.shared.fetch(url) }
                add(slug, MentionEntry(
                    name: chat.title,
                    target: .group(chatId: chat.id, projectId: nil),
                    picture: AnyView(GroupAvatar(chat: chat, model: model, size: Self.picture)),
                    pictureURL: url
                ))
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
        finish()
    }

    /// A fixed table, entries in the order the sources add them (tests).
    init(entries: [(String, MentionEntry)]) {
        for (handle, entry) in entries { add(handle, entry) }
        finish()
    }

    /// Two entities with one name (a chat member "Scout" with a photo and a directory "Scout"
    /// without one) keep the first added — chat people before the directory — never the
    /// dictionary's order, which differs per table and flipped the chip between the photo and
    /// the other one's initials ([feedback mention-chip-name-tie-ios-2026-10-29]).
    private func finish() {
        names = order.enumerated()
            .sorted { a, b in
                let (x, y) = (a.element.name, b.element.name)
                if x.count != y.count { return x.count > y.count }
                if x != y { return x < y }
                return a.offset < b.offset
            }
            .map { ($0.element.name, $0.element) }
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
                h.combine(chat.imageKey)
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
        if byHandle[key] == nil {
            byHandle[key] = entry
            order.append(entry)
        }
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

    /// The chip's lead: the picture `padX` from the pill's left edge, `gap` clear of the name,
    /// rendered once per entity. `fill` paints the pill's rounded left end into the image
    /// (iOS 17, no renderer); without it the image is the disc's own box on a clear ground.
    func leftCap(for entry: MentionEntry, fill: Color?, height: CGFloat, key: String) -> UIImage? {
        let landed = entry.pictureURL.map { AvatarImageCache.shared.image(for: $0) != nil } ?? false
        let cacheKey = "L|\(key)|\(entry.pictureURL ?? "")|\(landed)|\(fill?.description ?? "clear")|\(height)"
        if let hit = Self.capCache[cacheKey] { return hit }
        let view = ZStack(alignment: .leading) {
            if let fill {
                UnevenRoundedRectangle(topLeadingRadius: Self.radius, bottomLeadingRadius: Self.radius, bottomTrailingRadius: 0, topTrailingRadius: 0, style: .continuous)
                    .fill(fill)
            }
            entry.picture.padding(.leading, Self.padX)
        }
        .frame(width: Self.capWidth, height: height, alignment: .leading)
        let renderer = ImageRenderer(content: view)
        renderer.scale = UIScreen.main.scale
        guard let img = renderer.uiImage else { return nil }
        Self.capCache[cacheKey] = img
        return img
    }

    /// The chip's trail: `padX` of clear space after the name, or (iOS 17) the pill's
    /// rounded right end in `fill`.
    func rightCap(fill: Color?, height: CGFloat) -> UIImage? {
        let cacheKey = "R|\(fill?.description ?? "clear")|\(height)"
        if let hit = Self.capCache[cacheKey] { return hit }
        let view = UnevenRoundedRectangle(topLeadingRadius: 0, bottomLeadingRadius: 0, bottomTrailingRadius: Self.radius, topTrailingRadius: Self.radius, style: .continuous)
            .fill(fill ?? .clear)
            .frame(width: fill == nil ? Self.padX : Self.radius, height: height)
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
        for (index, (range, entry)) in chips.enumerated() {
            // Map the plain-string range onto the attributed string by character offset.
            let loOff = plain.distance(from: plain.startIndex, to: range.lowerBound)
            let hiOff = plain.distance(from: plain.startIndex, to: range.upperBound)
            let lo = attr.characters.index(attr.startIndex, offsetBy: loOff)
            let hi = attr.characters.index(attr.startIndex, offsetBy: hiOff)
            out = out + icons.text(AttributedString(attr[cursor..<lo]), lineHeight: lineHeight)
            // The name run: accent, a link `ChatView` routes to the pane.
            var name = AttributedString(attr[attr.index(afterCharacter: lo)..<hi])
            name.swiftUI.foregroundColor = link
            name.underlineStyle = nil
            name.link = Self.url(for: entry.target)
            name.swiftUI.font = .system(size: Theme.body, weight: .medium)
            let key = Self.key(for: entry.target)
            if #available(iOS 18.0, *) {
                // The pill is the chip's only background: a code span's or spoiler's box
                // behind the name would stack a second one inside it.
                name.swiftUI.backgroundColor = nil
                if let cap = leftCap(for: entry, fill: nil, height: Self.picture, key: key) {
                    out = out + Text(Image(uiImage: cap)).baselineOffset(Self.pictureBaselineOffset)
                }
                out = out + Text(name).customAttribute(MentionPill(chip: index, fill: pill))
                if let cap = rightCap(fill: nil, height: 1) {
                    out = out + Text(Image(uiImage: cap))
                }
            } else {
                name.swiftUI.backgroundColor = pill
                let dy = -Self.padY - 1
                let h = lineHeight + Self.padY * 2
                if let cap = leftCap(for: entry, fill: pill, height: h, key: key) {
                    out = out + Text(Image(uiImage: cap)).baselineOffset(dy)
                }
                out = out + Text(name)
                if let cap = rightCap(fill: pill, height: h) {
                    out = out + Text(Image(uiImage: cap)).baselineOffset(dy)
                }
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

// MARK: - The pill

/// Marks a chip's name run: which chip of the text it belongs to and the pill's colour.
@available(iOS 18.0, *)
struct MentionPill: TextAttribute {
    let chip: Int
    let fill: Color
}

/// Draws one pill per chip and line behind the text: from the lead image's left edge
/// (`capWidth` before the name, on the chip's first line) to the trail's right edge (`padX`
/// after the name, on its last line), centred on the name's glyph box. Text without
/// `MentionPill` runs draws as it would without the renderer.
@available(iOS 18.0, *)
struct MentionPillRenderer: TextRenderer {
    struct Pill: Equatable {
        var rect: CGRect
        var fill: Color
    }

    var displayPadding: EdgeInsets {
        EdgeInsets(top: MentionTable.padY + 1, leading: 0, bottom: MentionTable.padY + 1, trailing: 0)
    }

    func draw(layout: Text.Layout, in ctx: inout GraphicsContext) {
        for pill in Self.pills(in: layout) {
            ctx.fill(RoundedRectangle(cornerRadius: MentionTable.radius, style: .continuous).path(in: pill.rect), with: .color(pill.fill))
        }
        for line in layout {
            ctx.draw(line)
        }
    }

    static func pills(in layout: Text.Layout) -> [Pill] {
        var pills: [Pill] = []
        // Per chip: where its pills are in `pills`, the first one's index and the line last seen.
        var seen: [Int: (first: Int, last: Int, line: Int)] = [:]
        for (lineIndex, line) in layout.enumerated() {
            let baseline = line.typographicBounds.origin.y
            for run in line {
                guard let mark = run[MentionPill.self] else { continue }
                let r = run.typographicBounds.rect
                if let s = seen[mark.chip], s.line == lineIndex {
                    pills[s.last].rect = MentionTable.pillRect(minX: pills[s.last].rect.minX, maxX: max(pills[s.last].rect.maxX, r.maxX), baseline: baseline)
                    continue
                }
                pills.append(Pill(rect: MentionTable.pillRect(minX: r.minX, maxX: r.maxX, baseline: baseline), fill: mark.fill))
                let first = seen[mark.chip]?.first ?? pills.count - 1
                seen[mark.chip] = (first, pills.count - 1, lineIndex)
            }
        }
        for s in seen.values {
            let lead = pills[s.first].rect
            pills[s.first].rect = CGRect(x: lead.minX - MentionTable.capWidth, y: lead.minY, width: lead.width + MentionTable.capWidth, height: lead.height)
            pills[s.last].rect.size.width += MentionTable.padX
        }
        return pills
    }
}

extension View {
    /// Text inside draws its mention chips' pills (`MentionPillRenderer`); iOS 17 keeps the caps' own.
    @ViewBuilder
    func mentionPills() -> some View {
        if #available(iOS 18.0, *) {
            textRenderer(MentionPillRenderer())
        } else {
            self
        }
    }
}
