// Servers and channels (D88): the relay is the control plane (`GET /servers`:
// names, roles, channels, invite links, members); the shared Matrix client is
// the data plane (what is in the rooms). Users never see the word "Matrix".

import ArbosFFI
import Foundation

/// `Route.chat(projectId:)` slot for a server channel (like `peopleSlot` for people chats);
/// the chat id is the relay channel id.
let channelSlot = "channel"

struct ServerChannel: Identifiable, Equatable {
    var id: String
    var serverId: String
    var name: String
    var topic: String?
    /// Admins post; members read.
    var restricted: Bool
    var position: Int
    /// nil for a few seconds after creation while the relay makes the room.
    var roomId: String?
    var createdAt: Int64
    /// Hidden from my channel list (per user; the server page's Archived group brings it back).
    var archived: Bool = false

    static func parse(_ raw: Any) -> ServerChannel? {
        guard let d = raw as? [String: Any], let id = d["id"] as? String else { return nil }
        return ServerChannel(
            id: id,
            serverId: d["server_id"] as? String ?? "",
            name: d["name"] as? String ?? "general",
            topic: (d["topic"] as? String).flatMap { $0.isEmpty ? nil : $0 },
            restricted: (d["restricted"] as? NSNumber)?.boolValue ?? false,
            position: (d["position"] as? NSNumber)?.intValue ?? 0,
            roomId: (d["room_id"] as? String).flatMap { $0.isEmpty ? nil : $0 },
            createdAt: (d["created_at"] as? NSNumber)?.int64Value ?? 0,
            archived: (d["archived"] as? NSNumber)?.boolValue ?? false
        )
    }
}

/// The server's invite link (owner/admin only).
struct ServerInvite: Equatable {
    var code: String
    var url: String

    static func parse(_ raw: Any?) -> ServerInvite? {
        guard let d = raw as? [String: Any], let code = d["code"] as? String, let url = d["url"] as? String else { return nil }
        return ServerInvite(code: code, url: url)
    }
}

/// One of the owner's invite links (several per server since 2026-10-22): optional expiry and
/// a use count; `usable` is the relay's verdict right now.
struct ServerInviteLink: Identifiable, Equatable {
    var id: String
    var code: String
    var url: String
    var createdAt: Int64
    var expiresAt: Int64?
    var usesLeft: Int?
    var usable: Bool

    static func parse(_ raw: Any) -> ServerInviteLink? {
        guard let d = raw as? [String: Any], let id = d["id"] as? String, let code = d["code"] as? String else { return nil }
        return ServerInviteLink(
            id: id, code: code, url: d["url"] as? String ?? "",
            createdAt: (d["created_at"] as? NSNumber)?.int64Value ?? 0,
            expiresAt: (d["expires_at"] as? NSNumber)?.int64Value,
            usesLeft: (d["uses_left"] as? NSNumber)?.intValue,
            usable: (d["usable"] as? NSNumber)?.boolValue ?? true
        )
    }

    /// "7 days left · 3 uses left", "No limits", "Expired", "Used up".
    func terms(now: Int64) -> String {
        if !usable { return usesLeft == 0 ? "Used up" : "Expired" }
        var parts: [String] = []
        if let t = expiresAt {
            let days = (max(t - now, 0) + 86_399_999) / 86_400_000
            parts.append(days <= 1 ? "Expires today" : "\(days) days left")
        }
        if let n = usesLeft { parts.append("\(n) use\(n == 1 ? "" : "s") left") }
        return parts.isEmpty ? "No limits" : parts.joined(separator: " · ")
    }
}

struct ServerMember: Identifiable, Equatable {
    var id: String
    var handle: String
    var name: String?
    var avatarUrl: String?
    /// owner | admin | member
    var role: String
    var joinedAt: Int64
    var avatarColorIndex: Int? = nil
    var online: Bool = false
    var lastSeenAt: Int64? = nil

    var displayName: String { name?.isEmpty == false ? name! : handle }

    static func parse(_ raw: Any) -> ServerMember? {
        guard let d = raw as? [String: Any], let id = (d["user_id"] as? String) ?? (d["id"] as? String) else { return nil }
        return ServerMember(
            id: id,
            handle: d["handle"] as? String ?? "",
            name: d["name"] as? String,
            avatarUrl: d["avatar_url"] as? String,
            role: d["role"] as? String ?? "member",
            joinedAt: (d["joined_at"] as? NSNumber)?.int64Value ?? 0,
            avatarColorIndex: (d["avatar_color_index_effective"] as? NSNumber)?.intValue,
            online: (d["online"] as? NSNumber)?.boolValue ?? false,
            lastSeenAt: (d["last_seen_at"] as? NSNumber)?.int64Value
        )
    }
}

