// Plain Swift view of the Project doc and the Chat docs (arbos-design §5,
// crdt-sync §6). Mirrors `crates/arbos-mac/src/model.rs`; the sync layer
// fills these in and the views only read them.

import Foundation

typealias Id = String

enum BoxState: String, Codable, Equatable {
    case awake
    case waking
    case hibernatedLight = "hibernated_light"
    case hibernatedDeep = "hibernated_deep"
    case stoppedByUser = "stopped_by_user"
    case error

    var isAsleep: Bool {
        switch self {
        case .hibernatedLight, .hibernatedDeep, .stoppedByUser: return true
        case .awake, .waking, .error: return false
        }
    }

    /// Maps the relay's registry `state` plus the live `box` field.
    static func from(state: String, box: String) -> BoxState {
        switch state {
        case "awake": return .awake
        case "waking": return .waking
        case "hibernated_light": return .hibernatedLight
        case "hibernated_deep": return .hibernatedDeep
        case "stopped_by_user": return .stoppedByUser
        case "error": return .error
        default:
            switch box {
            case "awake": return .awake
            case "waking": return .waking
            case "error": return .error
            default: return .hibernatedLight
            }
        }
    }

    /// Wire `box` values: awake | waking | asleep | error (crdt-sync §8.2).
    static func fromWire(_ s: String) -> BoxState? {
        switch s {
        case "awake": return .awake
        case "waking": return .waking
        case "asleep": return .hibernatedLight
        case "error": return .error
        default: return nil
        }
    }
}

enum Effort: String, Codable, CaseIterable, Identifiable {
    case low, medium, high

    var id: String { rawValue }

    var label: String {
        switch self {
        case .low: return "Low"
        case .medium: return "Medium"
        case .high: return "High"
        }
    }

    static func from(_ s: String?) -> Effort {
        switch s ?? "" {
        case "high", "xhigh", "max": return .high
        case "medium": return .medium
        default: return .low
        }
    }
}

/// The user's one cloud computer (D70); projects are workspaces on it.
/// Reported by `GET /machine`; the project docs mirror `status.box
/// {state, since}` and `status.workspace {state, since}`.
struct Machine: Equatable {
    var state: BoxState
    var since: Int64?
    /// Human spec line, e.g. "Daytona · 4 vCPU · 8 GiB · 10 GiB disk · arbos-agent-0.2.0".
    var spec: String
    var provider: String = ""
    var sandboxId: String?
    var spendTodayUsd: Double = 0
    var spendCapUsd: Double = 0
    /// Workspace state per project id.
    var workspaces: [Id: WorkspaceState] = [:]
    /// `always_on` (D73): the relay never stops this box, so it is only ever
    /// awake or in error — no waking/sleeping states, and machine spend is
    /// informational (not taken from credits). False when the field is absent.
    var alwaysOn = false
    /// A manual Stop on an always-on box: the relay's sweep leaves it down
    /// until Start or a message. The one case an always-on box shows Stopped.
    var held = false
    /// Machine time today (`machine_today_usd`), its own ledger bucket — not
    /// part of credits or `spendTodayUsd`.
    var machineTodayUsd: Double = 0
    /// False until the relay has answered `/machine` once.
    var reported = false
    /// The relay's own `state` word before the always-on collapse (D82 first-login
    /// stages need the real waking → awake transition).
    var rawState: BoxState = .hibernatedLight
    /// `spec.cpu` / `spec.mem_gib` and `image.current` as reported, for the
    /// first-login detail lines ("Starting a 4-core machine with 8 GB of memory").
    var cpu: Int? = nil
    var memGib: Int? = nil
    var image: String? = nil

    static let unknown = Machine(state: .hibernatedLight, since: nil, spec: "")

    /// The state the UI may show: always-on boxes collapse everything but
    /// error onto awake (unless a manual Stop holds the box down).
    func coerced(_ s: BoxState) -> BoxState {
        alwaysOn && !held && s != .error ? .awake : s
    }

    var label: String {
        switch state {
        case .awake: return "Awake"
        case .waking: return "Waking…"
        case .hibernatedLight, .hibernatedDeep: return "Sleeping"
        case .stoppedByUser: return "Stopped"
        case .error: return "Needs attention"
        }
    }

