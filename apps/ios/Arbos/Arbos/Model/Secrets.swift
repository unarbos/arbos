// Secrets (D83, docs/spec-secrets.md): the relay's `/secrets` shapes as the app sees them.
// A Secret is a named value an agent may use but never read; an Access is one switch
// (secret × agent) with a note; Pause all is the kill switch. No value ever reaches the
// phone: the relay never returns one, and these types have no field for it.

import Foundation
import ArbosFFI

private enum SecretsDto {
    static func decode<T: Decodable>(_ type: T.Type, from v: Any?) -> T? {
        guard let v, JSONSerialization.isValidJSONObject(v),
              let data = try? JSONSerialization.data(withJSONObject: v) else { return nil }
        return try? JSONDecoder().decode(type, from: data)
    }
}

/// A person on the secret's People list (`secret.people[]`, spec §7.7): they may ask any agent
/// with access to use it, with no card to the owner.
struct SecretDirector: Identifiable, Equatable {
    var id: String { userId }
    var userId: String
    var handle: String
    var name: String?
    var avatarUrl: String?
    /// Constraints the agent must honour when this person asks
    /// (e.g. "only 1k usd of credit usage per day").
    var note: String
    var createdAt: Int64
    /// The row's switch.
    var enabled: Bool
    var disabledAt: Int64?
    /// `all` or `chat`.
    var scopeKind: String
    var lastAskedAt: Int64?

    var displayName: String {
        if let name, !name.isEmpty { return name }
        return handle.isEmpty ? userId : "@\(handle)"
    }

    init(dto: SecretDirectorDto) {
        self.userId = dto.user_id
        self.handle = dto.handle.trimmingCharacters(in: CharacterSet(charactersIn: "@"))
        self.name = dto.name
        self.avatarUrl = dto.avatar_url
        self.note = dto.note
        self.createdAt = dto.created_at
        self.enabled = dto.enabled ?? true
        self.disabledAt = dto.disabled_at
        self.scopeKind = dto.scope?.kind ?? "all"
        self.lastAskedAt = dto.last_asked_at
    }

    static func parse(_ v: Any?) -> SecretDirector? {
        SecretsDto.decode(SecretDirectorDto.self, from: v).map(SecretDirector.init(dto:))
    }
}

/// One agent's switch on a secret (`secret.agents[]`, `GET /projects/{id}/secrets items[]`).
struct SecretAgentAccess: Identifiable, Equatable {
    var id: String
    var projectId: String
    var name: String
    var enabled: Bool
    var note: String
    var hosts: [String]
    /// `all` or `chat`.
    var scopeKind: String
    var scopeChatId: String?
    var lastUsedAt: Int64?
    var disabledAt: Int64?

    init(dto: SecretAccessDto) {
        self.id = dto.access_id
        self.projectId = dto.project_id
        self.name = dto.name
        self.enabled = dto.enabled
        self.note = dto.note
        self.hosts = dto.hosts
        self.scopeKind = dto.scope.kind
        self.scopeChatId = dto.scope.chat_id
        self.lastUsedAt = dto.last_used_at
        self.disabledAt = dto.disabled_at
    }

    static func parse(_ v: Any?) -> SecretAgentAccess? {
        SecretsDto.decode(SecretAccessDto.self, from: v).map(SecretAgentAccess.init(dto:))
    }
}

struct SecretItem: Identifiable, Equatable {
    var id: String
    var name: String
    /// `$STRIPE_KEY` in the box; derived from the name.
    var env: String
    /// `key | login | text`.
    var kind: String
    var note: String
    var hosts: [String]
    var askEveryTime: Bool
    /// "Real value in the agent's environment": agents with access get the value itself as
    /// `$STRIPE_KEY`, no card per use (D83 decision 2 as decided).
    var injectEnv: Bool
    var expiresAt: Int64?
    var version: Int
    var createdAt: Int64
    var lastUsedAt: Int64?
    /// Logins only: the username is metadata, the password is the secret.
    var username: String?
    var vaultName: String
    var agents: [SecretAgentAccess]
    /// The People list: who (besides the owner) may ask an agent to use this secret.
    var directors: [SecretDirector]
    /// `refuse | card`: what a non-listed person's ask does (spec §7.7).
    var othersAsk: String
    /// Where it came from: `settings`, or `chat` + the agent it was handed to (secret hand-off).
    var sourceKind: String = "settings"
    var sourceProjectName: String? = nil

    /// "given in chat" for a hand-off secret, else nil (list subtitle).
    var sourceLabel: String? { sourceKind == "chat" ? "given in chat" : nil }

