// Telegram-style profile pages in Arbos/Grok Bot dress: a big avatar, name,
// status line, a row of glass action tiles, an info card, a pill of tabs,
// then the tab's list. Three pages share the scaffold: a person
// (`UserProfileView`), a group chat (`GroupProfileView`, people groups and
// shared project chats), and an agent (`AgentProfileView`, its own file).
// `AppModel.showProfile(_:)` opens one from anywhere; the root presents it.

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
enum ProfileTarget: Identifiable, Equatable {
    case user(ProfilePerson)
    case group(chatId: String, projectId: String?)
    case agent(projectId: String)

    var id: String {
        switch self {
        case .user(let p): return "user:\(p.id)"
        case .group(let c, _): return "group:\(c)"
        case .agent(let p): return "agent:\(p)"
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

struct ProfileHeader<Avatar: View>: View {
    @Environment(\.theme) private var theme
    let name: String
    let status: String
    var statusOnline = false
    @ViewBuilder var avatar: () -> Avatar

    var body: some View {
        VStack(spacing: 10) {
            avatar().frame(width: 112, height: 112)
            Text(name)
                .font(theme.font(28, weight: .semibold))
                .foregroundStyle(theme.text)
                .lineLimit(1)
                .padding(.horizontal, 24)
            Text(status)
                .font(theme.font(Theme.small))
                .foregroundStyle(statusOnline ? Color(hex: "#1084FE")! : theme.muted)
        }
        .padding(.top, -24)
        .frame(maxWidth: .infinity)
    }
}

struct ProfileAction: Identifiable {
    var id: String { label }
    let icon: String
    let label: String
    var enabled = true
    var soon = false
    var destructive = false
    let action: () -> Void
}

/// Telegram's row of square tiles (equal widths, blue icon + label).
struct ProfileActionRow: View {
    @Environment(\.theme) private var theme
    let actions: [ProfileAction]

    private var accent: Color { Color(hex: "#1084FE")! }

    var body: some View {
        HStack(spacing: 10) {
            ForEach(actions) { a in
                Button(action: a.action) {
                    VStack(spacing: 6) {
                        ZStack(alignment: .topTrailing) {
                            Image(systemName: a.icon).font(.system(size: 20, weight: .medium))
                            if a.soon {
                                Text("soon").font(theme.font(8, weight: .bold)).foregroundStyle(theme.bg)
                                    .padding(.horizontal, 4).padding(.vertical, 1)
                                    .background(Capsule().fill(theme.muted))
                                    .offset(x: 14, y: -8)
                            }
                        }
                        Text(a.label).font(theme.font(12))
                    }
                    .foregroundStyle(a.destructive ? theme.danger : (a.enabled ? accent : theme.muted))
                    .frame(maxWidth: .infinity).frame(height: 64)
                    .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(theme.card))
                }
                .buttonStyle(.plain)
                .disabled(!a.enabled)
                .accessibilityLabel(a.label + (a.soon ? ", coming soon" : ""))
            }
        }
        .padding(.horizontal, 16)
    }
}

struct ProfileCard<Content: View>: View {
    @Environment(\.theme) private var theme
    @ViewBuilder var content: () -> Content

    var body: some View {
        VStack(spacing: 0) { content() }
            .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(theme.card))
            .padding(.horizontal, 16)
    }
}

struct ProfileRowDivider: View {
    @Environment(\.theme) private var theme
    var inset: CGFloat = 16
    var body: some View { Rectangle().fill(theme.separator).frame(height: 0.5).padding(.leading, inset) }
}

/// Label-over-value row ("username" / "@handle"), optional trailing glyph.
struct ProfileInfoRow: View {
    @Environment(\.theme) private var theme
    let label: String
    let value: String
    var tint: Color? = nil
    var trailingIcon: String? = nil

    var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 3) {
                Text(label).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                Text(value).font(theme.font(Theme.body)).foregroundStyle(tint ?? theme.text).textSelection(.enabled)
            }
            Spacer()
            if let trailingIcon { Image(systemName: trailingIcon).font(.system(size: 18)).foregroundStyle(Color(hex: "#1084FE")!) }
        }
        .padding(.horizontal, 16).padding(.vertical, 12)
    }
}

/// Blue (or red) one-line action row inside a card.
struct ProfileLinkRow: View {
    @Environment(\.theme) private var theme
    let title: String
    var destructive = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack {
                Text(title).font(theme.font(Theme.body)).foregroundStyle(destructive ? theme.danger : Color(hex: "#1084FE")!)
                Spacer()
            }
            .padding(.horizontal, 16).frame(height: 48)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// Telegram's pill of tabs (dark capsule, light capsule on the selected one).
struct ProfileTabs: View {
    @Environment(\.theme) private var theme
    let tabs: [String]
    @Binding var selected: String