    static func specLine(cpu: Int?, memGib: Int?, diskGib: Int?, klass: String?, provider: String?) -> String {
        var parts: [String] = []
        if let provider, !provider.isEmpty { parts.append(provider.prefix(1).uppercased() + provider.dropFirst()) }
        if let cpu { parts.append("\(cpu) vCPU") }
        if let memGib { parts.append("\(memGib) GiB") }
        if let diskGib { parts.append("\(diskGib) GiB disk") }
        if let klass, !klass.isEmpty { parts.append(klass) }
        return parts.joined(separator: " · ")
    }
}

/// `status.workspace.state` of a project doc: the project's workspace on the machine.
enum WorkspaceState: String, Codable, Equatable {
    case active, idle, error
    case unknown

    /// D70 states: running | starting | stopped | error (older docs: active/idle).
    static func from(_ raw: String?) -> WorkspaceState {
        switch raw ?? "" {
        case "running", "starting", "active", "awake", "busy": return .active
        case "stopped", "idle", "asleep", "sleeping": return .idle
        case "error", "failed": return .error
        default: return .unknown
        }
    }
}

struct Project: Identifiable, Equatable, Codable {
    var id: Id
    var name: String
    /// One glyph (or two-letter monogram), drawn in the tile.
    var icon: String
    /// Hex `#rrggbb` accent for the selected tile border.
    var color: String
    var state: BoxState
    /// Relay `box_since` (ms); lets a wake that never finished age out.
    var stateSince: Int64?
    /// Human text for the S12/S13 banner, e.g. "Restoring from archive…".
    var stateDetail: String?
    var model: String
    var effort: Effort
    var spendTodayUsd: Double
    var spendCapUsd: Double
    /// `settings.modelNote` from the project doc; empty when every id is valid.
    var modelNote: String = ""
    /// `status.workspace.state` from the project doc; `.unknown` until the box writes it.
    var workspace: WorkspaceState = .unknown
    /// D72 step 3: a project someone else shared with me.
    var shared: Bool = false
    /// Owner's user id (`gh_…`) when the relay sends `owner` / `owner_id`; used to open their profile.
    var ownerId: String? = nil
    var ownerHandle: String? = nil
    /// Tree-character slug chosen by the owner (`avatar` on the relay); nil → hashed.
    var avatar: String? = nil
    /// Colour set name from the manifest (`avatar_color` on the relay); nil → mono.
    var avatarColor: String? = nil
    /// D79: the owner's palette pick (`avatar_color_index`, 0–6) and the relay's
    /// `avatar_color_index_effective`; nil → derived from the id (AvatarPalette.index).
    var avatarColorIndex: Int? = nil
    var avatarColorIndexEffective: Int? = nil
    var effectiveAvatarColorIndex: Int? { AvatarPalette.clamp(avatarColorIndexEffective) ?? AvatarPalette.clamp(avatarColorIndex) }
    /// Private standing instructions (owner only).
    var instructions: String = ""
    /// §9 public one-paragraph description (≤500), `PATCH {description}`.
    var description: String = ""
    /// §9 owner as sender{} with presence.
    var ownerName: String? = nil
    var ownerAvatarUrl: String? = nil
    var ownerLastSeenAt: Int64? = nil
    var ownerOnline: Bool = false
    /// The relay's `primary_chat` when it sends one; else the app picks (see AppModel.primaryChat).
    var primaryChatId: Id? = nil
    /// §10.3 call voice id (`voice` on the relay); nil = the relay's default voice.
    var voice: String? = nil
    /// §11 camera vision in agent calls (`vision_enabled`, default on).
    var visionEnabled: Bool = true
    /// §9 machine summary: awake|waking|asleep|error, class, snapshot, always_on.
    var machineState: String? = nil
    var machineClass: String? = nil
    var machineSnapshot: String? = nil
    var machineAlwaysOn: Bool = false

    var shortModel: String { Project.shortModel(model) }

    static func shortModel(_ id: String) -> String {
        id.split(separator: "/").last.map(String.init) ?? id
    }
}

struct Chat: Identifiable, Equatable, Codable {
    var id: Id
    var projectId: Id
    var title: String
    var isPinned: Bool
    var unread: Bool
    var lastMessageAt: Int64
    /// `chat` (a plain thread), `group` (an agent-hosted group chat other
    /// projects/people were added to — F6), `projects` (the system chat).
    var kind: String = "chat"
    /// Other projects in an agent-hosted group.
    var participants: [Id] = []

