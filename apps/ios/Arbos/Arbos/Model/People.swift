// People chats (D72 v1): DMs by GitHub handle, shared agents, project
// members. Shapes from internal/people-chats-api.md.

import Foundation
import ArbosFFI

struct Person: Identifiable, Equatable, Hashable, Codable {
    var id: String
    var handle: String
    var avatarUrl: String?
    var name: String?
    /// member (signed in) | invited (DM opened, never signed in) | github (not on Arbos yet)
    var status: String
    /// §9 presence: last seen (ms, minute-floored; nil = never) and online (< 90 s).
    var lastSeenAt: Int64? = nil
    var online: Bool = false
    /// D79 `avatar_color_index_effective` from the relay (nil → derived from `id`).
    var avatarColorIndex: Int? = nil

    var displayName: String { name?.isEmpty == false ? name! : handle }

    /// Memberwise init (kept explicit because `init(dto:)` would suppress synthesis).
    init(
        id: String,
        handle: String,
        avatarUrl: String? = nil,
        name: String? = nil,
        status: String = "member",
        lastSeenAt: Int64? = nil,
        online: Bool = false,
        avatarColorIndex: Int? = nil
    ) {
        self.id = id
        self.handle = handle
        self.avatarUrl = avatarUrl
        self.name = name
        self.status = status
        self.lastSeenAt = lastSeenAt
        self.online = online
        self.avatarColorIndex = avatarColorIndex
    }

    init(dto: UserCardDto) {
        self.init(
            id: dto.id,
            handle: dto.handle,
            avatarUrl: dto.avatar_url,
            name: dto.name,
            status: dto.status.isEmpty ? "member" : dto.status,
            lastSeenAt: dto.last_seen_at.flatMap { $0 },
            online: dto.online ?? false,
            avatarColorIndex: dto.avatar_color_index_effective.map(Int.init)
        )
    }

    static func parse(_ raw: Any) -> Person? {
        guard JSONSerialization.isValidJSONObject(raw),
              let data = try? JSONSerialization.data(withJSONObject: raw) else { return nil }
        do {
            return Person(dto: try JSONDecoder().decode(UserCardDto.self, from: data))
        } catch {
            NSLog("UserCardDto: \(error)")
            return nil
        }
    }
}

struct ChatMember: Identifiable, Equatable {
    enum Kind: Equatable { case user, agent }
    var id: String
    var kind: Kind
    /// `user:<id>` or `agent:<project_id>` as the relay writes it.
    var member: String
    var role: String
    var handle: String
    var avatarUrl: String?
    var name: String?
    var status: String
    /// When this member was added to the chat (relay `joined_at`, unix ms).
    var joinedAt: Int64
    /// Agents only.
    var project: Project?
    var ownerId: String?
    var ownerHandle: String?
    var replyPolicy: String?
    /// §9 presence for people; agents carry `description` instead.
    var lastSeenAt: Int64? = nil
    var online: Bool = false
    var description: String? = nil
    /// D79 `avatar_color_index_effective` from the relay (nil → derived from `id`).
    var avatarColorIndex: Int? = nil
    /// Group admin muted this person from posting (not notification mute).
    var silenced: Bool = false
    /// When the admin mute ends (ms). nil while muted = until unmuted.
    var silencedUntil: Int64? = nil
    /// Admin: owner granted mute/ban rights (default true). Meaningful only when role is admin.
    var canMuteBan: Bool = true

    var isAgent: Bool { kind == .agent }
    var displayName: String { kind == .agent ? (project?.name ?? handle) : (name?.isEmpty == false ? name! : handle) }

    /// True while an admin mute still blocks posting (honours timed `silencedUntil`).
    var isSilencedNow: Bool {
        guard silenced else { return false }
        if let until = silencedUntil {
            let now = Int64(Date().timeIntervalSince1970 * 1000)
            if until <= now { return false }
        }
        return true
    }
}

/// How long an admin mute lasts when silencing a group member from posting.
enum MemberMuteDuration: CaseIterable, Identifiable {
    case oneHour
    case eightHours
    case oneDay
    case untilUnmuted

    var id: String { title }

    var title: String {
        switch self {
        case .oneHour: return "For 1 hour"
        case .eightHours: return "For 8 hours"
        case .oneDay: return "For 24 hours"
        case .untilUnmuted: return "Until unmuted"
        }
    }

