// App-wide search on AppModel ([feedback iphone-bottom-search-bar-2026-10-25]):
// what the bottom bar finds and what a tapped row opens. The Mac twin is
// `AppState::palette_entries` / `open_palette_target` in crates/arbos-mac.

import Foundation
import UIKit

/// A message to scroll to once its chat is on screen.
struct MessageReveal: Equatable {
    var chatId: Id
    var messageId: Id
}

extension AppModel {
    /// Bring the index up to date with every loaded chat (agents' chats, DMs,
    /// groups, channels), re-reading only the chats whose version moved.
    func syncSearchIndex() {
        var live: Set<Id> = []
        func sync(_ chatId: Id, projectId: Id) {
            live.insert(chatId)
            searchIndex.sync(chatId: chatId, projectId: projectId, version: messagesVersion[chatId] ?? 0,
                             messages: { indexableMessages(of: chatId) },
                             who: { [weak self] m in MediaAlbum.senderName(m, projectName: { self?.project($0)?.name }) })
        }
        for p in projects {
            for c in chats(of: p.id) where !c.isSystem { sync(c.id, projectId: p.id) }
        }
        for c in peopleChats { sync(c.id, projectId: peopleSlot) }
        for s in servers {
            for ch in s.channels where channelRows[ch.id] != nil { sync(ch.id, projectId: channelSlot) }
        }
        searchIndex.retain(chatIds: live)
    }