    var body: some View {
        HStack(spacing: 2) {
            ForEach(tabs, id: \.self) { t in
                Button { withAnimation(.snappy(duration: 0.2)) { selected = t } } label: {
                    Text(t)
                        .font(theme.font(Theme.small, weight: .medium))
                        .foregroundStyle(theme.text)
                        .padding(.horizontal, 16).frame(height: 36)
                        .background(Capsule().fill(selected == t ? theme.field : .clear))
                        .overlay(Capsule().strokeBorder(selected == t ? theme.fieldBorder : .clear, lineWidth: 1))
                }
                .buttonStyle(.plain)
            }
        }
        .padding(4)
        .background(Capsule().fill(theme.card))
        .overlay(Capsule().strokeBorder(theme.fieldBorder, lineWidth: 1))
    }
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
            ProfileCard {
                ForEach(Array(filtered.enumerated()), id: \.element.id) { i, item in
                    rowView(item)
                    if i < filtered.count - 1 { ProfileRowDivider(inset: 68) }
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
        HStack(spacing: 12) {
            GlassCircle(size: 40) {
                Image(systemName: item.kind == .voice ? "waveform" : item.kind == .link ? "link" : "doc.fill")
                    .font(theme.font(16, weight: .medium)).foregroundStyle(theme.text)
            }
            VStack(alignment: .leading, spacing: 3) {
                Text(item.title).font(theme.font(Theme.body)).foregroundStyle(theme.text).lineLimit(1)
                Text(item.kind == .link ? item.subtitle : "\(item.fromHandle.map { "@\($0) · " } ?? "")\(Date(timeIntervalSince1970: Double(item.at) / 1000).formatted(date: .abbreviated, time: .omitted))")
                    .font(theme.font(Theme.tiny)).foregroundStyle(item.kind == .link ? Color(hex: "#1084FE")! : theme.muted).lineLimit(1)
            }
            Spacer()
        }
        .padding(.horizontal, 14).padding(.vertical, 10)
        .contentShape(Rectangle())
    }
}

/// A member row: avatar, name, online/last seen, owner/admin badge.
struct MemberStatusRow: View {
    @Environment(\.theme) private var theme
    let person: ProfilePerson
    let status: (text: String, online: Bool)
    var badge: String? = nil
    var isMe = false
    var agent: Project? = nil