    /// Relay `silenced_until` in ms, or nil for until unmuted.
    var silencedUntilMs: Int64? {
        let now = Date().timeIntervalSince1970 * 1000
        switch self {
        case .oneHour: return Int64(now + 60 * 60 * 1000)
        case .eightHours: return Int64(now + 8 * 60 * 60 * 1000)
        case .oneDay: return Int64(now + 24 * 60 * 60 * 1000)
        case .untilUnmuted: return nil
        }
    }
}

extension ChatMember {
    /// Maps a relay `ChatMemberDto` the same way the old hand JSON parse did.
    init(dto: ChatMemberDto) {
        let isAgent = dto.member.hasPrefix("agent:")
        let id = dto.member.split(separator: ":", maxSplits: 1).last.map { String($0) } ?? dto.member
        var project: Arbos.Project? = nil
        if isAgent, let p = dto.project {
            let avatarColor = (p.avatar_color.isEmpty ? nil : p.avatar_color)
                ?? dto.avatar_color.flatMap { $0.isEmpty ? nil : $0 }
            project = Arbos.Project(
                id: p.id,
                name: p.name.isEmpty ? "Agent" : p.name,
                icon: p.icon.isEmpty ? "A" : p.icon,
                color: p.color.isEmpty ? "#8b7cf6" : p.color,
                state: .awake,
                stateSince: nil,
                stateDetail: nil,
                model: "",
                effort: .low,
                spendTodayUsd: 0,
                spendCapUsd: 0,
                avatar: p.avatar.isEmpty ? nil : p.avatar,
                avatarColor: avatarColor
            )
            project?.avatarColorIndex = p.avatar_color_index.map(Int.init)
            project?.avatarColorIndexEffective = Int(p.avatar_color_index_effective)
            project?.avatarLeaf = p.avatar_leaf.map(Int.init)
            project?.avatarLeafColor = p.avatar_leaf_color.map(Int.init)
            // The uploaded / generated picture travels with the roster card (#1592); the
            // fetch URL is built by `AppModel.agent(member:)` where the relay token lives.
            project?.imageKey = p.image_key.flatMap { $0.isEmpty ? nil : $0 }
            project?.avatarVersion = p.avatar_version.map(Int.init) ?? 0
        }
        self.id = id
        self.kind = isAgent ? .agent : .user
        self.member = dto.member
        self.role = dto.role.isEmpty ? "member" : dto.role
        self.handle = dto.handle ?? (isAgent ? "agent" : "someone")
        self.avatarUrl = dto.avatar_url.flatMap { $0 }
        self.name = dto.name.flatMap { $0 }
        self.status = dto.status ?? "member"
        self.joinedAt = dto.joined_at
        self.project = project
        self.ownerId = dto.owner
        self.ownerHandle = dto.owner_handle.flatMap { $0 }
        self.replyPolicy = nil
        self.lastSeenAt = dto.last_seen_at.flatMap { $0 }
        self.online = dto.online ?? false
        self.avatarColorIndex = dto.avatar_color_index_effective.map(Int.init)
        self.description = dto.description ?? dto.project?.description
        self.silenced = dto.silenced ?? false
        self.silencedUntil = dto.silenced_until
        self.canMuteBan = dto.can_mute_ban ?? true
    }

    static func parse(_ raw: Any) -> ChatMember? {
        guard JSONSerialization.isValidJSONObject(raw),
              let data = try? JSONSerialization.data(withJSONObject: raw) else { return nil }
        do {
            return ChatMember(dto: try JSONDecoder().decode(ChatMemberDto.self, from: data))
        } catch {
            NSLog("ChatMemberDto: \(error)")
            return nil
        }
    }
}

/// How long to mute a group chat's notifications (home row / group info).
enum ChatMuteDuration: CaseIterable, Identifiable {
    case oneHour
    case eightHours
    case untilTurnedOn

    var id: String { title }

    var title: String {
        switch self {
        case .oneHour: return "For 1 hour"
        case .eightHours: return "For 8 hours"
        case .untilTurnedOn: return "Until I turn it back on"
        }
    }

    /// When this mute should end. `distantFuture` means until the user unmutes.
    var endDate: Date {
        switch self {
        case .oneHour: return Date().addingTimeInterval(60 * 60)
        case .eightHours: return Date().addingTimeInterval(8 * 60 * 60)
        case .untilTurnedOn: return Date.distantFuture
        }
    }
}

