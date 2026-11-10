// Settings → Agent access link: the account's agent tokens, from the relay's
// owner-side routes (`docs/agent-access-link-spec.md` §6.1, built in the relay as
// `crates/arbos-relay/src/agent_link.rs`):
//
//   POST   /me/agent-links            {label?, scope, ttl_hours?}  → the row plus `url`, once
//   GET    /me/agent-links                                         → every agent token
//   DELETE /me/agent-links/{id}                                    → 204, immediate
//
// The owner copies ONE thing and it is the URL. The mint reply carries no bearer: the
// agent fetches the URL and collects its own key from that one response, so nothing
// secret ever passes through the owner's hands and the thing they pasted is worthless
// the moment it is used. Nothing in this file ever reads or stores a bearer.
//
// `GET /me/agent-links` is a union over two storage homes, which is why a row carries
// `origin`: `link` is an `agent_links` row, `export` is a `devices` row with
// `kind = "reader"` — the long-lived read-only `adv_` token that predates links. The
// relay joins them so Mac and iPhone do not each write their own join.

import Foundation

/// What a link may do. Two values and no more, fixed when the link is made: there is no
/// route that raises one, so widening means revoke and mint again.
enum AgentLinkScope: String, CaseIterable, Identifiable, Equatable {
    case read
    case reply

    var id: String { rawValue }

    /// An unreadable scope is the weaker of the two, never the stronger (the relay's own rule).
    static func parse(_ raw: String?) -> AgentLinkScope {
        AgentLinkScope(rawValue: raw ?? "") ?? .read
    }

    /// The choice row's label.
    var label: String {
        switch self {
        case .read: return "Read only"
        case .reply: return "Read + reply as me"
        }
    }

    /// The consequence under the label — what it can and cannot do, in one line.
    var consequence: String {
        switch self {
        case .read: return "It can read your chats. It cannot post."
        case .reply: return "It can post as you. People see your name."
        }
    }

    /// The card's footer, which swaps with the choice: the dangerous sentence is never on
    /// screen while the safe option is selected, so it cannot become furniture the eye skips.
    /// Wording matches the Mac (`crates/arbos-mac/src/ui/agent_links.rs`).
    var footer: String {
        switch self {
        case .read:
            return "This link can only read. Nothing it does appears as a message from you."
        case .reply:
            return "Replies from this link are from you. Nobody you talk to can tell them apart from your own messages."
        }
    }

    /// The Create button names the scope, so the consequence is restated at the moment of
    /// commitment and there is no generic Create to press.
    var createAction: String {
        switch self {
        case .read: return "Create read-only link"
        case .reply: return "Create reply-as-me link"
        }
    }

    /// The capsule on a list row and in the copy sheet.
    var tag: String {
        switch self {
        case .read: return "read only"
        case .reply: return "replies as you"
        }
    }

    /// The copy sheet's restatement, which carries the expiry too: you should not be able to
    /// copy a reply-as-me link believing it is read only.
    func restatement(_ expiry: AgentLinkExpiry) -> String {
        switch self {
        case .read: return "This agent only reads your chats. Expires in \(expiry.label.lowercased())."
        case .reply: return "Anything this agent posts is from you. Expires in \(expiry.label.lowercased())."
        }
    }

    /// The glyph on the copy sheet's scope tile (an SF Symbol). The tile is not an identifier —
    /// its fill is what carries the warning's colour — so the two scopes wear different marks.
    /// Matches the Mac reveal sheet.
    var glyph: String {
        switch self {
        case .read: return "eye"
        case .reply: return "arrowshape.turn.up.left"
        }
    }

    /// Red means exactly one thing in this section: this can speak as you.
    var isDangerous: Bool { self == .reply }
}

/// How long a new link lives. The spec's set (§5.3) and no "Never": an agent link is a
/// session, not a standing integration, and the standing one is outside agents.
enum AgentLinkExpiry: Int, CaseIterable, Identifiable, Equatable {
    case oneHour = 1
    case oneDay = 24
    case sevenDays = 168
    case thirtyDays = 720

    var id: Int { rawValue }
    /// What `POST /me/agent-links` takes as `ttl_hours`.
    var ttlHours: Int { rawValue }
    static let standard: AgentLinkExpiry = .sevenDays

    var label: String {
        switch self {
        case .oneHour: return "1 hour"
        case .oneDay: return "24 hours"
        case .sevenDays: return "7 days"
        case .thirtyDays: return "30 days"
        }
    }
}