    /// "In chat with Arbos" — the Secret page's Added row, when it came from a chat.
    var addedLabel: String? {
        guard sourceKind == "chat" else { return nil }
        if let n = sourceProjectName, !n.isEmpty { return "In chat with \(n)" }
        return "In chat"
    }

    var kindLabel: String {
        switch kind {
        case "login": return "Login"
        case "text": return "Text"
        default: return "Key"
        }
    }

    var icon: String { kind == "login" ? "person.fill" : "key.fill" }

    init(dto: SecretDto) {
        self.id = dto.id
        self.name = dto.name
        self.env = dto.env
        self.kind = dto.kind
        self.note = dto.note
        self.hosts = dto.hosts
        self.askEveryTime = dto.ask_every_time
        self.injectEnv = dto.inject_env
        self.expiresAt = dto.expires_at
        self.version = Int(dto.version)
        self.createdAt = dto.created_at
        self.lastUsedAt = dto.last_used_at
        self.username = dto.username
        self.vaultName = dto.vault_name
        self.agents = dto.agents.map(SecretAgentAccess.init(dto:))
        let people = dto.people ?? []
        self.directors = (people.isEmpty ? dto.directors : people).map(SecretDirector.init(dto:))
        self.othersAsk = dto.others_ask ?? "refuse"
        self.sourceKind = dto.source?.kind ?? "settings"
        self.sourceProjectName = dto.source?.project_name
    }

    /// Reduced project-page secret (`ProjectSecretSummaryDto`) with empty agents/directors.
    init(summary: ProjectSecretSummaryDto) {
        self.id = summary.id
        self.name = summary.name
        self.env = summary.env
        self.kind = summary.kind
        self.note = summary.note
        self.hosts = summary.hosts
        self.askEveryTime = false
        self.injectEnv = summary.inject_env
        self.expiresAt = nil
        self.version = 1
        self.createdAt = 0
        self.lastUsedAt = summary.last_used_at
        self.username = nil
        self.vaultName = summary.vault_name
        self.agents = []
        self.directors = []
        self.othersAsk = "refuse"
    }

    static func parse(_ v: Any?) -> SecretItem? {
        SecretsDto.decode(SecretDto.self, from: v).map(SecretItem.init(dto:))
    }
}

/// One audit row (`activity[]`): names and hosts only, never a value.
struct SecretActivity: Identifiable, Equatable {
    var id: Int64
    var at: Int64
    /// `created | value_replaced | access_on | access_off | requested | decided | used | refused | paused | unpaused | expired | …`
    var action: String
    var name: String
    var targetKind: String
    var projectId: String?
    var projectName: String?
    var host: String?
    var by: String
    var reason: String?
    var decision: String?
    /// The person who asked the agent (handle), when it was not the owner (spec §7.7).
    var askedByHandle: String?
    /// A People row's handle on `person_on` / `person_off`.
    var personHandle: String?

    /// Typed fields from `SecretActivityDto`; `reason` / `decision` from raw `detail`
    /// (`#[typeshare(skip)]` on the Dto).
    init(dto: SecretActivityDto, raw: [String: Any] = [:]) {
        self.id = dto.id
        self.at = dto.at
        self.action = dto.action
        self.name = dto.name
        self.targetKind = dto.target_kind
        self.projectId = dto.project_id
        self.projectName = dto.project_name
        self.host = dto.host
        self.by = dto.by
        let detail = raw["detail"] as? [String: Any]
        self.reason = DocJSON.str(detail?["reason"])
        self.decision = DocJSON.str(detail?["decision"])
        self.askedByHandle = DocJSON.str(detail?["asked_by_handle"])
        self.personHandle = DocJSON.str(detail?["handle"])
    }

    /// "asked by @sam · 2 min ago" when a person other than the owner asked; else the time.
    var whenLine: String {
        if let h = askedByHandle, !h.isEmpty { return "asked by @\(h) · \(SecretsText.ago(at))" }
        return SecretsText.ago(at)
    }

    static func parse(_ v: Any?) -> SecretActivity? {
        guard let dto = SecretsDto.decode(SecretActivityDto.self, from: v) else { return nil }
        return SecretActivity(dto: dto, raw: v as? [String: Any] ?? [:])
    }

