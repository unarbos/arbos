// Calls v1 (internal/calls-v1-contract.md): a LiveKit room per call, one
// active call per chat. `sender{}` (the same shape as everywhere) names
// who started it and who is in it; LiveKit identity is `u_<user_id>` and
// the participant metadata is the sender JSON.

import Foundation
import ArbosFFI

enum CallKind: String, Codable {
    case voice, video
}

/// `sender{}`: `{kind, id, name, handle, avatar, avatar_color, avatar_url}`.
struct CallPerson: Identifiable, Equatable, Hashable {
    var id: String
    var kind: String
    var name: String?
    var handle: String
    var avatar: String?
    var avatarColor: String?
    var avatarUrl: String?
    /// D79 `avatar_color_index_effective` (nil → derived from `id`).
    var avatarColorIndex: Int? = nil

    var displayName: String { name?.isEmpty == false ? name! : "@\(handle)" }
    var isAgent: Bool { kind == "agent" }

    /// Memberwise init (kept explicit because `init(dto:)` would suppress synthesis).
    init(
        id: String,
        kind: String,
        name: String? = nil,
        handle: String,
        avatar: String? = nil,
        avatarColor: String? = nil,
        avatarUrl: String? = nil,
        avatarColorIndex: Int? = nil
    ) {
        self.id = id
        self.kind = kind
        self.name = name
        self.handle = handle
        self.avatar = avatar
        self.avatarColor = avatarColor
        self.avatarUrl = avatarUrl
        self.avatarColorIndex = avatarColorIndex
    }

    init(dto: SenderDto) {
        let emptyToNil: (String?) -> String? = { $0.flatMap { $0.isEmpty ? nil : $0 } }
        self.init(
            id: dto.id,
            kind: dto.kind.isEmpty ? "user" : dto.kind,
            name: emptyToNil(dto.name),
            handle: dto.handle.isEmpty ? dto.id : dto.handle,
            avatar: emptyToNil(dto.avatar),
            avatarColor: emptyToNil(dto.avatar_color),
            avatarUrl: emptyToNil(dto.avatar_url),
            avatarColorIndex: Int(dto.avatar_color_index_effective)
        )
    }

    static func parse(_ raw: Any?) -> CallPerson? {
        guard let raw, JSONSerialization.isValidJSONObject(raw),
              let data = try? JSONSerialization.data(withJSONObject: raw) else { return nil }
        do {
            return CallPerson(dto: try JSONDecoder().decode(SenderDto.self, from: data))
        } catch {
            NSLog("SenderDto: \(error)")
            return nil
        }
    }

    static func parse(metadata: String?) -> CallPerson? {
        guard let metadata, let data = metadata.data(using: .utf8) else { return nil }
        do {
            return CallPerson(dto: try JSONDecoder().decode(SenderDto.self, from: data))
        } catch {
            NSLog("SenderDto metadata: \(error)")
            return nil
        }
    }

    /// From a LiveKit identity `u_<id>` when there is no metadata yet.
    static func placeholder(identity: String, name: String?) -> CallPerson {
        let id = identity.hasPrefix("u_") ? String(identity.dropFirst(2)) : identity
        return CallPerson(id: id, kind: identity.hasPrefix("a_") ? "agent" : "user", name: name, handle: name ?? id,
                          avatar: nil, avatarColor: nil, avatarUrl: nil)
    }

    /// A people-chat member (human or agent) as a call tile before they join.
    static func from(member: ChatMember) -> CallPerson {
        if member.isAgent {
            return CallPerson(id: member.id, kind: "agent", name: member.project?.name ?? member.name,
                              handle: mentionSlug(member.project?.name ?? member.handle),
                              avatar: member.project?.avatar, avatarColor: member.project?.avatarColor, avatarUrl: nil)
        }
        var p = CallPerson(id: member.id, kind: "user", name: member.name, handle: member.handle,
                           avatar: nil, avatarColor: nil, avatarUrl: member.avatarUrl)
        p.avatarColorIndex = member.avatarColorIndex
        return p
    }
}

/// §8 `reachable[]` on a 201 start: can this callee be rung right now?
struct CallReachability: Equatable {
    var userId: String
    var handle: String
    var name: String?
    var anyLive: Bool
    var anyVoip: Bool
    /// Regular APNs token — they still get an Incoming-call banner / background wake.
    var anyPush: Bool

