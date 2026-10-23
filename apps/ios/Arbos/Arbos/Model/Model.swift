// Plain Swift view of the Project doc and the Chat docs (arbos-design §5,
// crdt-sync §6). Mirrors `crates/arbos-mac/src/model.rs`; the sync layer
// fills these in and the views only read them.

import Foundation
import ArbosFFI

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

    /// Memberwise init (kept explicit because `init(dto:)` would suppress synthesis).
    init(
        state: BoxState,
        since: Int64?,
        spec: String,
        provider: String = "",
        sandboxId: String? = nil,
        spendTodayUsd: Double = 0,
        spendCapUsd: Double = 0,
        workspaces: [Id: WorkspaceState] = [:],
        alwaysOn: Bool = false,
        held: Bool = false,
        machineTodayUsd: Double = 0,
        reported: Bool = false,
        rawState: BoxState = .hibernatedLight,
        cpu: Int? = nil,
        memGib: Int? = nil,
        image: String? = nil
    ) {
        self.state = state
        self.since = since
        self.spec = spec
        self.provider = provider
        self.sandboxId = sandboxId
        self.spendTodayUsd = spendTodayUsd
        self.spendCapUsd = spendCapUsd
        self.workspaces = workspaces
        self.alwaysOn = alwaysOn
        self.held = held
        self.machineTodayUsd = machineTodayUsd
        self.reported = reported
        self.rawState = rawState
        self.cpu = cpu
        self.memGib = memGib
        self.image = image
    }

    init(dto: MachineStatusDto) {
        var workspaces: [Id: WorkspaceState] = [:]
        for w in dto.workspaces {
            workspaces[w.project_id] = WorkspaceState.from(w.state)
        }
        let wire = BoxState.fromWire(dto.state) ?? .hibernatedLight
        self.init(
            state: dto.always_on && !dto.held && wire != .error ? .awake : wire,
            since: dto.since,
            spec: Machine.specLine(
                cpu: Int(dto.spec.cpu),
                memGib: Int(dto.spec.mem_gib),
                diskGib: Int(dto.spec.disk_gib),
                klass: dto.spec.`class`,
                provider: dto.provider
            ),
            provider: dto.provider ?? "",
            sandboxId: dto.sandbox_id,
            spendTodayUsd: dto.spend_today_usd,
            spendCapUsd: dto.spend_cap_usd,
            workspaces: workspaces,
            alwaysOn: dto.always_on,
            held: dto.held,
            machineTodayUsd: dto.machine_today_usd,
            reported: true,
            rawState: wire,
            cpu: Int(dto.spec.cpu),
            memGib: Int(dto.spec.mem_gib),
            image: dto.image.current ?? (dto.spec.`class`.isEmpty ? nil : dto.spec.`class`)
        )
    }

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

    /// Settings › Machine › "Where it runs" (D89): the host sandbox is "Hosted on arbos.life";
    /// other providers show their name. "Change machine" (GPU, your computer) comes later.
    var whereItRuns: String {
        switch provider {
        case "gvisor": return "Hosted on arbos.life"
        case "daytona": return "Hosted · Daytona"
        case "process", "docker": return "Relay host"
        case "": return "Hosted on arbos.life"
        default: return provider.prefix(1).uppercased() + provider.dropFirst()
        }
    }

    static func specLine(cpu: Int?, memGib: Int?, diskGib: Int?, klass: String?, provider: String?) -> String {
        var parts: [String] = []
        if let provider, !provider.isEmpty, provider != "gvisor" { parts.append(provider.prefix(1).uppercased() + provider.dropFirst()) }
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
    /// D92: the owner's leaf picks (`avatar_leaf` 0–8, `avatar_leaf_color` 0–9); nil → derived
    /// from the id by the shared rule (`LeafArt.pick`), the same pair the relay reports as
    /// `avatar_leaf_effective` / `avatar_leaf_color_effective`.
    var avatarLeaf: Int? = nil
    var avatarLeafColor: Int? = nil
    /// D92 amendment: the agent's uploaded / generated picture — a blob name in the project's own
    /// slot (`/blob/{id}/{key}`); wins over the leaf while set.
    var imageKey: String? = nil
    /// `imageKey` as a fetchable URL (the relay's `/blob/{id}/{key}?token=…`), filled in by the
    /// relay client; nil in the fixture.
    var imageURL: String? = nil
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
    /// "Who may message X" (agent-to-agent messaging): other owners' people / agents the
    /// owner allowed to message this agent. Empty unless I own it.
    var messageGrants: [MessageGrant] = []

    var shortModel: String { Project.shortModel(model) }

    static func shortModel(_ id: String) -> String {
        id.split(separator: "/").last.map(String.init) ?? id
    }
}