    /// One line for the activity card: "Research used STRIPE_KEY · api.stripe.com".
    var line: String {
        let agent = projectName ?? "An agent"
        let at = host.map { " · \($0)" } ?? ""
        switch action {
        case "used": return "\(agent) used \(name)\(at)"
        case "refused": return "Refused: \(agent) · \(name)\(at)"
        case "requested": return "\(agent) asked for \(name)\(at)"
        case "decided":
            switch decision ?? "" {
            case "denied": return "\(agent) asked for \(name). Denied."
            case "allowed_once": return "You let \(agent) use \(name) once"
            case "allowed_chat": return "You let \(agent) use \(name) in one chat"
            default: return "You let \(agent) use \(name)"
            }
        case "access_on": return by.hasPrefix("agent:") ? "\(agent) is using \(name)\(at)" : "You gave \(agent) access to \(name)"
        case "access_off": return "You turned \(name) off for \(agent)"
        case "created": return "You added \(name)"
        case "updated": return "You edited \(name)"
        case "value_replaced": return "You replaced the value of \(name)"
        case "scrubbed": return "You scrubbed \(name) from context"
        case "deleted": return "You deleted \(name)"
        case "paused": return by == "stop_machine" ? "Secrets paused with the machine" : "You paused all secrets"
        case "unpaused": return "You turned secrets back on"
        case "expired": return "\(name) expired"
        case "expiring": return "\(name) expires soon"
        case "vault_connected": return "You connected \(name)"
        case "vault_disconnected": return "You disconnected \(name)"
        case "vault_refreshed": return "\(name) refreshed"
        case "env_on": return "You put the real value of \(name) in the environment"
        case "env_off": return "You took the real value of \(name) out of the environment"
        case "injected": return "\(name) went into \(agent)'s environment"
        case "person_on", "director_on": return "You let \(personHandle.map { "@\($0)" } ?? "someone") ask for \(name)"
        case "person_off", "director_off": return "You turned \(name) off for \(personHandle.map { "@\($0)" } ?? "someone")"
        default: return "\(action.replacingOccurrences(of: "_", with: " ")) · \(name)"
        }
    }
}

/// A card the owner has not answered yet, as Settings lists it.
struct SecretRequestItem: Identifiable, Equatable {
    var id: String
    var projectId: String
    var projectName: String?
    var name: String
    var purpose: String
    var use: String
    var host: String?
    var status: String
    var createdAt: Int64
    /// The person who asked the agent, when it was not the owner (spec §7.7).
    var askedByHandle: String?
    var askedByName: String?

    var isPending: Bool { status == "pending" }

    /// `@sam` for a request a person made through the agent; nil for the owner's own.
    var forWhom: String? {
        guard let h = askedByHandle, !h.isEmpty else { return nil }
        return "@\(h)"
    }

    init(dto: SecretRequestDto) {
        self.id = dto.id
        self.projectId = dto.project_id
        self.projectName = dto.project_name
        self.name = dto.name
        self.purpose = dto.purpose
        self.use = dto.use
        self.host = dto.host
        self.status = dto.status
        self.createdAt = dto.created_at
        self.askedByHandle = dto.asked_by_handle
        self.askedByName = dto.asked_by_name
    }

    static func parse(_ v: Any?) -> SecretRequestItem? {
        SecretsDto.decode(SecretRequestDto.self, from: v).map(SecretRequestItem.init(dto:))
    }
}

/// A connected vault (Phase 2: 1Password). Listed by `GET /secrets` as `vaults[]`.
struct ConnectedVault: Identifiable, Equatable {
    var id: String
    var provider: String
    var name: String
    var itemsCount: Int
    var tokenExpiresAt: Int64?
    /// `ok | token_invalid | rate_limited | expired`.
    var status: String
    var readsUsedHour: Int?
    var readsPerHour: Int?
    var agents: [SecretAgentAccess]
    /// Items the owner put into the agents' environment as real values.
    var envItems: Int = 0

    var providerLabel: String { provider == "onepassword" ? "1Password" : provider.capitalized }

    init(dto: VaultDto) {
        self.id = dto.id
        self.provider = dto.provider
        self.name = dto.name
        self.itemsCount = Int(dto.items_count)
        self.tokenExpiresAt = dto.token_expires_at
        self.status = dto.status
        self.readsUsedHour = Int(dto.limits.reads_used_hour)
        self.readsPerHour = Int(dto.limits.reads_per_hour)
        self.agents = dto.agents.map(SecretAgentAccess.init(dto:))
        self.envItems = Int(dto.env_items)
    }

    /// Reduced project-page vault.
    init(summary: ProjectVaultSummaryDto) {
        self.id = summary.id
        self.provider = summary.provider
        self.name = summary.name
        self.itemsCount = Int(summary.items_count)
        self.tokenExpiresAt = nil
        self.status = summary.status
        self.readsUsedHour = nil
        self.readsPerHour = nil
        self.agents = []
        self.envItems = 0
    }

    static func parse(_ v: Any?) -> ConnectedVault? {
        SecretsDto.decode(VaultDto.self, from: v).map(ConnectedVault.init(dto:))
    }
}