struct PeopleChat: Identifiable, Equatable {
    var id: String
    var kind: String
    var createdBy: String
    var createdAt: Int64
    var title: String
    /// Blob key under `people:{id}` for an optional group picture.
    var imageKey: String? = nil
    /// D92 amendment: the group's leaf picture from the Create screen (the uploaded picture wins).
    var avatarLeaf: Int? = nil
    var avatarLeafColor: Int? = nil
    /// The leaf to draw; nil for a group without one — including the neutral pair (Jacob,
    /// 2026-10-25), which `isNeutralAvatar` names and draws as the title's initial on grey.
    var leafPick: LeafPick? {
        guard let l = avatarLeaf, let c = avatarLeafColor, !LeafArt.isNeutral(l) else { return nil }
        return LeafPick(leaf: UInt8(clamping: l), color: UInt8(clamping: c))
    }
    var isNeutralAvatar: Bool { LeafArt.isNeutral(avatarLeaf) }
    /// Optional about text for a people group (≤200 chars).
    var description: String = ""
    var pending: Bool
    var members: [ChatMember]
    var headSeq: Int64
    var readSeq: Int64
    var unread: Bool
    var lockState: String
    var lastActivityAt: Int64
    var myRole: String
    var muted: Bool
    /// Admin muted me from posting in this group.
    var silenced: Bool
    /// When my admin mute ends (ms). nil while muted = until unmuted.
    var silencedUntil: Int64? = nil
    /// Per-user sidebar archive (`archived_at` on the relay); nil = in the list.
    var archivedAt: Int64? = nil
    /// Banned people (owner/admin with mute-ban rights); Unban from the member list.
    var banned: [ChatMember] = []
    /// Members I blocked, as `user:<id>` — the relay's `blockedSenders` hint ([feedback
    /// blocked-state-ios-2026-10-26]): their rows are hidden for me alone (groups).
    var blockedSenders: [String] = []

    /// True while I am muted from posting (honours timed mute).
    var isSilencedNow: Bool {
        guard silenced else { return false }
        if let until = silencedUntil {
            let now = Int64(Date().timeIntervalSince1970 * 1000)
            if until <= now { return false }
        }
        return true
    }

    var humans: [ChatMember] { members.filter { !$0.isAgent } }
    var agents: [ChatMember] { members.filter { $0.isAgent } }

    /// Several humans, any agent, or `kind == group`: show a cluster, not a single person.
    /// Adding an agent to a DM turns the chat into a group for the header and profile.
    var isGroup: Bool { kind == "group" || humans.count > 2 || !agents.isEmpty }

    /// The other person of a DM.
    func counterpart(me: String?) -> ChatMember? {
        humans.first { $0.id != me } ?? humans.first
    }

    /// Who the empty composer names ("Message …") when nobody is picked: a DM's other person
    /// (also a DM that gained agents, like the header pill), otherwise the chat's own name —
    /// never one member of a group ("Message Arbonites", not "Message Jacob").
    func composerName(me: String?) -> String {
        if !isGroup || kind == "dm", let other = counterpart(me: me) { return other.displayName }
        return title
    }

    var lockLabel: String {
        lockState == "people_only" ? "People only" : "Visible to \(agents.map { "@\($0.ownerHandle ?? "owner")" }.joined(separator: ", "))'s agent"
    }
}

extension PeopleChat {
    /// Maps a relay `PeopleChatDto` the same way the old hand JSON parse did.
    init(dto: PeopleChatDto) {
        self.id = dto.id
        self.kind = dto.kind.isEmpty ? "dm" : dto.kind
        self.createdBy = dto.created_by
        self.createdAt = dto.created_at
        self.title = dto.title.isEmpty ? "Chat" : dto.title
        self.imageKey = dto.image_key.flatMap { $0.isEmpty ? nil : $0 }
        self.avatarLeaf = dto.avatar_leaf.map(Int.init)
        self.avatarLeafColor = dto.avatar_leaf_color.map(Int.init)
        self.description = dto.description?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        self.pending = dto.pending
        self.members = dto.members.map { ChatMember(dto: $0) }
        self.headSeq = dto.head_seq
        self.readSeq = dto.read_seq
        self.unread = dto.unread
        self.lockState = dto.lock_state.isEmpty ? "people_only" : dto.lock_state
        self.lastActivityAt = dto.last_activity_at
        self.myRole = dto.me.role.flatMap { $0 } ?? "member"
        self.muted = dto.me.muted ?? false
        self.silenced = dto.me.silenced ?? false
        self.silencedUntil = dto.me.silenced_until
        self.archivedAt = dto.archived_at.flatMap { $0 > 0 ? $0 : nil }
        self.banned = (dto.banned ?? []).map { ChatMember(dto: $0) }
        self.blockedSenders = dto.blocked_senders ?? []
    }