    var displayName: String { name?.isEmpty == false ? name! : "@\(handle)" }
    /// PushKit or a normal alert can reach them while Arbos is closed.
    var canRingClosed: Bool { anyVoip || anyPush }

    init(userId: String, handle: String, name: String?, anyLive: Bool, anyVoip: Bool, anyPush: Bool) {
        self.userId = userId
        self.handle = handle
        self.name = name
        self.anyLive = anyLive
        self.anyVoip = anyVoip
        self.anyPush = anyPush
    }

    init(dto: CallReachabilityDto) {
        self.init(
            userId: dto.user_id,
            handle: dto.handle.isEmpty ? dto.user_id : dto.handle,
            name: dto.name.isEmpty ? nil : dto.name,
            anyLive: dto.any_live,
            anyVoip: dto.any_voip,
            anyPush: dto.any_push
        )
    }

    static func parse(_ raw: Any?) -> CallReachability? {
        guard let raw, JSONSerialization.isValidJSONObject(raw),
              let data = try? JSONSerialization.data(withJSONObject: raw) else { return nil }
        do {
            return CallReachability(dto: try JSONDecoder().decode(CallReachabilityDto.self, from: data))
        } catch {
            NSLog("CallReachabilityDto: \(error)")
            return nil
        }
    }
}

struct Call: Identifiable, Equatable {
    var callId: String
    var chatId: String
    /// `people` for DMs, else the shared project's id.
    var projectId: String
    var kind: CallKind
    var room: String
    var livekitUrl: String
    var token: String?
    var tokenExpiresAt: Int64?
    var startedBy: CallPerson?
    var startedAt: Int64
    var endedAt: Int64?
    var reason: String?
    var participants: [CallPerson]
    /// Project agents that belong to this chat (Phase 2): they join through the voice worker
    /// and show as tiles before they connect.
    var agents: [CallPerson] = []
    /// Only on the 201 start response.
    var reachable: [CallReachability] = []
    /// `"open"` = anyone may speak; `"hosted"` = host + invited speakers only (bigger groups).
    var speakMode: String?
    /// People the host invited to speak (host is `startedBy`).
    var invitedSpeakers: [CallPerson] = []
    /// Whether this device may publish audio (from the join reply).
    var canSpeak: Bool?

    var id: String { callId }
    var isPeople: Bool { projectId == peopleSlot || projectId.isEmpty }
    var ended: Bool { endedAt != nil }
    var isHostedSpeak: Bool { speakMode == "hosted" }

    init(
        callId: String,
        chatId: String,
        projectId: String,
        kind: CallKind,
        room: String,
        livekitUrl: String,
        token: String? = nil,
        tokenExpiresAt: Int64? = nil,
        startedBy: CallPerson? = nil,
        startedAt: Int64,
        endedAt: Int64? = nil,
        reason: String? = nil,
        participants: [CallPerson] = [],
        agents: [CallPerson] = [],
        reachable: [CallReachability] = [],
        speakMode: String? = nil,
        invitedSpeakers: [CallPerson] = [],
        canSpeak: Bool? = nil
    ) {
        self.callId = callId
        self.chatId = chatId
        self.projectId = projectId
        self.kind = kind
        self.room = room
        self.livekitUrl = livekitUrl
        self.token = token
        self.tokenExpiresAt = tokenExpiresAt
        self.startedBy = startedBy
        self.startedAt = startedAt
        self.endedAt = endedAt
        self.reason = reason
        self.participants = participants
        self.agents = agents
        self.reachable = reachable
        self.speakMode = speakMode
        self.invitedSpeakers = invitedSpeakers
        self.canSpeak = canSpeak
    }

    init(dto: CallDto, reachable: [CallReachability] = []) {
        self.init(
            callId: dto.call_id,
            chatId: dto.chat_id,
            projectId: dto.project_id.isEmpty ? peopleSlot : dto.project_id,
            kind: CallKind(rawValue: dto.kind) ?? .voice,
            room: dto.room,
            livekitUrl: dto.livekit_url ?? "",
            token: dto.token.flatMap { $0.isEmpty ? nil : $0 },
            tokenExpiresAt: dto.token_expires_at,
            startedBy: CallPerson(dto: dto.started_by),
            startedAt: dto.started_at,
            endedAt: dto.ended_at,
            reason: dto.reason,
            participants: dto.participants.map(CallPerson.init(dto:)),
            agents: dto.agents.map(CallPerson.init(dto:)),
            reachable: reachable,
            speakMode: dto.speak_mode.flatMap { $0.isEmpty ? nil : $0 },
            invitedSpeakers: (dto.invited_speakers ?? []).map(CallPerson.init(dto:)),
            canSpeak: dto.can_speak
        )
    }