/// `GET /secrets`.
struct SecretsOverview: Equatable {
    var paused: Bool
    var pausedAt: Int64?
    var vaults: [ConnectedVault]
    var secrets: [SecretItem]
    var requestsPending: Int
    var activity: [SecretActivity]
    /// Vault providers this relay can connect (`["onepassword"]` once Phase 2 is deployed).
    var providers: [String] = []
    /// Remote flag `features.vault_connect`: the Connect 1Password flow is shown only when the
    /// relay says so (default off since 2026-10-21; code kept).
    var vaultConnect = false
    /// Cards the owner has not answered yet, for the Secrets root ("Waiting for you").
    var requests: [SecretRequestItem] = []
    var loaded = true

    /// Memberwise init (kept explicit because `init(dto:)` would suppress synthesis).
    init(
        paused: Bool,
        pausedAt: Int64?,
        vaults: [ConnectedVault],
        secrets: [SecretItem],
        requestsPending: Int,
        activity: [SecretActivity],
        providers: [String] = [],
        vaultConnect: Bool = false,
        requests: [SecretRequestItem] = [],
        loaded: Bool = true
    ) {
        self.paused = paused
        self.pausedAt = pausedAt
        self.vaults = vaults
        self.secrets = secrets
        self.requestsPending = requestsPending
        self.activity = activity
        self.providers = providers
        self.vaultConnect = vaultConnect
        self.requests = requests
        self.loaded = loaded
    }

    init(dto: SecretsOverviewDto, raw: [String: Any] = [:]) {
        self.paused = dto.paused
        self.pausedAt = dto.paused_at
        self.vaults = dto.vaults.map(ConnectedVault.init(dto:))
        self.secrets = dto.secrets.map(SecretItem.init(dto:))
        self.requestsPending = Int(dto.requests_pending)
        let rawActivity = (raw["activity"] as? [Any]) ?? []
        self.activity = zip(dto.activity, rawActivity).compactMap { row, rawRow in
            SecretActivity(dto: row, raw: rawRow as? [String: Any] ?? [:])
        }
        if self.activity.count != dto.activity.count {
            self.activity = dto.activity.map { SecretActivity(dto: $0) }
        }
        self.providers = dto.providers
        self.vaultConnect = dto.features.vault_connect ?? false
        self.requests = dto.requests.map(SecretRequestItem.init(dto:))
        self.loaded = true
    }

    static func parse(_ v: Any?) -> SecretsOverview? {
        guard let dto = SecretsDto.decode(SecretsOverviewDto.self, from: v) else { return nil }
        return SecretsOverview(dto: dto, raw: v as? [String: Any] ?? [:])
    }

    /// Settings row value: `4 secrets · 1Password`, `No secrets yet`, `Paused`.
    var rowValue: String {
        if paused { return "Paused" }
        if secrets.isEmpty && vaults.isEmpty { return "No secrets yet" }
        var s = "\(secrets.count) secret\(secrets.count == 1 ? "" : "s")"
        if let v = vaults.first { s += " · \(v.providerLabel)" }
        return s
    }
}

/// One row of the agent's Secrets page (`GET /projects/{id}/secrets items[]`): an Access on a
/// secret or on a whole vault.
struct ProjectSecretAccess: Identifiable, Equatable {
    var id: String
    var enabled: Bool
    var note: String
    var hosts: [String]
    var lastUsedAt: Int64?
    var disabledAt: Int64?
    var targetKind: String
    var secret: SecretItem?
    var vault: ConnectedVault?

    /// The vault row shows the vault name alone: its lock tile and "everything in this vault" say the rest.
    var title: String { secret?.env ?? vault?.name ?? "Secret" }
    var icon: String { vault != nil ? "lock.rectangle.stack.fill" : (secret?.icon ?? "key.fill") }

    init(dto: ProjectSecretItemDto) {
        self.id = dto.access_id
        self.enabled = dto.enabled
        self.note = dto.note
        self.hosts = dto.hosts
        self.lastUsedAt = dto.last_used_at
        self.disabledAt = dto.disabled_at
        self.targetKind = dto.target_kind ?? (dto.vault != nil ? "vault" : "secret")
        self.secret = dto.secret.map(SecretItem.init(summary:))
        self.vault = dto.vault.map(ConnectedVault.init(summary:))
    }

    static func parse(_ v: Any?) -> ProjectSecretAccess? {
        SecretsDto.decode(ProjectSecretItemDto.self, from: v).map(ProjectSecretAccess.init(dto:))
    }
}

