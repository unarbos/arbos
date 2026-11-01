// Servers and channels (D88) on AppModel. The relay is the control plane
// (`GET /servers`: names, roles, channels, invite link, members); the shared
// Matrix client (crates/arbos-matrix through ArbosFFI) is the data plane
// (room list, channel timelines, send, read). The home list combines both:
// names from the relay, previews / unread / last activity from the snapshot.

import Foundation
import os
import ArbosFFI

private let matrixLog = Logger(subsystem: "com.unarbos.arbos.ios", category: "matrix")

/// UniFFI callbacks arrive on the Rust sync task; hop to the main actor before touching AppModel.
final class MatrixBridgeListener: ArbosFFI.MatrixListener, @unchecked Sendable {
    private weak var model: AppModel?

    init(model: AppModel) {
        self.model = model
    }

    func onRooms(snapshot: ArbosFFI.MatrixSnapshot) {
        let model = self.model
        Task { @MainActor in model?.applyMatrixSnapshot(snapshot) }
    }

    func onTimeline(roomId: String, rows: [ArbosFFI.MatrixMessageRow]) {
        let model = self.model
        Task { @MainActor in model?.applyMatrixTimeline(roomId: roomId, rows: rows) }
    }

    func onConnection(connected: Bool, error: String?) {
        let model = self.model
        Task { @MainActor in model?.applyMatrixConnection(connected: connected, error: error) }
    }
}

extension AppModel {
    // MARK: Lookups

    func server(_ id: Id) -> Server? { servers.first { $0.id == id } }

    func channel(_ id: Id) -> ServerChannel? {
        for s in servers {
            if let c = s.channels.first(where: { $0.id == id }) { return c }
        }
        return nil
    }

    /// The snapshot row for a channel: by relay channel id, else by room id.
    func matrixChannelRow(_ channel: ServerChannel) -> ArbosFFI.MatrixChannelRow? {
        matrixSnapshot.channels.first {
            $0.channelId == channel.id || (channel.roomId != nil && $0.roomId == channel.roomId)
        }
    }

    func matrixSpaceRow(_ serverId: Id) -> ArbosFFI.MatrixSpaceRow? {
        matrixSnapshot.spaces.first { $0.serverId == serverId }
    }

    /// "Mine" in a channel: the client's member id (`user:<id>`), the account's until it runs.
    var channelMe: String? {
        matrixClient?.meMember() ?? myUserId
    }

    /// The room behind a channel — the snapshot is the source of truth; the relay's
    /// `room_id` fills in until the first snapshot lands.
    func channelRoomId(_ channel: ServerChannel) -> String? {
        matrixChannelRow(channel)?.roomId ?? channel.roomId
    }

    /// Rule 9: may I write here? The room's power levels decide; the relay's role is the fallback.
    func channelCanPost(_ channelId: Id) -> Bool {
        guard let c = channel(channelId) else { return true }
        if let row = matrixChannelRow(c) { return row.canPost }
        return !c.restricted || (server(c.serverId)?.isAdmin ?? false)
    }

    func channelRestricted(_ channel: ServerChannel) -> Bool {
        matrixChannelRow(channel)?.restricted ?? channel.restricted
    }

    func channelUnread(_ channel: ServerChannel) -> Bool {
        channelUnreadCount(channel) > 0
    }

    /// Missed messages in the channel (0 while it is on screen) — the count pill's number.
    func channelUnreadCount(_ channel: ServerChannel) -> Int {
        guard visibleChatId != channel.id else { return 0 }
        return Int(matrixChannelRow(channel)?.unread ?? 0)
    }

    /// Last activity (ms) for sorting and the time label; the relay's creation time until a row lands.
    func channelActivity(_ channel: ServerChannel) -> Int64 {
        let ts = matrixChannelRow(channel)?.lastTs ?? 0
        return ts > 0 ? ts : channel.createdAt
    }

    /// "You: text" / "@handle: text" / "Agent: text", one line, same shape as the people rows.
    func channelPreview(_ channel: ServerChannel) -> String {
        guard let row = matrixChannelRow(channel), !row.lastText.isEmpty else { return "No messages yet" }
        let line = row.lastText.split(separator: "\n").first.map(String.init) ?? ""
        let who = Self.previewWho(sender: row.lastSender, senderName: row.lastSenderName, me: channelMe)
        return (who.map { $0 + ": " } ?? "") + HomeView.plain(line)
    }