    func isHost(_ userId: String?) -> Bool {
        guard let userId, let startedBy else { return false }
        return startedBy.id == userId
    }

    func isInvitedSpeaker(_ userId: String) -> Bool {
        invitedSpeakers.contains { $0.id == userId }
    }

    func speakRole(of userId: String) -> String? {
        guard isHostedSpeak else { return nil }
        if startedBy?.id == userId { return "Host" }
        if isInvitedSpeaker(userId) { return "Speaker" }
        return nil
    }

    static func parse(_ raw: Any?) -> Call? {
        guard let raw, JSONSerialization.isValidJSONObject(raw),
              let data = try? JSONSerialization.data(withJSONObject: raw) else { return nil }
        let decoder = JSONDecoder()
        if let env = try? decoder.decode(CallEnvelopeDto.self, from: data) {
            return Call(dto: env.call)
        }
        if let dto = try? decoder.decode(CallDto.self, from: data) {
            return Call(dto: dto)
        }
        NSLog("CallDto: could not decode call")
        return nil
    }
}

/// "Call ended · 12 min" / "Call ended".
func callEndedLine(durationS: Int) -> String {
    durationS < 60 ? "Call ended" : "Call ended · \(durationS / 60) min"
}

/// One row in the in-call participants list (joined tile or still ringing).
struct CallRosterEntry: Identifiable, Equatable {
    var id: String
    var person: CallPerson
    var isSelf: Bool
    var speaking: Bool
    var muted: Bool
    var ringing: Bool
    /// "Host" / "Speaker" when the call is hosted; otherwise `nil`.
    var role: String?

    var statusLabel: String {
        if ringing { return "Ringing" }
        if speaking { return "Speaking" }
        if muted { return "Muted" }
        return ""
    }
}

/// Pure builder: self first, then LiveKit tiles, then ringing invitees; no duplicate ids.
enum CallRoster {
    static func build(
        me: CallPerson?,
        meMuted: Bool,
        tiles: [(id: String, person: CallPerson, speaking: Bool, micOn: Bool)],
        ringing: [CallPerson],
        call: Call?
    ) -> [CallRosterEntry] {
        var out: [CallRosterEntry] = []
        var seen = Set<String>()
        if let me {
            seen.insert(me.id)
            out.append(CallRosterEntry(
                id: me.id,
                person: me,
                isSelf: true,
                speaking: false,
                muted: meMuted,
                ringing: false,
                role: call?.speakRole(of: me.id)
            ))
        }
        for tile in tiles {
            let pid = tile.person.id
            if seen.contains(pid) { continue }
            seen.insert(pid)
            out.append(CallRosterEntry(
                id: pid,
                person: tile.person,
                isSelf: false,
                speaking: tile.speaking,
                muted: !tile.micOn,
                ringing: false,
                role: call?.speakRole(of: pid)
            ))
        }
        for person in ringing {
            if seen.contains(person.id) { continue }
            seen.insert(person.id)
            out.append(CallRosterEntry(
                id: person.id,
                person: person,
                isSelf: false,
                speaking: false,
                muted: false,
                ringing: true,
                role: call?.speakRole(of: person.id)
            ))
        }
        return out
    }
}

/// How mute should touch LiveKit: mute/unmute the published mic track.
/// Never `setMicrophone(enabled: false)` — that path can reconfigure the audio engine
/// and stop remote playback while CallKit owns the session.
enum MicMutePlan: Equatable {
    /// Mute the existing local mic publication.
    case muteTrack
    /// Unmute the existing local mic publication.
    case unmuteTrack
    /// No publication yet and we want to be heard: publish once (`setMicrophone(true)` only).
    case publish
    /// No publication and we want silence: leave unpublished (do not call setMicrophone).
    case leaveUnpublished

    /// Pure plan for the next mute toggle / CallKit mute action.
    static func plan(wantMuted: Bool, hasMicPublication: Bool) -> MicMutePlan {
        if hasMicPublication {
            return wantMuted ? .muteTrack : .unmuteTrack
        }
        return wantMuted ? .leaveUnpublished : .publish
    }

    /// True only when this plan would call `setMicrophone(enabled: false)`.
    var callsSetMicrophoneFalse: Bool { false }
}
