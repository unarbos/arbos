// Telegram-style profile pages in Arbos/Grok Bot dress: a big avatar, name,
// status line, a row of glass action tiles, an info card, a pill of tabs,
// then the tab's list. Three pages share the scaffold: a person
// (`UserProfileView`), a group chat (`GroupProfileView`, people groups and
// shared project chats), and an agent (`AgentProfileView`, its own file).
// `AppModel.showProfile(_:)` opens one from anywhere; the root presents it.

import ArbosFFI
import PhotosUI
import SwiftUI
import UIKit

/// Who a user page is about — from a chat member, a search result or a sender row.
struct ProfilePerson: Identifiable, Equatable, Hashable {
    var id: String
    var handle: String
    var name: String?
    var avatarUrl: String?
    /// §9 presence as last reported by the relay for this row.
    var lastSeenAt: Int64? = nil
    var online: Bool = false
    var avatarColorIndex: Int? = nil

    var displayName: String { name?.isEmpty == false ? name! : "@\(handle)" }

    init(id: String, handle: String, name: String?, avatarUrl: String?, lastSeenAt: Int64? = nil, online: Bool = false, avatarColorIndex: Int? = nil) {
        self.id = id; self.handle = handle; self.name = name; self.avatarUrl = avatarUrl; self.lastSeenAt = lastSeenAt; self.online = online; self.avatarColorIndex = avatarColorIndex
    }
    init(_ m: ChatMember) { self.init(id: m.id, handle: m.handle, name: m.name, avatarUrl: m.avatarUrl, lastSeenAt: m.lastSeenAt, online: m.online, avatarColorIndex: m.avatarColorIndex) }
    init(_ p: Person) { self.init(id: p.id, handle: p.handle, name: p.name, avatarUrl: p.avatarUrl, lastSeenAt: p.lastSeenAt, online: p.online, avatarColorIndex: p.avatarColorIndex) }
    init(_ p: ProjectMember) { self.init(id: p.id, handle: p.handle, name: p.name, avatarUrl: p.avatarUrl, lastSeenAt: p.lastSeenAt, online: p.online, avatarColorIndex: p.avatarColorIndex) }
}

/// What the root view presents.
enum ProfileTarget: Identifiable, Equatable, Hashable {
    case user(ProfilePerson)
    case group(chatId: String, projectId: String?)
    case agent(projectId: String)
    /// D88: a server's details (Channels | Members; admin rows for owner/admin).
    case server(serverId: String)
    /// D88 (#1317): a channel's own page — what the `# general` pill opens.
    case channel(channelId: String)

    /// Pages whose member rows push a person over them (back chevron returns
    /// here). The agent page runs its own NavigationStack, so it stays a root.
    var pushesPeople: Bool {
        if case .agent = self { return false }
        return true
    }

    var id: String {
        switch self {
        case .user(let p): return "user:\(p.id)"
        case .group(let c, _): return "group:\(c)"
        case .agent(let p): return "agent:\(p)"
        case .server(let s): return "server:\(s)"
        case .channel(let c): return "channel:\(c)"
        }
    }
}

// MARK: - Scaffold pieces

/// Back button + optional trailing button over the header.
struct ProfileTopBar<Trailing: View>: View {
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    @ViewBuilder var trailing: () -> Trailing

    var body: some View {
        HStack {
            Button { dismiss() } label: {
                GlassCircle { Image(systemName: "chevron.left").font(.system(size: 17, weight: .semibold)).foregroundStyle(theme.text) }
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Back")
            Spacer()
            trailing()
        }
        .padding(.horizontal, 16)
        .padding(.top, 16)
    }
}

/// The one row/card kit every pane page draws with (Jacob, 2026-10-20:
/// "make the settings, user and group panels consistent"). Settings groups,
/// profile cards and the agent editor all build on these, so a metric lives
/// in exactly one place:
///
/// * card: 16 pt continuous radius, `bubbleAssistant` fill (D80), hairline
///   between rows inset under the label;
/// * row: 48 pt minimum, 16 pt side padding, 14 pt gap; leading = a 30 pt
///   icon tile, a 36 pt picture, or nothing; trailing value in body size,
///   muted, tail-truncated; chevron 14 pt semibold placeholder-coloured;
/// * danger row: centred red label, same height;
/// * tabs: one capsule pill, `field` fill on the selected tab.
enum PaneKit {
    static let rowHeight: CGFloat = 48
    static let inset: CGFloat = 16
    static let gap: CGFloat = 14
    static let tile: CGFloat = 30
    static let avatar: CGFloat = 36
    static let radius: CGFloat = 16
    /// Where a row's hairline starts for a given leading element.
    static func separatorInset(_ leading: PaneLeading) -> CGFloat {
        switch leading {
        case .none: return inset
        case .tile: return inset + tile + gap
        case .avatar, .glyph: return inset + avatar + gap
        }
    }
}

/// What sits at a row's leading edge.
enum PaneLeading {
    case none
    /// Apple-style 30 pt icon tile (settings rows); `dot` = machine state.
    case tile(String, dot: Color? = nil)
    /// A 36 pt picture (people, agents, groups).
    case avatar(AnyView)
    /// A 36 pt box with a glyph in it (Add members, files).
    case glyph(String)
}

/// The grouped card: one fill, one radius, hairlines inset under the label.
struct PaneCard<Rows: View>: View {
    @Environment(\.theme) private var theme
    var header: String? = nil
    var footer: String? = nil
    /// Where the hairlines start; a card of tile rows passes `.tile`.
    var separatorInset: CGFloat = PaneKit.inset
    @ViewBuilder var rows: () -> Rows

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let header {
                Text(header.uppercased())
                    .font(theme.font(Theme.tiny, weight: .medium))
                    .foregroundStyle(theme.muted)
                    .padding(.leading, PaneKit.inset)
            }
            _VariadicView.Tree(PaneCardLayout(separator: theme.separator, inset: separatorInset)) { rows() }
                .background(RoundedRectangle(cornerRadius: PaneKit.radius, style: .continuous).fill(theme.bubbleAssistant))
            if let footer {
                Text(footer)
                    .font(theme.font(Theme.tiny))
                    .foregroundStyle(theme.muted)
                    .padding(.horizontal, PaneKit.inset)
            }
        }
    }
}

private struct PaneCardLayout: _VariadicView_UnaryViewRoot {
    let separator: Color
    let inset: CGFloat

    @ViewBuilder
    func body(children: _VariadicView.Children) -> some View {
        let last = children.last?.id
        VStack(spacing: 0) {
            ForEach(children) { child in
                child
                if child.id != last {
                    Rectangle()
                        .fill(separator)
                        .frame(height: 1 / UIScreen.main.scale)
                        .padding(.leading, inset)
                }
            }
        }
    }
}

/// One row: leading, title (+ optional subtitle), optional trailing value,
/// chevron when it pushes a page.
struct PaneRow: View {
    @Environment(\.theme) private var theme
    var leading: PaneLeading = .none
    let title: String
    var subtitle: String? = nil
    var value: String? = nil
    var valueTint: Color? = nil
    var chevron: Bool = false
    var titleTint: Color? = nil
    /// A control at the trailing edge (a Toggle, a ✕); drawn after the value.
    var trailing: AnyView? = nil