    func serverUnread(_ server: Server) -> Bool {
        serverUnreadCount(server) > 0
    }

    /// Missed messages across the server: the space row's count plus its channels'.
    func serverUnreadCount(_ server: Server) -> Int {
        Int(matrixSpaceRow(server.id)?.unread ?? 0) + server.channels.reduce(0) { $0 + channelUnreadCount($1) }
    }

    func serverActivity(_ server: Server) -> Int64 {
        var ts = matrixSpaceRow(server.id)?.lastTs ?? 0
        for c in server.channels { ts = max(ts, matrixChannelRow(c)?.lastTs ?? 0) }
        return ts > 0 ? ts : server.createdAt
    }

    /// The space row carries "general · text"; fall back to the newest channel row.
    func serverPreview(_ server: Server) -> String {
        if let space = matrixSpaceRow(server.id), !space.lastText.isEmpty {
            return HomeView.plain(space.lastText.split(separator: "\n").first.map(String.init) ?? "")
        }
        let rows = server.channels.compactMap { matrixChannelRow($0) }.filter { !$0.lastText.isEmpty }
        guard let newest = rows.max(by: { $0.lastTs < $1.lastTs }) else { return "No messages yet" }
        let line = newest.lastText.split(separator: "\n").first.map(String.init) ?? ""
        return "\(newest.name) · " + HomeView.plain(line)
    }

    /// The label before a channel preview's text, from the newest row's member string and
    /// Matrix display name — a person by `@handle` (the Matrix name IS the handle, never shown
    /// as a name), me as "You", an agent by name, a system line bare. The same rule as
    /// `HomeView.peoplePreview` for groups ([feedback server-member-names-2026-10-28]).
    static func previewWho(sender: String, senderName: String, me: String?) -> String? {
        let name = senderName.trimmingCharacters(in: .whitespaces)
        if let me, !me.isEmpty, sender.caseInsensitiveCompare(me) == .orderedSame { return "You" }
        guard !name.isEmpty, sender != "system" else { return nil }
        return sender.hasPrefix("user:") ? "@\(name)" : name
    }

    // MARK: Channel senders (display names from the rosters)

    /// Who a channel row is from, as the rosters know them: the servers' member lists (by user
    /// id, then handle), then every people chat's members and the directory. A Matrix row names
    /// its sender by handle only (`arbos_matrix::message_row`), so this is where a channel bubble
    /// gets the display name a group bubble shows ([feedback server-member-names-2026-10-28];
    /// Affinity, Arbonites 2026-10-28: "referred to as our handles instead of our usernames in server").
    func channelSender(sender: String, handle: String?) -> ServerMember? {
        guard sender.hasPrefix("user:") else { return nil }
        let id = String(sender.dropFirst("user:".count))
        let handle = handle.flatMap { $0.isEmpty ? nil : $0 }
        func same(_ a: String, _ b: String) -> Bool { a.caseInsensitiveCompare(b) == .orderedSame }
        func byHandle(_ h: String) -> Bool { handle.map { same(h, $0) } ?? false }
        if let m = servers.lazy.flatMap(\.members).first(where: { m in
            (!m.id.isEmpty && same(m.id, id)) || byHandle(m.handle)
        }) {
            return m
        }
        if let p = peopleChats.lazy.flatMap(\.members).first(where: { m in
            m.kind == .user && (same(m.member, sender) || byHandle(m.handle))
        }) {
            return ServerMember(id: id, handle: p.handle, name: p.name, avatarUrl: p.avatarUrl, role: "member", joinedAt: 0)
        }
        if let p = directory.first(where: { p in same(p.id, id) || byHandle(p.handle) }) {
            return ServerMember(id: id, handle: p.handle, name: p.name, avatarUrl: p.avatarUrl, role: "member", joinedAt: 0)
        }
        return nil
    }

    /// Fill a person's channel row from the roster: the display name (when the roster has one)
    /// and the picture (when the row carries none). Returns whether anything changed.
    @discardableResult
    static func fillChannelSender(_ m: inout Message, from roster: ServerMember?) -> Bool {
        guard let roster else { return false }
        var changed = false
        if (m.senderName ?? "").trimmingCharacters(in: .whitespaces).isEmpty,
           let name = roster.name?.trimmingCharacters(in: .whitespaces), !name.isEmpty {
            m.senderName = name
            changed = true
        }
        if (m.senderAvatar ?? "").isEmpty, let url = roster.avatarUrl, !url.isEmpty {
            m.senderAvatar = url
            changed = true
        }
        return changed
    }

