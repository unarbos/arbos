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
    func createServer(name: String, discColor: UInt8? = nil, picture: DraftPicture? = nil) async -> Bool {
        do {
            var s = try await relay.createServer(name: name, discColor: discColor)
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

    /// D92 amendment: a server's disc colour (nil = initials); drops the uploaded picture.
    func setServerDisc(_ id: Id, color: UInt8?) async -> Bool {
        if let i = servers.firstIndex(where: { $0.id == id }) {
            servers[i].avatarDiscColor = color.map { Int($0) }
            servers[i].imageKey = nil
        }
        guard !isFixture else { return true }
        do {
            try await relay.setServerDisc(id, color: color)
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
        let channelIds = Set(server(id)?.channels.map(\.id) ?? [])
        for c in channelIds {
            channelRows[c] = nil
            caughtUp.remove(c)
            channelAtStart.remove(c)
        }
        servers.removeAll { $0.id == id }
        // Every screen about this server goes, and the first one takes the rest with it:
        // its channel list, one of its channels, and — since 2026-11-01 — the server's or
        // a channel's own profile page, which is a pushed route rather than a sheet.
        if let i = path.firstIndex(where: { route in
            if case .server(let sid) = route, sid == id { return true }
            if case .profile(.server(let sid)) = route, sid == id { return true }
            return shows(channelIds: channelIds, route: route)
        }) {
            path.removeSubrange(i...)
        }
    }

    /// A channel that is archived or deleted gives way to its server's channel list, from
    /// whichever of its screens the user is on — the channel itself, or the channel's own
    /// profile page, which is a pushed route since 2026-11-01 and can sit either over the
    /// channel or straight on top of the channel list. The model owns this rather than the
    /// page calling `dismiss()` afterwards: two navigations for one action pop twice.
    private func leaveScreensAbout(_ channel: ServerChannel) {
        guard let i = path.firstIndex(where: { shows(channelIds: [channel.id], route: $0) }) else { return }
        path.removeSubrange(i...)
        if !path.contains(.server(serverId: channel.serverId)) {
            path.append(.server(serverId: channel.serverId))
        }
    }

    /// Does this route show one of these channels — the channel itself, or its own page?
    private func shows(channelIds: Set<Id>, route: Route) -> Bool {
        switch route {
        case .server: return false
        case .chat(let chatId, _): return channelIds.contains(chatId)
        case .profile(let target):
            switch target {
            case .channel(let channelId): return channelIds.contains(channelId)
            case .user, .group, .agent, .server: return false
            }
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
        if archived { leaveScreensAbout(channel) }
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
            leaveScreensAbout(channel)
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
            guard let creds = try await relay.matrixSession() else {
                // No homeserver on this relay (any more): no chat is read from one.
                applyNativeRooms([:])
                return
            }
            let listener = MatrixBridgeListener(model: self)
            let session = ArbosFFI.MatrixSession(homeserver: creds.homeserver, userId: creds.userId,
                                                 accessToken: creds.accessToken, deviceId: creds.deviceId,
                                                 peopleNative: creds.peopleNative, historyKey: creds.historyKey)
            // No key, no client: without it the store holding the room keys would be written
            // unencrypted. The next call tries again (the Keychain may only have been locked).
            guard let storeKey = WalletVault.storeKey(device: creds.deviceId) else {
                matrixLog.notice("matrix start skipped: the store key is not available")
                return
            }
            let crypto = ArbosFFI.MatrixCrypto(storeDir: WalletVault.storeDir(device: creds.deviceId),
                                               storeKey: storeKey)
            let client = try await matrixStart(session: session, crypto: crypto, listener: listener)
            // Signed out (or started twice) while the handshake ran.
            guard credentials != nil, matrixClient == nil else {
                client.stop()
                return
            }
            matrixListener = listener
            matrixClient = client
            MatrixMediaProtocol.serve { [weak self] chat, reference in
                guard let (client, room) = await self?.nativeTarget(chat) else { throw URLError(.resourceUnavailable) }
                let media = try await client.media(roomId: room, reference: reference)
                return (media.mime, media.bytes)
            }
            matrixConnected = true
            peopleNative = creds.peopleNative
            // Switched off (the rollback): every people chat is its Loro room again from the
            // next `refreshPeople` on.
            if !peopleNative { applyNativeRooms([:]) }
            applyMatrixSnapshot(await client.snapshot())
            await refreshNativeRooms()
            // Waits for the first sync's word on the key backup; never holds up the start.
            Task { await ensureKeys(client) }
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
        peopleNative = false
        applyNativeRooms([:])
        for id in channelRows.keys { caughtUp.remove(id) }
        channelRows = [:]
        channelAtStart = []
        channelLoading = []
        historyAsked = []
    }

    func applyMatrixSnapshot(_ snapshot: ArbosFFI.MatrixSnapshot) {
        matrixSnapshot = snapshot
        // A channel opened before its room was known (just created, or the client was still
        // starting): open it now that the snapshot lists the room.
        if let id = visibleChatId, channel(id) != nil, !caughtUp.contains(id) {
            openChannel(id)
        }
        // A native people chat is shown once the homeserver lists its room.
        openNativeRooms()
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
        if let chat = nativeRooms.first(where: { $0.value == roomId })?.key {
            // A people chat only one of the relay and the homeserver vouches for is not shown.
            return nativeRoom(chat) == nil ? nil : chat
        }
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
        // A native people chat's pending rows are the client's own: it takes a failed one back.
        let native = nativeRooms[channelId] != nil
        var decoded = native
            ? Self.nativeMessages(rows, chatId: channelId, members: nativeMembers(channelId))
            : rows.compactMap { Self.decodeMatrixRow($0, chatId: channelId) }
        for i in decoded.indices { _ = fillChannelSender(&decoded[i]) }
        let ids = Set(decoded.map(\.id))
        let cutoff = nowMs() - 60_000
        for m in channelRows[channelId] ?? [] where !native && m.pending && !ids.contains(m.id) && m.createdAt > cutoff {
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

    /// The homeserver room a chat's rows are read from: a channel's, or a native people chat's.
    private func timelineRoomId(_ chatId: Id) -> String? {
        if let c = channel(chatId) { return channelRoomId(c) }
        return nativeRoom(chatId)
    }

    /// `openRoom`: the newest page, then live rows through the listener. Safe to call again.
    func openChannel(_ channelId: Id) {
        guard let client = matrixClient, let roomId = timelineRoomId(channelId),
              !channelLoading.contains(channelId) else { return }
        channelLoading.insert(channelId)
        Task {
            defer { channelLoading.remove(channelId) }
            do {
                let rows = try await client.openRoom(roomId: roomId)
                setChannelRows(channelId, rows)
                // A native people chat is also read off screen (`openNativeRooms`): that is
                // not the person reading it.
                if nativeRooms[channelId] == nil || visibleChatId == channelId {
                    try? await client.markRead(roomId: roomId)
                }
                if nativeRooms[channelId] != nil { await carryHistory(channelId, client: client, roomId: roomId) }
            } catch {
                matrixLog.notice("open room failed: \(error.localizedDescription, privacy: .public)")
                // Paint the empty transcript rather than a spinner that never ends. A native
                // people chat is asked for again instead (`openNativeRooms`).
                if nativeRooms[channelId] == nil { caughtUp.insert(channelId) }
            }
        }
    }

    /// Carry a native chat's earlier history from its Loro room into its homeserver room,
    /// once per launch: the relay hands out signed rows a batch at a time, the Matrix client
    /// sends them, and the relay is told which events they became. Stops at the first thing
    /// that fails (the next launch, or another member's device, takes it up there) and when
    /// another device is at it.
    private func carryHistory(_ chatId: Id, client: ArbosFFI.MatrixClient, roomId: String) async {
        guard historyAsked.insert(chatId).inserted else { return }
        while true {
            guard let page = try? await relay.matrixHistoryPage(chatId),
                  let said = (try? JSONSerialization.jsonObject(with: Data(page.utf8))) as? [String: Any],
                  let rows = said["rows"] as? [Any], !rows.isEmpty else { return }
            do {
                let report = try await client.carryHistory(roomId: roomId, pageJson: page)
                try await relay.matrixHistoryCarried(chatId, report: JSONSerialization.jsonObject(with: Data(report.utf8)))
            } catch {
                matrixLog.notice("history not carried: \(error.localizedDescription, privacy: .public)")
                return
            }
            guard said["more"] as? NSNumber === kCFBooleanTrue else { return }
        }
    }

    func markChannelRead(_ channelId: Id) {
        guard let client = matrixClient, let roomId = timelineRoomId(channelId) else { return }
        Task { try? await client.markRead(roomId: roomId) }
    }

    /// One older page when the transcript is scrolled near its top; stops at the room's start.
    func loadOlderChannel(_ channelId: Id) {
        guard let client = matrixClient, let roomId = timelineRoomId(channelId),
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

    // MARK: Native people chats (phase 3)
    //
    // The Mac's rule (crates/arbos-mac/src/sync/relay.rs), kept the same here: the relay names
    // a people chat's room only when every member is a flagged person (no agent, nobody
    // unflagged); this phone takes that answer only as written (`parsePeopleRooms`) and reads
    // and writes the room only once the homeserver lists that very room as that very chat
    // (`confirmedRoom`). Everything else stays in its Loro room.

    /// What a native chat answers to a verb the homeserver path does not carry yet (the Mac's
    /// words).
    static let nativeTextOnly = "This chat can only send text and files for now"

    /// `GET /me/matrix/people` as the relay writes it, `{rooms: {<chat_id>: <room_id>}}` and
    /// nothing else, or nil when the answer is not that (what was known is kept). Kept: the
    /// people chats this phone lists, each with a room id no other chat is given.
    static func parsePeopleRooms(_ raw: Any, people: Set<Id>) -> [Id: String]? {
        guard let d = raw as? [String: Any], d.count == 1, let rooms = d["rooms"] as? [String: String] else { return nil }
        let uses = Dictionary(rooms.values.map { ($0, 1) }, uniquingKeysWith: +)
        return rooms.filter { chat, room in
            people.contains(chat) && room.hasPrefix("!") && room.utf8.count <= 255 && uses[room] == 1
        }
    }

    /// The room a native chat is shown from: the one the relay names for it (`relay`), and
    /// only once the homeserver lists that very room as that very chat (`homeserver`, already
    /// checked against the room's create event). A room only one of them vouches for is not shown.
    static func confirmedRoom(relay: [Id: String], homeserver: [ArbosFFI.MatrixPeopleRow], chatId: Id) -> String? {
        guard let room = relay[chatId],
              homeserver.contains(where: { $0.roomId == room && $0.chatId == chatId }) else { return nil }
        return room
    }

    /// The homeserver room of a native chat (`confirmedRoom`).
    func nativeRoom(_ chatId: Id) -> String? {
        Self.confirmedRoom(relay: nativeRooms, homeserver: matrixSnapshot.people, chatId: chatId)
    }

    /// The client and room a native chat is read through, once both exist.
    func nativeTarget(_ chatId: Id) -> (ArbosFFI.MatrixClient, String)? {
        guard let client = matrixClient, let room = nativeRoom(chatId) else { return nil }
        return (client, room)
    }

    /// The homeserver's row for a native chat: its unread count and whether it is encrypted.
    func nativePeopleRow(_ chatId: Id) -> ArbosFFI.MatrixPeopleRow? {
        guard let room = nativeRoom(chatId) else { return nil }
        return matrixSnapshot.people.first { $0.roomId == room }
    }

    /// Ask the relay which people chats are on the homeserver. Only a session told
    /// `people_native` asks; a failed or malformed answer keeps what was known.
    func refreshNativeRooms() async {
        guard peopleNative, credentials != nil, !isFixture else { return }
        guard let raw = try? await relay.matrixPeopleRooms(),
              let rooms = Self.parsePeopleRooms(raw, people: Set(peopleChats.map(\.id))) else {
            matrixLog.notice("bad or no /me/matrix/people")
            return
        }
        // Signed out, or switched off, while the relay answered.
        guard peopleNative else { return }
        applyNativeRooms(rooms)
        openNativeRooms()
        // `link` goes live with a Loro room; an account whose every chat is native may have
        // none left to say the relay is reachable. It just answered.
        if link == .connecting, !nativeRooms.isEmpty { link = .live }
    }

    /// Read every native chat's room once: the newest page, then live rows through the
    /// listener, so an unopened chat still previews the way a synced Loro doc does.
    // Read from the device's event store; the homeserver is asked only for a chat the store
    // holds less than a page of.
    private func openNativeRooms() {
        for chat in nativeRooms.keys where !caughtUp.contains(chat) { openChannel(chat) }
    }

    /// `user:<id>` of everyone in a people chat, in the relay's spelling.
    private func nativeMembers(_ chatId: Id) -> [String] {
        (myUserId.map { [$0] } ?? []) + (peopleChat(chatId)?.humans.map(\.member) ?? [])
    }

    /// A native chat's rows as the transcript's messages. Who sent a row is the account the
    /// homeserver says sent it (`row.sender`). That names a person by Matrix localpart, which
    /// is lower-case; `members` (`user:<id>`, the relay's spelling) puts the id back so "mine"
    /// and the roster lookups match, for a row's sender and for who reacted.
    static func nativeMessages(_ rows: [ArbosFFI.MatrixMessageRow], chatId: Id, members: [String]) -> [Message] {
        func exact(_ who: String) -> String {
            members.first { $0.caseInsensitiveCompare(who) == .orderedSame } ?? who
        }
        return rows.compactMap { row in
            guard var m = decodeMatrixRow(row, chatId: chatId) else { return nil }
            m.chatId = chatId
            m.sender = exact(row.sender)
            m.pending = row.pending
            for i in m.reactions.indices { m.reactions[i].senders = m.reactions[i].senders.map(exact) }
            return m
        }
    }

    /// The homeserver's event for a row of a native chat, by message id.
    private func nativeEventId(_ client: ArbosFFI.MatrixClient, room: String, messageId: Id) async -> String? {
        (try? await client.openRoom(roomId: room))?.first { $0.id == messageId && !$0.pending }?.eventId
    }

    /// A message into a native chat; `replyTo` is a row id of this chat. The row shows through
    /// the timeline (`pending`) before the homeserver has it and leaves again if the send
    /// fails. The client encrypts it in an encrypted room or does not send it.
    ///
    /// Files go where the chat's messages are readable from (the Mac's rule, `native_send`).
    /// In an end-to-end encrypted chat each is encrypted on this phone and its ciphertext kept
    /// on the homeserver, the key inside the message; the relay sees none of it. In any other
    /// native chat they are the relay's blobs, like a Loro room's, so the row the relay
    /// carries back to the Loro room still finds them.
    func sendNativePeople(chatId: Id, text: String, replyTo: Id?, attachments: [PendingAttachment] = []) {
        guard let client = matrixClient, let room = nativeRoom(chatId) else {
            setSendError(chatId, UserError.of("send your message", .offline(deviceOffline: false), "Not connected to the server yet."))
            return
        }
        let messageId = newId()
        // Not knowing counts as encrypted, as it does for the client.
        let encrypted = nativePeopleRow(chatId)?.encrypted ?? true
        Task {
            do {
                var answers: String?
                if let replyTo {
                    answers = await nativeEventId(client, room: room, messageId: replyTo)
                    guard answers != nil else {
                        failRefused("send your message", "That message is no longer here.")
                        return
                    }
                }
                if attachments.isEmpty {
                    if let answers {
                        _ = try await client.sendReply(roomId: room, text: text, replyTo: answers, localId: messageId)
                    } else {
                        _ = try await client.sendText(roomId: room, text: text, localId: messageId)
                    }
                } else if encrypted {
                    let files = attachments.map {
                        ArbosFFI.MatrixUpload(name: $0.name, mime: $0.mime,
                                              kind: Self.attachmentKind(mime: $0.mime, name: $0.name), bytes: $0.data)
                    }
                    _ = try await client.sendFiles(roomId: room, text: text, files: files, replyTo: answers, localId: messageId)
                } else {
                    var parts: [String] = []
                    for a in attachments {
                        let key = try await uploadBlob(slot: "\(peopleSlot):\(chatId)", name: a.name, mime: a.mime, data: a.data)
                        parts.append(try Self.attachmentPart(Attachment(
                            name: a.name, mime: a.mime, blobRef: key, kind: Self.attachmentKind(mime: a.mime, name: a.name))))
                    }
                    _ = try await client.sendParts(roomId: room, text: text, partsJson: parts, replyTo: answers, localId: messageId)
                }
            } catch {
                fail("send your message", error)
            }
        }
    }

    /// The `attachment` part of a row for a file already on the relay, as the chat doc writes
    /// it (crdt-sync §6.2), as JSON.
    static func attachmentPart(_ attachment: Attachment) throws -> String {
        let body = try JSONSerialization.jsonObject(with: JSONEncoder().encode(attachment))
        let part: [String: Any] = ["id": newId(), "kind": "attachment", "attachment": body]
        return String(decoding: try JSONSerialization.data(withJSONObject: part), as: UTF8.self)
    }

    /// Flip my reaction on a native chat's row. The row changes when the homeserver's event
    /// arrives.
    func reactNativePeople(chatId: Id, messageId: Id, emoji: String) {
        guard let client = matrixClient, let room = nativeRoom(chatId), let me = myUserId else { return }
        // Sent without its skin tone, the way the transcript groups them (`DocJSON.reactions`).
        // Ceiling: one another app put on with a skin tone is not taken back from here.
        let key = MessageReaction.normalizeEmoji(emoji)
        let mine = messages(of: chatId).first { $0.id == messageId }?.reactions
            .contains { $0.emoji == key && $0.senders.contains(me) } ?? false
        Task {
            guard let event = await nativeEventId(client, room: room, messageId: messageId) else { return }
            do {
                try await client.react(roomId: room, eventId: event, key: key, on: !mine)
                Haptics.pick()
            } catch {
                fail("update the reaction", error)
            }
        }
    }

    /// Remove a native chat's message; the homeserver decides whether this person may.
    func deleteNativePeople(chatId: Id, messageId: Id) {
        guard let client = matrixClient, let room = nativeRoom(chatId) else { return }
        Task {
            guard let event = await nativeEventId(client, room: room, messageId: messageId) else { return }
            do {
                try await client.redact(roomId: room, eventId: event)
                Haptics.pick()
            } catch {
                fail("delete the message", error)
            }
        }
    }
}

/// Files of a native chat kept on the homeserver (an attachment whose `ref` is
/// `mx:<event>/<part>`), for everything that loads an attachment by URL: `AppModel.blobURL`
/// answers such a ref with a URL of this scheme, and this answers the URL with what the
/// Matrix client decrypts. Nothing leaves the phone under it and the URL cache keeps none of
/// it. Ceiling: `URLSession.shared` only, which is what every attachment loader here uses.
final class MatrixMediaProtocol: URLProtocol, @unchecked Sendable {
    static let scheme = "arbos-mx"
    /// What an attachment's `ref` starts with when its file is on the homeserver
    /// (`arbos_matrix::rows::MEDIA_REF`).
    static let refPrefix = "mx:"

    typealias Fetch = @Sendable (_ chatId: Id, _ reference: String) async throws -> (mime: String, bytes: Data)
    private static let lock = NSLock()
    private static var fetch: Fetch?

    /// Answer this scheme with `fetch` from now on.
    static func serve(_ fetch: @escaping Fetch) {
        lock.withLock {
            if self.fetch == nil { URLProtocol.registerClass(MatrixMediaProtocol.self) }
            self.fetch = fetch
        }
    }

    /// The URL of the file `key` names in the people chat whose blob slot is `slot`, or nil
    /// when `key` is a relay blob's.
    static func url(slot: String, key: String) -> URL? {
        guard key.hasPrefix(refPrefix), slot.hasPrefix("\(peopleSlot):") else { return nil }
        var comps = URLComponents()
        comps.scheme = scheme
        comps.host = "media"
        comps.path = "/" + slot.dropFirst(peopleSlot.count + 1)
        comps.queryItems = [URLQueryItem(name: "ref", value: String(key.dropFirst(refPrefix.count)))]
        return comps.url
    }

    private var loading: Task<Void, Never>?

    override class func canInit(with request: URLRequest) -> Bool { request.url?.scheme == scheme }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let url = request.url, let comps = URLComponents(url: url, resolvingAgainstBaseURL: false),
              let reference = comps.queryItems?.first(where: { $0.name == "ref" })?.value,
              let fetch = Self.lock.withLock({ Self.fetch }) else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        let chat = String(comps.path.dropFirst())
        loading = Task {
            do {
                let (mime, bytes) = try await fetch(chat, reference)
                guard !Task.isCancelled,
                      let response = HTTPURLResponse(url: url, statusCode: 200, httpVersion: "HTTP/1.1",
                                                     headerFields: ["Content-Type": mime]) else { return }
                client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
                client?.urlProtocol(self, didLoad: bytes)
                client?.urlProtocolDidFinishLoading(self)
            } catch {
                client?.urlProtocol(self, didFailWithError: error)
            }
        }
    }

    override func stopLoading() { loading?.cancel() }
}