    /// The rows for a typed query (already normalised by `HomeSearch.normalizeQuery`),
    /// ungrouped; `GlobalSearch.arrange` orders them for the list.
    func globalSearchEntries(query q: String) -> [GlobalSearch.Entry] {
        var out: [GlobalSearch.Entry] = []
        if !q.isEmpty {
            out.append(contentsOf: GlobalSearch.settingsEntries(query: q, agents: searchAgentRows()))
        }
        let me = user?.id
        var seenHandles: Set<String> = []

        // Agents (my own and shared-in): by name, chat title, owner; else a message.
        for p in projects {
            let chatList = chats(of: p.id).filter { !$0.isSystem }
            var members: [HomeSearch.Member] = []
            if p.shared, let owner = p.ownerHandle, owner != user?.login {
                members.append(HomeSearch.Member(handle: owner, name: p.ownerName))
            }
            guard let hit = HomeSearch.titledHit(title: p.name, description: p.description, members: members,
                                                 chats: chatList.map { HomeSearch.ChatTitle(id: $0.id, title: $0.title) },
                                                 messages: { [] }, q: q) else { continue }
            let when = chatList.map(\.lastMessageAt).max() ?? 0
            let subtitle = hit.reason ?? (p.description.trimmingCharacters(in: .whitespaces).isEmpty ? "Agent" : p.description)
            let indirect = hit.kind != .title || hit.reason != nil
            let target: GlobalSearch.Target
            if let chatId = hit.chatId { target = .chat(chatId: chatId, projectId: p.id) } else { target = .project(p.id) }
            out.append(GlobalSearch.Entry(kind: .agent, title: p.name, subtitle: subtitle, when: when,
                                          rank: q.isEmpty ? .everything : (indirect ? .indirect : .of(name: p.name, handle: "", title: p.description, q: q)),
                                          target: target, picture: .project(p.id)))
        }

        // DMs and groups: the person's name / handle, the group's title or a member.
        for c in peopleChats {
            let when = max(c.lastActivityAt, messages(of: c.id).last?.createdAt ?? 0)
            if !c.isGroup, let other = c.counterpart(me: me) {
                seenHandles.insert(other.handle.lowercased())
                guard let hit = HomeSearch.dmHit(name: other.displayName, handle: other.handle, messages: { [] }, q: q) else { continue }
                out.append(GlobalSearch.Entry(kind: .person, title: other.displayName, subtitle: hit.reason ?? "@\(other.handle)", when: when,
                                              rank: q.isEmpty ? .everything : .of(name: other.displayName, handle: other.handle, q: q),
                                              target: .people(c.id), picture: .peopleChat(c.id)))
                continue
            }
            let members = c.members.filter { $0.id != me }.map {
                HomeSearch.Member(handle: $0.handle, name: $0.isAgent ? $0.displayName : $0.name, isAgent: $0.isAgent)
            }
            guard let hit = HomeSearch.titledHit(title: c.title, members: members, messages: { [] }, q: q) else { continue }
            let title = c.title.isEmpty ? "Group" : c.title
            out.append(GlobalSearch.Entry(kind: .group, title: title,
                                          subtitle: hit.reason ?? GlobalSearch.groupSubtitle(members: c.humans.count), when: when,
                                          rank: q.isEmpty ? .everything : (hit.kind == .title && hit.reason == nil ? .of(title, q: q) : .indirect),
                                          target: .people(c.id), picture: .peopleChat(c.id)))
        }

        // People without a DM yet: group members, shared agents' owners, the directory.
        if !q.isEmpty {
            var candidates: [(handle: String, name: String?, url: String?, id: String?, color: Int?)] = []
            for c in peopleChats where c.isGroup {
                for m in c.humans where m.id != me {
                    candidates.append((m.handle, m.name, m.avatarUrl, m.id, m.avatarColorIndex))
                }
            }
            for p in projects where p.shared {
                if let h = p.ownerHandle, h != user?.login { candidates.append((h, p.ownerName, nil, p.ownerId, nil)) }
            }
            for p in directory where p.id != me {
                candidates.append((p.handle, p.name, p.avatarUrl, p.id, p.avatarColorIndex))
            }
            for cand in candidates {
                let key = cand.handle.lowercased()
                guard !key.isEmpty, !seenHandles.contains(key) else { continue }
                let display = cand.name?.isEmpty == false ? cand.name! : cand.handle
                let rank = GlobalSearch.Rank.of(name: display, handle: cand.handle, q: q)
                guard rank != .indirect else { continue }
                seenHandles.insert(key)
                out.append(GlobalSearch.Entry(kind: .person, title: display, subtitle: "@\(cand.handle)", when: 0, rank: rank,
                                              target: .person(handle: cand.handle),
                                              picture: .person(url: cand.url, handle: cand.handle, name: cand.name, id: cand.id, colorIndex: cand.color)))
            }
        }

        // Servers, and each matching channel as its own row.
        for s in servers {
            let when = serverActivity(s)
            if q.isEmpty || EntitySearch.matchText(s.name, query: q) != nil {
                out.append(GlobalSearch.Entry(kind: .server, title: s.name,
                                              subtitle: GlobalSearch.serverSubtitle(channels: s.liveChannels.count, members: s.memberCount),
                                              when: when, rank: q.isEmpty ? .everything : .of(s.name, q: q), target: .server(s.id), picture: .server(s.id)))
            }
            guard !q.isEmpty else { continue }
            for ch in s.liveChannels where EntitySearch.matchText(ch.name, query: q) != nil {
                out.append(GlobalSearch.Entry(kind: .channel, title: "#\(ch.name)", subtitle: s.name, when: when,
                                              rank: .of(ch.name, q: q), target: .channel(serverId: s.id, channelId: ch.id), picture: .server(s.id)))
            }
        }

        guard !q.isEmpty else { return out }

        // Message text anywhere on this phone; files, pictures and links.
        syncSearchIndex()
        for hit in searchIndex.textHits(q: q) {
            let t = hit.text
            out.append(GlobalSearch.Entry(kind: .message, title: searchPlaceName(chatId: t.chatId, projectId: t.projectId),
                                          subtitle: "\(t.who) · \(hit.snippet)", when: t.createdAt, rank: .indirect,
                                          target: .message(chatId: t.chatId, projectId: t.projectId, messageId: t.messageId),
                                          picture: searchPlacePicture(chatId: t.chatId, projectId: t.projectId)))
        }
        for c in searchIndex.contentHits(q: q) {
            let place = searchPlaceName(chatId: c.chatId, projectId: c.projectId)
            switch c.kind {
            case .link:
                guard let url = c.url else { continue }
                out.append(GlobalSearch.Entry(kind: .link, title: GlobalSearch.linkHost(c.name), subtitle: "Link · \(c.who) in \(place)",
                                              when: c.createdAt, rank: .of(c.name, q: q), target: .link(url)))
            case .media, .file:
                let kindLabel = Attachment(name: c.name, mime: c.mime, blobRef: nil, bytes: c.bytes).kindLabel
                let size = Attachment(name: c.name, mime: c.mime, blobRef: nil, bytes: c.bytes).sizeLabel
                let subtitle = ([kindLabel, size].filter { !$0.isEmpty } + ["\(c.who) in \(place)"]).joined(separator: " · ")
                out.append(GlobalSearch.Entry(kind: c.kind == .media ? .media : .file, title: c.name, subtitle: subtitle,
                                              when: c.createdAt, rank: .of(c.name, q: q),
                                              target: .item(chatId: c.chatId, projectId: c.projectId, itemId: c.itemId),
                                              picture: c.kind == .media ? thumbnailPicture(c) : .none))
            }
        }
        return out
    }

    /// The chat's name for a message / file row: the person for a DM, the group,
    /// the agent, or `#channel · Server`.
    func searchPlaceName(chatId: Id, projectId: Id) -> String {
        if projectId == peopleSlot, let c = peopleChat(chatId) {
            if c.isGroup { return c.title.isEmpty ? "Group" : c.title }
            return c.counterpart(me: user?.id)?.displayName ?? c.title
        }
        if projectId == channelSlot {
            for s in servers {
                if let ch = s.channels.first(where: { $0.id == chatId }) { return "#\(ch.name) · \(s.name)" }
            }
            return "Channel"
        }
        return project(projectId)?.name ?? "Chat"
    }

