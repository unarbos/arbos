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

/// The Create sheet's pending picture, applied once the entity exists (Jacob, 2026-10-25: the
/// sheet offers Upload and Generate only; with neither the entity is stored neutral —
/// `avatar_leaf` = `LeafArt.neutral`, the name's initial on a grey disc — never an automatic
/// leaf).
enum DraftPicture: Equatable {
    /// The photo as chosen, already the encoded master (PNG or JPEG, ≤ 1024 px —
    /// `PictureMaster`); uploaded into the new entity's slot.
    case jpeg(Data)
    /// A picture the relay already holds in my own slot (`users:{me}`): copied over.
    case generated(key: String)
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
    /// `imageKey` as a fetchable URL — the relay's public picture route
    /// `/projects/{id}/avatar.jpg?v={avatarVersion}` (`Project.pictureURL`), filled in by the
    /// relay client and by `AppModel.applyAgentAvatar`; nil in the fixture and without a picture.
    var imageURL: String? = nil
    /// The relay's stamp (ms) of the agent's last avatar change (`avatar_version`); 0 when
    /// unknown. Among the copies of one agent (my record, every roster card) the highest stamp
    /// is drawn, so a change never flips back to the old picture (Jacob, 2026-10-25).
    var avatarVersion: Int = 0
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
    /// Per-user sidebar archive (`archived_at` on `GET /projects`); nil = in the list.
    var archivedAt: Int64? = nil
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
        self.avatarVersion = dto.avatar_version.map(Int.init) ?? 0
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
        self.archivedAt = dto.archived_at.flatMap { $0 }
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

/// Where a GIF / sticker came from (`source` on the attachment map). Old
/// messages and pastes have none (feedback app-01M5TGH18WV3QCK5KZT6XKG7FG).
struct AttachmentSource: Equatable {
    var provider: String
    var id: String
    var pageUrl: String

    /// `pageUrl` as an https URL for the "via <provider>" caption, or `nil`
    /// when the scheme is anything else (feedback app-01M61EZD0GJ2XGV1EW4SCVXENK).
    static func httpsPageURL(from string: String) -> URL? {
        guard let url = URL(string: string),
              url.scheme?.lowercased() == "https",
              url.host != nil
        else { return nil }
        return url
    }
}

struct Attachment: Codable, Equatable {
    var name: String
    var mime: String
    var blobRef: String?
    /// Producer hint (`image`, `audio`, …); clients may also infer from mime/name.
    var kind: String? = nil
    /// Size on the relay, when the producer knew it (the file card's "PDF · 1.8 MB").
    var bytes: Int64? = nil
    /// The fallback picture of a chart bubble: names the `chart` part of the same message.
    /// A client that draws the chart hides this attachment ([feedback chart-bubble-ios-2026-10-26]).
    var chartPart: String? = nil
    /// A picture that lives with its provider (a picked GIF, ARB-544): no `ref`; the bytes come
    /// through the relay's pass-through (`GET /gif/media?u=`), nothing is stored.
    var url: String? = nil
    /// GIF-picker attribution; when set the bubble shows "via \<provider\>".
    var source: AttachmentSource? = nil

    enum CodingKeys: String, CodingKey {
        case name, mime, kind, bytes, chartPart, url, source
        case blobRef = "ref"
    }

    init(name: String, mime: String, blobRef: String?, kind: String? = nil, bytes: Int64? = nil,
         chartPart: String? = nil, url: String? = nil, source: AttachmentSource? = nil) {
        self.name = name
        self.mime = mime
        self.blobRef = blobRef
        self.kind = kind
        self.bytes = bytes
        self.chartPart = chartPart
        self.url = url
        self.source = source
    }

    /// A picker sticker (`kind: sticker`): drawn at a fixed size with no bubble.
    var isSticker: Bool { kind?.lowercased() == "sticker" }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        mime = try c.decode(String.self, forKey: .mime)
        blobRef = try c.decodeIfPresent(String.self, forKey: .blobRef)
        kind = try c.decodeIfPresent(String.self, forKey: .kind)
        bytes = try c.decodeIfPresent(Int64.self, forKey: .bytes)
        chartPart = try c.decodeIfPresent(String.self, forKey: .chartPart)
        url = try c.decodeIfPresent(String.self, forKey: .url)
        if let map = try c.decodeIfPresent([String: String].self, forKey: .source),
           let provider = map["provider"], let id = map["id"],
           let pageUrl = map["pageUrl"] ?? map["page_url"] {
            source = AttachmentSource(provider: provider, id: id, pageUrl: pageUrl)
        } else {
            source = nil
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(name, forKey: .name)
        try c.encode(mime, forKey: .mime)
        try c.encodeIfPresent(blobRef, forKey: .blobRef)
        try c.encodeIfPresent(kind, forKey: .kind)
        try c.encodeIfPresent(bytes, forKey: .bytes)
        try c.encodeIfPresent(chartPart, forKey: .chartPart)
        try c.encodeIfPresent(url, forKey: .url)
        if let source {
            try c.encode(
                ["provider": source.provider, "id": source.id, "pageUrl": source.pageUrl],
                forKey: .source
            )
        }
    }

    /// Stored on the relay: a `ref` that is not a `local:` key still uploading.
    var isOnRelay: Bool {
        guard let ref = blobRef else { return false }
        return !ref.isEmpty && !ref.hasPrefix("local:")
    }

    /// The cache-key prefix of a provider-hosted picture, whose bytes come through the
    /// relay's GIF pass-through instead of `/blob` (Mac `URL_KEY_PREFIX`).
    static let urlKeyPrefix = "url:"

    /// What the decoded picture is cached and fetched under (Mac `preview_key`): the blob
    /// `ref`, or `url:<url>` for a picked GIF that lives with its provider (ARB-544).
    var previewKey: String? {
        if let ref = blobRef { return ref }
        guard let url, !url.isEmpty else { return nil }
        return Self.urlKeyPrefix + url
    }