    private func fillChannelSender(_ m: inout Message) -> Bool {
        guard m.sender.hasPrefix("user:"), m.sender.caseInsensitiveCompare(channelMe ?? "") != .orderedSame else { return false }
        return Self.fillChannelSender(&m, from: channelSender(sender: m.sender, handle: m.senderHandle))
    }

    /// A roster arrived (server members, the list): name the channel rows it now knows. Decoded
    /// rows are kept, so a channel opened before `GET /servers/{id}` answered would otherwise
    /// keep reading handles.
    func refreshChannelSenders() {
        for (channelId, rows) in channelRows {
            var rows = rows
            var changed = false
            for i in rows.indices {
                if fillChannelSender(&rows[i]) { changed = true }
            }
            if changed { channelRows[channelId] = rows }
        }
    }

    /// Members come only with `GET /servers/{id}`: ask once per server per session for any
    /// server the list brought without them, so channel bubbles and the previews can name
    /// people from the first paint.
    func wantServerRosters() {
        for s in servers where s.members.isEmpty && !serverRostersRequested.contains(s.id) {
            serverRostersRequested.insert(s.id)
            Task { await refreshServer(s.id) }
        }
    }

    // MARK: Navigation

    /// A server opens on its channel list; the user picks a channel (no skip to `#general`).
    func open(server: Server) {
        path = [.server(serverId: server.id)]
    }

    func open(channel: ServerChannel) {
        path = [.server(serverId: channel.serverId), .chat(chatId: channel.id, projectId: channelSlot)]
    }

    /// Rig: `ARBOS_OPEN_SERVER=<name or id>[:list|:home|:#channel]` — the server's channel
    /// list (default and `:list`), the chat list with its row (`:home`), or one channel
    /// (`:#name`), once the registry lists it and a first login has ended (that one lands
    /// on its own chat). Debug builds only, like every `DebugHooks` value.
    func applyOpenServerHook() {
        guard !openServerHookDone, firstLogin == nil, let spec = DebugHooks.openServer else { return }
        let parts = spec.split(separator: ":", maxSplits: 1).map(String.init)
        guard let key = parts.first,
              let server = servers.first(where: { $0.id == key || $0.name.lowercased() == key.lowercased() })
        else { return }
        openServerHookDone = true
        switch parts.dropFirst().first {
        case "list"?:
            path = [.server(serverId: server.id)]
        case "home"?:
            path = []
        case "panel"?:
            // The server page (the channel list's header pill), on `ARBOS_PANEL_TAB`.
            path = [.server(serverId: server.id)]
            showProfile(.server(serverId: server.id))
        case "page"?, "page-settings"?:
            // The channel page the header pill opens (the Settings tab with `page-settings`).
            open(server: server)
            if let g = server.general {
                showProfile(.channel(channelId: g.id))
                openServerHookTab = parts.dropFirst().first == "page-settings" ? "Settings" : nil
            }
        case let name?:
            let wanted = name.hasPrefix("#") ? String(name.dropFirst()) : name
            if let c = server.channels.first(where: { $0.name == wanted }) { open(channel: c) } else { open(server: server) }
        case nil:
            open(server: server)
        }
    }

    // MARK: Relay (control plane)

    /// `GET /servers`; refetched on the 15 s loop and on every `server_*` push.
    func refreshServers() async {
        guard credentials != nil, !isFixture else { return }
        do {
            var list = try await relay.servers()
            // The list has no members; keep the ones a profile page already fetched.
            for i in list.indices where list[i].members.isEmpty {
                if let prev = server(list[i].id) { list[i].members = prev.members }
            }
            // The 15 s poll: a write that changes nothing still publishes the whole model.
            // `Server` is Equatable over every stored property, so a list that compares
            // equal carries no change and this guard cannot swallow a real one.
            // `rememberServersAvailable` stays unconditional — it is a store write, not a
            // publish, and it is the only thing that persists the flag.
            if list != servers { servers = list }
            reconcileArchivedServers(list)
            if !serversAvailable { serversAvailable = true }
            rememberServersAvailable(true)
            wantServerRosters()
            refreshChannelSenders()
            applyOpenServerHook()
        } catch let e as RelayError where e.status == 404 || e.status == 405 || e.status == 503 {
            // Relays without servers: nothing server-related.
            if !servers.isEmpty { servers = [] }
            if serversAvailable { serversAvailable = false }
            rememberServersAvailable(false)
        } catch {
            // Keep the last list; the loop tries again.
        }
    }