/// One agent token of my account, as `GET /me/agent-links` returns it. Never a bearer and
/// never a URL: the relay does not send either on this route, on any row, of either origin.
struct AgentAccessLink: Identifiable, Equatable {
    var id: String
    /// `link` (an `agent_links` row) or `export` (a reader token minted before links existed).
    var origin: String
    var label: String
    var scope: AgentLinkScope
    var createdAt: Int64
    /// `nil` on an export token, which never expires — one of the reasons the row says so.
    var expiresAt: Int64?
    /// When the URL was fetched. `nil` means nobody has picked the link up yet.
    var openedAt: Int64?
    var revokedAt: Int64?
    var lastSeenAt: Int64?
    /// Not revoked and not past its expiry, as the relay computed it against its own clock.
    var live: Bool
    /// The relay's own sentence about a row that is honestly unlike the others (an export
    /// token keeps no activity record). Shown as the list's footnote, never invented here.
    var note: String?

    /// An export reader token: the earlier generation of this idea. It is listed here so the
    /// account has one place that means "tokens I gave an agent", but it is not called a link.
    var isExportToken: Bool { origin == "export" }

    static func parse(_ v: Any?) -> AgentAccessLink? {
        guard let d = v as? [String: Any], let id = d["id"] as? String else { return nil }
        let note = (d["note"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        return AgentAccessLink(
            id: id,
            origin: (d["origin"] as? String) ?? "link",
            label: (d["label"] as? String) ?? "",
            scope: AgentLinkScope.parse(d["scope"] as? String),
            createdAt: (d["created_at"] as? NSNumber)?.int64Value ?? 0,
            expiresAt: (d["expires_at"] as? NSNumber)?.int64Value,
            openedAt: (d["opened_at"] as? NSNumber)?.int64Value,
            revokedAt: (d["revoked_at"] as? NSNumber)?.int64Value,
            lastSeenAt: (d["last_seen_at"] as? NSNumber)?.int64Value,
            live: (d["live"] as? Bool) ?? ((d["revoked_at"] as? NSNumber) == nil),
            note: note
        )
    }

    /// The row's name, never empty.
    var displayName: String {
        let t = label.trimmingCharacters(in: .whitespaces)
        return t.isEmpty ? "Agent link" : t
    }

    /// "Created Tue · used 2 h ago" / "Created 25 Oct · never opened": coarse for the fact you
    /// already know, fine for the fact you are checking. A link you handed to an outside
    /// debugger makes "never opened" the whole answer to whether the agent ever picked it up.
    func subtitle(now: Int64 = Int64(Date().timeIntervalSince1970 * 1000)) -> String {
        "Created \(AgentAccessLink.createdLabel(createdAt, now: now)) · \(usedLabel(now: now))"
    }

    /// A weekday inside the last week ("Tue"), the date before that ("25 Oct"). The full clock
    /// time did not fit the row on either platform.
    static func createdLabel(_ ms: Int64, now: Int64 = Int64(Date().timeIntervalSince1970 * 1000)) -> String {
        guard ms > 0 else { return "—" }
        let date = Date(timeIntervalSince1970: Double(ms) / 1000)
        let f = DateFormatter()
        f.setLocalizedDateFormatFromTemplate(now - ms < 7 * 24 * 3_600_000 ? "EEE" : "d MMM")
        return f.string(from: date)
    }

    /// "used 2 h ago", or "never opened" written in words rather than left blank.
    /// Matches the Mac: a link nobody has fetched stays "never opened" even if a last-seen
    /// stamp somehow arrives without an open.
    func usedLabel(now: Int64 = Int64(Date().timeIntervalSince1970 * 1000)) -> String {
        guard let opened = openedAt, opened > 0 else { return "never opened" }
        let seen = lastSeenAt.flatMap { $0 > 0 ? $0 : nil } ?? opened
        return "used \(AgentAccessLink.ago(seen, now: now))"
    }

    /// "4 min ago" / "3 d ago", the Devices page's own scale.
    static func ago(_ ms: Int64, now: Int64 = Int64(Date().timeIntervalSince1970 * 1000)) -> String {
        let min = max(now - ms, 0) / 60_000
        if min < 2 { return "just now" }
        if min < 60 { return "\(min) min ago" }
        if min < 24 * 60 { return "\(min / 60) h ago" }
        return "\(min / (24 * 60)) d ago"
    }

    /// The longest label the relay accepts; a longer name is cut rather than sent and refused.
    static let labelMax = 60

    /// The label to send for a link the owner named `typed`. Never empty: the relay stores one
    /// literal "Agent link" for a blank, so numbering is composed here (`nextLabel`) the way
    /// the Mac does it.
    static func labelFor(typed: String, items: [AgentAccessLink]) -> String {
        let trimmed = typed.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty { return nextLabel(items) }
        return String(trimmed.prefix(labelMax))
    }

    /// "Agent link", then "Agent link 2", past whatever is already live.
    static func nextLabel(_ items: [AgentAccessLink]) -> String {
        let base = "Agent link"
        let taken = Set(active(items).map { $0.label.trimmingCharacters(in: .whitespaces) })
        if !taken.contains(base) { return base }
        var n = 2
        while taken.contains("\(base) \(n)") { n += 1 }
        return "\(base) \(n)"
    }

    /// The list the page draws: live rows only, newest first. The relay returns revoked and
    /// expired rows as well — the section is "Active links", and Revoke means nothing on a
    /// link that is already stopped.
    static func active(_ list: [AgentAccessLink]) -> [AgentAccessLink] {
        list.filter(\.live).sorted { $0.createdAt > $1.createdAt }
    }

    /// An export reader token as this list's row, built on the phone from the Devices list.
    ///
    /// The relay's own list already joins the two storage homes, so this is only reached when
    /// `GET /me/agent-links` is not there to answer: the relay half of the feature deploys
    /// separately from this build. Without it, filtering reader rows out of Devices would
    /// leave a live token invisible and unrevokeable from the phone — the ordering hazard in
    /// `internal/agent-link-export-token-relationship.md` §3.
    static func fromReaderDevice(_ d: AuthDevice) -> AgentAccessLink {
        AgentAccessLink(
            id: d.id,
            origin: "export",
            label: d.displayName,
            scope: .read,
            createdAt: d.createdAt,
            expiresAt: nil,
            openedAt: nil,
            revokedAt: d.revokedAt,
            lastSeenAt: d.lastSeenAt,
            live: d.isLive,
            note: "This token predates agent links: it never expires and keeps no activity record."
        )
    }

    /// The mockups' three rows for the fixture build: a reply-as-me link in use, a read-only
    /// link nobody has picked up, and the Grok Bot reader token that moved out of Devices.
    static func fixture(now: Int64 = Int64(Date().timeIntervalSince1970 * 1000)) -> [AgentAccessLink] {
        [
            AgentAccessLink(id: "lnk-fx-claude", origin: "link", label: "Jacob's Claude", scope: .reply,
                            createdAt: now - 2 * 24 * 3_600_000, expiresAt: now + 5 * 24 * 3_600_000,
                            openedAt: now - 2 * 24 * 3_600_000 + 60_000, revokedAt: nil,
                            lastSeenAt: now - 2 * 3_600_000, live: true, note: nil),
            // A minute older than the link above, so the list's "newest first" has one answer.
            AgentAccessLink(id: "lnk-fx-codex", origin: "link", label: "Codex debugger", scope: .read,
                            createdAt: now - 2 * 24 * 3_600_000 - 60_000, expiresAt: now + 5 * 24 * 3_600_000,
                            openedAt: nil, revokedAt: nil, lastSeenAt: nil, live: true, note: nil),
            AgentAccessLink.fromReaderDevice(
                AuthDevice(id: "dev-fx-grokbot", name: "GrokBot-Arbos", kind: "reader",
                           createdAt: now - 9 * 24 * 3_600_000, lastSeenAt: now - 3 * 24 * 3_600_000,
                           current: false)
            ),
        ]
    }
}

/// What the copy sheet shows once: the one URL, and the scope and expiry it carries. There is
/// no bearer here because `POST /me/agent-links` does not send one.
struct AgentLinkReveal: Identifiable, Equatable {
    var id: String
    var label: String
    var scope: AgentLinkScope
    var expiry: AgentLinkExpiry
    /// `https://relay.arbos.life/agent-link/open/<open-secret>` — single use, 15 minutes.
    var url: String

    static func parse(_ v: Any?, expiry: AgentLinkExpiry) -> AgentLinkReveal? {
        guard let d = v as? [String: Any],
              let id = d["id"] as? String,
              let url = d["url"] as? String, !url.isEmpty else { return nil }
        return AgentLinkReveal(
            id: id,
            label: (d["label"] as? String) ?? "",
            scope: AgentLinkScope.parse(d["scope"] as? String),
            expiry: expiry,
            url: url
        )
    }

    /// `relay.arbos.life/…/9kT2vQ`: the host, then the tail of the secret. The full URL is
    /// what Copy writes; this is only what the row can fit.
    static func shortUrl(_ raw: String) -> String {
        var s = raw
        for p in ["https://", "http://"] where s.hasPrefix(p) { s.removeFirst(p.count) }
        let parts = s.split(separator: "/")
        guard let host = parts.first, parts.count > 1, let tail = parts.last else { return s }
        return "\(host)/…/\(tail.suffix(6))"
    }
}

extension RelayAPI {
    /// `GET /me/agent-links`. A relay without the route answers 404, which the caller reads
    /// as "this relay has no agent links yet" rather than as a failure.
    func agentLinks() async throws -> [AgentAccessLink] {
        guard let list = try await json("GET", "/me/agent-links") as? [Any] else {
            throw RelayError(status: 0, message: "Bad agent links reply")
        }
        return list.compactMap(AgentAccessLink.parse)
    }

    /// `POST /me/agent-links {label, scope, ttl_hours}` → the row and its URL, returned once.
    /// `label` is always sent (never blank): the caller composed it with `labelFor`.
    func createAgentLink(label: String, scope: AgentLinkScope, expiry: AgentLinkExpiry) async throws -> AgentLinkReveal {
        let body: [String: Any] = [
            "label": label,
            "scope": scope.rawValue,
            "ttl_hours": expiry.ttlHours,
        ]
        let v = try await json("POST", "/me/agent-links", body: body)
        guard let reveal = AgentLinkReveal.parse(v, expiry: expiry) else {
            throw RelayError(status: 500, message: "The relay answered without a link.")
        }
        return reveal
    }

    func revokeAgentLink(_ id: String) async throws {
        _ = try await request("DELETE", "/me/agent-links/\(id)")
    }
}

extension AppModel {
    /// The page's list: what the relay joined, plus any reader token the relay could not join
    /// because its half of the feature is not deployed yet (deduplicated by id, so the local
    /// rows disappear the moment the relay starts sending them).
    var agentLinkRows: [AgentAccessLink] {
        var rows = AgentAccessLink.active(agentLinks)
        let known = Set(rows.map(\.id))
        for d in AuthDevice.agentTokens(devices) where !known.contains(d.id) {
            rows.append(AgentAccessLink.fromReaderDevice(d))
        }
        return rows
    }

    /// Settings → Agent access link opened: read the list (the fixture seeds the mockups' three).
    func refreshAgentLinks() async {
        if isFixture {
            agentLinks = AgentAccessLink.fixture()
            agentLinksMintable = true
            agentLinksLoaded = true
            return
        }
        guard credentials != nil else { return }
        do {
            agentLinks = try await relay.agentLinks()
            agentLinksMintable = true
            agentLinksError = nil
        } catch let error as RelayError where error.status == 404 {
            // This relay predates the feature. The page still lists and revokes the reader
            // tokens it can see through Devices; it does not offer a Create that would 404.
            agentLinks = []
            agentLinksMintable = false
            agentLinksError = nil
        } catch {
            agentLinksError = UserError.classify("load your agent links", error)
        }
        agentLinksLoaded = true
    }

    /// Mint one. Returns the reveal for the copy sheet, or nil after a refusal the page's
    /// note has already claimed. The name that goes on the wire is composed here so a blank
    /// field never becomes five identical "Agent link" rows.
    func createAgentLink(label: String, scope: AgentLinkScope, expiry: AgentLinkExpiry) async -> AgentLinkReveal? {
        let name = AgentAccessLink.labelFor(typed: label, items: agentLinks)
        if isFixture {
            // Fixture URL: a short open-secret tail only — never a real bearer (`arbl_…`).
            let id = "lnk-fx-\(Int(Date().timeIntervalSince1970 * 1000) % 100_000)"
            let now = Int64(Date().timeIntervalSince1970 * 1000)
            agentLinks.insert(
                AgentAccessLink(id: id, origin: "link", label: name, scope: scope,
                                createdAt: now, expiresAt: now + Int64(expiry.ttlHours) * 3_600_000,
                                openedAt: nil, revokedAt: nil, lastSeenAt: nil, live: true, note: nil),
                at: 0
            )
            return AgentLinkReveal(id: id, label: name, scope: scope, expiry: expiry,
                                   url: "https://relay.arbos.life/agent-link/open/fxOpen1")
        }
        do {
            let reveal = try await relay.createAgentLink(label: name, scope: scope, expiry: expiry)
            await refreshAgentLinks()
            return reveal
        } catch {
            agentLinksError = UserError.classify("create the link", error)
            return nil
        }
    }

    /// Revoke one, of either origin. The row goes at once: revocation is immediate on the
    /// relay, so the list should not pretend to wait for it.
    func revokeAgentLink(_ id: String) {
        agentLinks.removeAll { $0.id == id }
        AuthDevice.removeOther(&devices, id: id)
        guard !isFixture else { return }
        Task {
            do {
                try await relay.revokeAgentLink(id)
            } catch {
                agentLinksError = UserError.classify("revoke that link", error)
            }
            await refreshAgentLinks()
            await refreshDevices()
        }
    }
}