    var isGroup: Bool { kind == "group" }
    /// The agent's "projects" system chat (§9b: shown even with multi-chat off).
    var isSystem: Bool { kind == "projects" }
}

/// §10.3 one entry of `GET /voices`.
struct Voice: Identifiable, Equatable {
    var id: String
    var name: String
    var description: String?
    var seconds: Double?
    var sampleURL: URL?
}

struct VoiceCatalog: Equatable {
    var defaultId: String
    var voices: [Voice]
}

extension Chat: Hashable {
    func hash(into hasher: inout Hasher) { hasher.combine(id) }
}

enum Role: String, Codable {
    case user, assistant, system, project
}

enum MessageStatus: String, Codable {
    case queued, streaming, done, error, cancelled
}

enum TurnKind: String, Codable {
    case reply, act, new, spawn, wake
    case subagentResult = "subagent_result"
    case other

    /// Progress messages carry status events only; no prose is expected.
    var isProgress: Bool { self == .spawn || self == .wake }
}

enum StatusKind: String, Codable {
    case deciding, working, step, queued, done, failed, stopped
    case askUser = "waiting_user"
    case handoff = "handed_off"
    case note
    case other
}

struct ToolCall: Codable, Equatable {
    var id: Id
    var name: String
}

struct Attachment: Codable, Equatable {
    var name: String
    var mime: String
    var blobRef: String?
    /// Producer hint (`image`, `audio`, …); clients may also infer from mime/name.
    var kind: String? = nil

    enum CodingKeys: String, CodingKey {
        case name, mime, kind
        case blobRef = "ref"
    }

    /// True for a still image the transcript should show as a tappable preview.
    var isImage: Bool {
        if let kind, kind.lowercased() == "image" { return true }
        if mime.lowercased().hasPrefix("image/") { return true }
        let ext: String = {
            guard let dot = name.lastIndex(of: ".") else { return "" }
            return String(name[name.index(after: dot)...]).lowercased()
        }()
        return ["png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "bmp", "tif", "tiff"].contains(ext)
    }
}

/// One answer row of a `choice` / `multi_select` question widget.
struct ChoiceOption: Equatable, Hashable {
    var id: String
    var label: String
}

/// A `question` part (design §10 widgets): what the agent asked, the answer
/// rows when the widget is a choice, and whether it is still open. Widgets
/// the phone cannot draw (text, date, form) carry no options; the composer
/// answers those.
struct Question: Equatable {
    var questionId: String
    var question: String
    var options: [ChoiceOption]
    var multi: Bool
    /// `widget.type` as written, "" for a plain question.
    var widgetType: String
    /// Picked option ids (or the typed text) once answered.
    var answer: [String]
    var open: Bool
    /// Set when this "question" is really an S18 approval card (`questionId` = `approval:<id>`).
    var approvalId: String? = nil
    /// D83: the relay request the card decides (`payload.secretRequestId`).
    var secretRequestId: String? = nil

    var hasChoices: Bool { !options.isEmpty }
}

/// One row of `messages[].parts` (crdt-sync §6.2). Unknown kinds decode to
/// `.unknown` and render as nothing, per the "additive only" rule.
enum Part: Equatable {
    case text(id: Id, text: String)
    case reasoning(id: Id)
    case tool(id: Id, call: ToolCall, isError: Bool)
    case attachment(id: Id, attachment: Attachment)
    case error(id: Id, code: String, message: String)
    case status(id: Id, code: StatusKind, message: String)
    case question(id: Id, question: Question)
    case subagentResult(id: Id)
    case unknown(id: Id)

    var id: Id {
        switch self {
        case .text(let id, _), .reasoning(let id), .tool(let id, _, _), .attachment(let id, _),
             .error(let id, _, _), .status(let id, _, _), .question(let id, _), .subagentResult(let id), .unknown(let id):
            return id
        }
    }

    var questionValue: Question? {
        if case .question(_, let q) = self { return q }
        return nil
    }

    var isStatus: Bool {
        if case .status = self { return true }
        return false
    }

    var textValue: String? {
        if case .text(_, let t) = self { return t }
        return nil
    }
}

/// One emoji reaction on a message: who picked this emoji (sender ids like `user:…`).
/// Several different emojis can sit on the same message at once.
struct MessageReaction: Equatable, Hashable {
    var emoji: String
    var senders: [String]