struct Server: Identifiable, Equatable {
    var id: String
    var name: String
    var imageKey: String?
    var ownerUserId: String
    var spaceRoomId: String?
    var createdAt: Int64
    /// owner | admin | member (empty on open-server search hits before join)
    var myRole: String
    var memberCount: Int
    var channels: [ServerChannel]
    var generalChannelId: String?
    /// D79 `avatar_color_index_effective` (nil → derived from `id`).
    var avatarColorIndex: Int?
    /// D92 amendment: the server's leaf picture from the Create screen; nil → initials (or the
    /// neutral initial when `isNeutralAvatar`, Jacob 2026-10-25).
    var avatarLeaf: Int? = nil
    var avatarLeafColor: Int? = nil
    var leafPick: LeafPick? {
        guard let l = avatarLeaf, let c = avatarLeafColor, !LeafArt.isNeutral(l) else { return nil }
        return LeafPick(leaf: UInt8(clamping: l), color: UInt8(clamping: c))
    }
    var isNeutralAvatar: Bool { LeafArt.isNeutral(avatarLeaf) }
    /// Anyone on Arbos can search and join without an invite link.
    var isOpen: Bool
    /// Only for owner/admin.
    var invite: ServerInvite?
    /// Owner only: every live link (expired / used-up ones included, so the owner sees why).
    var invites: [ServerInviteLink] = []
    /// Only from `GET /servers/{id}` / `POST /servers`.
    var members: [ServerMember]
    /// Per-user sidebar archive (`archived_at` on `GET /servers`); nil = in the list.
    var archivedAt: Int64? = nil

    var isOwner: Bool { myRole == "owner" }
    var isAdmin: Bool { myRole == "owner" || myRole == "admin" }
    /// Already in this server (search hits for open servers you are not in have an empty role).
    var isMember: Bool { !myRole.isEmpty }

    /// The channels in my list (not archived), in order.
    var liveChannels: [ServerChannel] { channels.filter { !$0.archived } }
    /// The channels I archived, in order.
    var archivedChannels: [ServerChannel] { channels.filter(\.archived) }

    /// Where opening the server lands (rule 3).
    var general: ServerChannel? {
        channels.first { $0.id == generalChannelId } ?? channels.first { $0.name == "general" } ?? channels.first
    }

    static func parse(_ raw: Any) -> Server? {
        guard let d = raw as? [String: Any], let id = d["id"] as? String else { return nil }
        let channels = ((d["channels"] as? [Any]) ?? []).compactMap(ServerChannel.parse)
            .sorted { $0.position == $1.position ? $0.createdAt < $1.createdAt : $0.position < $1.position }
        return Server(
            id: id,
            name: d["name"] as? String ?? "Server",
            imageKey: (d["image_key"] as? String).flatMap { $0.isEmpty ? nil : $0 },
            ownerUserId: d["owner_user_id"] as? String ?? "",
            spaceRoomId: (d["space_room_id"] as? String).flatMap { $0.isEmpty ? nil : $0 },
            createdAt: (d["created_at"] as? NSNumber)?.int64Value ?? 0,
            myRole: d["my_role"] as? String ?? "member",
            memberCount: (d["member_count"] as? NSNumber)?.intValue ?? 0,
            channels: channels,
            generalChannelId: (d["general_channel_id"] as? String).flatMap { $0.isEmpty ? nil : $0 },
            avatarColorIndex: (d["avatar_color_index_effective"] as? NSNumber)?.intValue,
            avatarLeaf: (d["avatar_leaf"] as? NSNumber)?.intValue,
            avatarLeafColor: (d["avatar_leaf_color"] as? NSNumber)?.intValue,
            isOpen: (d["open"] as? Bool) ?? (d["open"] as? NSNumber)?.boolValue ?? false,
            invite: ServerInvite.parse(d["invite"]),
            invites: ((d["invites"] as? [Any]) ?? []).compactMap(ServerInviteLink.parse),
            members: ((d["members"] as? [Any]) ?? []).compactMap(ServerMember.parse),
            archivedAt: (d["archived_at"] as? NSNumber)?.int64Value
        )
    }
}

/// `GET /me/matrix`: this device's session for the shared Matrix client.
struct MatrixCredentials: Equatable {
    var homeserver: String
    var userId: String
    var accessToken: String
    var deviceId: String

    static func parse(_ raw: Any) -> MatrixCredentials? {
        guard let d = raw as? [String: Any],
              let homeserver = d["homeserver"] as? String, !homeserver.isEmpty,
              let userId = d["user_id"] as? String, !userId.isEmpty,
              let token = d["access_token"] as? String, !token.isEmpty
        else { return nil }
        return MatrixCredentials(homeserver: homeserver, userId: userId, accessToken: token, deviceId: d["device_id"] as? String ?? "")
    }
}