    /// Mock b: names on people / agent / file rows are medium; mock g:
    /// settings labels are regular.
    private var titleWeight: Font.Weight {
        switch leading {
        case .avatar, .glyph: return .medium
        case .none, .tile: return .regular
        }
    }

    var body: some View {
        HStack(spacing: PaneKit.gap) {
            leadingView
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(theme.font(Theme.body, weight: titleWeight))
                    .foregroundStyle(titleTint ?? theme.text)
                    .lineLimit(1)
                if let subtitle, !subtitle.isEmpty {
                    Text(subtitle)
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                        .lineLimit(1)
                }
            }
            .layoutPriority(1)
            Spacer(minLength: 8)
            if let value, !value.isEmpty {
                // The value takes what the title leaves and clips at its end
                // (Apple's Settings), not in the middle.
                Text(value)
                    .font(theme.font(Theme.body))
                    .foregroundStyle(valueTint ?? theme.muted)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            if let trailing { trailing }
            if chevron {
                Image(systemName: "chevron.right")
                    .font(theme.font(14, weight: .semibold))
                    .foregroundStyle(theme.placeholder)
            }
        }
        .padding(.horizontal, PaneKit.inset)
        .padding(.vertical, 6)
        .frame(minHeight: PaneKit.rowHeight)
        .contentShape(Rectangle())
    }

    @ViewBuilder private var leadingView: some View {
        switch leading {
        case .none:
            EmptyView()
        case .tile(let icon, let dot):
            ZStack(alignment: .bottomTrailing) {
                RoundedRectangle(cornerRadius: 7.5, style: .continuous)
                    .fill(theme.isDark ? Color(hex: "#5C5C60")! : Color(hex: "#6E6E73")!)
                    .frame(width: PaneKit.tile, height: PaneKit.tile)
                    .overlay(
                        Image(systemName: icon)
                            .font(theme.font(15, weight: .semibold))
                            .foregroundStyle(.white)
                    )
                if let dot {
                    Circle().fill(dot).frame(width: 9, height: 9)
                        .overlay(Circle().strokeBorder(theme.bubbleAssistant, lineWidth: 1.5))
                        .offset(x: 3, y: 3)
                }
            }
        case .avatar(let view):
            view.frame(width: PaneKit.avatar, height: PaneKit.avatar)
        case .glyph(let icon):
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(theme.field)
                .frame(width: PaneKit.avatar, height: PaneKit.avatar)
                .overlay(Image(systemName: icon).font(theme.font(17, weight: .medium)).foregroundStyle(theme.muted))
        }
    }
}

/// The destructive action at the end of a pane: a centred red label.
struct PaneDangerRow: View {
    @Environment(\.theme) private var theme
    let title: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(theme.font(Theme.body))
                .foregroundStyle(theme.danger)
                .frame(maxWidth: .infinity)
                .frame(minHeight: PaneKit.rowHeight)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// A small caps title over a section of a pane ("Images · 3", "Files · 1").
struct PaneSectionTitle: View {
    @Environment(\.theme) private var theme
    let title: String
    init(_ title: String) { self.title = title }
    var body: some View {
        Text(title.uppercased())
            .font(theme.font(Theme.tiny, weight: .medium))
            .foregroundStyle(theme.muted)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, PaneKit.inset * 2)
    }
}

/// The one capsule tab pill (Profile | Settings, Links | Groups, Members | Library).
struct PaneTabs: View {
    @Environment(\.theme) private var theme
    let tabs: [String]
    @Binding var selected: String

    // Redesign mocks b/g (same kit as the Mac): bare labels, the current one
    // on a capsule, centred — no track, no border.
    var body: some View {
        HStack(spacing: 4) {
            ForEach(tabs, id: \.self) { t in
                Button { withAnimation(.snappy(duration: 0.2)) { selected = t } } label: {
                    Text(t)
                        .font(theme.font(Theme.body, weight: selected == t ? .semibold : .regular))
                        .foregroundStyle(selected == t ? theme.text : theme.muted)
                        .padding(.horizontal, 16).frame(height: 32)
                        .background(Capsule().fill(selected == t ? theme.bubbleAssistant : .clear))
                }
                .buttonStyle(.plain)
            }
        }
        .frame(maxWidth: .infinity)
    }
}

/// The hero: 112 pt picture, name, subtitle (@handle, member count, …).
struct PaneHeader<Avatar: View>: View {
    static var avatarSize: CGFloat { 112 }
    @Environment(\.theme) private var theme
    let name: String
    let subtitle: String
    @ViewBuilder var avatar: () -> Avatar

    var body: some View {
        VStack(spacing: 10) {
            avatar().frame(width: Self.avatarSize, height: Self.avatarSize)
            Text(name)
                .font(theme.font(28, weight: .semibold))
                .foregroundStyle(theme.text)
                .lineLimit(1)
                .padding(.horizontal, 24)
            Text(subtitle)
                .font(theme.font(Theme.small))
                .foregroundStyle(theme.muted)
        }
        .frame(maxWidth: .infinity)
    }
}

/// The crayon on an editable hero picture (own profile, people groups): a
/// small pencil disc at the bottom-right, same on every pane.
private struct PaneCrayon: ViewModifier {
    @Environment(\.theme) private var theme
    let on: Bool
    func body(content: Content) -> some View {
        content.overlay(alignment: .bottomTrailing) {
            if on {
                Image(systemName: "pencil")
                    .font(theme.font(13, weight: .semibold))
                    .foregroundStyle(theme.text)
                    .frame(width: 30, height: 30)
                    .background(Circle().fill(theme.field))
                    .overlay(Circle().strokeBorder(theme.bg, lineWidth: 2))
            }
        }
    }
}

extension View {
    func paneCrayon(_ on: Bool = true) -> some View { modifier(PaneCrayon(on: on)) }
}

// ----- the shared pane kit (PaneKit) and the names the pages use ------------

/// A grouped card of rows (the kit's card; hairlines inset to the label).
struct ProfileCard<Content: View>: View {
    var separatorInset: CGFloat = PaneKit.inset
    @ViewBuilder var content: () -> Content
    var body: some View {
        PaneCard(separatorInset: separatorInset) { content() }
            .padding(.horizontal, PaneKit.inset)
    }
}

/// One-line action row inside a card: the kit's row (red = the danger row).
struct ProfileLinkRow: View {
    let title: String
    var destructive = false
    let action: () -> Void

    var body: some View {
        if destructive {
            PaneDangerRow(title: title, action: action)
        } else {
            Button(action: action) { PaneRow(title: title, chevron: true) }.buttonStyle(.plain)
        }
    }
}

/// The tab pill at the pane's insets (profile pages lay out edge to edge).
struct ProfileTabs: View {
    let tabs: [String]
    @Binding var selected: String
    var body: some View { PaneTabs(tabs: tabs, selected: $selected).padding(.horizontal, PaneKit.inset) }
}