    /// iMessage-style set that renders cleanly in SwiftUI context menus.
    static let palette: [String] = ["👍", "❤️", "😂", "😮", "😢", "🙏"]

    /// Toggle `sender` on/off for `emoji`, keeping other emoji reactions intact.
    static func toggling(_ list: [MessageReaction], emoji: String, sender: String) -> [MessageReaction] {
        var byEmoji: [String: [String]] = [:]
        for r in list where !r.emoji.isEmpty && !r.senders.isEmpty {
            byEmoji[r.emoji] = r.senders
        }
        var people = byEmoji[emoji] ?? []
        if let i = people.firstIndex(of: sender) {
            people.remove(at: i)
        } else {
            people.append(sender)
        }
        if people.isEmpty {
            byEmoji.removeValue(forKey: emoji)
        } else {
            byEmoji[emoji] = people
        }
        return MessageReaction.palette.compactMap { e in
            guard let s = byEmoji[e], !s.isEmpty else { return nil }
            return MessageReaction(emoji: e, senders: s)
        } + byEmoji.keys.filter { !MessageReaction.palette.contains($0) }.sorted().compactMap { e in
            guard let s = byEmoji[e], !s.isEmpty else { return nil }
            return MessageReaction(emoji: e, senders: s)
        }
    }
}

struct Message: Identifiable, Equatable {
    var id: Id
    var chatId: Id
    var role: Role
    var sender: String
    var status: MessageStatus
    var turn: TurnKind?
    var decision: String?
    var subagentId: String?
    var subagentName: String?
    /// Messages exchanged with the subagent on a hand-off row (1 = brief sent, 2 = result back).
    var handoffMessages: Int? = nil
    var inReplyTo: String?
    var usage: [String: Double]
    var model: String?
    var createdAt: Int64
    var completedAt: Int64?
    /// True until the relay `ACK`s the client's push (S3 faint echo). Client-only.
    var pending: Bool
    var parts: [Part]
    /// Count of parts the box flagged `hidden: true` (tool details).
    var hiddenCount: Int
    // People chats (D72): who wrote it, who an agent answered, what was mentioned.
    var senderHandle: String? = nil
    var senderName: String? = nil
    var senderAvatar: String? = nil
    var ownerHandle: String? = nil
    var askedByHandle: String? = nil
    var mentions: [String] = []
    var replyTo: String? = nil
    /// What the agent is doing right now ("Cloning RaoFoundation/subtensor"),
    /// written by the harness while the turn runs; the working line shows it.
    var activity: String? = nil
    /// Voice-call transcript line (`voice: true` / `source: "voice"` in the doc).
    /// Kept in the chat doc for agent context; the transcript UI does not render it.
    var voice: Bool = false
    /// D82: the agent's canned first line, written by the device that created the
    /// project (`greeting: true`); written once per chat, never treated as user text.
    var greeting: Bool = false
    /// Emoji reactions on this message (`reactions` map in the chat doc). Multiple emojis allowed.
    var reactions: [MessageReaction] = []

    var isStreaming: Bool { status == .streaming }

    var plainText: String {
        parts.compactMap(\.textValue).joined(separator: "\n")
    }

    var textLength: Int {
        parts.compactMap(\.textValue).reduce(0) { $0 + $1.utf8.count }
    }

    var askUserText: String? {
        for part in parts {
            if case .status(_, .askUser, let message) = part { return message }
        }
        return nil
    }

    /// The `question` parts of this message (agentd writes one per `ask_user`).
    var questions: [Question] { parts.compactMap(\.questionValue) }

    /// The newest open question, when one exists.
    var openQuestion: Question? { questions.last { $0.open } }