    /// `GET /servers/{id}` — members for the profile page.
    @discardableResult
    func refreshServer(_ id: Id) async -> Server? {
        guard credentials != nil, !isFixture else { return server(id) }
        guard let s = try? await relay.server(id) else { return server(id) }
        upsert(server: s)
        return s
    }

    private func upsert(server s: Server) {
        if let i = servers.firstIndex(where: { $0.id == s.id }) {
            var merged = s
            if merged.members.isEmpty { merged.members = servers[i].members }
            servers[i] = merged
        } else {
            servers.append(s)
        }
        refreshChannelSenders()
    }

    /// Rule 8: "Server" in the `+` menu → `POST /servers {name}`, then its channel list.
    func createServer(name: String, leaf: LeafPick? = nil, picture: DraftPicture? = nil) async -> Bool {
        do {
            var s = try await relay.createServer(name: name, leaf: leaf)
            if let picture, let jpeg = await draftJpeg(picture) {
                let key = "server-\(Int(Date().timeIntervalSince1970 * 1000)).\(PictureMaster.ext(jpeg))"
                if (try? await relay.putBlob(projectId: "servers:\(s.id)", key: key, mime: PictureMaster.mime(jpeg), data: jpeg)) != nil,
                   (try? await relay.patchServer(s.id, imageKey: key)) != nil,
                   let fresh = try? await relay.server(s.id) {
                    s = fresh
                }
            }
            upsert(server: s)
            open(server: s)
            await startMatrixIfNeeded()
            return true
        } catch {
            fail("create the server", error)
            return false
        }
    }

    /// Rule 8: "Join a server" → `POST /servers/join {code}` (the code or the whole link).
    func joinServer(code: String) async -> Bool {
        do {
            let s = try await relay.joinServer(code: code)
            upsert(server: s)
            open(server: s)
            await startMatrixIfNeeded()
            return true
        } catch {
            let type = (error as? RelayError)?.type
            let said: String? = type == "banned" ? "You can't join this server."
                : type == "not_found" || (error as? RelayError)?.status == 404 ? "That invite link is not valid."
                : nil
            fail(said.map { UserError.refused("join the server", $0) } ?? UserError.classify("join the server", error))
            return false
        }
    }

    /// Join an open server by id (no invite link).
    func joinOpenServer(_ id: Id) async -> Bool {
        do {
            let s = try await relay.joinOpenServer(id: id)
            upsert(server: s)
            open(server: s)
            await startMatrixIfNeeded()
            return true
        } catch {
            let type = (error as? RelayError)?.type
            let said: String? = type == "banned" ? "You can't join this server."
                : type == "invite_required" ? "This server needs an invite link."
                : type == "not_found" || (error as? RelayError)?.status == 404 ? "That server is not available."
                : nil
            fail(said.map { UserError.refused("join the server", $0) } ?? UserError.classify("join the server", error))
            return false
        }
    }

    /// `GET /servers/search?q=` — open servers matching the name (ones you are not in).
    func searchOpenServers(_ q: String) async -> [Server] {
        guard credentials != nil, !isFixture, serversAvailable else { return [] }
        return (try? await relay.searchOpenServers(q)) ?? []
    }

    /// D92 amendment: a server's leaf picture (nil = initials); drops the uploaded picture.
    func setServerLeaf(_ id: Id, pick: LeafPick?) async -> Bool {
        if let i = servers.firstIndex(where: { $0.id == id }) {
            servers[i].avatarLeaf = pick.map { Int($0.leaf) }
            servers[i].avatarLeafColor = pick.map { Int($0.color) }
            servers[i].imageKey = nil
        }
        guard !isFixture else { return true }
        do {
            try await relay.setServerLeaf(id, pick: pick)
            await refreshServers()
            return true
        } catch {
            fail("save the server picture", error)
            await refreshServers()
            return false
        }
    }