/// Last-seen text: the relay's `online`/`last_seen_at` (§9) merged with the
/// presence frames seen this session; "last seen recently" when unknown.
func lastSeenLine(_ at: Int64?, online: Bool = false, now: Int64 = nowMs()) -> (text: String, online: Bool) {
    if online { return ("online", true) }
    guard let at, at > 0 else { return ("last seen recently", false) }
    let age = max(0, now - at) / 1000
    if age < 90 { return ("online", true) }
    if age < 3600 { return ("last seen \(age / 60) min ago", false) }
    if age < 86_400 {
        let d = Date(timeIntervalSince1970: Double(at) / 1000)
        return ("last seen today at \(d.formatted(date: .omitted, time: .shortened))", false)
    }
    if age < 7 * 86_400 { return ("last seen \(age / 86_400) day\(age / 86_400 == 1 ? "" : "s") ago", false) }
    return ("last seen a long time ago", false)
}

// MARK: - Media / files / voice / links from a chat's messages

struct ChatItem: Identifiable, Equatable {
    enum Kind { case media, file, voice, link }
    var id: String
    var kind: Kind
    var title: String
    var subtitle: String
    var url: URL?
    var at: Int64
    var fromHandle: String?
    var mime: String
}

enum ChatMedia {
    private static let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)

    @MainActor
    static func items(messages: [Message], slot: String, model: AppModel) -> [ChatItem] {
        var out: [ChatItem] = []
        for m in messages {
            for part in m.parts {
                switch part {
                case .attachment(let id, let a):
                    // Same rules as the transcript (`Attachment.isImage`) so a photo
                    // lands under Media, not Files.
                    let kind: ChatItem.Kind = a.mime.hasPrefix("audio/") || a.kind == "audio" ? .voice
                        : (a.isImage || a.mime.hasPrefix("video/") || a.kind == "video") ? .media : .file
                    let url = a.blobRef.flatMap { model.blobURL(projectId: slot, key: $0) }
                    out.append(ChatItem(id: "\(m.id)#\(id)", kind: kind, title: a.name,
                                        subtitle: a.mime, url: url, at: m.createdAt, fromHandle: m.senderHandle, mime: a.mime))
                case .text(let id, let text):
                    guard let detector else { continue }
                    let ns = text as NSString
                    for match in detector.matches(in: text, range: NSRange(location: 0, length: ns.length)) {
                        guard let url = match.url, url.scheme?.hasPrefix("http") == true else { continue }
                        out.append(ChatItem(id: "\(m.id)#\(id)#\(match.range.location)", kind: .link, title: url.host() ?? url.absoluteString,
                                            subtitle: url.absoluteString, url: url, at: m.createdAt, fromHandle: m.senderHandle, mime: "text/uri-list"))
                    }
                default: continue
                }
            }
        }
        return out.sorted { $0.at > $1.at }
    }
}

struct ChatItemsList: View {
    @Environment(\.theme) private var theme
    let items: [ChatItem]
    let kind: ChatItem.Kind
    let empty: String

    private var filtered: [ChatItem] { items.filter { $0.kind == kind } }

    var body: some View {
        if filtered.isEmpty {
            Text(empty).font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                .frame(maxWidth: .infinity).padding(.top, 36).padding(.horizontal, 32).multilineTextAlignment(.center)
        } else if kind == .media {
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 2), count: 3), spacing: 2) {
                ForEach(filtered) { item in
                    // Size from the grid column (fit), then paint the image in an
                    // overlay. `aspectRatio(..., .fill)` + AsyncImage used the
                    // image's intrinsic size and blew the ScrollView (white sheet).
                    Color.clear
                        .aspectRatio(1, contentMode: .fit)
                        .overlay {
                            ZStack {
                                Rectangle().fill(theme.card)
                                if let url = item.url, !item.mime.hasPrefix("video/") {
                                    AsyncImage(url: url) { img in
                                        img.resizable().scaledToFill()
                                    } placeholder: {
                                        ProgressView().tint(theme.muted)
                                    }
                                } else {
                                    Image(systemName: "film").font(.system(size: 22)).foregroundStyle(theme.muted)
                                }
                            }
                        }
                        .clipped()
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
            .padding(.horizontal, 16)
        } else {
            ProfileCard(separatorInset: PaneKit.separatorInset(.glyph(""))) {
                ForEach(filtered) { item in
                    rowView(item)
                }
            }
        }
    }

    private func rowView(_ item: ChatItem) -> some View {
        Group {
            if let url = item.url {
                SwiftUI.Link(destination: url) { rowLabel(item) }
            } else {
                rowLabel(item)
            }
        }
        .buttonStyle(.plain)
    }

    private func rowLabel(_ item: ChatItem) -> some View {
        let when = Date(timeIntervalSince1970: Double(item.at) / 1000).formatted(date: .abbreviated, time: .omitted)
        return PaneRow(
            leading: .glyph(item.kind == .voice ? "waveform" : item.kind == .link ? "link" : "doc.fill"),
            title: item.title,
            subtitle: item.kind == .link ? item.subtitle : "\(item.fromHandle.map { "@\($0) · " } ?? "")\(when)"
        )
    }
}

// MARK: - User page