    var hasErrorPart: Bool {
        parts.contains { if case .error = $0 { return true } else { return false } }
    }
}

/// Connection to the relay, S14.
enum Link: Equatable {
    case connecting, live, reconnecting
}

func nowMs() -> Int64 {
    Int64(Date().timeIntervalSince1970 * 1000)
}

// MARK: - JSON decoding of doc rows

/// Decodes a `messages[i]` map (as Foundation JSON) into a `Message`.
/// Tolerant: every field but `id` has a default, so the schema stays additive.
enum DocJSON {
    static func message(_ raw: Any, chatId: Id) -> Message? {
        guard let m = raw as? [String: Any], let id = m["id"] as? String else { return nil }
        let role = Role(rawValue: str(m["role"]) ?? "") ?? .assistant
        let status = MessageStatus(rawValue: str(m["status"]) ?? "") ?? .done
        var turn: TurnKind? = nil
        if let t = m["turn"] as? String {
            turn = TurnKind(rawValue: t) ?? .other
        } else if let t = m["turn"] as? [String: Any], let k = t["kind"] as? String {
            turn = TurnKind(rawValue: k) ?? .other
        }
        var parts: [Part] = []
        var hidden = 0
        for p in (m["parts"] as? [Any]) ?? [] {
            guard let pm = p as? [String: Any] else { continue }
            if (pm["hidden"] as? Bool) == true || (pm["hidden"] as? NSNumber)?.boolValue == true {
                hidden += 1
                continue
            }
            parts.append(part(pm))
        }
        // Core projected rows put hidden tool/etc. parts in `hiddenParts` instead
        // of `hidden: true` inside `parts` (read_rows_json / project_hidden_parts).
        if let hp = m["hiddenParts"] as? [Any] {
            hidden += hp.count
        }
        var usage: [String: Double] = [:]
        if let u = m["usage"] as? [String: Any] {
            for (k, v) in u {
                if let n = v as? NSNumber { usage[k] = n.doubleValue }
            }
        }
        return Message(
            id: id,
            chatId: str(m["chatId"]) ?? chatId,
            role: role,
            sender: str(m["sender"]) ?? "",
            status: status,
            turn: turn,
            decision: str(m["decision"]),
            subagentId: str(m["subagentId"]),
            subagentName: str(m["subagentName"]),
            handoffMessages: int(m["handoffMessages"]).map { Int($0) },
            inReplyTo: str(m["inReplyTo"]),
            usage: usage,
            model: str(m["model"]),
            createdAt: int(m["createdAt"]) ?? 0,
            completedAt: int(m["completedAt"]),
            pending: false,
            parts: parts,
            hiddenCount: hidden,
            senderHandle: str(m["senderHandle"]),
            senderName: str(m["senderName"]),
            senderAvatar: str(m["senderAvatar"]),
            ownerHandle: str(m["ownerHandle"]),
            askedByHandle: str(m["askedByHandle"]),
            mentions: (m["mentions"] as? [Any])?.compactMap { $0 as? String } ?? [],
            replyTo: str(m["replyTo"]),
            activity: str(m["activity"]).flatMap { $0.isEmpty ? nil : $0 },
            voice: bool(m["voice"]) || str(m["source"]) == "voice",
            greeting: bool(m["greeting"]),
            reactions: reactions(m["reactions"])
        )
    }

    /// `reactions` is flat `"emoji|sender" → true` (arbos-core) and/or legacy
    /// emoji → map of sender id → true. Unknown shapes are ignored.
    static func reactions(_ raw: Any?) -> [MessageReaction] {
        guard let map = raw as? [String: Any] else { return [] }
        var byEmoji: [String: [String]] = [:]
        for (key, v) in map {
            guard !key.isEmpty else { continue }
            if let people = v as? [String: Any] {
                // Legacy nested bucket. Merge into any flat-key senders already
                // collected for this emoji (core project_reactions does the same).
                for (sid, flag) in people {
                    guard !sid.isEmpty, bool(flag) else { continue }
                    byEmoji[key, default: []].append(sid)
                }
                continue
            }
            // Flat key: "emoji|sender" → true.
            guard bool(v), let sep = key.firstIndex(of: "|") else { continue }
            let emoji = String(key[..<sep])
            let sid = String(key[key.index(after: sep)...])
            guard !emoji.isEmpty, !sid.isEmpty else { continue }
            byEmoji[emoji, default: []].append(sid)
        }
        byEmoji = byEmoji.mapValues { Array(Set($0)).sorted() }
        let ordered = MessageReaction.palette.compactMap { e -> MessageReaction? in
            guard let s = byEmoji[e] else { return nil }
            return MessageReaction(emoji: e, senders: s)
        }
        let rest = byEmoji.keys.filter { !MessageReaction.palette.contains($0) }.sorted().compactMap { e -> MessageReaction? in
            guard let s = byEmoji[e] else { return nil }
            return MessageReaction(emoji: e, senders: s)
        }
        return ordered + rest
    }