    /// What this attachment is called when it stands in for the whole message — a reply
    /// preview. The kind, never the file name: a picked GIF's name is the provider's numeric
    /// id (`6178590884800169.gif`), which tells a reader nothing
    /// ([feedback gif-send-and-playback-2026-10-30]).
    var mediaLabel: String {
        mime.lowercased() == "image/gif" ? "GIF" : kindLabel
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
    /// #1544: a muted line at the row's trailing edge ("scratch only: /tmp and /work/tmp").
    var hint: String? = nil
    /// Drawn in red (Deny).
    var destructive: Bool = false
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
    /// #1544: a destructive-shell card with standing options; its footer names Settings.
    var shellCard: Bool = false
    /// Secret hand-off: the request card (`payload.kind = secret_offer`) — Name + masked
    /// Value + Save, no answer rows. The payload carries names only.
    var secretOffer: SecretOffer? = nil

    var hasChoices: Bool { !options.isEmpty }
}

/// What the secret request card needs from its payload (secret hand-off, 2026-10-26).
struct SecretOffer: Equatable {
    /// Suggested vault name, editable (`STRIPE_KEY`).
    var name: String
    var purpose: String
    var agent: String
    var projectId: String
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
    /// Chart bubble ([feedback chart-bubble-ios-2026-10-26]): the structured chart `post_chart`
    /// posted. `spec` is the validated `arbos_core::chart::Chart` as JSON (what the FFI reads),
    /// `table` its markdown table, `fallback` the PNG attachment part this client hides.
    case chart(id: Id, specVersion: Int, spec: String, table: String, fallback: String?)
    /// Link preview ([feedback link-embeds-ios-2026-10-27]): the card the relay unfurled for
    /// the message's first bare URL. `preview: "off"` on the message hides it.
    case linkPreview(id: Id, preview: LinkPreview)
    /// Secret hand-off: the message's value moved into the vault; the row is the ghost
    /// "Secret saved as NAME". Names only — there is no value field.
    case secretRedacted(id: Id, name: String, savedBy: String, agent: String?)
    case unknown(id: Id)

    var id: Id {
        switch self {
        case .text(let id, _), .reasoning(let id), .tool(let id, _, _), .attachment(let id, _),
             .error(let id, _, _), .status(let id, _, _), .question(let id, _), .subagentResult(let id),
             .chart(let id, _, _, _, _), .linkPreview(let id, _), .secretRedacted(let id, _, _, _),
             .unknown(let id):
            return id
        }
    }

    /// The vault name when this is a `secret_redacted` ghost part.
    var secretRedactedName: String? {
        if case .secretRedacted(_, let name, _, _) = self { return name }
        return nil
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

    /// Quick-react set on the long-press capsule (Discord / Telegram / WhatsApp
    /// style). Any other emoji comes from the emoji keyboard.
    static let palette: [String] = ["❤️", "👍🏽", "💪🏽", "👎🏻", "👌🏾", "🤝", "🔥", "💯"]

    /// Fitzpatrick skin-tone modifiers (U+1F3FB … U+1F3FF). Matches
    /// `arbos_core::doc::normalize_reaction_emoji` so `👍🏽` and `👍🏿` are one reaction.
    private static let skinToneScalars: ClosedRange<UInt32> = 0x1F3FB...0x1F3FF

    /// Drop skin-tone modifiers; other marks (ZWJ, VS16) stay.
    static func normalizeEmoji(_ emoji: String) -> String {
        guard !emoji.isEmpty else { return emoji }
        let stripped = String(emoji.unicodeScalars.filter { !skinToneScalars.contains($0.value) })
        return stripped.isEmpty ? emoji : stripped
    }

    /// True when both strings are the same reaction after skin-tone stripping.
    static func sameBase(_ a: String, _ b: String) -> Bool {
        normalizeEmoji(a) == normalizeEmoji(b)
    }

    /// Toggle `sender` on/off for `emoji`, keeping other emoji reactions intact.
    /// Skin-tone variants of the same base share one bucket.
    static func toggling(_ list: [MessageReaction], emoji: String, sender: String) -> [MessageReaction] {
        let base = normalizeEmoji(emoji)
        guard !base.isEmpty else { return list }
        var byBase: [String: [String]] = [:]
        for r in list where !r.emoji.isEmpty && !r.senders.isEmpty {
            let key = normalizeEmoji(r.emoji)
            guard !key.isEmpty else { continue }
            byBase[key, default: []].append(contentsOf: r.senders)
        }
        byBase = byBase.mapValues { Array(Set($0)) }
        var people = byBase[base] ?? []
        if let i = people.firstIndex(of: sender) {
            people.remove(at: i)
        } else {
            people.append(sender)
        }
        if people.isEmpty {
            byBase.removeValue(forKey: base)
        } else {
            byBase[base] = people
        }
        return orderedReactions(byBase)
    }

    /// Palette order first (matched by base), then the rest alphabetically.
    /// Chip glyphs are the untoned base so they match core `normalize_reaction_emoji`
    /// (👍🏽 and 👍🏿 both read as 👍).
    static func orderedReactions(_ byBase: [String: [String]]) -> [MessageReaction] {
        var used = Set<String>()
        var out: [MessageReaction] = []
        for e in palette {
            let base = normalizeEmoji(e)
            guard let s = byBase[base], !s.isEmpty, !used.contains(base) else { continue }
            used.insert(base)
            out.append(MessageReaction(emoji: base, senders: s))
        }
        let rest = byBase.keys.filter { !used.contains($0) }.sorted().compactMap { base -> MessageReaction? in
            guard let s = byBase[base], !s.isEmpty else { return nil }
            return MessageReaction(emoji: base, senders: s)
        }
        return out + rest
    }
}

/// The `job` map on a background job's hidden row (duplex §2.3). Only the id and the
/// title are read; `kind` / `touches` are the box's business.
struct JobRef: Equatable {
    var id: String
    var title: String

    init?(_ raw: Any?) {
        guard let m = raw as? [String: Any], let id = m["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        self.title = (m["title"] as? String) ?? ""
    }
}

/// Where an open job is in its life — the two states the relay lists
/// (`activity.jobs[].state`, duplex §2.4). Finished, failed, cancelled and interrupted
/// jobs are not listed: their outcome is a bubble in the chat.
enum JobState: String, Equatable {
    /// Waiting for a slot or a resource lock; not steerable yet.
    case queued
    case running
}

/// One open background job as the relay's activity record lists it
/// (`GET /projects/{id}` → `activity.jobs[]`): the rows of the tasks sheet.
struct Job: Identifiable, Equatable {
    var id: String
    var title: String
    var state: JobState
    /// The job's newest step in plain words ("Reading tripadvisor.com"); empty while it
    /// starts; "Waiting for <title>" while it queues behind a lock.
    var step: String
    var elapsedS: Int
    /// The user message that asked for it: Steer prefills a reply to it.
    var askMessageId: String?
    var chatId: String?