struct UserProfileView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let person: ProfilePerson
    @State private var tab = "Links"
    @State private var sharedProjects: [Project] = []
    @State private var confirmBlock = false

    /// Stable user id: prefer a fresh card, then a handle lookup (agent-owner rows
    /// sometimes open this page with `id == handle`), then the value we were given.
    private var peerId: String { fresh?.id ?? model.personId(handle: person.handle) ?? person.id }
    private var peerHandle: String { fresh?.handle ?? person.handle }
    private func isPeer(_ m: ChatMember) -> Bool {
        m.id == peerId || m.id == person.id
            || m.handle.caseInsensitiveCompare(peerHandle) == .orderedSame
    }
    private var dm: PeopleChat? {
        model.peopleChats.first { $0.kind == "dm" && $0.humans.contains(where: isPeer) }
    }
    @State private var fresh: Person?
    private var isMe: Bool { peerId == model.user?.id || person.id == model.user?.id }
    private var items: [ChatItem] {
        guard let dm else { return [] }
        return ChatMedia.items(messages: model.messages(of: dm.id), slot: "\(peopleSlot):\(dm.id)", model: model)
    }
    /// People groups we share — same rule as Mac: every non-DM space that lists this person.
    /// Match by id or handle; `isGroup` covers `kind == group` and multi-human chats whose kind lagged.
    private var sharedGroups: [PeopleChat] {
        model.peopleChats.filter { $0.isGroup && $0.humans.contains(where: isPeer) }
    }

    var body: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            ScrollView {
                VStack(spacing: 18) {
                    Color.clear.frame(height: 80)
                    // The shared read-only layout (2026-10-20, topbar-unify f/g):
                    // picture, name, @handle; no last-seen, no action row, no
                    // username field, no Open chat / Add members.
                    PaneHeader(name: person.displayName, subtitle: "@\(person.handle)") {
                        PersonAvatar(url: person.avatarUrl, handle: person.handle, size: 112, name: person.name, id: person.id, colorIndex: person.avatarColorIndex)
                    }
                    ProfileTabs(tabs: ["Links", "Groups"], selected: $tab)
                    if tab == "Links" {
                        ChatItemsList(items: items, kind: .link, empty: dm == nil ? "No chat with \(person.displayName) yet." : "No links shared yet.")
                    } else {
                        groupsList
                    }
                    if !isMe {
                        // The one action.
                        ProfileCard {
                            ProfileLinkRow(title: "Block user", destructive: true) { confirmBlock = true }
                        }
                    }
                    Color.clear.frame(height: 40)
                }
            }
            ProfileTopBar { EmptyView() }
        }
        .confirmationDialog("Block @\(person.handle)?", isPresented: $confirmBlock, titleVisibility: .visible) {
            Button("Block", role: .destructive) {
                if let m = dm?.humans.first(where: isPeer) { model.block(m) } else { model.block(handle: peerHandle) }
                dismiss()
            }
        } message: { Text("They cannot message you or add you to chats.") }
        .task {
            await model.refreshPeople()
            let id = model.personId(handle: person.handle) ?? person.id
            fresh = await model.fetchUser(id)
            let who = ProfilePerson(
                id: fresh?.id ?? id,
                handle: fresh?.handle ?? person.handle,
                name: fresh?.name ?? person.name,
                avatarUrl: fresh?.avatarUrl ?? person.avatarUrl,
                lastSeenAt: fresh?.lastSeenAt ?? person.lastSeenAt,
                online: fresh?.online ?? person.online,
                avatarColorIndex: fresh?.avatarColorIndex ?? person.avatarColorIndex
            )
            sharedProjects = await model.sharedProjects(with: who)
        }
    }

    private var groupsList: some View {
        let groups = sharedGroups
        return Group {
            if groups.isEmpty && sharedProjects.isEmpty {
                Text("No groups in common yet.").font(theme.font(Theme.small)).foregroundStyle(theme.muted).padding(.top, 36)
            } else {
                ProfileCard(separatorInset: PaneKit.separatorInset(.glyph(""))) {
                    ForEach(groups) { g in
                        Button { dismiss(); model.open(peopleChat: g) } label: {
                            PaneRow(leading: .glyph("person.2.fill"), title: g.title, subtitle: "\(g.humans.count) members", chevron: true)
                        }
                        .buttonStyle(.plain)
                    }
                    ForEach(sharedProjects) { p in
                        Button { dismiss(); model.open(project: p) } label: {
                            PaneRow(
                                leading: .avatar(AnyView(AgentAvatar(project: p, theme: theme, size: PaneKit.avatar, still: true))),
                                title: p.name,
                                subtitle: "shared project · @\(p.ownerHandle ?? model.user?.login ?? "")",
                                chevron: true
                            )
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
    }

}

// MARK: - Group page

struct GroupProfileView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let chatId: String
    /// nil for people groups; the project for a shared project chat.
    let projectId: String?
    @State private var tab = "Members"
    @State private var projectMembers: [ProjectMember] = []
    @State private var showAdd = false
    @State private var showEdit = false
    @State private var pendingRemoveId: String?
    @State private var pendingRemoveName: String?

    private var people: PeopleChat? { model.peopleChat(chatId) }
    private var project: Project? { projectId.flatMap { model.project($0) } }
    private var title: String { project?.name ?? people?.title ?? "Group" }
    private var slot: String { projectId ?? "\(peopleSlot):\(chatId)" }
    private var items: [ChatItem] { ChatMedia.items(messages: model.messages(of: chatId), slot: slot, model: model) }

    private struct Row: Identifiable {
        var id: String
        var person: ProfilePerson
        var role: String
        var agent: Project?
    }

    private var rows: [Row] {
        if let people {
            return people.members.map { m in
                Row(id: m.id, person: ProfilePerson(m), role: m.role, agent: m.isAgent ? m.project : nil)
            }
        }
        var out = projectMembers.map { Row(id: $0.id, person: ProfilePerson($0), role: $0.role, agent: nil) }
        if let project { out.insert(Row(id: project.id, person: ProfilePerson(id: project.id, handle: mentionSlug(project.name), name: project.name, avatarUrl: nil), role: "agent", agent: project), at: 0) }
        return out
    }

    private var memberCount: Int { rows.filter { $0.agent == nil }.count }

    /// Owners (and admins, when a chat has that role) can remove other people.
    /// Check `me.role` and our row in `members` — parse defaults `myRole` to
    /// `"member"` when `me` is missing, which would otherwise hide remove.
    private var canManageMembers: Bool {
        if people?.myRole == "owner" || people?.myRole == "admin" { return true }
        let rowRole = people?.humans.first(where: { $0.id == model.user?.id })?.role
        return rowRole == "owner" || rowRole == "admin"
    }

    /// People: owners/admins remove anyone but themselves and other owners.
    /// Agents: only the agent's own owner (the relay's rule for
    /// `DELETE /chats/{id}/agents/{project}`), and only in people chats.
    private func canRemove(_ r: Row) -> Bool {
        if let agent = r.agent {
            guard people != nil, let me = model.user?.id else { return false }
            let member = people?.agents.first { $0.id == agent.id }
            return (member?.ownerId ?? agent.ownerId) == me
        }
        return canManageMembers && r.person.id != model.user?.id && r.role != "owner"
    }

    private var groupImageURL: String? {
        guard let people, let key = people.imageKey, !key.isEmpty else { return nil }
        return model.blobURL(projectId: "\(peopleSlot):\(chatId)", key: key)?.absoluteString
    }

    private var clusterMembers: [ChatMember] {
        guard let people else { return [] }
        let others = people.humans.filter { $0.id != model.user?.id }
        return others.isEmpty ? people.humans : others
    }

    var body: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            ScrollView {
                VStack(spacing: 18) {
                    Color.clear.frame(height: 80)
                    // The shared pane layout (panels-unify, 2026-10-21): picture
                    // (crayon when a people group — it opens the edit sheet), name,
                    // member count, Members | Library, one leave row at the end.
                    PaneHeader(name: title, subtitle: "\(memberCount) member\(memberCount == 1 ? "" : "s")") {
                        if let project {
                            AgentAvatar(project: project, theme: theme, size: 112, idle: true)
                        } else {
                            Button { showEdit = true } label: {
                                AvatarCluster(members: clusterMembers, size: 72, imageUrl: groupImageURL, leaf: people?.leafPick)
                                    .frame(width: PaneHeader<EmptyView>.avatarSize, height: PaneHeader<EmptyView>.avatarSize)
                                    .paneCrayon()
                                    .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Edit group name and picture")
                        }
                    }
                    ProfileTabs(tabs: ["Members", "Library"], selected: $tab)
                    if tab == "Members" {
                        membersList
                    } else {
                        library
                    }
                    // No Mute / Leave here (Jacob, 2026-10-20, same as the Mac): leaving
                    // or deleting a group is the chat list's Delete; mute is gone.
                    Color.clear.frame(height: 40)
                }
            }
            ProfileTopBar { EmptyView() }
        }
        .confirmationDialog(
            pendingRemoveName.map { "Remove \($0) from \(title)?" } ?? "Remove from chat?",
            isPresented: Binding(
                get: { pendingRemoveId != nil },
                set: { if !$0 { pendingRemoveId = nil; pendingRemoveName = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Remove", role: .destructive) {
                if let id = pendingRemoveId {
                    if rows.first(where: { $0.id == id })?.agent != nil {
                        model.removeAgent(chatId: chatId, projectId: id)
                    } else {
                        model.removeMember(chatId: chatId, userId: id)
                    }
                }
                pendingRemoveId = nil
                pendingRemoveName = nil
            }
            Button("Cancel", role: .cancel) {
                pendingRemoveId = nil
                pendingRemoveName = nil
            }
        }
        .sheet(isPresented: $showAdd) {
            if let project {
                ProjectMembersSheet(project: project).environmentObject(model).environment(\.theme, theme)
            } else {
                // The New group picker aimed at this group: its members show as
                // already in, Add puts the picks into it (not into a new group).
                // A sheet over the profile sheet, like GroupEditSheet: no
                // presentation / colour-scheme modifiers here — they unseat
                // the presenting sheet.
                NavigationStack {
                    NewGroupView(onDone: { showAdd = false }, addTo: chatId)
                }
                .tint(theme.text)
                .environmentObject(model)
                .environment(\.theme, theme)
            }
        }
        .sheet(isPresented: $showEdit) {
            GroupEditSheet(chatId: chatId).environmentObject(model).environment(\.theme, theme)
        }
        .task {
            if let projectId { projectMembers = await model.projectMembers(projectId) }
        }
        .onAppear {
            // Rig: `ARBOS_SHOW=addmembers` lands on the Add members picker.
            if DebugHooks.show == "addmembers", project == nil {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { showAdd = true }
            }
        }
    }

    /// Members: Add members, then every person (name, @handle) and agent
    /// ("Agent · run by …") as kit rows — the Mac group pane's list.
    private var membersList: some View {
        ProfileCard(separatorInset: PaneKit.separatorInset(.glyph(""))) {
            Button { showAdd = true } label: {
                PaneRow(leading: .glyph("person.badge.plus"), title: "Add members")
            }
            .buttonStyle(.plain)
            ForEach(rows) { r in
                memberRow(r)
            }
        }
    }

    private func memberSubtitle(_ r: Row) -> String {
        if let a = r.agent {
            let member = people?.agents.first { $0.id == a.id }
            let ownerId = member?.ownerId ?? a.ownerId
            if ownerId == nil || ownerId == model.user?.id { return "Agent · run by you" }
            if let h = member?.ownerHandle ?? a.ownerHandle { return "Agent · run by @\(h)" }
            return "Agent"
        }
        return "@\(r.person.handle)"
    }

    private func memberRow(_ r: Row) -> some View {
        let isMe = r.agent == nil && r.person.id == model.user?.id
        let title = r.agent?.name ?? (isMe ? "\(r.person.displayName) (you)" : r.person.displayName)
        let avatar: AnyView = r.agent.map { AnyView(AgentAvatar(project: $0, theme: theme, size: PaneKit.avatar, still: true)) }
            ?? AnyView(PersonAvatar(url: r.person.avatarUrl, handle: r.person.handle, size: PaneKit.avatar, name: r.person.name, id: r.person.id, colorIndex: r.person.avatarColorIndex))
        // Jacob (2026-10-20): a small ✕ at the trailing edge removes a member in
        // one tap plus the confirmation; owners only.
        let remove: AnyView? = canRemove(r) ? AnyView(
            Button {
                pendingRemoveId = r.person.id
                pendingRemoveName = "@\(r.person.handle)"
            } label: {
                Image(systemName: "xmark")
                    .font(theme.font(12, weight: .semibold))
                    .foregroundStyle(theme.muted)
                    .frame(width: 32, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.borderless)
            .accessibilityLabel("Remove @\(r.person.handle)")
        ) : nil
        // borderless (not .plain): .plain steals long-press on recent iOS,
        // so the remove context menu never appears (same as HomeView rows).
        return Button {
            if let a = r.agent { model.showProfile(.agent(projectId: a.id)) } else { model.showProfile(.user(r.person)) }
        } label: {
            PaneRow(leading: .avatar(avatar), title: title, subtitle: memberSubtitle(r), trailing: remove)
        }
        .buttonStyle(.borderless)
        .contextMenu {
            if canRemove(r) {
                Button("Remove from chat", role: .destructive) {
                    pendingRemoveId = r.person.id
                    pendingRemoveName = "@\(r.person.handle)"
                }
            }
        }
    }

    /// Library: every image, then every file shared in the chat — one place.
    private var library: some View {
        let media = items.filter { $0.kind == .media }
        let files = items.filter { $0.kind == .file }
        let voice = items.filter { $0.kind == .voice }
        return Group {
            if media.isEmpty && files.isEmpty && voice.isEmpty {
                Text("Nothing shared here yet.").font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                    .frame(maxWidth: .infinity).padding(.top, 36)
            } else {
                if !media.isEmpty {
                    VStack(alignment: .leading, spacing: 8) {
                        PaneSectionTitle("Images · \(media.count)")
                        ChatItemsList(items: media, kind: .media, empty: "")
                    }
                }
                if !files.isEmpty {
                    VStack(alignment: .leading, spacing: 8) {
                        PaneSectionTitle("Files · \(files.count)")
                        ChatItemsList(items: files, kind: .file, empty: "")
                    }
                }
                if !voice.isEmpty {
                    VStack(alignment: .leading, spacing: 8) {
                        PaneSectionTitle("Voice · \(voice.count)")
                        ChatItemsList(items: voice, kind: .voice, empty: "")
                    }
                }
            }
        }
    }

}

// MARK: - Server page (D88)

/// A server's details on the shared pane kit (rule 5): disc, name, member
/// count, Channels | Members, the invite link (rule 6), then the one action at
/// the end. Owners and admins get the admin rows (rename, add / rename /
/// delete a channel, "Admins only post", roles, remove, ban, reset link);
/// members see the same page read-only with Leave.
struct ServerProfileView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let serverId: String
    @State private var tab = "Channels"
    @State private var showRename = false
    @State private var renameDraft = ""
    @State private var showNewChannel = false
    @State private var renamingChannel: ServerChannel?
    @State private var channelDraft = ""
    @State private var deletingChannel: ServerChannel?
    @State private var removingMember: ServerMember?
    @State private var banningMember: ServerMember?
    @State private var confirmLeave = false
    @State private var confirmDelete = false
    @State private var confirmReset = false
    @State private var copied = false
    @State private var loadedMembers = false

    private var server: Server? { model.server(serverId) }
    private var isAdmin: Bool { server?.isAdmin ?? false }
    private var isOwner: Bool { server?.isOwner ?? false }
    private var memberCount: Int { max(server?.memberCount ?? 0, server?.members.count ?? 0) }

    var body: some View {
        serverDialogs(memberDialogs(channelDialogs(frame)))
            .sheet(isPresented: $showNewChannel) {
                NewChannelSheet(serverId: serverId).environmentObject(model).environment(\.theme, theme)
            }
            .task {
                await model.refreshServer(serverId)
                loadedMembers = true
            }
    }

    /// The page without its dialogs: split out so the compiler type-checks each part alone.
    private var frame: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            ScrollView {
                VStack(spacing: 18) {
                    Color.clear.frame(height: 80)
                    if let server {
                        header(server)
                        ProfileTabs(tabs: ["Channels", "Members"], selected: $tab)
                        if tab == "Channels" {
                            channelsList(server)
                        } else {
                            membersList(server)
                        }
                        if isAdmin {
                            openCard(server)
                            if let invite = server.invite {
                                inviteCard(invite)
                            }
                        }
                        actions(server)
                    } else {
                        Text("This server is no longer available.")
                            .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                            .padding(.top, 80)
                    }
                    Color.clear.frame(height: 40)
                }
            }
            ProfileTopBar { EmptyView() }
        }
    }

    @ViewBuilder
    private func header(_ server: Server) -> some View {
        PaneHeader(name: server.name, subtitle: "\(memberCount) member\(memberCount == 1 ? "" : "s")") {
            if isAdmin {
                Button {
                    renameDraft = server.name
                    showRename = true
                } label: {
                    ServerAvatar(server: server, size: PaneHeader<EmptyView>.avatarSize)
                        .paneCrayon()
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Rename server")
            } else {
                ServerAvatar(server: server, size: PaneHeader<EmptyView>.avatarSize)
            }
        }
    }

    private func actions(_ server: Server) -> some View {
        ProfileCard {
            if isAdmin {
                ProfileLinkRow(title: "Rename server") {
                    renameDraft = server.name
                    showRename = true
                }
            }
            if isOwner {
                ProfileLinkRow(title: "Delete server", destructive: true) { confirmDelete = true }
            } else {
                ProfileLinkRow(title: "Leave server", destructive: true) { confirmLeave = true }
            }
        }
    }

    private func channelDialogs<V: View>(_ v: V) -> some View {
        v
            .alert("Rename server", isPresented: $showRename) {
                TextField("Server name", text: $renameDraft)
                Button("Save") { Task { await model.renameServer(serverId, name: renameDraft) } }
                Button("Cancel", role: .cancel) { }
            }
            .alert("Rename channel", isPresented: Binding(
                get: { renamingChannel != nil },
                set: { if !$0 { renamingChannel = nil } }
            )) {
                TextField("Channel name", text: $channelDraft)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                Button("Save") {
                    if let c = renamingChannel { Task { await model.renameChannel(c, name: channelDraft) } }
                    renamingChannel = nil
                }
                Button("Cancel", role: .cancel) { renamingChannel = nil }
            } message: {
                Text("Letters, digits and dashes.")
            }
            .confirmationDialog(
                deletingChannel.map { "Delete #\($0.name)?" } ?? "Delete channel?",
                isPresented: Binding(
                    get: { deletingChannel != nil },
                    set: { if !$0 { deletingChannel = nil } }
                ),
                titleVisibility: .visible
            ) {
                Button("Delete channel", role: .destructive) {
                    if let c = deletingChannel { Task { await model.deleteChannel(c) } }
                    deletingChannel = nil
                }
                Button("Cancel", role: .cancel) { deletingChannel = nil }
            } message: {
                Text("Its messages go for everyone in the server.")
            }
    }

    private func memberDialogs<V: View>(_ v: V) -> some View {
        v
            .confirmationDialog(
                removingMember.map { "Remove @\($0.handle)?" } ?? "Remove member?",
                isPresented: Binding(
                    get: { removingMember != nil },
                    set: { if !$0 { removingMember = nil } }
                ),
                titleVisibility: .visible
            ) {
                Button("Remove", role: .destructive) {
                    if let m = removingMember { Task { await model.removeServerMember(serverId: serverId, userId: m.id, ban: false) } }
                    removingMember = nil
                }
                Button("Cancel", role: .cancel) { removingMember = nil }
            } message: {
                Text("They can join again with the invite link.")
            }
            .confirmationDialog(
                banningMember.map { "Ban @\($0.handle)?" } ?? "Ban member?",
                isPresented: Binding(
                    get: { banningMember != nil },
                    set: { if !$0 { banningMember = nil } }
                ),
                titleVisibility: .visible
            ) {
                Button("Ban", role: .destructive) {
                    if let m = banningMember { Task { await model.removeServerMember(serverId: serverId, userId: m.id, ban: true) } }
                    banningMember = nil
                }
                Button("Cancel", role: .cancel) { banningMember = nil }
            } message: {
                Text("They are removed and the invite link no longer works for them.")
            }
    }

    private func serverDialogs<V: View>(_ v: V) -> some View {
        v
            .confirmationDialog("Leave \(server?.name ?? "this server")?", isPresented: $confirmLeave, titleVisibility: .visible) {
                Button("Leave", role: .destructive) {
                    Task {
                        if await model.leaveServer(serverId) { dismiss() }
                    }
                }
            } message: { Text("You can come back with an invite link.") }
            .confirmationDialog("Delete \(server?.name ?? "this server")?", isPresented: $confirmDelete, titleVisibility: .visible) {
                Button("Delete server", role: .destructive) {
                    Task {
                        if await model.deleteServer(serverId) { dismiss() }
                    }
                }
            } message: { Text("Every channel and message goes, for everyone.") }
            .confirmationDialog("Reset the invite link?", isPresented: $confirmReset, titleVisibility: .visible) {
                Button("Reset link", role: .destructive) { Task { await model.resetServerInvite(serverId) } }
            } message: { Text("The old link stops working. People already in stay.") }
    }

    // MARK: Channels

    @ViewBuilder
    private func channelsList(_ server: Server) -> some View {
        ProfileCard(separatorInset: PaneKit.separatorInset(.glyph(""))) {
            if isAdmin {
                // Rule 7: owners/admins add channels; members never see this row.
                Button { showNewChannel = true } label: {
                    PaneRow(leading: .glyph("plus"), title: "Add channel")
                }
                .buttonStyle(.plain)
            }
            ForEach(server.liveChannels) { c in
                channelRow(c, in: server)
            }
        }
        if !server.archivedChannels.isEmpty {
            // Channels I archived (row menu → Archive): Unarchive brings one back to my list.
            VStack(alignment: .leading, spacing: 8) {
                PaneSectionTitle("Archived")
                ProfileCard(separatorInset: PaneKit.separatorInset(.glyph(""))) {
                    ForEach(server.archivedChannels) { c in
                        PaneRow(leading: .glyph("archivebox"), title: c.name,
                                subtitle: model.channelRestricted(c) ? "Admins only post" : nil,
                                trailing: AnyView(
                                    Button("Unarchive") { Task { await model.archiveChannel(c, false) } }
                                        .font(theme.font(Theme.small, weight: .medium))
                                        .buttonStyle(.bordered)
                                        .tint(theme.muted)
                                ))
                    }
                }
            }
        }
    }

    private func channelRow(_ c: ServerChannel, in server: Server) -> some View {
        let restricted = model.channelRestricted(c)
        let isGeneral = c.id == server.generalChannelId || (server.generalChannelId == nil && c.name == "general")
        let menu: AnyView? = isAdmin ? AnyView(
            Menu {
                Button {
                    channelDraft = c.name
                    renamingChannel = c
                } label: { Label("Rename", systemImage: "pencil") }
                Button {
                    Task { await model.setChannelRestricted(c, !restricted) }
                } label: {
                    Label("Admins only post", systemImage: restricted ? "checkmark" : "lock")
                }
                if !isGeneral {
                    Divider()
                    Button(role: .destructive) { deletingChannel = c } label: { Label("Delete channel", systemImage: "trash") }
                }
            } label: {
                Image(systemName: "ellipsis.circle")
                    .font(theme.font(17))
                    .foregroundStyle(theme.muted)
                    .frame(width: 32, height: 44)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("Channel options")
        ) : nil
        return Button {
            dismiss()
            model.open(channel: c)
        } label: {
            PaneRow(leading: .glyph("number"), title: c.name,
                    subtitle: restricted ? "Admins only post" : "Everyone posts",
                    trailing: menu)
        }
        .buttonStyle(.borderless)
    }

    // MARK: Members

    private func membersList(_ server: Server) -> some View {
        Group {
            if server.members.isEmpty {
                if loadedMembers {
                    Text("No members to show.").font(theme.font(Theme.small)).foregroundStyle(theme.muted).padding(.top, 36)
                } else {
                    HStack { Spacer(); ProgressView().tint(theme.muted); Spacer() }.padding(.top, 36)
                }
            } else {
                ProfileCard(separatorInset: PaneKit.separatorInset(.glyph(""))) {
                    ForEach(sortedMembers(server)) { m in
                        memberRow(m)
                    }
                }
            }
        }
    }

    /// Owner first, then admins, then members; each group by name.
    private func sortedMembers(_ server: Server) -> [ServerMember] {
        func rank(_ r: String) -> Int { r == "owner" ? 0 : r == "admin" ? 1 : 2 }
        return server.members.sorted {
            if rank($0.role) != rank($1.role) { return rank($0.role) < rank($1.role) }
            return $0.displayName.localizedCaseInsensitiveCompare($1.displayName) == .orderedAscending
        }
    }

    private func roleLabel(_ role: String) -> String {
        switch role {
        case "owner": return "Owner"
        case "admin": return "Admin"
        default: return "Member"
        }
    }

    private func memberRow(_ m: ServerMember) -> some View {
        let isMe = m.id == model.user?.id
        let title = isMe ? "\(m.displayName) (you)" : m.displayName
        // Owners manage everyone else; admins manage members only.
        let canManage = !isMe && m.role != "owner" && (isOwner || (isAdmin && m.role == "member"))
        let menu: AnyView? = canManage ? AnyView(
            Menu {
                if isOwner {
                    if m.role == "admin" {
                        Button { Task { await model.setServerRole(serverId: serverId, userId: m.id, role: "member") } } label: {
                            Label("Make member", systemImage: "person")
                        }
                    } else {
                        Button { Task { await model.setServerRole(serverId: serverId, userId: m.id, role: "admin") } } label: {
                            Label("Make admin", systemImage: "person.badge.key")
                        }
                    }
                    Divider()
                }
                Button(role: .destructive) { removingMember = m } label: { Label("Remove", systemImage: "person.badge.minus") }
                Button(role: .destructive) { banningMember = m } label: { Label("Ban", systemImage: "hand.raised") }
            } label: {
                Image(systemName: "ellipsis.circle")
                    .font(theme.font(17))
                    .foregroundStyle(theme.muted)
                    .frame(width: 32, height: 44)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("Member options")
        ) : nil
        let person = ProfilePerson(id: m.id, handle: m.handle, name: m.name, avatarUrl: m.avatarUrl,
                                   lastSeenAt: m.lastSeenAt, online: m.online, avatarColorIndex: m.avatarColorIndex)
        return Button {
            model.showProfile(.user(person))
        } label: {
            PaneRow(
                leading: .avatar(AnyView(PersonAvatar(url: m.avatarUrl, handle: m.handle, size: PaneKit.avatar, name: m.name, id: m.id, colorIndex: m.avatarColorIndex))),
                title: title,
                subtitle: "@\(m.handle) · \(roleLabel(m.role))",
                trailing: menu
            )
        }
        .buttonStyle(.borderless)
    }

    // MARK: Open + invite (rule 6)

    private func openCard(_ server: Server) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            PaneCard(header: "Discovery", footer: "When open, anyone on Arbos can find this server by name and join without an invite link.") {
                PaneRow(
                    title: "Open server",
                    subtitle: "Searchable · no invite needed",
                    trailing: AnyView(
                        Toggle("", isOn: Binding(
                            get: { server.isOpen },
                            set: { on in Task { await model.setServerOpen(serverId, open: on) } }
                        ))
                        .labelsHidden()
                        .tint(theme.online)
                        .accessibilityLabel("Open server")
                    )
                )
            }
        }
        .padding(.horizontal, PaneKit.inset)
    }

    private func inviteCard(_ invite: ServerInvite) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            PaneCard(header: "Invite link", footer: isOwner ? "Anyone with the link can join. Reset it to stop old copies working." : "Anyone with the link can join.") {
                PaneRow(title: "Link", value: invite.url)
                Button {
                    UIPasteboard.general.string = invite.url
                    copied = true
                    Task { @MainActor in
                        try? await Task.sleep(for: .seconds(1.5))
                        copied = false
                    }
                } label: {
                    PaneRow(title: copied ? "Copied" : "Copy link")
                }
                .buttonStyle(.plain)
                if let url = URL(string: invite.url) {
                    ShareLink(item: url) {
                        PaneRow(title: "Share link", chevron: true)
                    }
                    .buttonStyle(.plain)
                }
                if isOwner {
                    ProfileLinkRow(title: "Reset link") { confirmReset = true }
                }
            }
        }
        .padding(.horizontal, PaneKit.inset)
    }
}

/// Finds the SwiftUI-hosted `UITextField` and makes a double-tap select the
/// whole string. The system default only selects one word, which is awkward for
/// comma-separated / hyphenated auto names like "a, b-c, d".
private struct SelectAllOnDoubleTap: UIViewRepresentable {
    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> UIView {
        let view = ProbeView()
        view.coordinator = context.coordinator
        return view
    }

    func updateUIView(_ uiView: UIView, context: Context) {
        context.coordinator.install(from: uiView)
    }

    final class ProbeView: UIView {
        weak var coordinator: Coordinator?
        override func didMoveToWindow() {
            super.didMoveToWindow()
            coordinator?.install(from: self)
        }
    }

    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        private weak var textField: UITextField?

        func install(from probe: UIView) {
            guard textField == nil, probe.window != nil else { return }
            guard let tf = Self.nearestTextField(from: probe) else { return }
            let gr = UITapGestureRecognizer(target: self, action: #selector(doubleTapped(_:)))
            gr.numberOfTapsRequired = 2
            gr.delegate = self
            tf.addGestureRecognizer(gr)
            textField = tf
        }

        @objc private func doubleTapped(_ gr: UITapGestureRecognizer) {
            guard let tf = gr.view as? UITextField else { return }
            if !tf.isFirstResponder { tf.becomeFirstResponder() }
            // After UIKit's word-select runs, expand to the full string.
            DispatchQueue.main.async { tf.selectAll(nil) }
        }

        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
            true
        }

        private static func nearestTextField(from view: UIView) -> UITextField? {
            var node: UIView? = view.superview
            while let current = node {
                if let tf = current as? UITextField { return tf }
                if let tf = findTextField(in: current, skipping: view) { return tf }
                node = current.superview
            }
            return nil
        }

        private static func findTextField(in root: UIView, skipping: UIView) -> UITextField? {
            for sub in root.subviews where sub !== skipping {
                if let tf = sub as? UITextField { return tf }
                if let tf = findTextField(in: sub, skipping: skipping) { return tf }
            }
            return nil
        }
    }
}