    static func part(_ pm: [String: Any]) -> Part {
        let id = str(pm["id"]) ?? UUID().uuidString
        switch str(pm["kind"]) ?? "" {
        case "text":
            return .text(id: id, text: str(pm["text"]) ?? "")
        case "reasoning":
            return .reasoning(id: id)
        case "tool":
            let call = pm["call"] as? [String: Any]
            return .tool(
                id: id,
                call: ToolCall(id: str(call?["id"]) ?? "", name: str(call?["name"]) ?? "tool"),
                isError: (pm["isError"] as? Bool) ?? false
            )
        case "attachment":
            // Nested `{ attachment: { name, mime, ref } }` is the schema; tolerate a
            // flat part map if a writer omitted the wrapper.
            let a = (pm["attachment"] as? [String: Any]) ?? pm
            return .attachment(
                id: id,
                attachment: Attachment(
                    name: str(a["name"]) ?? "attachment",
                    mime: str(a["mime"]) ?? "application/octet-stream",
                    blobRef: str(a["ref"]),
                    kind: str(a["kind"]).flatMap { $0 == "attachment" ? nil : $0 }
                )
            )
        case "error":
            return .error(id: id, code: str(pm["code"]) ?? "error", message: str(pm["message"]) ?? "")
        case "status":
            let code = StatusKind(rawValue: str(pm["code"]) ?? "") ?? .other
            return .status(id: id, code: code, message: str(pm["message"]) ?? "")
        case "question":
            return .question(id: id, question: question(pm))
        case "approval":
            return .question(id: id, question: approval(pm))
        case "subagent_result":
            return .subagentResult(id: id)
        default:
            return .unknown(id: id)
        }
    }

    /// `{ questionId, question, widget?: { type, options? }, answer?, state }`.
    static func question(_ pm: [String: Any]) -> Question {
        let widget = pm["widget"] as? [String: Any]
        let type = str(widget?["type"]) ?? ""
        var options: [ChoiceOption] = []
        if type == "choice" || type == "multi_select" {
            for o in (widget?["options"] as? [Any]) ?? [] {
                guard let om = o as? [String: Any], let oid = str(om["id"]) else { continue }
                options.append(ChoiceOption(id: oid, label: str(om["label"]) ?? oid))
            }
        }
        var answer: [String] = []
        switch pm["answer"] {
        case let s as String: answer = [s]
        case let list as [Any]: answer = list.compactMap { $0 as? String }
        case let n as NSNumber: answer = [n.stringValue]
        default: break
        }
        return Question(
            questionId: str(pm["questionId"]) ?? id(pm),
            question: str(pm["question"]) ?? "",
            options: options,
            multi: type == "multi_select",
            widgetType: type,
            answer: answer,
            open: (str(pm["state"]) ?? "open") != "answered"
        )
    }

    /// An S18 approval part as a question card (D76): the title is the question; the answers
    /// are the card's four rows for a secret request (`payload.answers`), else Approve / Reject.
    /// `decidedBy` carries the relay's decision (`arbos:secrets:allowed_always`) so an answered
    /// card can show which row was picked.
    static func approval(_ pm: [String: Any]) -> Question {
        let approvalId = str(pm["approvalId"]) ?? id(pm)
        let payload = pm["payload"] as? [String: Any]
        var options: [ChoiceOption] = []
        for o in (payload?["answers"] as? [Any]) ?? [] {
            guard let om = o as? [String: Any], let oid = str(om["id"]) else { continue }
            options.append(ChoiceOption(id: oid, label: str(om["label"]) ?? oid))
        }
        if options.isEmpty {
            options = [ChoiceOption(id: "approve", label: "Approve"), ChoiceOption(id: "reject", label: "Reject")]
        }
        let state = str(pm["state"]) ?? "pending"
        let decidedBy = str(pm["decidedBy"]) ?? ""
        var picked: [String] = []
        switch state {
        case "approved":
            if decidedBy.hasSuffix(":allowed_once") { picked = ["once"] }
            else if decidedBy.hasSuffix(":allowed_chat") { picked = ["chat"] }
            else if decidedBy.hasSuffix(":allowed_always") { picked = ["always"] }
            else if options.contains(where: { $0.id == "approve" }) { picked = ["approve"] }
            else if options.count == 4 { picked = ["always"] }
        case "rejected":
            picked = options.contains(where: { $0.id == "deny" }) ? ["deny"] : ["reject"]
        default:
            break
        }
        var title = (str(pm["title"]) ?? "Approve this?").trimmingCharacters(in: .whitespaces)
        if !title.hasSuffix("?") {
            while title.hasSuffix(".") { title.removeLast() }
            title += "?"
        }
        return Question(
            questionId: "approval:\(approvalId)",
            question: title,
            options: options,
            multi: false,
            widgetType: "approval",
            answer: picked,
            open: state == "pending",
            approvalId: approvalId,
            secretRequestId: str(payload?["secretRequestId"])
        )
    }