    var body: some View {
        HStack(spacing: 12) {
            if let agent {
                AgentAvatar(project: agent, theme: theme, size: 40, still: true)
            } else {
                PersonAvatar(url: person.avatarUrl, handle: person.handle, size: 40, name: person.name, id: person.id, colorIndex: person.avatarColorIndex)
            }
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(agent?.name ?? person.displayName).font(theme.font(Theme.body)).foregroundStyle(theme.text).lineLimit(1)
                    if isMe { Text("you").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted) }
                }
                Text(status.text).font(theme.font(Theme.tiny)).foregroundStyle(status.online ? Color(hex: "#1084FE")! : theme.muted)
            }
            Spacer()
            if let badge {
                Text(badge).font(theme.font(12, weight: .medium))
                    .foregroundStyle(badge == "owner" ? Color(hex: "#8B5CF6")! : theme.online)
                    .padding(.horizontal, 8).padding(.vertical, 3)
                    .background(Capsule().fill((badge == "owner" ? Color(hex: "#8B5CF6")! : theme.online).opacity(0.15)))
            }
        }
        .padding(.horizontal, 14).padding(.vertical, 9)
        .contentShape(Rectangle())
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
                    ProfileHeader(name: person.displayName, status: "@\(person.handle)", statusOnline: false) {
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
                ProfileCard {
                    ForEach(groups) { g in
                        Button { dismiss(); model.open(peopleChat: g) } label: {
                            HStack(spacing: 12) {
                                GlassCircle(size: 40) { Image(systemName: "person.2.fill").font(.system(size: 15)).foregroundStyle(theme.text) }
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(g.title).font(theme.font(Theme.body)).foregroundStyle(theme.text)
                                    Text("\(g.humans.count) members").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                                }
                                Spacer()
                            }
                            .padding(.horizontal, 14).padding(.vertical, 9).contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        ProfileRowDivider(inset: 68)
                    }
                    ForEach(sharedProjects) { p in
                        Button { dismiss(); model.open(project: p) } label: {
                            HStack(spacing: 12) {
                                AgentAvatar(project: p, theme: theme, size: 40, still: true)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(p.name).font(theme.font(Theme.body)).foregroundStyle(theme.text)
                                    Text("shared project · @\(p.ownerHandle ?? model.user?.login ?? "")").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                                }
                                Spacer()
                            }
                            .padding(.horizontal, 14).padding(.vertical, 9).contentShape(Rectangle())
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
    @State private var muted = false
    @State private var projectMembers: [ProjectMember] = []
    @State private var showAdd = false
    @State private var showEdit = false
    @State private var confirmLeave = false
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
                    ProfileHeader(name: title, status: "\(memberCount) member\(memberCount == 1 ? "" : "s")") {
                        if let project {
                            AgentAvatar(project: project, theme: theme, size: 112)
                        } else {
                            AvatarCluster(members: clusterMembers, size: 72, imageUrl: groupImageURL)
                                .frame(width: 112, height: 112)
                                .contentShape(Rectangle())
                                .onTapGesture { showEdit = true }
                        }
                    }
                    ProfileActionRow(actions: [
                        ProfileAction(icon: muted ? "bell.slash.fill" : "bell.fill", label: muted ? "unmute" : "mute") { toggleMute() },
                        ProfileAction(icon: "magnifyingglass", label: "search") {
                            model.pendingSearchChat = chatId
                            dismiss()
                        },
                        ProfileAction(icon: "rectangle.portrait.and.arrow.right", label: "leave", enabled: people != nil || project?.shared == true) { confirmLeave = true },
                        ProfileAction(icon: "ellipsis", label: "more") {
                            if people != nil {
                                showEdit = true
                            } else if let projectId {
                                model.showProfile(.agent(projectId: projectId))
                            }
                        },
                    ])
                    ProfileTabs(tabs: ["Members", "Media", "Files", "Voice", "Links"], selected: $tab)
                    switch tab {
                    case "Members": membersList
                    case "Media": ChatItemsList(items: items, kind: .media, empty: "No photos or videos shared yet.")
                    case "Files": ChatItemsList(items: items, kind: .file, empty: "No files shared yet.")
                    case "Voice": ChatItemsList(items: items, kind: .voice, empty: "No voice messages yet.")
                    default: ChatItemsList(items: items, kind: .link, empty: "No links shared yet.")
                    }
                    Color.clear.frame(height: 40)
                }
            }
            ProfileTopBar {
                if people != nil {
                    Button("Edit") { showEdit = true }
                        .font(theme.font(Theme.body, weight: .medium)).foregroundStyle(theme.text)
                        .padding(.horizontal, 14).frame(height: Theme.control)
                        .background(Capsule().fill(theme.field))
                } else if project != nil, project?.shared != true {
                    Button("Edit") { model.showProfile(.agent(projectId: projectId!)) }
                        .font(theme.font(Theme.body, weight: .medium)).foregroundStyle(theme.text)
                        .padding(.horizontal, 14).frame(height: Theme.control)
                        .background(Capsule().fill(theme.field))
                }
            }
        }
        .confirmationDialog("Leave \(title)?", isPresented: $confirmLeave, titleVisibility: .visible) {
            Button("Leave", role: .destructive) {
                if people != nil { model.leavePeopleChat(chatId) }
                dismiss()
            }
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
                MembersSheet(chatId: chatId).environmentObject(model).environment(\.theme, theme)
            }
        }
        .sheet(isPresented: $showEdit) {
            GroupEditSheet(chatId: chatId).environmentObject(model).environment(\.theme, theme)
        }
        .task {
            muted = people?.muted ?? false
            if let projectId { projectMembers = await model.projectMembers(projectId) }
        }
    }

    private var membersList: some View {
        ProfileCard {
            Button { showAdd = true } label: {
                HStack(spacing: 12) {
                    GlassCircle(size: 40) { Image(systemName: "person.badge.plus").font(.system(size: 16, weight: .medium)).foregroundStyle(Color(hex: "#1084FE")!) }
                    Text("Add Members").font(theme.font(Theme.body)).foregroundStyle(Color(hex: "#1084FE")!)
                    Spacer()
                }
                .padding(.horizontal, 14).padding(.vertical, 9).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            ForEach(rows) { r in
                ProfileRowDivider(inset: 68)
                // borderless (not .plain): .plain steals long-press on recent iOS,
                // so the remove context menu never appears (same as HomeView rows).
                HStack(spacing: 0) {
                    Button {
                        if let a = r.agent { model.showProfile(.agent(projectId: a.id)) } else { model.showProfile(.user(r.person)) }
                    } label: {
                        MemberStatusRow(person: r.person,
                                        status: r.agent != nil ? (model.isStreaming(project: r.agent!.id) ? "working" : "idle", false)
                                            : (r.person.id == model.user?.id ? ("online", true)
                                               : lastSeenLine(max(r.person.lastSeenAt ?? 0, model.lastSeen[r.person.id] ?? 0), online: r.person.online)),
                                        badge: r.role == "owner" ? "owner" : (r.role == "admin" ? "admin" : nil),
                                        isMe: r.person.id == model.user?.id, agent: r.agent)
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
                    // Jacob (2026-10-20): a small ✕ at the trailing edge removes a
                    // member in one tap plus the confirmation; owners only.
                    if canRemove(r) {
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
                        .padding(.trailing, 8)
                    }
                }
            }
        }
    }

    private func toggleMute() {
        muted.toggle()
        model.setMuted(chatId: chatId, muted: muted)
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