extension Project {
    /// Maps a relay `ProjectDto` the same way the old hand JSON parse did.
    init(dto: ProjectDto) {
        let ownerHandle = dto.owner_handle.flatMap { $0 } ?? dto.owner.handle
        self.id = dto.id
        self.name = dto.name.isEmpty ? "Project" : dto.name
        self.icon = dto.icon.isEmpty ? "Ω" : dto.icon
        self.color = dto.color.isEmpty ? "#8b7cf6" : dto.color
        self.state = Arbos.BoxState.from(state: dto.state, box: dto.box)
        self.stateSince = dto.box_since
        self.stateDetail = nil
        self.model = dto.model ?? "anthropic/claude-haiku-5.5"
        self.effort = .low
        self.spendTodayUsd = dto.spend_today_usd
        self.spendCapUsd = dto.spend_cap_usd
        self.shared = dto.shared ?? false
        self.ownerId = dto.owner.id
        self.ownerHandle = ownerHandle.isEmpty ? nil : ownerHandle
        self.avatar = dto.avatar.isEmpty ? nil : dto.avatar
        self.avatarColor = dto.avatar_color.isEmpty ? nil : dto.avatar_color
        self.avatarColorIndex = dto.avatar_color_index.map(Int.init)
        self.avatarColorIndexEffective = Int(dto.avatar_color_index_effective)
        self.avatarLeaf = dto.avatar_leaf.map(Int.init)
        self.avatarLeafColor = dto.avatar_leaf_color.map(Int.init)
        self.imageKey = dto.image_key.flatMap { $0.isEmpty ? nil : $0 }
        self.instructions = dto.instructions
        self.description = dto.description
        self.ownerName = dto.owner.name
        self.ownerAvatarUrl = dto.owner.avatar_url
        self.ownerLastSeenAt = dto.owner.last_seen_at
        self.ownerOnline = dto.owner.online
        self.primaryChatId = dto.primary_chat_id.flatMap { $0.isEmpty ? nil : $0 }
        self.voice = dto.voice.flatMap { $0.isEmpty ? nil : $0 }
        self.visionEnabled = dto.vision_enabled
        self.machineState = dto.machine.state
        self.machineClass = dto.machine.`class`
        self.machineSnapshot = dto.machine.snapshot
        self.machineAlwaysOn = dto.machine.always_on
        self.messageGrants = (dto.message_grants ?? []).map(MessageGrant.init(dto:))
    }
}

/// One row of an agent's "Who may message X" card (`message_grants` on the project).
struct MessageGrant: Identifiable, Equatable, Codable {
    /// `user:<id>` / `project:<id>`: the key for PUT / DELETE.
    var grantee: String
    /// `person` or `agent`.
    var kind: String
    var id: String
    var name: String?
    var handle: String?
    var avatarUrl: String?
    var enabled: Bool
    var note: String
    var createdAt: Int64
    var lastAt: Int64?

    var displayName: String { name ?? handle.map { "@\($0)" } ?? id }