/// Name the group and optionally pick a picture. Saves with `PATCH /chats/{id}`.
struct GroupEditSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let chatId: String
    @State private var name = ""
    @State private var photoItem: PhotosPickerItem?
    @State private var preview: UIImage?
    @State private var jpegData: Data?
    @State private var saving = false
    @FocusState private var nameFocused: Bool

    private var people: PeopleChat? { model.peopleChat(chatId) }

    private var clusterMembers: [ChatMember] {
        guard let people else { return [] }
        let others = people.humans.filter { $0.id != model.user?.id }
        return others.isEmpty ? people.humans : others
    }

    private var existingImageURL: String? {
        guard let people, let key = people.imageKey, !key.isEmpty else { return nil }
        return model.blobURL(projectId: "\(peopleSlot):\(chatId)", key: key)?.absoluteString
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 22) {
                    PhotosPicker(selection: $photoItem, matching: .images) {
                        ZStack(alignment: .bottomTrailing) {
                            if let preview {
                                Image(uiImage: preview)
                                    .resizable().scaledToFill()
                                    .frame(width: 112, height: 112)
                                    .clipShape(Circle())
                            } else {
                                AvatarCluster(members: clusterMembers, size: 72, imageUrl: existingImageURL)
                                    .frame(width: 112, height: 112)
                            }
                            GlassCircle(size: 36) {
                                Image(systemName: "camera.fill")
                                    .font(theme.font(14, weight: .semibold))
                                    .foregroundStyle(theme.text)
                            }
                            .offset(x: 4, y: 4)
                        }
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Choose group image")

                    VStack(alignment: .leading, spacing: 8) {
                        Text("Group name").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                        TextField("", text: $name, prompt: Text("Name this group").foregroundStyle(theme.placeholder))
                            .font(theme.font(Theme.body))
                            .foregroundStyle(theme.text)
                            .autocorrectionDisabled()
                            .focused($nameFocused)
                            .background(SelectAllOnDoubleTap())
                            .padding(.horizontal, 14).frame(height: Theme.fieldHeight)
                            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(theme.field))
                            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(theme.fieldBorder, lineWidth: 1))
                    }
                    .padding(.horizontal, 16)

                    Text("A name and picture help everyone recognise this chat. Leave the name blank to use member handles.")
                        .font(theme.font(Theme.tiny))
                        .foregroundStyle(theme.muted)
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, 28)

                    Color.clear.frame(height: 24)
                }
                .padding(.top, 28)
            }
            .background(theme.bg)
            .navigationTitle("Edit group")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(theme.bg, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") { dismiss() }.foregroundStyle(theme.text)
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button { Task { await save() } } label: {
                        Text(saving ? "Saving…" : "Save")
                            .font(theme.font(Theme.small, weight: .semibold))
                            .foregroundStyle(theme.onPrimary)
                            .padding(.horizontal, 12).frame(height: 32)
                            .background(Capsule().fill(theme.primary))
                    }
                    .buttonStyle(.plain)
                    .disabled(saving)
                    .opacity(saving ? 0.55 : 1)
                }
            }
            .onAppear {
                name = people?.title ?? ""
                nameFocused = true
            }
            .onChange(of: photoItem) { _, item in
                guard let item else { return }
                Task {
                    guard let data = try? await item.loadTransferable(type: Data.self),
                          let img = UIImage(data: data) else { return }
                    let square = Self.square(img, side: 512)
                    preview = square
                    jpegData = square.jpegData(compressionQuality: 0.85)
                }
            }
        }
    }

    private func save() async {
        saving = true
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        var ok = await model.patchPeopleChat(chatId, title: trimmed)
        if ok, let jpegData {
            ok = await model.setGroupImage(chatId: chatId, jpeg: jpegData)
        }
        saving = false
        if ok { dismiss() }
    }

    /// Centre-crop to a square and resize.
    private static func square(_ image: UIImage, side: CGFloat) -> UIImage {
        let s = min(image.size.width, image.size.height)
        let rect = CGRect(x: (image.size.width - s) / 2, y: (image.size.height - s) / 2, width: s, height: s)
        let renderer = UIGraphicsImageRenderer(size: CGSize(width: side, height: side))
        return renderer.image { _ in
            guard let cg = image.cgImage?.cropping(to: rect) else {
                image.draw(in: CGRect(origin: .zero, size: CGSize(width: side, height: side)))
                return
            }
            UIImage(cgImage: cg, scale: image.scale, orientation: image.imageOrientation)
                .draw(in: CGRect(origin: .zero, size: CGSize(width: side, height: side)))
        }
    }
}

