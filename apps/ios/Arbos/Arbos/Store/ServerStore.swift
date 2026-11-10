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
    private let owner: MatrixLifetime.Ticket

    init(model: AppModel, owner: MatrixLifetime.Ticket) {
        self.model = model
        self.owner = owner
    }

    func onRooms(snapshot: ArbosFFI.MatrixSnapshot) {
        let model = self.model
        Task { @MainActor [owner] in
            guard let model, model.acceptsMatrix(owner) else { return }
            model.applyMatrixSnapshot(snapshot)
        }
    }

    func onTimeline(roomId: String, rows: [ArbosFFI.MatrixMessageRow]) {
        let model = self.model
        Task { @MainActor [owner] in
            guard let model, model.acceptsMatrix(owner) else { return }
            model.applyMatrixTimeline(roomId: roomId, rows: rows)
        }
    }

    func onConnection(connected: Bool, error: String?) {
        let model = self.model
        Task { @MainActor [owner] in
            guard let model, model.acceptsMatrix(owner) else { return }
            model.applyMatrixConnection(connected: connected, error: error)
        }
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
    func wantServerRosters(expectedOwner: WorkLifetime.Ticket? = nil) {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return }
        for s in servers where s.members.isEmpty && !serverRostersRequested.contains(s.id) {
            serverRostersRequested.insert(s.id)
            Task { await refreshServer(s.id, expectedOwner: work.owner) }
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
    func refreshServers(expectedOwner: WorkLifetime.Ticket? = nil) async {
        guard credentials != nil, !isFixture,
              let work = accountWork(expectedOwner: expectedOwner) else { return }
        do {
            var list = try await work.api.servers()
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return }
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
            wantServerRosters(expectedOwner: work.owner)
            refreshChannelSenders()
            applyOpenServerHook()
        } catch let e as RelayError where e.status == 404 || e.status == 405 || e.status == 503 {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return }
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
    func refreshServer(_ id: Id, expectedOwner: WorkLifetime.Ticket? = nil) async -> Server? {
        if let expectedOwner, !accountLifetime.accepts(expectedOwner) { return nil }
        guard credentials != nil, !isFixture else { return server(id) }
        guard let work = accountWork(expectedOwner: expectedOwner) else { return nil }
        let fresh = try? await work.api.server(id)
        guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return nil }
        guard let s = fresh else { return server(id) }
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
    func createServer(name: String, discColor: UInt8? = nil, picture: DraftPicture? = nil, expectedOwner: WorkLifetime.Ticket? = nil) async -> Bool {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return false }
        do {
            var s = try await work.api.createServer(name: name, discColor: discColor)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            if let picture, let jpeg = await draftJpeg(picture) {
                guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
                let key = "server-\(Int(Date().timeIntervalSince1970 * 1000)).\(PictureMaster.ext(jpeg))"
                if (try? await work.api.putBlob(projectId: "servers:\(s.id)", key: key, mime: PictureMaster.mime(jpeg), data: jpeg)) != nil,
                   (try? await work.api.patchServer(s.id, imageKey: key)) != nil,
                   let fresh = try? await work.api.server(s.id) {
                    s = fresh
                }
            }
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            upsert(server: s)
            open(server: s)
            await startMatrixIfNeeded(expectedOwner: work.owner)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            return true
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            fail("create the server", error)
            return false
        }
    }

    /// Rule 8: "Join a server" → `POST /servers/join {code}` (the code or the whole link).
    func joinServer(code: String, expectedOwner: WorkLifetime.Ticket? = nil) async -> Bool {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return false }
        do {
            let s = try await work.api.joinServer(code: code)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            upsert(server: s)
            open(server: s)
            await startMatrixIfNeeded(expectedOwner: work.owner)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            return true
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            let type = (error as? RelayError)?.type
            let said: String? = type == "banned" ? "You can't join this server."
                : type == "not_found" || (error as? RelayError)?.status == 404 ? "That invite link is not valid."
                : nil
            fail(said.map { UserError.refused("join the server", $0) } ?? UserError.classify("join the server", error))
            return false
        }
    }

    /// Join an open server by id (no invite link).
    func joinOpenServer(_ id: Id, expectedOwner: WorkLifetime.Ticket? = nil) async -> Bool {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return false }
        do {
            let s = try await work.api.joinOpenServer(id: id)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            upsert(server: s)
            open(server: s)
            await startMatrixIfNeeded(expectedOwner: work.owner)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            return true
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
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
    func searchOpenServers(_ q: String, expectedOwner: WorkLifetime.Ticket? = nil) async -> [Server] {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return [] }
        guard credentials != nil, !isFixture, serversAvailable else { return [] }
        let result = (try? await work.api.searchOpenServers(q)) ?? []
        guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return [] }
        return result
    }

    /// D92 amendment: a server's disc colour (nil = initials); drops the uploaded picture.
    func setServerDisc(_ id: Id, color: UInt8?, expectedOwner: WorkLifetime.Ticket? = nil) async -> Bool {
        if let expectedOwner, !accountLifetime.accepts(expectedOwner) { return false }
        let work = accountWork(expectedOwner: expectedOwner)
        guard isFixture || work != nil else { return false }
        if let i = servers.firstIndex(where: { $0.id == id }) {
            servers[i].avatarDiscColor = color.map { Int($0) }
            servers[i].imageKey = nil
        }
        guard !isFixture else { return true }
        guard let work else { return false }
        do {
            try await work.api.setServerDisc(id, color: color)
            await refreshServers(expectedOwner: work.owner)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            return true
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            fail("save the server picture", error)
            await refreshServers(expectedOwner: work.owner)
            return false
        }
    }

    /// D92 amendment: a server picture — upload the master (PNG / JPEG) into `servers:{id}` and set it.
    func setServerImage(_ id: Id, jpeg: Data, expectedOwner: WorkLifetime.Ticket? = nil) async -> Bool {
        if let expectedOwner, !accountLifetime.accepts(expectedOwner) { return false }
        let work = accountWork(expectedOwner: expectedOwner)
        guard isFixture || work != nil else { return false }
        let key = "server-\(Int(Date().timeIntervalSince1970 * 1000)).\(PictureMaster.ext(jpeg))"
        if isFixture {
            if let i = servers.firstIndex(where: { $0.id == id }) { servers[i].imageKey = key }
            return true
        }
        guard let work else { return false }
        do {
            _ = try await work.api.putBlob(projectId: "servers:\(id)", key: key, mime: PictureMaster.mime(jpeg), data: jpeg)
            return await setServerImageKey(id, key: key, expectedOwner: work.owner)
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            fail("upload the picture", error)
            return false
        }
    }

    /// A server picture from a key the relay already holds (the Generate tab's answer).
    func setServerImageKey(_ id: Id, key: String, expectedOwner: WorkLifetime.Ticket? = nil) async -> Bool {
        if let expectedOwner, !accountLifetime.accepts(expectedOwner) { return false }
        let work = accountWork(expectedOwner: expectedOwner)
        guard isFixture || work != nil else { return false }
        if let i = servers.firstIndex(where: { $0.id == id }) { servers[i].imageKey = key }
        guard !isFixture else { return true }
        guard let work else { return false }
        do {
            try await work.api.patchServer(id, imageKey: key)
            await refreshServers(expectedOwner: work.owner)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            return true
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            fail("save the server picture", error)
            await refreshServers(expectedOwner: work.owner)
            return false
        }
    }

    /// The Generate tab for a server (owner / admins): the relay paints one picture.
    func generateServerAvatar(_ id: Id, prompt: String, expectedOwner: WorkLifetime.Ticket? = nil) async throws -> String {
        if let expectedOwner, !accountLifetime.accepts(expectedOwner) { throw CancellationError() }
        let work = accountWork(expectedOwner: expectedOwner)
        guard isFixture || work != nil else { throw CancellationError() }
        if isFixture {
            try? await Task.sleep(nanoseconds: 1_200_000_000)
            return "fixture-gen-\(prompt.lowercased().replacingOccurrences(of: " ", with: "-"))"
        }
        guard let work else { throw CancellationError() }
        let key = try await work.api.generateServerAvatar(id, prompt: prompt)
        guard accountLifetime.accepts(work.owner), !Task.isCancelled else { throw CancellationError() }
        return key
    }

    func renameServer(_ id: Id, name: String, expectedOwner: WorkLifetime.Ticket? = nil) async -> Bool {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return false }
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return false }
        if let i = servers.firstIndex(where: { $0.id == id }) { servers[i].name = trimmed }
        do {
            try await work.api.patchServer(id, name: trimmed)
            await refreshServers(expectedOwner: work.owner)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            return true
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            fail("rename the server", error)
            await refreshServers(expectedOwner: work.owner)
            return false
        }
    }

    /// Owner/admin: open = anyone can search and join without an invite link.
    func setServerOpen(_ id: Id, open: Bool, expectedOwner: WorkLifetime.Ticket? = nil) async -> Bool {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return false }
        if let i = servers.firstIndex(where: { $0.id == id }) { servers[i].isOpen = open }
        do {
            try await work.api.patchServer(id, open: open)
            await refreshServer(id, expectedOwner: work.owner)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            return true
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            fail("update the server", error)
            await refreshServers(expectedOwner: work.owner)
            return false
        }
    }

    /// Owner only: the server goes for everyone.
    func deleteServer(_ id: Id, expectedOwner: WorkLifetime.Ticket? = nil) async -> Bool {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return false }
        do {
            try await work.api.deleteServer(id)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            forgetServer(id)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            return true
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            fail("delete the server", error)
            return false
        }
    }

    func leaveServer(_ id: Id, expectedOwner: WorkLifetime.Ticket? = nil) async -> Bool {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return false }
        do {
            try await work.api.leaveServer(id)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            forgetServer(id)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            return true
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
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
    func createChannel(serverId: Id, name: String, restricted: Bool, expectedOwner: WorkLifetime.Ticket? = nil) async -> ServerChannel? {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return nil }
        do {
            let c = try await work.api.createChannel(serverId: serverId, name: Self.channelSlug(name), restricted: restricted)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return nil }
            if let i = servers.firstIndex(where: { $0.id == serverId }),
               !servers[i].channels.contains(where: { $0.id == c.id }) {
                servers[i].channels.append(c)
            }
            await refreshServers(expectedOwner: work.owner)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return nil }
            return channel(c.id) ?? c
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return nil }
            fail("create the channel", error)
            return nil
        }
    }

    func renameChannel(_ channel: ServerChannel, name: String, expectedOwner: WorkLifetime.Ticket? = nil) async -> Bool {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return false }
        let slug = Self.channelSlug(name)
        guard !slug.isEmpty else { return false }
        do {
            try await work.api.patchChannel(serverId: channel.serverId, channelId: channel.id, name: slug)
            await refreshServers(expectedOwner: work.owner)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            return true
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            fail("rename the channel", error)
            return false
        }
    }

    /// Channel page › Settings: the topic (owner/admin; ≤ 200 chars, empty clears it).
    func setChannelTopic(_ channel: ServerChannel, topic: String, expectedOwner: WorkLifetime.Ticket? = nil) async {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return }
        let topic = String(topic.trimmingCharacters(in: .whitespacesAndNewlines).prefix(200))
        if let si = servers.firstIndex(where: { $0.id == channel.serverId }),
           let ci = servers[si].channels.firstIndex(where: { $0.id == channel.id }) {
            servers[si].channels[ci].topic = topic.isEmpty ? nil : topic
        }
        do {
            try await work.api.patchChannel(serverId: channel.serverId, channelId: channel.id, topic: topic)
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return }
            fail("change the topic", error)
        }
        await refreshServers(expectedOwner: work.owner)
    }

    func isChannelMuted(_ channel: ServerChannel) -> Bool { mutedChannelIds.contains(channel.id) }

    func toggleChannelMute(_ channel: ServerChannel) {
        if mutedChannelIds.contains(channel.id) { mutedChannelIds.remove(channel.id) } else { mutedChannelIds.insert(channel.id) }
    }

    func setChannelRestricted(_ channel: ServerChannel, _ restricted: Bool, expectedOwner: WorkLifetime.Ticket? = nil) async {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return }
        if let si = servers.firstIndex(where: { $0.id == channel.serverId }),
           let ci = servers[si].channels.firstIndex(where: { $0.id == channel.id }) {
            servers[si].channels[ci].restricted = restricted
        }
        do {
            try await work.api.patchChannel(serverId: channel.serverId, channelId: channel.id, restricted: restricted)
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return }
            fail("change who posts", error)
        }
        await refreshServers(expectedOwner: work.owner)
    }

    /// Row menu → Archive / Unarchive: hide a channel from my list or bring it back. An archived
    /// channel that is open gives way to the channel list.
    func archiveChannel(_ channel: ServerChannel, _ archived: Bool, expectedOwner: WorkLifetime.Ticket? = nil) async {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return }
        if let si = servers.firstIndex(where: { $0.id == channel.serverId }),
           let ci = servers[si].channels.firstIndex(where: { $0.id == channel.id }) {
            servers[si].channels[ci].archived = archived
        }
        if archived { leaveScreensAbout(channel) }
        do {
            try await work.api.archiveChannel(serverId: channel.serverId, channelId: channel.id, archived: archived)
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return }
            fail(archived ? "archive the channel" : "unarchive the channel", error)
        }
        await refreshServers(expectedOwner: work.owner)
    }

    func deleteChannel(_ channel: ServerChannel, expectedOwner: WorkLifetime.Ticket? = nil) async -> Bool {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return false }
        do {
            try await work.api.deleteChannel(serverId: channel.serverId, channelId: channel.id)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            channelRows[channel.id] = nil
            caughtUp.remove(channel.id)
            leaveScreensAbout(channel)
            await refreshServers(expectedOwner: work.owner)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            return true
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            fail((error as? RelayError)?.status == 409
                 ? UserError.refused("delete the channel", "#general stays.")
                 : UserError.classify("delete the channel", error))
            return false
        }
    }

    /// Rule 6: owner resets the invite link (the old one stops working).
    /// Members tab → Add members (the main way in since 2026-10-22): one call per new person.
    func addServerMembers(serverId: Id, userIds: [String], expectedOwner: WorkLifetime.Ticket? = nil) async -> Bool {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return false }
        var ok = true
        for uid in userIds {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
            do { try await work.api.addServerMember(serverId: serverId, userId: uid) } catch {
                guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
                ok = false
                fail("add them", error)
            }
        }
        await refreshServer(serverId, expectedOwner: work.owner)
        guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return false }
        return ok
    }

    /// Settings › Invite links › New link (owner).
    func createServerInvite(_ id: Id, expires: Bool, singleUse: Bool, expectedOwner: WorkLifetime.Ticket? = nil) async {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return }
        do {
            _ = try await work.api.createServerInvite(id, expires: expires, maxUses: singleUse ? 1 : nil)
            await refreshServer(id, expectedOwner: work.owner)
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return }
            fail("make an invite link", error)
        }
    }

    /// Settings › Invite links › revoke (owner): gone from the list at once.
    func revokeServerInvite(_ id: Id, inviteId: String, expectedOwner: WorkLifetime.Ticket? = nil) async {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return }
        if let si = servers.firstIndex(where: { $0.id == id }) {
            servers[si].invites.removeAll { $0.id == inviteId }
        }
        do { try await work.api.revokeServerInvite(id, inviteId: inviteId) } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return }
            fail("revoke the link", error)
        }
        await refreshServer(id, expectedOwner: work.owner)
    }

    /// `arbos://join/<code>` (the site's invite page): join right away and land in the server.
    func joinFromLink(code: String, expectedOwner: WorkLifetime.Ticket? = nil) async {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return }
        let code = code.trimmingCharacters(in: CharacterSet(charactersIn: "/ "))
        guard !code.isEmpty else { return }
        _ = await joinServer(code: code, expectedOwner: work.owner)
    }

    func resetServerInvite(_ id: Id, expectedOwner: WorkLifetime.Ticket? = nil) async -> ServerInvite? {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return nil }
        do {
            let inv = try await work.api.resetServerInvite(id)
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return nil }
            if let i = servers.firstIndex(where: { $0.id == id }) { servers[i].invite = inv }
            return inv
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return nil }
            fail("reset the invite link", error)
            return nil
        }
    }

    func setServerRole(serverId: Id, userId: String, role: String, expectedOwner: WorkLifetime.Ticket? = nil) async {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return }
        do {
            try await work.api.setServerMemberRole(serverId: serverId, userId: userId, role: role)
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return }
            fail("change the role", error)
        }
        await refreshServer(serverId, expectedOwner: work.owner)
    }

    func removeServerMember(serverId: Id, userId: String, ban: Bool, expectedOwner: WorkLifetime.Ticket? = nil) async {
        guard let work = accountWork(expectedOwner: expectedOwner) else { return }
        do {
            try await work.api.removeServerMember(serverId: serverId, userId: userId, ban: ban)
        } catch {
            guard accountLifetime.accepts(work.owner), !Task.isCancelled else { return }
            fail("remove the member", error)
        }
        await refreshServer(serverId, expectedOwner: work.owner)
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
    func startMatrixIfNeeded(expectedOwner: WorkLifetime.Ticket? = nil) async {
        guard credentials != nil, !isFixture, matrixClient == nil,
              let work = accountWork(expectedOwner: expectedOwner),
              let owner = matrixLifetime.start(account: work.owner) else { return }
        defer { matrixLifetime.failed(owner) }
        let requestAPI = work.api
        do {
            guard let creds = try await requestAPI.matrixSession(), acceptsMatrix(owner) else { return }
            let listener = MatrixBridgeListener(model: self, owner: owner)
            let session = ArbosFFI.MatrixSession(homeserver: creds.homeserver, userId: creds.userId,
                                                 accessToken: creds.accessToken, deviceId: creds.deviceId)
            let client = try await matrixStart(session: session, listener: listener)
            // Signed out (or started twice) while the handshake ran.
            guard acceptsMatrix(owner), matrixClient == nil else {
                client.stop()
                return
            }
            matrixListener = listener
            matrixClient = client
            matrixLifetime.ready(owner)
            matrixConnected = true
            let snapshot = await client.snapshot()
            guard acceptsMatrix(owner) else { return }
            applyMatrixSnapshot(snapshot)
        } catch {
            matrixLog.notice("matrix start failed: \(error.localizedDescription, privacy: .public)")
        }
    }

    func acceptsMatrix(_ owner: MatrixLifetime.Ticket) -> Bool {
        credentials != nil && accountLifetime.accepts(owner.account) && matrixLifetime.accepts(owner)
    }

    func stopMatrix() {
        matrixLifetime.stop()
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
        guard case .running(let owner) = matrixLifetime.state, acceptsMatrix(owner),
              let c = channel(channelId), let client = matrixClient, let roomId = channelRoomId(c),
              !channelLoading.contains(channelId) else { return }
        channelLoading.insert(channelId)
        Task {
            guard acceptsMatrix(owner), !Task.isCancelled else { return }
            defer { if acceptsMatrix(owner) { channelLoading.remove(channelId) } }
            do {
                let rows = try await client.openRoom(roomId: roomId)
                guard acceptsMatrix(owner), !Task.isCancelled else { return }
                setChannelRows(channelId, rows)
                try? await client.markRead(roomId: roomId)
            } catch {
                guard acceptsMatrix(owner), !Task.isCancelled else { return }
                matrixLog.notice("open room failed: \(error.localizedDescription, privacy: .public)")
                // Paint the empty transcript rather than a spinner that never ends.
                caughtUp.insert(channelId)
            }
        }
    }

    func markChannelRead(_ channelId: Id) {
        guard case .running(let owner) = matrixLifetime.state, acceptsMatrix(owner),
              let c = channel(channelId), let client = matrixClient, let roomId = channelRoomId(c) else { return }
        Task {
            guard acceptsMatrix(owner), !Task.isCancelled else { return }
            try? await client.markRead(roomId: roomId)
        }
    }

    /// One older page when the transcript is scrolled near its top; stops at the room's start.
    func loadOlderChannel(_ channelId: Id) {
        guard case .running(let owner) = matrixLifetime.state, acceptsMatrix(owner),
              let c = channel(channelId), let client = matrixClient, let roomId = channelRoomId(c),
              caughtUp.contains(channelId), !channelAtStart.contains(channelId),
              !channelLoading.contains(channelId) else { return }
        channelLoading.insert(channelId)
        Task {
            guard acceptsMatrix(owner), !Task.isCancelled else { return }
            defer { if acceptsMatrix(owner) { channelLoading.remove(channelId) } }
            do {
                if let rows = try await client.loadOlder(roomId: roomId) {
                    guard acceptsMatrix(owner), !Task.isCancelled else { return }
                    setChannelRows(channelId, rows)
                } else {
                    guard acceptsMatrix(owner), !Task.isCancelled else { return }
                    channelAtStart.insert(channelId)
                }
            } catch {
                guard acceptsMatrix(owner), !Task.isCancelled else { return }
                matrixLog.notice("load older failed: \(error.localizedDescription, privacy: .public)")
            }
        }
    }

    /// Send into a channel: the row shows at once with a client ULID as its id; the echo
    /// arrives in the timeline with the same id (`life.arbos.id`).
    func sendChannel(chatId channelId: Id, text: String) {
        guard let c = channel(channelId), let me = user else { return }
        guard let client = matrixClient, let roomId = channelRoomId(c),
              case .running(let owner) = matrixLifetime.state, acceptsMatrix(owner) else {
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
            guard acceptsMatrix(owner), !Task.isCancelled else { return }
            do {
                _ = try await client.sendText(roomId: roomId, text: text, localId: messageId)
            } catch {
                guard acceptsMatrix(owner), !Task.isCancelled else { return }
                channelRows[channelId]?.removeAll { $0.id == messageId }
                fail("send your message", error)
            }
        }
    }
}