    /// Is this person already in the chat? Me (`meId`) counts as in. A handle
    /// match is enough: search results and roster rows need not share an id.
    func hasPerson(_ p: Person, meId: String?) -> Bool {
        if let meId, p.id == meId { return true }
        return humans.contains { $0.id == p.id || $0.handle.lowercased() == p.handle.lowercased() }
    }

    /// Is this agent (by project id) already in the chat?
    func hasAgent(_ projectId: String) -> Bool {
        agents.contains { $0.id == projectId || $0.project?.id == projectId }
    }

    static func parse(_ raw: Any) -> PeopleChat? {
        guard JSONSerialization.isValidJSONObject(raw),
              let data = try? JSONSerialization.data(withJSONObject: raw) else { return nil }
        do {
            return PeopleChat(dto: try JSONDecoder().decode(PeopleChatDto.self, from: data))
        } catch {
            NSLog("PeopleChatDto: \(error)")
            return nil
        }
    }
}

/// The Add members picker's one agent section ([feedback add-your-agent-row-removal-ios-2026-10-27];
/// the Mac's `people::AGENTS_SECTION_LABEL` / `others_agents_in` are the twin): titled "Agents",
/// my agents first (pickable), then the other members' agents already in the chat — greyed,
/// already in; only their owner adds or removes them — so the section is the chat's whole agent
/// roster. The separate "Add agent" / "Add your agent" rows are gone on both platforms.
enum AddMembersAgents {
    static let sectionTitle = "Agents"
    static let peopleTitle = "People"

    /// The other members' agents already in `chat` (not mine — mine are the pickable rows),
    /// by name, narrowed by what was typed (name or @handle). Nothing outside Add members.
    static func others(in chat: PeopleChat?, meId: String?, query: String) -> [ChatMember] {
        guard let chat else { return [] }
        let needle = query.trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "@")).lowercased()
        return chat.agents
            .filter { $0.ownerId == nil || $0.ownerId != meId }
            .filter { needle.isEmpty || $0.displayName.lowercased().contains(needle) || $0.handle.lowercased().contains(needle) }
            .sorted { $0.displayName.localizedCaseInsensitiveCompare($1.displayName) == .orderedAscending }
    }

    /// The row's second line for another member's agent: `@handle · run by @owner`.
    static func note(for agent: ChatMember) -> String? {
        agent.ownerHandle.map { "run by @\($0)" }
    }
}

/// The relay calls "Add" makes for one group: new people by handle, new agents
/// by project id, both on `chatId` — never a new group. Whoever is already in
/// (people, me, agents) is dropped so a stale pick never hits `already_member`.
struct AddMembersPlan: Equatable {
    var chatId: String
    var handles: [String]
    var agentIds: [String]

    static func make(chat: PeopleChat, meId: String?, picked: [Person], pickedAgents: [Project]) -> AddMembersPlan {
        AddMembersPlan(
            chatId: chat.id,
            handles: picked.filter { !chat.hasPerson($0, meId: meId) }.map(\.handle),
            agentIds: pickedAgents.map(\.id).filter { !chat.hasAgent($0) }
        )
    }

    var isEmpty: Bool { handles.isEmpty && agentIds.isEmpty }
}

/// A membership notice this device shows before the relay's own row arrives (ARB-393): the
/// adding phone writes "<me> added <names>" into its transcript the moment Add is tapped, so
/// the line never waits on the round trip. The relay stays the author of record — when its
/// system row with the same words lands, the local copy is dropped; a failed add or a row that
/// never comes removes it after `ttlMs`. Mirrors crates/arbos-mac `model::OptimisticNotice`.
struct OptimisticNotice: Equatable {
    /// Row id prefix: a snapshot never carries these, so they are easy to tell apart.
    static let idPrefix = "local-notice:"
    /// How long an unconfirmed local notice may stay on screen.
    static let ttlMs: Int64 = 60_000
    /// The relay stamps its row with its own clock; a row this much older than the local one
    /// still counts as the same notice.
    static let skewMs: Int64 = 15_000

    var id: Id
    var text: String
    var createdAt: Int64
    /// The same facts the relay's row will carry (actor = me, subjects by id), so this copy
    /// is worded from the live roster like that row and is matched to it by facts, not by
    /// words a stale local `/me` name could get wrong ("Const added Aureum", 2026-10-26).
    var notice: MembershipNotice? = nil

    init(text: String, now: Int64, notice: MembershipNotice? = nil) {
        id = Self.idPrefix + newId()
        self.text = text
        createdAt = now
        self.notice = notice
    }