    init(id: String, title: String, state: JobState, step: String, elapsedS: Int,
         askMessageId: String? = nil, chatId: String? = nil) {
        self.id = id
        self.title = title
        self.state = state
        self.step = step
        self.elapsedS = elapsedS
        self.askMessageId = askMessageId
        self.chatId = chatId
    }

    /// One relay row; nil when it has no id. Unknown states read as running.
    init?(_ raw: Any) {
        guard let m = raw as? [String: Any], let id = m["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        self.title = (m["title"] as? String) ?? ""
        self.state = JobState(rawValue: (m["state"] as? String) ?? "") ?? .running
        self.step = (m["step"] as? String) ?? ""
        self.elapsedS = Int((m["elapsed_s"] as? NSNumber)?.int64Value ?? 0)
        self.askMessageId = (m["ask_message_id"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        self.chatId = (m["chat_id"] as? String).flatMap { $0.isEmpty ? nil : $0 }
    }

    /// `activity.jobs` as the relay sends it — `null`, missing or not a list all read as
    /// "no open jobs"; a row that does not parse is skipped, never fatal (the sheet must
    /// degrade to nothing on an older relay).
    static func list(fromActivity activity: Any?) -> [Job] {
        guard let a = activity as? [String: Any], let rows = a["jobs"] as? [Any] else { return [] }
        return rows.compactMap { Job($0) }
    }

    /// Only a running job has a session to steer; a queued one has not started.
    var canSteer: Bool { state == .running && askMessageId != nil }

    /// The row's second line (contract, "Drawer, per state"): the live step, "Working" for a
    /// running job with no step yet, "Starting" for a queued job with no step, the
    /// "Waiting for <title>" step as the box wrote it.
    var statusLine: String {
        let s = step.trimmingCharacters(in: .whitespacesAndNewlines)
        if !s.isEmpty { return s }
        switch state {
        case .queued: return "Starting"
        case .running: return "Working"
        }
    }

    /// `elapsed_s` as m:ss (an hour and more as h:mm:ss).
    var elapsedLabel: String {
        let s = max(0, elapsedS)
        if s >= 3600 {
            return String(format: "%d:%02d:%02d", s / 3600, (s / 60) % 60, s % 60)
        }
        return String(format: "%d:%02d", s / 60, s % 60)
    }

    /// "1 task" / "N tasks": the chip's words.
    static func countLabel(_ n: Int) -> String {
        n == 1 ? "1 task" : "\(n) tasks"
    }

    /// The presence text without the box's "· N tasks" suffix — the chip carries the
    /// count, so the words are not said twice on one line.
    static func stripCountSuffix(_ text: String) -> String {
        guard let range = text.range(of: " · ", options: .backwards) else { return text }
        let tail = String(text[range.upperBound...])
        let head = String(text[..<range.lowerBound])
        for suffix in [" tasks", " task"] where tail.hasSuffix(suffix) {
            let n = tail.dropLast(suffix.count)
            if !n.isEmpty, n.allSatisfy(\.isNumber) { return head }
        }
        return text
    }
}

/// One entry of a job's exchange (`GET /projects/{id}/jobs/{jobId}` → `entries[]`,
/// `internal/duplex-rollout-client-contract.md`): the parent's prompt, the job's own words, a
/// tool step (plain-words `label`; raw I/O stays in Details), a steer the user sent, the result.
enum RolloutKind: String, Equatable {
    case prompt, text, tool, steer, result, other
}

struct RolloutEntry: Equatable, Identifiable {
    var seq: Int64
    var id: Int64 { seq }
    /// ms since the epoch.
    var at: Int64
    var kind: RolloutKind
    var text: String?
    var label: String?
    var name: String?
    /// `running` | `done` | `failed` on a tool step or the result.
    var status: String?
    var durationMs: Int64?

    init(seq: Int64, at: Int64, kind: RolloutKind, text: String? = nil, label: String? = nil,
         name: String? = nil, status: String? = nil, durationMs: Int64? = nil) {
        self.seq = seq; self.at = at; self.kind = kind; self.text = text; self.label = label
        self.name = name; self.status = status; self.durationMs = durationMs
    }

    init?(_ raw: Any) {
        guard let m = raw as? [String: Any], let seq = (m["seq"] as? NSNumber)?.int64Value else { return nil }
        self.seq = seq
        self.at = (m["at"] as? NSNumber)?.int64Value ?? 0
        self.kind = RolloutKind(rawValue: (m["kind"] as? String) ?? "") ?? .other
        self.text = m["text"] as? String
        self.label = m["label"] as? String
        self.name = m["name"] as? String
        self.status = m["status"] as? String
        self.durationMs = (m["duration_ms"] as? NSNumber)?.int64Value
    }

    var isLive: Bool { status == "running" }

    /// "2.1 s" / "1:04" for a step's duration.
    var durationLabel: String? {
        guard let ms = durationMs else { return nil }
        if ms < 60_000 { return String(format: "%.1f s", Double(ms) / 1000) }
        return String(format: "%d:%02d", ms / 60_000, (ms / 1000) % 60)
    }
}

/// One job's full exchange — the job tab's document.
struct JobRollout: Equatable {
    var id: String
    var title: String
    var kind: String
    var state: JobState
    var step: String
    var elapsedS: Int
    var startedAt: Int64?
    var endedAt: Int64?
    var chatId: String?
    var askMessageId: String?
    var parentProjectId: String
    var parentName: String
    var brief: String
    var entries: [RolloutEntry]
    var toolCalls: Int
    var nextSeq: Int64
    /// Client-only: the relay answered 404 — the server half is not live yet.
    var unavailable: Bool = false

    init(id: String, title: String, kind: String = "", state: JobState = .running, step: String = "",
         elapsedS: Int = 0, startedAt: Int64? = nil, endedAt: Int64? = nil, chatId: String? = nil,
         askMessageId: String? = nil, parentProjectId: String = "", parentName: String = "",
         brief: String = "", entries: [RolloutEntry] = [], toolCalls: Int = 0, nextSeq: Int64 = 0,
         unavailable: Bool = false) {
        self.id = id; self.title = title; self.kind = kind; self.state = state; self.step = step
        self.elapsedS = elapsedS; self.startedAt = startedAt; self.endedAt = endedAt; self.chatId = chatId
        self.askMessageId = askMessageId; self.parentProjectId = parentProjectId; self.parentName = parentName
        self.brief = brief; self.entries = entries; self.toolCalls = toolCalls; self.nextSeq = nextSeq
        self.unavailable = unavailable
    }

    /// The relay's document; nil when it has no id.
    init?(_ raw: Any) {
        guard let m = raw as? [String: Any], let id = m["id"] as? String, !id.isEmpty else { return nil }
        let parent = m["parent"] as? [String: Any]
        self.init(
            id: id,
            title: (m["title"] as? String) ?? "",
            kind: (m["kind"] as? String) ?? "",
            state: JobState(rawValue: (m["state"] as? String) ?? "") ?? .running,
            step: (m["step"] as? String) ?? "",
            elapsedS: Int((m["elapsed_s"] as? NSNumber)?.int64Value ?? 0),
            startedAt: (m["started_at"] as? NSNumber)?.int64Value,
            endedAt: (m["ended_at"] as? NSNumber)?.int64Value,
            chatId: (m["chat_id"] as? String).flatMap { $0.isEmpty ? nil : $0 },
            askMessageId: (m["ask_message_id"] as? String).flatMap { $0.isEmpty ? nil : $0 },
            parentProjectId: (parent?["project_id"] as? String) ?? "",
            parentName: (parent?["name"] as? String) ?? "",
            brief: (m["brief"] as? String) ?? "",
            entries: ((m["entries"] as? [Any]) ?? []).compactMap { RolloutEntry($0) }.sorted { $0.seq < $1.seq },
            toolCalls: Int((m["tool_calls"] as? NSNumber)?.int64Value ?? 0),
            nextSeq: (m["next_seq"] as? NSNumber)?.int64Value ?? 0
        )
    }

    /// A header-only document from the chip's row while the real one loads.
    static func placeholder(_ job: Job, parentName: String) -> JobRollout {
        JobRollout(id: job.id, title: job.title, state: job.state, step: job.step, elapsedS: job.elapsedS,
                   chatId: job.chatId, askMessageId: job.askMessageId, parentName: parentName)
    }

    /// The final entry's status once the job ended (`done` | `failed` | `cancelled`).
    var outcome: String? { entries.last { $0.kind == .result }?.status }
    /// The job is still running: the tab polls and shows Stop.
    var isRunning: Bool { endedAt == nil && outcome == nil }

    /// Merge an incremental answer (`?after=<seq>`): header fields follow the newer document;
    /// entries are appended by `seq`, an entry already held is replaced (a step goes running → done).
    mutating func merge(_ newer: JobRollout) {
        var kept = entries
        for e in newer.entries {
            if let i = kept.firstIndex(where: { $0.seq == e.seq }) { kept[i] = e } else { kept.append(e) }
        }
        kept.sort { $0.seq < $1.seq }
        var out = newer
        out.entries = kept
        out.nextSeq = max(newer.nextSeq, nextSeq)
        out.unavailable = false
        self = out
    }

    /// "Running · 0:51" / "Finished · 6:12" / "Stopped · 1:02".
    var statusShort: String {
        let word: String
        if isRunning { word = state == .queued ? "Queued" : "Running" }
        else if outcome == "failed" { word = "Failed" }
        else if outcome == "cancelled" { word = "Stopped" }
        else { word = "Finished" }
        let s = max(0, elapsedS)
        let clock = s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, (s / 60) % 60, s % 60)
                              : String(format: "%d:%02d", s / 60, s % 60)
        return "\(word) · \(clock)"
    }
}

/// A cross-chat reply reference (crdt-sync §6.2 `replyRef`, core `ReplyRef`): where the
/// quoted message lives plus a snapshot of it, so a reader who is not in the source chat
/// still sees the quote.
struct ReplyRef: Equatable, Hashable {
    var chatId: Id
    /// `"people"` for DMs and people groups, else the project id.
    var projectId: String
    var messageId: Id
    var chatTitle: String
    /// `user:<id>` / `agent:<project>` / `agent` of the quoted row.
    var sender: String
    var senderName: String
    var senderHandle: String
    var text: String

    /// The snapshot keeps this much of the quoted message (core `REPLY_REF_TEXT_CHARS`).
    static let snapshotChars = 240

    init(chatId: Id, projectId: String, messageId: Id, chatTitle: String, sender: String,
         senderName: String, senderHandle: String, text: String) {
        self.chatId = chatId; self.projectId = projectId; self.messageId = messageId
        self.chatTitle = chatTitle; self.sender = sender; self.senderName = senderName
        self.senderHandle = senderHandle; self.text = text
    }

    /// The row's `replyRef` map, when it is well-formed (chat and message ids present).
    init?(row: Any?) {
        guard let m = row as? [String: Any],
              let chatId = m["chatId"] as? String, !chatId.isEmpty,
              let messageId = m["messageId"] as? String, !messageId.isEmpty
        else { return nil }
        let s = { (k: String) -> String in (m[k] as? String) ?? "" }
        self.init(chatId: chatId, projectId: s("projectId").isEmpty ? "people" : s("projectId"),
                  messageId: messageId, chatTitle: s("chatTitle"), sender: s("sender"),
                  senderName: s("senderName"), senderHandle: s("senderHandle"), text: s("text"))
    }

    /// Who the quote is from, for "Replying to <who> in <chat>".
    var who: String {
        let name = senderName.trimmingCharacters(in: .whitespaces)
        if !name.isEmpty { return name }
        let handle = senderHandle.trimmingCharacters(in: .whitespaces)
        if !handle.isEmpty { return "@\(handle)" }
        return "a message"
    }

    /// "Replying to Jacob in Arbonites" — the source line above the quote.
    var label: String {
        let place = chatTitle.trimmingCharacters(in: .whitespaces)
        return place.isEmpty ? "Replying to \(who)" : "Replying to \(who) in \(place)"
    }

    /// Whitespace-collapsed first `snapshotChars` characters, cut on a word with an ellipsis.
    static func snapshot(_ text: String) -> String {
        let flat = text.split(whereSeparator: \.isWhitespace).joined(separator: " ")
        guard flat.count > snapshotChars else { return flat }
        var cut = String(flat.prefix(snapshotChars))
        if let space = cut.lastIndex(of: " "), cut.distance(from: cut.startIndex, to: space) > snapshotChars / 2 {
            cut = String(cut[..<space])
        }
        while let last = cut.last, ",;:.".contains(last) { cut.removeLast() }
        return cut + "…"
    }
}

/// The facts behind a membership system line (relay `notices::Notice`).
struct MembershipNotice: Equatable {
    struct Subject: Equatable {
        /// `user` or `agent`.
        var kind: String
        var id: String
    }

    /// `added` | `removed` | `left` | `joined`.
    var kind: String
    /// User id of the person who acted.
    var actor: String
    var subjects: [Subject]

    init(kind: String, actor: String, subjects: [Subject]) {
        self.kind = kind
        self.actor = actor
        self.subjects = subjects
    }

    init?(row: Any?) {
        guard let m = row as? [String: Any],
              let kind = m["kind"] as? String, !kind.isEmpty,
              let actor = m["actor"] as? String, !actor.isEmpty else { return nil }
        self.kind = kind
        self.actor = actor
        subjects = ((m["subjects"] as? [Any]) ?? []).compactMap { raw in
            guard let s = raw as? [String: Any],
                  let k = s["kind"] as? String, let id = s["id"] as? String, !id.isEmpty else { return nil }
            return Subject(kind: k, id: id)
        }
    }

    /// The line worded from `nameOf(kind, id)` — the live roster name of a `user` / `agent`.
    /// nil when anyone named is unknown here (a person who left, a deleted agent): the caller
    /// then shows the row's stored words.
    func text(nameOf: (String, String) -> String?) -> String? {
        guard let who = nameOf("user", actor) else { return nil }
        var names: [String] = []
        for s in subjects {
            guard let n = nameOf(s.kind, s.id) else { return nil }
            names.append(n)
        }
        switch kind {
        case "added": return names.isEmpty ? nil : "\(who) added \(OptimisticNotice.joinNames(names))"
        case "removed": return names.isEmpty ? nil : "\(who) removed \(OptimisticNotice.joinNames(names))"
        case "left": return "\(who) left"
        case "joined": return "\(who) joined"
        default: return nil
        }
    }
}

/// What a message says and shows, as one thing (`Message.body`): the prose, every attachment
/// that is on the relay (a chart's fallback picture excluded), and every chart with its
/// fallback — the Mac `MessageBody`.
struct MessageBody: Equatable {
    var text: String
    var attachments: [Attachment]
    var charts: [ChartBody]

    var hasMedia: Bool { !attachments.isEmpty || !charts.isEmpty }
}

/// A chart bubble as a message carries it: the `chart` part's fields plus its fallback
/// picture when the message has one.
struct ChartBody: Equatable {
    var id: Id
    var specVersion: Int
    var spec: String
    var table: String
    var fallback: Attachment?
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
    /// A background worker the user never named (agentd, Grok parity trait 15): the transcript
    /// hides this spawn row — Grok shows no "N messages with …" line for its own workers — while
    /// the working line still reads its `activity`. An agent-to-agent `send_message(to)` row
    /// carries no flag and keeps its "Messaged ◉ X" line.
    var worker: Bool = false
    /// Duplex design §2.3: the background job this row stands for (`job: { id, title, kind,
    /// touches }`). Wordless and `hidden`; it exists so the presence line can read its
    /// `activity` and so the client knows a job is live in this chat.
    var job: JobRef? = nil
    /// The box asks the transcript to draw nothing for this row (job rows).
    var hidden: Bool = false
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
    /// Bubble menu → Forward: the original sender's name; the bubble draws
    /// "Forwarded from <name>" above the text (`forwardedFrom` on the row).
    var forwardedFrom: String? = nil
    /// Bubble menu → Reply in DM: the quoted message in another chat with its snapshot
    /// (`replyRef` on the row, crdt-sync §6.2); the bubble draws the quote header plus
    /// "Replying to <who> in <chat>".
    var replyRef: ReplyRef? = nil
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
    /// A membership line's facts (relay `notices::Notice`): actor and subjects by id, so the
    /// line is worded from the live roster and a rename applies to old lines too ("Const
    /// added Aureum" → "Jacob added Aureum", 2026-10-26). nil on rows older than the field.
    var notice: MembershipNotice? = nil
    /// `"off"` once the sender removed the link preview for everyone (#2066).
    var preview: String? = nil

    var isStreaming: Bool { status == .streaming }

    /// The link preview the relay attached, unless the sender switched it off.
    var linkPreview: (id: Id, preview: LinkPreview)? {
        if preview == "off" { return nil }
        for part in parts {
            if case .linkPreview(let id, let p) = part, !p.title.trimmingCharacters(in: .whitespaces).isEmpty {
                return (id, p)
            }
        }
        return nil
    }

    /// Does this message carry the `chart` part `partId`? (Its fallback picture is hidden only
    /// when the chart it stands in for is really here.)
    func hasChartPart(_ partId: String) -> Bool {
        parts.contains { if case .chart(let id, _, _, _, _) = $0 { return id == partId } else { return false } }
    }

    /// The message's body, in part order — the one accessor Forward, Copy and any other
    /// "take this message" action read, so they all see the same parts (the Mac
    /// `Message::body()`, [feedback forward-attachments-ios-2026-10-27]). `nil` when there
    /// is nothing to take: no words, no attachment on the relay, no chart. A picture-only
    /// message has a body like any other.
    var body: MessageBody? {
        let text = plainText.trimmingCharacters(in: .whitespacesAndNewlines)
        var fallbackIds = Set<String>()
        for part in parts {
            if case .chart(_, _, _, _, let fallback) = part, let fallback { fallbackIds.insert(fallback) }
        }
        var attachments: [Attachment] = []
        var charts: [ChartBody] = []
        for part in parts {
            switch part {
            case .attachment(let id, let a) where !fallbackIds.contains(id) && a.isOnRelay:
                attachments.append(a)
            case .chart(let id, let specVersion, let spec, let table, let fallback):
                let picture = fallback.flatMap { f in
                    parts.lazy.compactMap { p -> Attachment? in
                        if case .attachment(let aid, let a) = p, aid == f, a.isOnRelay { return a }
                        return nil
                    }.first
                }
                charts.append(ChartBody(id: id, specVersion: specVersion, spec: spec, table: table, fallback: picture))
            default:
                break
            }
        }
        guard !text.isEmpty || !attachments.isEmpty || !charts.isEmpty else { return nil }
        return MessageBody(text: text, attachments: attachments, charts: charts)
    }

    /// Does the message hold an attachment whose upload has not finished (a `local:` key)?
    var hasUploadingAttachment: Bool {
        parts.contains {
            if case .attachment(_, let a) = $0, let ref = a.blobRef { return ref.hasPrefix("local:") }
            return false
        }
    }

    var plainText: String {
        parts.compactMap(\.textValue).joined(separator: "\n")
    }

    /// A finished row with no words, nothing to look at and no named sender (`user:`,
    /// `agent:`, `project:`): in a people chat it is a blank line from no one (Arbonites,
    /// 2026-10-28: a box's hand-off row and job row, bare `sender: "agent"`).
    var isSenderlessBlank: Bool {
        guard role == .user || role == .assistant, !isStreaming,
              !["user:", "agent:", "project:"].contains(where: { sender.hasPrefix($0) }) else { return false }
        return parts.allSatisfy { part in
            switch part {
            case .text(_, let t): return t.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            case .reasoning, .status, .subagentResult, .unknown: return true
            case .tool, .attachment, .error, .question, .chart, .linkPreview, .secretRedacted: return false
            }
        }
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
            worker: bool(m["worker"]),
            job: JobRef(m["job"]),
            hidden: bool(m["hidden"]),
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
            forwardedFrom: str(m["forwardedFrom"]).flatMap { $0.isEmpty ? nil : $0 },
            replyRef: ReplyRef(row: m["replyRef"]),
            activity: str(m["activity"]).flatMap { $0.isEmpty ? nil : $0 },
            voice: bool(m["voice"]) || str(m["source"]) == "voice",
            greeting: bool(m["greeting"]),
            reactions: reactions(m["reactions"]),
            notice: MembershipNotice(row: m["notice"]),
            preview: str(m["preview"])
        )
    }

    /// `reactions` is flat `"emoji|sender" → true` (arbos-core) and/or legacy
    /// emoji → map of sender id → true. Unknown shapes are ignored.
    /// Skin-tone variants of the same base merge into one chip.
    static func reactions(_ raw: Any?) -> [MessageReaction] {
        guard let map = raw as? [String: Any] else { return [] }
        var byBase: [String: [String]] = [:]
        for (key, v) in map {
            guard !key.isEmpty else { continue }
            if let people = v as? [String: Any] {
                // Nested bucket (projected or legacy). Merge by base emoji.
                let base = MessageReaction.normalizeEmoji(key)
                guard !base.isEmpty else { continue }
                for (sid, flag) in people {
                    guard !sid.isEmpty, bool(flag) else { continue }
                    byBase[base, default: []].append(sid)
                }
                continue
            }
            // Flat key: "emoji|sender" → true.
            guard bool(v), let sep = key.firstIndex(of: "|") else { continue }
            let emoji = String(key[..<sep])
            let sid = String(key[key.index(after: sep)...])
            guard !emoji.isEmpty, !sid.isEmpty else { continue }
            let base = MessageReaction.normalizeEmoji(emoji)
            guard !base.isEmpty else { continue }
            byBase[base, default: []].append(sid)
        }
        byBase = byBase.mapValues { Array(Set($0)).sorted() }
        return MessageReaction.orderedReactions(byBase)
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
            let source: AttachmentSource? = {
                guard let map = a["source"] as? [String: Any],
                      let provider = str(map["provider"]), !provider.isEmpty,
                      let sid = str(map["id"]),
                      let pageUrl = str(map["pageUrl"]) ?? str(map["page_url"]), !pageUrl.isEmpty
                else { return nil }
                return AttachmentSource(provider: provider, id: sid, pageUrl: pageUrl)
            }()
            return .attachment(
                id: id,
                attachment: Attachment(
                    name: str(a["name"]) ?? "attachment",
                    mime: str(a["mime"]) ?? "application/octet-stream",
                    blobRef: str(a["ref"]),
                    kind: str(a["kind"]).flatMap { $0 == "attachment" ? nil : $0 },
                    bytes: (a["bytes"] as? NSNumber)?.int64Value,
                    chartPart: str(a["chartPart"]),
                    // A picked GIF lives with its provider (ARB-544): without `url` the row
                    // had neither ref nor url and drew the broken-picture glyph.
                    url: str(a["url"]),
                    source: source
                )
            )
        case "chart":
            // The spec travels as JSON text: the core parses and lays it out through the FFI.
            let spec: String = {
                guard let obj = pm["chart"], JSONSerialization.isValidJSONObject(obj),
                      let data = try? JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys]),
                      let text = String(data: data, encoding: .utf8) else { return "" }
                return text
            }()
            return .chart(
                id: id,
                specVersion: (pm["specVersion"] as? NSNumber)?.intValue ?? 0,
                spec: spec,
                table: str(pm["table"]) ?? "",
                fallback: str(pm["fallback"])
            )
        case "link_preview":
            guard let p = LinkPreview.parse(pm) else { return .unknown(id: id) }
            return .linkPreview(id: id, preview: p)
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
        case "secret_redacted":
            let name = str(pm["name"]).flatMap { $0.isEmpty ? nil : $0 } ?? str(pm["env"]) ?? ""
            return .secretRedacted(id: id, name: name, savedBy: str(pm["savedBy"]) ?? "", agent: str(pm["agent"]))
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
    static func approval(_ pm: [String: Any], agentName: String? = nil) -> Question {
        let approvalId = str(pm["approvalId"]) ?? id(pm)
        let payload = pm["payload"] as? [String: Any]
        var options: [ChoiceOption] = []
        for o in (payload?["answers"] as? [Any]) ?? [] {
            guard let om = o as? [String: Any], let oid = str(om["id"]) else { continue }
            options.append(ChoiceOption(id: oid, label: str(om["label"]) ?? oid))
        }
        // #1544: a destructive-shell card offers A Allow once · B Always allow <agent> in <class>
        // · C … anywhere · D Deny (`payload.grant_options`); the agent's name comes in `agentName`.
        let grantOptions = (payload?["grant_options"] as? [Any]) ?? []
        var shellCard = false
        if options.isEmpty, !grantOptions.isEmpty {
            shellCard = true
            // The card fills in the agent's name when it draws ("Always allow Doug in /tmp").
            let agent = agentName ?? "{agent}"
            let rawClass = str(payload?["class"]) ?? "this"
            let ruleClass = rawClass == "file-delete" ? "file deletes" : rawClass
            options = [ChoiceOption(id: "once", label: "Allow once")]
            for o in grantOptions {
                guard let om = o as? [String: Any], let cls = str(om["path_class"]) else { continue }
                let label = str(om["label"]) ?? cls
                let detail = str(om["detail"]) ?? ""
                if cls == "anywhere" {
                    options.append(ChoiceOption(id: "grant:anywhere", label: "Always allow \(agent) anywhere",
                                                hint: "no more cards for \(ruleClass) from \(agent), in \(detail)"))
                } else {
                    options.append(ChoiceOption(id: "grant:\(cls)", label: "Always allow \(agent) in \(label)", hint: detail))
                }
            }
            options.append(ChoiceOption(id: "deny", label: "Deny", destructive: true))
        }
        if options.isEmpty {
            options = [ChoiceOption(id: "approve", label: "Approve"), ChoiceOption(id: "reject", label: "Reject")]
        }
        let state = str(pm["state"]) ?? "pending"
        let decidedBy = str(pm["decidedBy"]) ?? ""
        var picked: [String] = []
        switch state {
        case "approved" where shellCard:
            if let g = str(pm["grant"]), !g.isEmpty { picked = ["grant:\(g)"] } else { picked = ["once"] }
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
        // Secret hand-off request card: no answer rows; the card draws its own fields.
        var offer: SecretOffer?
        if str(payload?["kind"]) == "secret_offer" {
            offer = SecretOffer(name: str(payload?["name"]) ?? "", purpose: str(payload?["purpose"]) ?? "",
                                agent: str(payload?["agent"]) ?? "", projectId: str(payload?["projectId"]) ?? "")
            options = []
            if state == "approved" { picked = ["saved"] }
        }
        var title = (str(pm["title"]) ?? "Approve this?").trimmingCharacters(in: .whitespaces)
        if offer == nil, !title.hasSuffix("?") {
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
            secretRequestId: str(payload?["secretRequestId"]),
            shellCard: shellCard,
            secretOffer: offer
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

// MARK: - Job tabs (feedback subagent-job-tabs-ios-2026-10-25)

/// Sub-agent job tabs (Jacob, 2026-10-25 19:41 UTC): every background job an agent starts is
/// its own chat screen next to the agent's chat — the brief as the first message, the job's words,
/// its tool steps as system lines and the result as messages, live while it runs. Typing there
/// steers the job (a reply under the job's ask in the parent chat, the wire the Steer button
/// uses); Stop sends `stop { jobId }`. A finished job keeps its tab until closed. The chat
/// itself still draws nothing for the job (the hidden `job` row). The Mac's `jobtabs`.
enum JobTabs {
    static let chatPrefix = "job:"

    static func chatId(_ jobId: String) -> Id { chatPrefix + jobId }
    static func jobId(_ chatId: String) -> String? {
        chatId.hasPrefix(chatPrefix) ? String(chatId.dropFirst(chatPrefix.count)) : nil
    }
    static func isJobChat(_ chatId: String) -> Bool { chatId.hasPrefix(chatPrefix) }

    /// The tab entry for a job under `projectId` (`kind: "job"`, client-side only).
    static func tab(projectId: Id, jobId: String, title: String, at: Int64) -> Chat {
        Chat(id: chatId(jobId), projectId: projectId,
             title: title.trimmingCharacters(in: .whitespaces).isEmpty ? "Job" : title,
             isPinned: false, unread: false, lastMessageAt: at, kind: "job")
    }

    /// "Read tripadvisor.com · 4.8 s" done, "Reading delsoulsurf.com…" live.
    static func stepLine(_ e: RolloutEntry) -> String {
        let words = e.label ?? e.text ?? ""
        if e.isLive { return words + "…" }
        if let d = e.durationLabel { return "\(words) · \(d)" }
        return words
    }

    private static func base(_ chat: Id, id: String, role: Role, status: MessageStatus, at: Int64) -> Message {
        Message(id: id, chatId: chat, role: role, sender: role == .user ? "user" : (role == .system ? "system" : "agent"),
                status: status, turn: nil, decision: nil, subagentId: nil, subagentName: nil, inReplyTo: nil,
                usage: [:], model: nil, createdAt: at, completedAt: at, pending: false, parts: [], hiddenCount: 0)
    }

    /// A steer typed in the tab, echoed at once (pending until the box's entry replaces it);
    /// the same row, re-homed, is the parent chat's reply under the ask.
    static func typedSteer(chat: Id, messageId: String, text: String, ask: String) -> Message {
        var m = base(chat, id: messageId, role: .user, status: .done, at: nowMs())
        m.replyTo = ask
        m.pending = true
        m.parts = [.text(id: "\(messageId)-t", text: text)]
        return m
    }

    /// The tab's transcript from the job's exchange: the brief (and every steer) as a user
    /// line, the job's words and its result as agent lines, each tool step as a system line,
    /// and — while the job runs — a wordless streaming agent row whose `activity` is the live
    /// step, so the presence row reads it and the composer shows Stop. `pending` are steers
    /// typed in the tab the box has not echoed yet.
    static func messages(_ r: JobRollout, pending: [Message] = []) -> [Message] {
        let chat = chatId(r.id)
        var out: [Message] = []
        var liveStep: String? = nil
        if r.unavailable {
            let id = "\(chat):unavailable"
            var m = base(chat, id: id, role: .system, status: .done, at: nowMs())
            m.parts = [.text(id: "\(id)-t", text: "This task's transcript is not available on this relay yet.")]
            out.append(m)
        }
        for e in r.entries {
            let id = "\(chat):\(e.seq)"
            let at = e.at > 0 ? e.at : (r.startedAt ?? 0)
            switch e.kind {
            case .prompt, .steer:
                var m = base(chat, id: id, role: .user, status: .done, at: at)
                m.parts = [.text(id: "\(id)-t", text: e.text ?? "")]
                out.append(m)
            case .text, .result:
                var m = base(chat, id: id, role: .assistant, status: .done, at: at)
                if let t = e.text, !t.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    m.parts.append(.text(id: "\(id)-t", text: t))
                }
                if e.kind == .result {
                    let (code, words): (StatusKind, String) = e.status == "failed" ? (.failed, "Failed")
                        : (e.status == "cancelled" ? (.stopped, "Stopped") : (.done, "Done"))
                    m.parts.append(.status(id: "\(id)-s", code: code, message: words))
                    if e.status == "cancelled" { m.status = .cancelled }
                }
                out.append(m)
            case .tool:
                if e.isLive { liveStep = e.label ?? e.text }
                var m = base(chat, id: id, role: .system, status: .done, at: at)
                m.parts = [.text(id: "\(id)-t", text: stepLine(e))]
                out.append(m)
            case .other:
                break
            }
        }
        for p in pending {
            let echoed = r.entries.contains { $0.kind == .steer && ($0.text ?? "").trimmingCharacters(in: .whitespaces) == p.plainText.trimmingCharacters(in: .whitespaces) }
            if !echoed { out.append(p) }
        }
        if r.isRunning {
            var live = base(chat, id: "\(chat):live", role: .assistant, status: .streaming, at: nowMs())
            live.completedAt = nil
            live.activity = liveStep ?? (r.step.trimmingCharacters(in: .whitespaces).isEmpty ? "\(r.title) is working" : r.step)
            live.parts = [.status(id: "\(chat):live-s", code: .working, message: "Working…")]
            out.append(live)
        }
        return out
    }
}


extension Project {
    /// The one URL every surface fetches an agent's picture from: the relay's public route for
    /// the agent's *current* picture, stamped with `avatar_version` so a change is a new URL and
    /// no cache (URLCache, `AvatarImageCache`) ever serves the old bytes. Why not the blob slot:
    /// `/blob/{project}/{key}` is readable by the owner and the project's guests only, so a
    /// viewer who merely shares a group with the agent got 403 and drew the leaf (Jacob,
    /// 2026-10-25, "Hezbola"). nil without a picture or without a relay.
    static func pictureURL(httpUrl: String, id: Id, imageKey: String?, version: Int) -> String? {
        guard let imageKey, !imageKey.isEmpty, !httpUrl.isEmpty else { return nil }
        return "\(httpUrl)/projects/\(id)/avatar.jpg?v=\(version)"
    }
}

/// What a `link_preview` part carries (relay `unfurl.rs`, spec 1): the card's words and its
/// picture, pre-formatted — the app parses nothing from the site. Everything but `title` is
/// optional ([feedback link-embeds-ios-2026-10-27]).
struct LinkPreview: Equatable, Hashable {
    struct Picture: Equatable, Hashable {
        var ref: String
        var width: Int
        var height: Int
        var mime: String
        var art: Bool
    }
    struct Diff: Equatable, Hashable {
        var additions: Int
        var deletions: Int
        var files: Int
    }

    var specVersion: Int = 1
    var url: String
    var canonicalUrl: String
    /// `spotify` `youtube` `x` `github` `bluesky` `vimeo` `loom` `soundcloud` `apple_music`
    /// `google_maps` `wikipedia` `arbos` `file` `web`.
    var provider: String
    var providerName: String
    /// `song` `album` … `pull_request` `issue` `repo` `article` `image` `file` `place` `page`.
    var type: String
    var title: String
    var subtitle: String? = nil
    var description: String? = nil
    /// Footer facts in order ("Song", "4:03", "opened 4 days ago").
    var facts: [String] = []
    /// `open` `merged` `closed` `draft`.
    var state: String? = nil
    var diff: Diff? = nil
    /// "18:24" on a video thumbnail.
    var duration: String? = nil
    var image: Picture? = nil
    /// The final host ("github.com"): the shared link-icon tile picks its mark by it.
    var siteHost: String? = nil
    /// Reserved for the shared link-icon tile (a blob in the chat's slot); unused here.
    var siteIcon: Picture? = nil
    /// `compact` (art row) or `expanded` (media on top).
    var layout: String = "compact"
    /// The chat's blob slot, set by the transcript builder so the card can load its picture.
    var slot: String = ""

    var expanded: Bool { layout == "expanded" && image != nil }
    var isPost: Bool { type == "post" }
    var isGitHubItem: Bool { type == "pull_request" || type == "issue" || type == "gist" }
    /// Where a tap goes: the canonical URL (tracking parameters gone), else the typed one.
    var openURL: URL? { URL(string: canonicalUrl.isEmpty ? url : canonicalUrl) }
    /// "<title> — <provider>" for quote headers, copies and one-line previews.
    var oneLine: String { providerName.isEmpty ? title : "\(title) — \(providerName)" }

    /// Was `text` nothing but the link the card stands for? Then the card is the bubble.
    func isLinkOnly(_ text: String) -> Bool {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty, t.rangeOfCharacter(from: .whitespacesAndNewlines) == nil else { return false }
        let trimSlash: (String) -> String = { $0.hasSuffix("/") ? String($0.dropLast()) : $0 }
        return t == url.trimmingCharacters(in: .whitespaces) || t == canonicalUrl.trimmingCharacters(in: .whitespaces)
            || trimSlash(t) == trimSlash(url.trimmingCharacters(in: .whitespaces))
    }

    static func parse(_ pm: [String: Any]) -> LinkPreview? {
        func str(_ v: Any?) -> String? { v as? String }
        func int(_ v: Any?) -> Int? { (v as? NSNumber)?.intValue }
        func picture(_ v: Any?) -> Picture? {
            guard let d = v as? [String: Any], let ref = str(d["ref"]), !ref.isEmpty else { return nil }
            return Picture(ref: ref, width: int(d["width"]) ?? 0, height: int(d["height"]) ?? 0,
                           mime: str(d["mime"]) ?? "image/jpeg", art: (d["art"] as? Bool) ?? false)
        }
        guard let title = str(pm["title"])?.trimmingCharacters(in: .whitespacesAndNewlines), !title.isEmpty else { return nil }
        var p = LinkPreview(url: str(pm["url"]) ?? "", canonicalUrl: str(pm["canonicalUrl"]) ?? "",
                            provider: str(pm["provider"]) ?? "web", providerName: str(pm["providerName"]) ?? "",
                            type: str(pm["type"]) ?? "page", title: title)
        p.specVersion = int(pm["specVersion"]) ?? 1
        p.subtitle = str(pm["subtitle"]).flatMap { $0.isEmpty ? nil : $0 }
        p.description = str(pm["description"]).flatMap { $0.isEmpty ? nil : $0 }
        p.facts = (pm["facts"] as? [Any])?.compactMap { $0 as? String } ?? []
        p.state = str(pm["state"])
        if let d = pm["diff"] as? [String: Any] {
            p.diff = Diff(additions: int(d["additions"]) ?? 0, deletions: int(d["deletions"]) ?? 0, files: int(d["files"]) ?? 0)
        }
        p.duration = str(pm["duration"])
        p.image = picture(pm["image"])
        p.siteHost = str(pm["siteHost"])
        p.siteIcon = picture(pm["siteIcon"])
        p.layout = str(pm["layout"]) ?? "compact"
        return p
    }
}