    /// D92 amendment: a server picture — upload the master (PNG / JPEG) into `servers:{id}` and set it.
    func setServerImage(_ id: Id, jpeg: Data) async -> Bool {
        let key = "server-\(Int(Date().timeIntervalSince1970 * 1000)).\(PictureMaster.ext(jpeg))"
        if isFixture {
            if let i = servers.firstIndex(where: { $0.id == id }) { servers[i].imageKey = key }
            return true
        }
        do {
            _ = try await relay.putBlob(projectId: "servers:\(id)", key: key, mime: PictureMaster.mime(jpeg), data: jpeg)
            return await setServerImageKey(id, key: key)
        } catch {
            fail("upload the picture", error)
            return false
        }
    }

    /// A server picture from a key the relay already holds (the Generate tab's answer).
    func setServerImageKey(_ id: Id, key: String) async -> Bool {
        if let i = servers.firstIndex(where: { $0.id == id }) { servers[i].imageKey = key }
        guard !isFixture else { return true }
        do {
            try await relay.patchServer(id, imageKey: key)
            await refreshServers()
            return true
        } catch {
            fail("save the server picture", error)
            await refreshServers()
            return false
        }
    }

    /// The Generate tab for a server (owner / admins): the relay paints one picture.
    func generateServerAvatar(_ id: Id, prompt: String) async throws -> String {
        if isFixture {
            try? await Task.sleep(nanoseconds: 1_200_000_000)
            return "fixture-gen-\(prompt.lowercased().replacingOccurrences(of: " ", with: "-"))"
        }
        return try await relay.generateServerAvatar(id, prompt: prompt)
    }