    private static func id(_ pm: [String: Any]) -> String {
        str(pm["id"]) ?? UUID().uuidString
    }

    static func str(_ v: Any?) -> String? {
        if let s = v as? String { return s }
        return nil
    }

    static func bool(_ v: Any?) -> Bool {
        if let b = v as? Bool { return b }
        if let n = v as? NSNumber { return n.boolValue }
        return false
    }

    static func int(_ v: Any?) -> Int64? {
        if let n = v as? NSNumber { return n.int64Value }
        if let i = v as? Int64 { return i }
        if let i = v as? Int { return Int64(i) }
        if let d = v as? Double { return Int64(d) }
        return nil
    }
}

/// `connectors.status` as the relay reports it (F1).
enum ConnectorStatus: String, Equatable {
    case connected
    case needsAuth = "needs_auth"
    case error
    case other

    init(wire: String) {
        switch wire {
        case "connected": self = .connected
        case "needs_auth": self = .needsAuth
        case "error": self = .error
        default: self = .other
        }
    }

    var label: String {
        switch self {
        case .connected: return "Connected"
        case .needsAuth: return "Needs sign-in"
        case .error: return "Error"
        case .other: return "Unknown"
        }
    }
}

/// One connected account (`GET /connectors` item). Never carries a token.
struct Connector: Identifiable, Equatable {
    var id: Id
    var provider: String
    var label: String
    var status: ConnectorStatus
    var lastError: String?
    var mock: Bool
}

/// One catalog entry (`GET /connectors/catalog`).
struct ConnectorProvider: Identifiable, Equatable {
    var id: String { provider }
    var provider: String
    var label: String
    /// `oauth | oauth_or_token | token`.
    var connect: String
    var oauthConfigured: Bool

    var hasOauth: Bool { connect == "oauth" || connect == "oauth_or_token" }
    var acceptsToken: Bool { connect == "token" || connect == "oauth_or_token" }
}

/// Settings → Connectors state.
struct ConnectorsView: Equatable {
    var mock = false
    var providers: [ConnectorProvider] = []
    var items: [Connector] = []
    var loaded = false

    func connector(for provider: String) -> Connector? {
        items.first { $0.provider == provider }
    }
}

// MARK: - Goals (long-running objectives, feedback #1091)

/// One step in a goal's plan (`plan[]`).
struct GoalStep: Identifiable, Equatable {
    var id: String
    var title: String
    /// `todo | doing | done | skipped` (free-form; UI treats unknown as todo).
    var status: String

    static func parse(_ v: Any?) -> GoalStep? {
        if let s = v as? String {
            let t = s.trimmingCharacters(in: .whitespaces)
            guard !t.isEmpty else { return nil }
            return GoalStep(id: t, title: t, status: "todo")
        }
        guard let m = v as? [String: Any] else { return nil }
        let title = DocJSON.str(m["title"]) ?? DocJSON.str(m["text"]) ?? ""
        guard !title.isEmpty else { return nil }
        return GoalStep(
            id: DocJSON.str(m["id"]) ?? title,
            title: title,
            status: DocJSON.str(m["status"]) ?? "todo"
        )
    }
}

/// One dated progress note (`progress_log[]`).
struct GoalLogEntry: Identifiable, Equatable {
    var id: String { "\(at)-\(text.prefix(24))" }
    var at: Int64
    var text: String