struct ProjectSecrets: Equatable {
    var paused: Bool
    var owner: Bool
    var items: [ProjectSecretAccess]
    var requests: [SecretRequestItem]
    var activity: [SecretActivity]

    init(dto: ProjectSecretsDto, raw: [String: Any] = [:]) {
        self.paused = dto.paused
        self.owner = dto.owner
        self.items = dto.items.map(ProjectSecretAccess.init(dto:))
        self.requests = dto.requests.map(SecretRequestItem.init(dto:))
        let rawActivity = (raw["activity"] as? [Any]) ?? []
        self.activity = zip(dto.activity, rawActivity).compactMap { row, rawRow in
            SecretActivity(dto: row, raw: rawRow as? [String: Any] ?? [:])
        }
        if self.activity.count != dto.activity.count {
            self.activity = dto.activity.map { SecretActivity(dto: $0) }
        }
    }

    static func parse(_ v: Any?) -> ProjectSecrets? {
        guard let dto = SecretsDto.decode(ProjectSecretsDto.self, from: v) else { return nil }
        return ProjectSecrets(dto: dto, raw: v as? [String: Any] ?? [:])
    }
}

enum SecretsText {
    /// The note field: what agents may do with the key. Same words on both platforms.
    static let notePlaceholder = "What may agents do with it? e.g. read-only · confirm before charging · may spend up to $20 without asking"
    static let noteHint = "Agents follow this as the rule for the key — for egress and for programs that read the real value. Say what it is for, what is allowed, and any spend limit; without one they confirm before anything that charges."
    static let accessNoteHint = "Shown whenever the agent sees this secret, on top of the secret's own note. Allowed actions and spend limits for this agent, e.g. reports only · may spend up to $5 a day."
    static let vaultNoteHint = "The rule for every item in this vault, for the agents you pick: what it is for, what is allowed, any spend limit. No spending guidance means they confirm before anything that charges."

    /// The variables a secret's real value takes: "$STRIPE_KEY", or
    /// "$GITHUB_LOGIN_USERNAME and $GITHUB_LOGIN_PASSWORD" for a login.
    static func realVarList(_ s: SecretItem) -> String {
        s.kind == "login" ? "$\(s.env)_USERNAME and $\(s.env)_PASSWORD" : "$\(s.env)"
    }

    /// "used 2 min ago" · "never used" · "turned off Oct 12".
    static func usage(lastUsedAt: Int64?, disabledAt: Int64? = nil, enabled: Bool = true) -> String {
        if !enabled, let d = disabledAt { return "turned off \(shortDate(d))" }
        guard let t = lastUsedAt else { return "never used" }
        return "used \(ago(t))"
    }

    /// "asked 2 min ago" · "never asked" · "turned off Oct 12" for a People row.
    static func asked(lastAskedAt: Int64?, disabledAt: Int64? = nil, enabled: Bool = true) -> String {
        if !enabled, let d = disabledAt { return "turned off \(shortDate(d))" }
        guard let t = lastAskedAt else { return "never asked" }
        return "asked \(ago(t))"
    }

    static func ago(_ ms: Int64) -> String {
        let date = Date(timeIntervalSince1970: Double(ms) / 1000)
        if Date().timeIntervalSince(date) < 60 { return "just now" }
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .short
        return f.localizedString(for: date, relativeTo: Date())
    }

    static func shortDate(_ ms: Int64) -> String {
        let date = Date(timeIntervalSince1970: Double(ms) / 1000)
        let f = DateFormatter()
        f.setLocalizedDateFormatFromTemplate(Calendar.current.isDate(date, equalTo: Date(), toGranularity: .year) ? "MMM d" : "MMM d yyyy")
        return f.string(from: date)
    }

    /// Name rules from the relay (§4): letters, digits, space, `_`, `-`, `.`; ≤ 64.
    static func envName(_ name: String) -> String {
        var s = ""
        for ch in name.trimmingCharacters(in: .whitespaces) {
            if ch.isASCII && (ch.isLetter || ch.isNumber) { s.append(ch.uppercased()) } else { s.append("_") }
        }
        while s.contains("__") { s = s.replacingOccurrences(of: "__", with: "_") }
        s = s.trimmingCharacters(in: CharacterSet(charactersIn: "_"))
        if let f = s.first, f.isNumber { s = "_" + s }
        return s
    }

    static func validName(_ name: String) -> Bool {
        let t = name.trimmingCharacters(in: .whitespaces)
        guard !t.isEmpty, t.count <= 64 else { return false }
        guard t.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "_" || $0 == "-" || $0 == "." || $0 == " ") }) else { return false }
        return !envName(t).isEmpty
    }
}