    func renameServer(_ id: Id, name: String) async -> Bool {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return false }
        if let i = servers.firstIndex(where: { $0.id == id }) { servers[i].name = trimmed }
        do {
            try await relay.patchServer(id, name: trimmed)
            await refreshServers()
            return true
        } catch {
            fail("rename the server", error)
            await refreshServers()
            return false
        }
    }

    /// Owner/admin: open = anyone can search and join without an invite link.
    func setServerOpen(_ id: Id, open: Bool) async -> Bool {
        if let i = servers.firstIndex(where: { $0.id == id }) { servers[i].isOpen = open }
        do {
            try await relay.patchServer(id, open: open)
            await refreshServer(id)
            return true
        } catch {
            fail("update the server", error)
            await refreshServers()
            return false
        }
    }

    /// Owner only: the server goes for everyone.
    func deleteServer(_ id: Id) async -> Bool {
        do {
            try await relay.deleteServer(id)
            forgetServer(id)
            return true
        } catch {
            fail("delete the server", error)
            return false
        }
    }

    func leaveServer(_ id: Id) async -> Bool {
        do {
            try await relay.leaveServer(id)
            forgetServer(id)
            return true
        } catch {
            fail("leave the server", error)
            return false
        }
    }

    private func forgetServer(_ id: Id) {
        if let s = server(id) {
            for c in s.channels {
                channelRows[c.id] = nil
                caughtUp.remove(c.id)
                channelAtStart.remove(c.id)
            }
        }
        servers.removeAll { $0.id == id }
        if path.contains(where: { if case .server(let sid) = $0 { return sid == id } else { return false } }) {
            path = []
        }
    }

    /// Rule 7: a new channel (owner/admin): name (letters, digits, dashes), admins-only toggle.
    func createChannel(serverId: Id, name: String, restricted: Bool) async -> ServerChannel? {
        do {
            let c = try await relay.createChannel(serverId: serverId, name: Self.channelSlug(name), restricted: restricted)
            if let i = servers.firstIndex(where: { $0.id == serverId }),
               !servers[i].channels.contains(where: { $0.id == c.id }) {
                servers[i].channels.append(c)
            }
            await refreshServers()
            return channel(c.id) ?? c
        } catch {
            fail("create the channel", error)
            return nil
        }
    }

    func renameChannel(_ channel: ServerChannel, name: String) async -> Bool {
        let slug = Self.channelSlug(name)
        guard !slug.isEmpty else { return false }
        do {
            try await relay.patchChannel(serverId: channel.serverId, channelId: channel.id, name: slug)
            await refreshServers()
            return true
        } catch {
            fail("rename the channel", error)
            return false
        }
    }

    /// Channel page › Settings: the topic (owner/admin; ≤ 200 chars, empty clears it).
    func setChannelTopic(_ channel: ServerChannel, topic: String) async {
        let topic = String(topic.trimmingCharacters(in: .whitespacesAndNewlines).prefix(200))
        if let si = servers.firstIndex(where: { $0.id == channel.serverId }),
           let ci = servers[si].channels.firstIndex(where: { $0.id == channel.id }) {
            servers[si].channels[ci].topic = topic.isEmpty ? nil : topic
        }
        do {
            try await relay.patchChannel(serverId: channel.serverId, channelId: channel.id, topic: topic)
        } catch {
            fail("change the topic", error)
        }
        await refreshServers()
    }

    func isChannelMuted(_ channel: ServerChannel) -> Bool { mutedChannelIds.contains(channel.id) }

    func toggleChannelMute(_ channel: ServerChannel) {
        if mutedChannelIds.contains(channel.id) { mutedChannelIds.remove(channel.id) } else { mutedChannelIds.insert(channel.id) }
    }

    func setChannelRestricted(_ channel: ServerChannel, _ restricted: Bool) async {
        if let si = servers.firstIndex(where: { $0.id == channel.serverId }),
           let ci = servers[si].channels.firstIndex(where: { $0.id == channel.id }) {
            servers[si].channels[ci].restricted = restricted
        }
        do {
            try await relay.patchChannel(serverId: channel.serverId, channelId: channel.id, restricted: restricted)
        } catch {
            fail("change who posts", error)
        }
        await refreshServers()
    }

    /// Row menu → Archive / Unarchive: hide a channel from my list or bring it back. An archived
    /// channel that is open gives way to the channel list.
    func archiveChannel(_ channel: ServerChannel, _ archived: Bool) async {
        if let si = servers.firstIndex(where: { $0.id == channel.serverId }),
           let ci = servers[si].channels.firstIndex(where: { $0.id == channel.id }) {
            servers[si].channels[ci].archived = archived
        }
        if archived, case .chat(let id, _)? = path.last, id == channel.id {
            path = [.server(serverId: channel.serverId)]
        }
        do {
            try await relay.archiveChannel(serverId: channel.serverId, channelId: channel.id, archived: archived)
        } catch {
            fail(archived ? "archive the channel" : "unarchive the channel", error)
        }
        await refreshServers()
    }

    func deleteChannel(_ channel: ServerChannel) async -> Bool {
        do {
            try await relay.deleteChannel(serverId: channel.serverId, channelId: channel.id)
            channelRows[channel.id] = nil
            caughtUp.remove(channel.id)
            if case .chat(let id, _)? = path.last, id == channel.id {
                path = [.server(serverId: channel.serverId)]
            }
            await refreshServers()
            return true
        } catch {
            fail((error as? RelayError)?.status == 409
                 ? UserError.refused("delete the channel", "#general stays.")
                 : UserError.classify("delete the channel", error))
            return false
        }
    }

    /// Rule 6: owner resets the invite link (the old one stops working).
    /// Members tab → Add members (the main way in since 2026-10-22): one call per new person.
    func addServerMembers(serverId: Id, userIds: [String]) async -> Bool {
        var ok = true
        for uid in userIds {
            do { try await relay.addServerMember(serverId: serverId, userId: uid) } catch {
                ok = false
                fail("add them", error)
            }
        }
        await refreshServer(serverId)
        return ok
    }

    /// Settings › Invite links › New link (owner).
    func createServerInvite(_ id: Id, expires: Bool, singleUse: Bool) async {
        do {
            _ = try await relay.createServerInvite(id, expires: expires, maxUses: singleUse ? 1 : nil)
            await refreshServer(id)
        } catch {
            fail("make an invite link", error)
        }
    }

    /// Settings › Invite links › revoke (owner): gone from the list at once.
    func revokeServerInvite(_ id: Id, inviteId: String) async {
        if let si = servers.firstIndex(where: { $0.id == id }) {
            servers[si].invites.removeAll { $0.id == inviteId }
        }
        do { try await relay.revokeServerInvite(id, inviteId: inviteId) } catch {
            fail("revoke the link", error)
        }
        await refreshServer(id)
    }

    /// `arbos://join/<code>` (the site's invite page): join right away and land in the server.
    func joinFromLink(code: String) async {
        let code = code.trimmingCharacters(in: CharacterSet(charactersIn: "/ "))
        guard !code.isEmpty else { return }
        _ = await joinServer(code: code)
    }

    func resetServerInvite(_ id: Id) async -> ServerInvite? {
        do {
            let inv = try await relay.resetServerInvite(id)
            if let i = servers.firstIndex(where: { $0.id == id }) { servers[i].invite = inv }
            return inv
        } catch {
            fail("reset the invite link", error)
            return nil
        }
    }

    func setServerRole(serverId: Id, userId: String, role: String) async {
        do {
            try await relay.setServerMemberRole(serverId: serverId, userId: userId, role: role)
        } catch {
            fail("change the role", error)
        }
        await refreshServer(serverId)
    }

    func removeServerMember(serverId: Id, userId: String, ban: Bool) async {
        do {
            try await relay.removeServerMember(serverId: serverId, userId: userId, ban: ban)
        } catch {
            fail("remove the member", error)
        }
        await refreshServer(serverId)
    }

    /// Channel names: lower-case letters, digits and dashes (rule 7).
    static func channelSlug(_ raw: String) -> String {
        let lowered = raw.lowercased().replacingOccurrences(of: " ", with: "-")
        let kept = lowered.unicodeScalars.filter { CharacterSet.alphanumerics.contains($0) || $0 == "-" }
        var out = String(String.UnicodeScalarView(kept))
        while out.contains("--") { out = out.replacingOccurrences(of: "--", with: "-") }
        return out.trimmingCharacters(in: CharacterSet(charactersIn: "-"))
    }

    // MARK: Matrix (data plane)

    /// Start the shared client once `GET /me/matrix` answers; a relay without servers (503
    /// `servers_off`) leaves `matrixClient` nil and nothing server-related shows.
    func startMatrixIfNeeded() async {
        guard credentials != nil, !isFixture, matrixClient == nil, !matrixStarting else { return }
        matrixStarting = true
        defer { matrixStarting = false }
        do {
            guard let creds = try await relay.matrixSession() else { return }
            let listener = MatrixBridgeListener(model: self)
            let session = ArbosFFI.MatrixSession(homeserver: creds.homeserver, userId: creds.userId,
                                                 accessToken: creds.accessToken, deviceId: creds.deviceId)
            let client = try await matrixStart(session: session, listener: listener)
            // Signed out (or started twice) while the handshake ran.
            guard credentials != nil, matrixClient == nil else {
                client.stop()
                return
            }
            matrixListener = listener
            matrixClient = client
            matrixConnected = true
            applyMatrixSnapshot(await client.snapshot())
        } catch {
            matrixLog.notice("matrix start failed: \(error.localizedDescription, privacy: .public)")
        }
    }

    func stopMatrix() {
        matrixClient?.stop()
        matrixClient = nil
        matrixListener = nil
        matrixSnapshot = ArbosFFI.MatrixSnapshot(spaces: [], channels: [])
        matrixConnected = true
        matrixDisconnectedSince = nil
        for id in channelRows.keys { caughtUp.remove(id) }
        channelRows = [:]
        channelAtStart = []
        channelLoading = []
    }

    func applyMatrixSnapshot(_ snapshot: ArbosFFI.MatrixSnapshot) {
        matrixSnapshot = snapshot
        // A channel opened before its room was known (just created, or the client was still
        // starting): open it now that the snapshot lists the room.
        if let id = visibleChatId, channel(id) != nil, !caughtUp.contains(id) {
            openChannel(id)
        }
    }

    func applyMatrixConnection(connected: Bool, error: String?) {
        if connected {
            matrixDisconnectedSince = nil
        } else if matrixConnected || matrixDisconnectedSince == nil {
            matrixDisconnectedSince = Date()
            scheduleLinkPillRepaint()
        }
        matrixConnected = connected
        if let error, !connected {
            matrixLog.notice("matrix link down: \(error, privacy: .public)")
        }
    }

    /// The full row list of an open room, after every new message.
    func applyMatrixTimeline(roomId: String, rows: [ArbosFFI.MatrixMessageRow]) {
        guard let channelId = channelId(forRoom: roomId) else { return }
        setChannelRows(channelId, rows)
        if visibleChatId == channelId { markChannelRead(channelId) }
    }

    private func channelId(forRoom roomId: String) -> Id? {
        if let row = matrixSnapshot.channels.first(where: { $0.roomId == roomId }), !row.channelId.isEmpty {
            return row.channelId
        }
        for s in servers {
            if let c = s.channels.first(where: { $0.roomId == roomId }) { return c.id }
        }
        return nil
    }

    /// Decode the chat-doc rows and keep optimistic sends whose echo has not landed yet
    /// (the echo carries the same id, so it simply replaces the pending row).
    private func setChannelRows(_ channelId: Id, _ rows: [ArbosFFI.MatrixMessageRow]) {
        var decoded = rows.compactMap { Self.decodeMatrixRow($0, chatId: channelId) }
        for i in decoded.indices { _ = fillChannelSender(&decoded[i]) }
        let ids = Set(decoded.map(\.id))
        let cutoff = nowMs() - 60_000
        for m in channelRows[channelId] ?? [] where m.pending && !ids.contains(m.id) && m.createdAt > cutoff {
            decoded.append(m)
        }
        decoded.sort { $0.createdAt == $1.createdAt ? $0.id < $1.id : $0.createdAt < $1.createdAt }
        channelRows[channelId] = decoded
        caughtUp.insert(channelId)
    }

    /// `MatrixMessageRow.json` is a chat-doc message row: the app's decoder reads it as is. A
    /// person's row carries `senderHandle` and no `senderName` (the Matrix display name is the
    /// handle); an agent's row keeps its name. The roster fills a person's name afterwards
    /// (`fillChannelSender`).
    static func decodeMatrixRow(_ row: ArbosFFI.MatrixMessageRow, chatId: Id) -> Message? {
        guard let data = row.json.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data),
              var m = DocJSON.message(json, chatId: chatId) else { return nil }
        if m.createdAt == 0 { m.createdAt = row.createdAt }
        if !m.sender.hasPrefix("user:"), (m.senderName ?? "").isEmpty, !row.senderName.isEmpty {
            m.senderName = row.senderName
        }
        return m
    }

    // MARK: Channel timelines

    /// `openRoom`: the newest page, then live rows through the listener. Safe to call again.
    func openChannel(_ channelId: Id) {
        guard let c = channel(channelId), let client = matrixClient, let roomId = channelRoomId(c),
              !channelLoading.contains(channelId) else { return }
        channelLoading.insert(channelId)
        Task {
            defer { channelLoading.remove(channelId) }
            do {
                let rows = try await client.openRoom(roomId: roomId)
                setChannelRows(channelId, rows)
                try? await client.markRead(roomId: roomId)
            } catch {
                matrixLog.notice("open room failed: \(error.localizedDescription, privacy: .public)")
                // Paint the empty transcript rather than a spinner that never ends.
                caughtUp.insert(channelId)
            }
        }
    }

    func markChannelRead(_ channelId: Id) {
        guard let c = channel(channelId), let client = matrixClient, let roomId = channelRoomId(c) else { return }
        Task { try? await client.markRead(roomId: roomId) }
    }

    /// One older page when the transcript is scrolled near its top; stops at the room's start.
    func loadOlderChannel(_ channelId: Id) {
        guard let c = channel(channelId), let client = matrixClient, let roomId = channelRoomId(c),
              caughtUp.contains(channelId), !channelAtStart.contains(channelId),
              !channelLoading.contains(channelId) else { return }
        channelLoading.insert(channelId)
        Task {
            defer { channelLoading.remove(channelId) }
            do {
                if let rows = try await client.loadOlder(roomId: roomId) {
                    setChannelRows(channelId, rows)
                } else {
                    channelAtStart.insert(channelId)
                }
            } catch {
                matrixLog.notice("load older failed: \(error.localizedDescription, privacy: .public)")
            }
        }
    }

    /// Send into a channel: the row shows at once with a client ULID as its id; the echo
    /// arrives in the timeline with the same id (`life.arbos.id`).
    func sendChannel(chatId channelId: Id, text: String) {
        guard let c = channel(channelId), let me = user else { return }
        guard let client = matrixClient, let roomId = channelRoomId(c) else {
            setSendError(channelId, UserError.of("send your message", .offline(deviceOffline: false), "Not connected to the server yet."))
            return
        }
        let messageId = newId()
        var m = Message(id: messageId, chatId: channelId, role: .user, sender: client.meMember(), status: .done, turn: nil,
                        decision: nil, subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil,
                        createdAt: nowMs(), completedAt: nowMs(), pending: true,
                        parts: [.text(id: "\(messageId)-t", text: text)], hiddenCount: 0)
        m.senderHandle = me.login
        m.senderName = me.name
        m.senderAvatar = me.avatarUrl
        channelRows[channelId, default: []].append(m)
        Task {
            do {
                _ = try await client.sendText(roomId: roomId, text: text, localId: messageId)
            } catch {
                channelRows[channelId]?.removeAll { $0.id == messageId }
                fail("send your message", error)
            }
        }
    }
}