// MARK: - Channel (D88, #1317)

/// A channel's page: minimal until the channel profile (Profile | Settings) is approved — the
/// `#` disc, the name (never with a leading `#`: the glyph shows it), the topic, who posts,
/// the server's member count, and a row that pushes the server's page.
struct ChannelProfileView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let channelId: String

    private var channel: ServerChannel? { model.channel(channelId) }
    private var server: Server? { channel.flatMap { model.server($0.serverId) } }

    var body: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            ScrollView {
                VStack(spacing: 18) {
                    Color.clear.frame(height: 80)
                    if let channel, let server {
                        let restricted = model.channelRestricted(channel)
                        let count = max(server.memberCount, server.members.count)
                        PaneHeader(name: channel.name,
                                   subtitle: "\(count) member\(count == 1 ? "" : "s") · \(server.name)") {
                            RoundedRectangle(cornerRadius: PaneHeader<EmptyView>.avatarSize * 0.3, style: .continuous)
                                .fill(theme.field)
                                .overlay(
                                    Text("#")
                                        .font(theme.font(PaneHeader<EmptyView>.avatarSize * 0.5, weight: .semibold))
                                        .foregroundStyle(theme.muted)
                                )
                        }
                        PaneCard {
                            if let topic = channel.topic, !topic.trimmingCharacters(in: .whitespaces).isEmpty {
                                PaneRow(title: topic)
                            }
                            PaneRow(title: "Posting", value: restricted ? "Admins only" : "Everyone")
                        }
                        PaneCard {
                            Button {
                                model.showProfile(.server(serverId: server.id))
                            } label: {
                                PaneRow(leading: .avatar(AnyView(
                                    ServerAvatar(server: server, size: PaneKit.avatar)
                                )), title: server.name, subtitle: "Server", chevron: true)
                            }
                            .buttonStyle(.plain)
                        }
                    } else {
                        Text("This channel is no longer available.")
                            .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                            .padding(.top, 80)
                    }
                    Color.clear.frame(height: 40)
                }
            }
            ProfileTopBar { EmptyView() }
        }
        .task {
            if let channel { await model.refreshServer(channel.serverId) }
        }
    }
}