    private func searchPlacePicture(chatId: Id, projectId: Id) -> GlobalSearch.Picture {
        if projectId == peopleSlot { return .peopleChat(chatId) }
        if projectId == channelSlot {
            if let s = servers.first(where: { $0.channels.contains { $0.id == chatId } }) { return .server(s.id) }
            return .none
        }
        return .project(projectId)
    }

    private func thumbnailPicture(_ c: SearchIndex.Content) -> GlobalSearch.Picture {
        guard let m = messages(of: c.chatId).first(where: { $0.id == c.messageId }) else { return .none }
        for part in m.parts {
            if case .attachment(let id, let a) = part, "\(m.id)#\(id)" == c.itemId, a.isImage {
                return .thumbnail(MediaAlbum.item(id: c.itemId, message: m, attachment: a, kind: .image,
                                                  slot: blobSlot(chatId: c.chatId, projectId: c.projectId),
                                                  projectName: { [weak self] in self?.project($0)?.name }))
            }
        }
        return .none
    }

    /// The blob slot a chat's attachments live under (what the Library passes as `slot`).
    func blobSlot(chatId: Id, projectId: Id) -> String {
        if projectId == peopleSlot { return "\(peopleSlot):\(chatId)" }
        if projectId == channelSlot { return "\(channelSlot):\(chatId)" }
        return projectId
    }

    /// What a tapped row opens. Returns the Library item to present when the row
    /// is a picture / file (the caller owns the viewer); nil otherwise.
    @discardableResult
    func open(searchTarget target: GlobalSearch.Target) -> ChatItem? {
        switch target {
        case .project(let id):
            if let p = project(id) { open(project: p) }
        case .chat(let chatId, let projectId):
            if let c = chat(chatId) { open(chat: c) } else if let p = project(projectId) { open(project: p) }
        case .people(let id):
            if let c = peopleChat(id) { open(peopleChat: c) }
        case .message(let chatId, let projectId, let messageId):
            pendingReveal = MessageReveal(chatId: chatId, messageId: messageId)
            if projectId == peopleSlot, let c = peopleChat(chatId) {
                open(peopleChat: c)
            } else if projectId == channelSlot, let ch = servers.lazy.flatMap(\.channels).first(where: { $0.id == chatId }) {
                open(channel: ch)
            } else if let c = chat(chatId) {
                open(chat: c)
            } else {
                pendingReveal = nil
            }
        case .server(let id):
            if let s = servers.first(where: { $0.id == id }) { open(server: s) }
        case .channel(_, let channelId):
            if let ch = servers.lazy.flatMap(\.channels).first(where: { $0.id == channelId }) { open(channel: ch) }
        case .person(let handle):
            Task { _ = await openDM(handle: handle) }
        case .item(let chatId, let projectId, let itemId):
            let slot = blobSlot(chatId: chatId, projectId: projectId)
            return ChatMedia.items(messages: messages(of: chatId), slot: slot, model: self).first { $0.id == itemId }
        case .link(let url):
            UIApplication.shared.open(url)
        case .setting(let route):
            open(settingsRoute: route)
        }
        return nil
    }

    /// My owned agents as the Settings rows need them: the open chat's agent
    /// first, then by activity ([feedback search-settings-ios-2026-10-26]).
    func searchAgentRows() -> [GlobalSearch.AgentRow] {
        var current: Id?
        if case .chat(_, let pid)? = path.last { current = pid }
        let owned = projects.filter { !$0.shared && !isArchived($0.id) }
        let activity: (Project) -> Int64 = { p in self.chats(of: p.id).map(\.lastMessageAt).max() ?? 0 }
        let ordered = owned.sorted { a, b in
            if (a.id == current) != (b.id == current) { return a.id == current }
            return activity(a) > activity(b)
        }
        return ordered.map { p in
            GlobalSearch.AgentRow(projectId: p.id, name: p.name, model: p.shortModel, reasoning: p.effort.label,
                                  voice: p.voice.flatMap { id in voiceCatalog?.voices.first { $0.id == id }?.name ?? id.capitalized } ?? "Default")
        }
    }

    /// A Settings row tapped in search: Settings lands on the page (the
    /// `ARBOS_SETTINGS_PAGE` word `SettingsView(landing:)` understands), the
    /// feedback sheet opens, or the agent's chat opens with its pane on the
    /// setting's picker.
    func open(settingsRoute route: GlobalSearch.SettingsRoute) {
        switch route {
        case .page(let page):
            pendingSettingsLanding = page.rawValue
        case .profile:
            pendingSettingsLanding = "profile"
        case .feedback:
            FeedbackCenter.shared.open(screen: "search")
        case .agent(let projectId, let setting):
            guard let p = project(projectId) else { return }
            open(project: p)
            pendingAgentSetting = setting
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
                self?.showProfile(.agent(projectId: projectId))
            }
        }
    }
}