    init(dto: MessageGrantDto) {
        grantee = dto.grantee
        kind = dto.kind
        id = dto.id
        name = dto.name.flatMap { $0.isEmpty ? nil : $0 }
        handle = dto.handle.flatMap { $0.isEmpty ? nil : $0 }
        avatarUrl = dto.avatar_url
        enabled = dto.enabled
        note = dto.note
        createdAt = dto.created_at
        lastAt = dto.last_at
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

extension Chat {
    /// Maps a relay `ChatDto` the same way the old hand JSON parse did.
    init(dto: ChatDto) {
        self.id = dto.id
        self.projectId = dto.project_id
        self.title = dto.title.isEmpty ? "New chat" : dto.title
        self.isPinned = dto.is_pinned
        self.unread = false
        self.lastMessageAt = dto.created_at
        self.kind = dto.kind.isEmpty ? "chat" : dto.kind
        self.participants = dto.participants
    }
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
    /// Size on the relay, when the producer knew it (the file card's "PDF · 1.8 MB").
    var bytes: Int64? = nil

    enum CodingKeys: String, CodingKey {
        case name, mime, kind, bytes
        case blobRef = "ref"
    }

    /// "1.8 MB" style size, empty when unknown (Mac `size_label`).
    var sizeLabel: String {
        guard let b = bytes else { return "" }
        if b < 1024 { return "\(b) B" }
        if b < 1024 * 1024 { return "\(b / 1024) KB" }
        if b < 1024 * 1024 * 1024 { return String(format: "%.1f MB", Double(b) / 1_048_576) }
        return String(format: "%.2f GB", Double(b) / 1_073_741_824)
    }

    /// Mac `AttachmentKind::label`: Image, Video, Slides, Document, Spreadsheet, PDF, Audio, File.
    var kindLabel: String {
        let ext = (name as NSString).pathExtension.lowercased()
        let m = mime.lowercased()
        switch ext {
        case "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "heic": return "Image"
        case "mp4", "mov", "webm", "mkv": return "Video"
        case "pptx", "ppt", "key", "odp": return "Slides"
        case "docx", "doc", "odt", "rtf", "md", "txt": return "Document"
        case "xlsx", "xls", "csv", "ods": return "Spreadsheet"
        case "pdf": return "PDF"
        case "mp3", "wav", "m4a", "ogg", "flac": return "Audio"
        default: break
        }
        if m.hasPrefix("image/") { return "Image" }
        if m.hasPrefix("video/") { return "Video" }
        if m.hasPrefix("audio/") { return "Audio" }
        if m == "application/pdf" { return "PDF" }
        return "File"
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
    /// Agent-to-agent messaging: this reply answered a message another agent sent into the
    /// chat; the name drawn in the "Replied to ◉ X" line under the bubble.
    var agentReplyTo: String? = nil
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

/// Connection to the relay, S14 (coarse, app-wide: Home's "Still connecting…"
/// and Settings → Relay). The chat header's status pill reads `LinkState` per room.
enum Link: Equatable {
    case connecting, live, reconnecting
}

/// One chat's link to the relay as the header pill shows it ([feedback
/// connection-pill]). Ordered by priority: when several signals disagree (this
/// room's socket, the relay's HTTP poll) the highest wins — offline > relay down
/// > reconnecting > syncing > restart soon.
enum LinkState: Int, Comparable {
    /// Socket up and caught up: nothing to show.
    case live
    /// The relay said it restarts in a few seconds (`restart_soon` frame): the socket is
    /// still up; a clean close follows (#1340).
    case restartSoon
    /// Socket up, catch-up in flight (HELLO sent, rows still arriving).
    case syncing
    /// The socket dropped or the dial timed out; redialing with backoff.
    case reconnecting
    /// The relay refused the connection or answered 5xx: the phone is online, the relay is not.
    case relayDown
    /// No network from the phone (DNS failed, network unreachable, airplane mode).
    case offline

    static func < (a: LinkState, b: LinkState) -> Bool { a.rawValue < b.rawValue }

    /// The pill's text; nil when there is nothing to say.
    var label: String? {
        switch self {
        case .live: return nil
        case .restartSoon: return "Reconnecting shortly…"
        case .syncing: return "Syncing…"
        case .reconnecting: return "Reconnecting…"
        case .relayDown: return "Relay down"
        case .offline: return "Offline"
        }
    }

    /// What a failed relay call says about the link: a transport verdict when the
    /// error carries one, else a plain reconnect.
    static func fault(_ error: Error) -> LinkState {
        if let relay = error as? RelayError {
            return (500..<600).contains(relay.status) ? .relayDown : .reconnecting
        }
        let ns = error as NSError
        guard ns.domain == NSURLErrorDomain else { return .reconnecting }
        switch URLError.Code(rawValue: ns.code) {
        case .notConnectedToInternet, .networkConnectionLost, .cannotFindHost, .dnsLookupFailed,
             .internationalRoamingOff, .dataNotAllowed, .callIsActive:
            return .offline
        case .cannotConnectToHost, .badServerResponse, .secureConnectionFailed,
             .serverCertificateUntrusted, .serverCertificateHasBadDate, .serverCertificateHasUnknownRoot,
             .serverCertificateNotYetValid:
            return .relayDown
        default:
            return .reconnecting
        }
    }
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
            agentReplyTo: str(m["agentReplyTo"]).flatMap { $0.isEmpty ? nil : $0 },
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
                    kind: str(a["kind"]).flatMap { $0 == "attachment" ? nil : $0 },
                    bytes: (a["bytes"] as? NSNumber)?.int64Value
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

extension Connector {
    init(dto: ConnectorDto) {
        self.init(
            id: dto.id,
            provider: dto.provider,
            label: dto.label.isEmpty ? dto.provider : dto.label,
            status: ConnectorStatus(wire: dto.status),
            lastError: dto.last_error,
            mock: dto.mock
        )
    }
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

extension ConnectorProvider {
    init(dto: ProviderDto) {
        self.init(
            provider: dto.provider,
            label: dto.label.isEmpty ? dto.provider : dto.label,
            connect: dto.connect.isEmpty ? "oauth" : dto.connect,
            oauthConfigured: dto.oauth_configured
        )
    }
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

extension ConnectorsView {
    init(catalog: ConnectorsCatalogDto, list: ConnectorsListDto) {
        self.init(
            mock: list.mock,
            providers: catalog.providers.map(ConnectorProvider.init(dto:)),
            items: list.items.map(Connector.init(dto:)),
            loaded: true
        )
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

    /// Typed fields from `GoalDto`; `plan` / `progress_log` / linked ids from the raw dict
    /// (those stay free-form and are `#[typeshare(skip)]` on the Dto).
    init(dto: GoalDto, raw: [String: Any] = [:]) {
        self.id = dto.id
        self.projectId = dto.project_id
        self.projectName = dto.project_name
        self.chatId = dto.chat_id
        self.title = dto.title
        self.outcome = dto.outcome
        self.plan = ((raw["plan"] as? [Any]) ?? []).compactMap(GoalStep.parse)
        self.status = dto.status
        self.progressLog = ((raw["progress_log"] as? [Any]) ?? []).compactMap(GoalLogEntry.parse)
        self.nextStep = dto.next_step
        self.checkInCron = dto.check_in_cron
        self.checkInTz = dto.check_in_tz
        self.routineId = dto.routine_id
        self.autonomy = dto.autonomy
        self.linkedChatIds = (raw["linked_chat_ids"] as? [Any])?.compactMap { $0 as? String } ?? []
        self.linkedSubagentIds = (raw["linked_subagent_ids"] as? [Any])?.compactMap { $0 as? String } ?? []
        self.waitingQuestion = dto.waiting_question
        self.createdAt = dto.created_at
        self.updatedAt = dto.updated_at
        self.lastMovedAt = dto.last_moved_at
    }

    static func parse(_ v: Any?) -> GoalItem? {
        guard let v, JSONSerialization.isValidJSONObject(v),
              let data = try? JSONSerialization.data(withJSONObject: v),
              let dto = try? JSONDecoder().decode(GoalDto.self, from: data) else { return nil }
        return GoalItem(dto: dto, raw: v as? [String: Any] ?? [:])
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