    /// "A", "A and B", "A, B and C" — the relay's `notices::join_names`, so the local line
    /// reads exactly like the row that replaces it.
    static func joinNames(_ names: [String]) -> String {
        switch names.count {
        case 0: return ""
        case 1: return names[0]
        case 2: return "\(names[0]) and \(names[1])"
        default: return names.dropLast().joined(separator: ", ") + " and " + names[names.count - 1]
        }
    }

    /// "<who> added <names>"; nil with nobody to name.
    static func addedLine(who: String, names: [String]) -> String? {
        names.isEmpty ? nil : "\(who) added \(joinNames(names))"
    }

    /// The transcript row for this notice: a centred system line like the relay's.
    func message(chatId: Id) -> Message {
        Message(
            id: id, chatId: chatId, role: .system, sender: "system",
            status: .done, turn: nil, decision: nil, subagentId: nil, subagentName: nil,
            inReplyTo: nil, usage: [:], model: nil,
            createdAt: createdAt, completedAt: createdAt, pending: false,
            parts: [.text(id: "\(id)-t", text: text)], hiddenCount: 0,
            notice: notice
        )
    }

    /// The relay's row for this notice: a system line with the same facts (or, for a relay
    /// that writes none, the same words), written no earlier than the local one (minus
    /// clock skew).
    func isConfirmed(by m: Message) -> Bool {
        guard m.role == .system, !m.id.hasPrefix(Self.idPrefix), m.createdAt + Self.skewMs >= createdAt else {
            return false
        }
        if let mine = notice, let theirs = m.notice { return mine == theirs }
        return m.plainText.trimmingCharacters(in: .whitespacesAndNewlines) == text
    }

    /// Fold a chat's local notices into a fresh snapshot from the relay: a notice the snapshot
    /// confirms (same words, relay row) or one past its TTL is dropped; the rest are re-inserted
    /// in time order so they survive the list replacement. Returns how many were confirmed.
    @discardableResult
    static func reconcile(_ list: inout [Message], notices: inout [OptimisticNotice], chatId: Id, now: Int64) -> Int {
        var confirmed = 0
        notices.removeAll { n in
            if list.contains(where: { n.isConfirmed(by: $0) }) {
                confirmed += 1
                return true
            }
            return now - n.createdAt >= ttlMs
        }
        // Snapshots never carry local rows; a stale copy from an earlier merge must not double up.
        list.removeAll { $0.id.hasPrefix(idPrefix) }
        for n in notices {
            let at = (list.lastIndex { $0.createdAt <= n.createdAt }).map { $0 + 1 } ?? 0
            list.insert(n.message(chatId: chatId), at: at)
        }
        return confirmed
    }
}

struct ProjectMember: Identifiable, Equatable {
    var id: String
    var handle: String
    var avatarUrl: String?
    var name: String?
    var status: String
    var role: String
    var lastSeenAt: Int64? = nil
    var online: Bool = false

    var avatarColorIndex: Int? = nil
}

extension ProjectMember {
    /// Maps a relay `ProjectMemberDto` the same way the old hand JSON parse did.
    init(dto: ProjectMemberDto) {
        self.id = dto.user_id.isEmpty ? dto.id : dto.user_id
        self.handle = dto.handle ?? ""
        self.avatarUrl = dto.avatar_url
        self.name = dto.name
        self.status = dto.status.isEmpty ? "member" : dto.status
        self.role = dto.role.isEmpty ? "member" : dto.role
        self.lastSeenAt = dto.last_seen_at
        self.online = dto.online
        self.avatarColorIndex = Int(dto.avatar_color_index_effective)
    }
}

/// `GET /me/pushes` item: `id`/`created_at` from `PushItemDto`, event from the same JSON object.
struct PushItem: Identifiable, Equatable {
    var id: String
    var createdAt: Int64
    var event: PushEventDto

    static func == (a: PushItem, b: PushItem) -> Bool { a.id == b.id && a.createdAt == b.createdAt }

    static func parse(_ raw: Any) -> PushItem? {
        guard JSONSerialization.isValidJSONObject(raw),
              let data = try? JSONSerialization.data(withJSONObject: raw) else { return nil }
        do {
            let meta = try JSONDecoder().decode(PushItemDto.self, from: data)
            let event = try JSONDecoder().decode(PushEventDto.self, from: data)
            return PushItem(id: meta.id, createdAt: meta.created_at, event: event)
        } catch {
            return nil
        }
    }
}

/// People-chat room slot used in room names, blob paths and doc meta.
let peopleSlot = "people"

/// Mention slug for an agent member: project name lower-cased, spaces → "-".
func mentionSlug(_ name: String) -> String {
    name.lowercased().replacingOccurrences(of: " ", with: "-")
}