    static func parse(_ v: Any?) -> GoalLogEntry? {
        guard let m = v as? [String: Any], let text = DocJSON.str(m["text"]), !text.isEmpty else { return nil }
        return GoalLogEntry(at: DocJSON.int(m["at"]) ?? 0, text: text)
    }
}

/// A long-running objective the agent tracks across days.
struct GoalItem: Identifiable, Equatable {
    var id: String
    var projectId: String
    var projectName: String?
    var chatId: String?
    var title: String
    var outcome: String
    var plan: [GoalStep]
    /// `active | blocked | waiting | paused | done`
    var status: String
    var progressLog: [GoalLogEntry]
    var nextStep: String
    var checkInCron: String?
    var checkInTz: String
    var routineId: String?
    /// `ask | suggest | act`
    var autonomy: String
    var linkedChatIds: [String]
    var linkedSubagentIds: [String]
    var waitingQuestion: String?
    var createdAt: Int64
    var updatedAt: Int64
    var lastMovedAt: Int64?

    var isOpen: Bool {
        switch status {
        case "active", "blocked", "waiting": return true
        default: return false
        }
    }

    var statusLabel: String {
        switch status {
        case "active": return "Active"
        case "blocked": return "Blocked"
        case "waiting": return "Waiting on you"
        case "paused": return "Paused"
        case "done": return "Done"
        default: return status.capitalized
        }
    }

    var autonomyLabel: String {
        switch autonomy {
        case "act": return "Act freely"
        case "suggest": return "Suggest first"
        default: return "Ask first"
        }
    }

    /// Human check-in cadence from the cron, when we recognize common presets.
    var checkInLabel: String {
        guard let c = checkInCron?.trimmingCharacters(in: .whitespaces), !c.isEmpty else {
            return "Manual"
        }
        switch c {
        case "0 9 * * *": return "Every day"
        case "0 9 * * 1-5": return "Weekdays"
        case "0 9 * * 1": return "Weekly"
        default: return c
        }
    }

    static func parse(_ v: Any?) -> GoalItem? {
        guard let m = v as? [String: Any], let id = DocJSON.str(m["id"]) else { return nil }
        return GoalItem(
            id: id,
            projectId: DocJSON.str(m["project_id"]) ?? "",
            projectName: DocJSON.str(m["project_name"]),
            chatId: DocJSON.str(m["chat_id"]),
            title: DocJSON.str(m["title"]) ?? id,
            outcome: DocJSON.str(m["outcome"]) ?? "",
            plan: ((m["plan"] as? [Any]) ?? []).compactMap(GoalStep.parse),
            status: DocJSON.str(m["status"]) ?? "active",
            progressLog: ((m["progress_log"] as? [Any]) ?? []).compactMap(GoalLogEntry.parse),
            nextStep: DocJSON.str(m["next_step"]) ?? "",
            checkInCron: DocJSON.str(m["check_in_cron"]),
            checkInTz: DocJSON.str(m["check_in_tz"]) ?? "UTC",
            routineId: DocJSON.str(m["routine_id"]),
            autonomy: DocJSON.str(m["autonomy"]) ?? "ask",
            linkedChatIds: (m["linked_chat_ids"] as? [Any])?.compactMap { $0 as? String } ?? [],
            linkedSubagentIds: (m["linked_subagent_ids"] as? [Any])?.compactMap { $0 as? String } ?? [],
            waitingQuestion: DocJSON.str(m["waiting_question"]),
            createdAt: DocJSON.int(m["created_at"]) ?? 0,
            updatedAt: DocJSON.int(m["updated_at"]) ?? 0,
            lastMovedAt: DocJSON.int(m["last_moved_at"])
        )
    }
}

func newId() -> Id {
    ULID.generate()
}

/// Crockford base32 ULID: 48-bit ms timestamp + 80 random bits (26 chars).
enum ULID {
    private static let alphabet = Array("0123456789ABCDEFGHJKMNPQRSTVWXYZ")

    static func generate() -> String {
        var out = [Character](repeating: "0", count: 26)
        var t = UInt64(Date().timeIntervalSince1970 * 1000)
        for i in stride(from: 9, through: 0, by: -1) {
            out[i] = alphabet[Int(t & 0x1f)]
            t >>= 5
        }
        var rng = SystemRandomNumberGenerator()
        for i in 10..<26 {
            out[i] = alphabet[Int(rng.next() % 32)]
        }
        return String(out)
    }
}
