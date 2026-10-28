// App-wide search from the iPhone's bottom bar (Jacob, 2026-10-25, [feedback
// iphone-bottom-search-bar-2026-10-25]): the bottom tab bar is gone and ONE
// search bar sits in its place — nothing beside it. Typing finds everything,
// the Mac palette's way (crates/arbos-mac `palette.rs`, ⌘⇧F): agents, people
// (DMs and people met in a group or the directory), groups, servers and their
// channels, message text anywhere on this phone, and the files, pictures and
// links shared in chats. The ranked name/handle matching of `HomeSearch` stays
// the matcher for chat rows; this module decides what the rows are, how they
// group and in what order they come. Sections, copy and caps are the Mac's.

import ArbosFFI
import Foundation

/// The home screen's bottom chrome: what sits under the list.
enum HomeChrome {
    /// A control on the home screen's bottom edge.
    enum BottomControl: Equatable {
        case tabBar
        case searchBar
        case button(String)
    }

    /// The one control the bottom edge carries (Jacob: "a search bar. Nothing else").
    static let bottom: [BottomControl] = [.searchBar]

    /// Search placeholder — the Mac palette's word.
    static let searchPlaceholder = "Search"

    /// Where the four tabs' destinations live now that the bar is gone.
    /// Copy for the report and the tests; the views wire the real actions.
    static let homes: [(String, String)] = [
        ("Settings", "home header: the account picture (left) opens Settings"),
        ("Contacts", "home header: + → New message lists people and the directory; the search bar's People & agents"),
        ("Calls", "a chat's header call buttons and a person's pane; no call history list existed"),
        ("Unread", "the app icon badge and each row's unread dot / count pill"),
        ("New chat", "home header: the + circle"),
    ]
}

enum GlobalSearch {
    /// What a row is; decides its picture and its section.
    enum Kind: Equatable {
        case agent, person, group, server, channel, message, media, file, link
        /// A Settings page, sub-page or an agent's setting row.
        case setting
    }

    /// The group a row is listed under. Order = list order.
    enum Section: Int, Comparable, CaseIterable {
        case recent, people, chats, servers, settings, messages, content

        static func < (lhs: Section, rhs: Section) -> Bool { lhs.rawValue < rhs.rawValue }

        /// The sections a typed query can fill, in list order.
        static let withQuery: [Section] = [.people, .chats, .servers, .settings, .messages, .content]

        /// The Mac palette's headings.
        var label: String {
            switch self {
            case .recent: return "Recent"
            case .people: return "People & agents"
            case .chats: return "Chats"
            case .servers: return "Servers & channels"
            case .settings: return "Settings"
            case .messages: return "Messages"
            case .content: return "Files, media & links"
            }
        }
    }

    /// How well a row's own name matched: decides the order inside a section.
    /// Mirrors the Mac palette's `Rank` over the shared matcher (`EntitySearch`).
    enum Rank: Equatable, Comparable {
        /// The name, @handle or title matched through the shared matcher:
        /// exact > prefix > word-start, name before handle before title.
        case direct(kind: SearchMatchKind, field: SearchMatchField)
        /// Something other than the name matched: a member, a chat title, a message.
        case indirect

        /// Every row's rank on an empty query (nothing to order by but activity).
        static let everything = Rank.direct(kind: .prefix, field: .name)

        /// The core's `Match::order` (0 best): exact / prefix / word-start on
        /// name then handle (0–5), then the title hits (6–8); indirect last.
        var order: Int {
            switch self {
            case .direct(let kind, let field):
                let k: Int
                switch kind {
                case .exact: k = 0
                case .prefix: k = 1
                case .wordStart: k = 2
                }
                switch field {
                case .name: return k * 2
                case .handle: return k * 2 + 1
                case .title: return 6 + k
                }
            case .indirect:
                return 9
            }
        }

        static func < (lhs: Rank, rhs: Rank) -> Bool { lhs.order < rhs.order }

        /// Rank a name (a title, a channel, a file name) against the typed query.
        static func of(_ name: String, q: String) -> Rank {
            of(name: name, handle: "", q: q)
        }

        /// Rank an entity (display name, @handle, title) against the typed query.
        static func of(name: String, handle: String, title: String? = nil, q: String) -> Rank {
            guard let m = EntitySearch.match(name: name, handle: handle, title: title, query: q) else { return .indirect }
            return .direct(kind: m.kind, field: m.field)
        }
    }

    /// What opening a row does.
    enum Target: Equatable {
        /// Open an agent's landing chat.
        case project(Id)
        /// Open one chat under an agent (a chat-title hit).
        case chat(chatId: Id, projectId: Id)
        /// Open a DM or group.
        case people(Id)
        /// Open a chat (agent, people or channel) scrolled to the message that matched.
        case message(chatId: Id, projectId: Id, messageId: Id)
        /// Enter the server (its channel list).
        case server(Id)
        /// Open one channel of a server.
        case channel(serverId: Id, channelId: Id)
        /// Start (or open) the DM with a person who has no chat yet.
        case person(handle: String)
        /// Open a picture / video / file in the viewer — the Library item.
        case item(chatId: Id, projectId: Id, itemId: String)
        /// Open a link in Safari.
        case link(URL)
        /// Open Settings on a page, the feedback sheet, my Profile, or an agent's setting.
        case setting(SettingsRoute)
    }

    /// A page Settings can land on; the raw value is the `ARBOS_SETTINGS_PAGE`
    /// word `SettingsView(landing:)` already understands.
    enum SettingsPageRoute: String, CaseIterable, Equatable {
        case appearance, notifications, secrets, devices, machine, about, privacy, credits
        case newSecret = "new-secret"
        case connectOnePassword = "connect"
        case textSize = "textsize"
        case connectedApps = "connectors"
        /// The Settings list itself (On this iPhone has no page of its own).
        case root = "settings"
    }

    /// One of the agent pane's settings rows.
    enum AgentSetting: Equatable {
        case model, reasoning, voice
    }

    /// Where a Settings row lands.
    enum SettingsRoute: Equatable {
        case page(SettingsPageRoute)
        case feedback
        case profile
        case agent(projectId: Id, setting: AgentSetting)
    }

    /// An owned agent as the Settings rows need it (the open chat's agent first).
    struct AgentRow: Equatable {
        var projectId: Id
        var name: String
        var model: String
        var reasoning: String
        var voice: String
    }

    /// Which of the iPhone's pages a shared index item is; nil for the Mac-only
    /// Updates page and for the per-agent items (built with the agent id).
    static func settingsRoute(for item: SettingsItem) -> SettingsRoute? {
        switch item {
        case .appearance, .appearanceOpacity: return .page(.appearance)
        case .notifications: return .page(.notifications)
        case .secrets: return .page(.secrets)
        case .secretsNew: return .page(.newSecret)
        case .secretsConnectOnePassword: return .page(.connectOnePassword)
        case .devices: return .page(.devices)
        case .onThisDevice: return .page(.root)
        case .machine: return .page(.machine)
        case .updates: return nil
        case .sendFeedback: return .feedback
        case .about: return .page(.about)
        case .privacy: return .page(.privacy)
        case .textSize: return .page(.textSize)
        case .credits: return .page(.credits)
        case .connectedApps: return .page(.connectedApps)
        case .profile: return .profile
        case .agentModel, .agentReasoning, .agentVoice: return nil
        }
    }

    /// The SF Symbol a Settings row's tile carries — the Settings list's own.
    static func settingsGlyph(_ route: SettingsRoute) -> String {
        switch route {
        case .page(let page):
            switch page {
            case .appearance: return "circle.lefthalf.filled"
            case .notifications: return "bell.badge.fill"
            case .secrets, .newSecret, .connectOnePassword: return "key.fill"
            case .devices: return "laptopcomputer"
            case .machine: return "desktopcomputer"
            case .about: return "info.circle.fill"
            case .privacy: return "hand.raised.fill"
            case .credits: return "creditcard.fill"
            case .textSize: return "textformat.size"
            case .connectedApps: return "link"
            case .root: return "iphone"
            }
        case .feedback: return "exclamationmark.bubble.fill"
        case .profile: return "person.crop.circle"
        case .agent(_, let setting):
            switch setting {
            case .model: return "cpu"
            case .reasoning: return "sparkles"
            case .voice: return "waveform"
            }
        }
    }

    /// "opacity" → "Opacity": the matched keyword as the subtitle's first word.
    static func keywordLabel(_ keyword: String) -> String {
        guard let first = keyword.first else { return "" }
        return first.uppercased() + keyword.dropFirst()
    }

    /// The Settings rows for the typed query: one per matching page or
    /// sub-page, one per owned agent for Model · Reasoning · Voice (`agents`
    /// in list order). `when` carries that order so `arrange` keeps it.
    static func settingsEntries(query: String, agents: [AgentRow]) -> [Entry] {
        var out: [Entry] = []
        for hit in settingsSearch(query: query) {
            let rank = Rank.direct(kind: hit.kind, field: hit.keyword == nil ? .name : .title)
            let lead = hit.keyword.map(keywordLabel)
            let path = hit.entry.path
            if hit.entry.perAgent {
                let setting: AgentSetting
                switch hit.entry.item {
                case .agentModel: setting = .model
                case .agentReasoning: setting = .reasoning
                default: setting = .voice
                }
                for (i, agent) in agents.enumerated() {
                    let value: String
                    switch setting {
                    case .model: value = agent.model
                    case .reasoning: value = agent.reasoning
                    case .voice: value = agent.voice
                    }
                    var subtitle = "\(agent.name) · \(path)"
                    if !value.isEmpty { subtitle += " · \(value)" }
                    if let lead { subtitle = "\(lead) · \(subtitle)" }
                    out.append(Entry(kind: .setting, title: hit.entry.title, subtitle: subtitle,
                                     when: Int64.max - Int64(i), rank: rank,
                                     target: .setting(.agent(projectId: agent.projectId, setting: setting)),
                                     picture: .project(agent.projectId)))
                }
                continue
            }
            guard let route = settingsRoute(for: hit.entry.item) else { continue }
            let subtitle = lead.map { "\($0) · \(path)" } ?? path
            out.append(Entry(kind: .setting, title: hit.entry.title, subtitle: subtitle,
                             when: Int64.max - Int64(out.count), rank: rank,
                             target: .setting(route), picture: .settingsTile(settingsGlyph(route))))
        }
        return out
    }

    /// One result row.
    struct Entry: Identifiable, Equatable {
        var kind: Kind
        var title: String
        var subtitle: String
        /// Newest activity (ms); orders rows inside a section.
        var when: Int64
        /// Name hit (the shared matcher's verdict) or indirect (member, chat title, message).
        var rank: Rank
        var target: Target
        /// Picture source: a project id, a people chat id, a server id, a person's
        /// avatar URL + handle, a picture's viewer item for a thumbnail; none → the kind's glyph.
        var picture: Picture = .none
        var id: String { "\(kind):\(title):\(target)" }

        /// A person with no chat yet: opening the row starts the DM.
        var startsADM: Bool {
            if case .person = target { return true }
            return false
        }

        /// Which section this row lists under when a query is typed.
        var section: Section {
            switch kind {
            case .agent, .person:
                return rank == .indirect ? .chats : .people
            case .group:
                return .chats
            case .server, .channel:
                return .servers
            case .message:
                return .messages
            case .media, .file, .link:
                return .content
            case .setting:
                return .settings
            }
        }

        /// A Settings row whose title or keyword *is* the query: the section
        /// moves to the top of the list.
        var isExactSetting: Bool {
            guard kind == .setting, case .direct(let kind, _) = rank else { return false }
            return kind == .exact
        }
    }

    /// What a row draws at its leading edge.
    enum Picture: Equatable {
        case none
        case project(Id)
        case peopleChat(Id)
        case server(Id)
        case person(url: String?, handle: String, name: String?, id: String?, colorIndex: Int?)
        case thumbnail(MediaItem)
        /// The Settings list's grey tile with this SF Symbol.
        case settingsTile(String)
        /// A link's tile (`LinkTile`: brand mark, the relay's icon, or the host's letter).
        case link(URL)
    }

    /// Rows per section with a query (the list scrolls, but a long tail of
    /// message hits would bury the servers and files).
    static let sectionCap = 8
    /// Rows on the empty query (recent chats and servers by activity).
    static let recentCap = 24

    struct Group: Equatable {
        var section: Section
        var rows: [Entry]
    }

    /// Group and order `entries` for the list. Empty `query`: one **Recent**
    /// section by activity (chats and servers; no files, messages or DM-less
    /// people). Otherwise the five sections in order, each with its name hits
    /// first, newest first, capped at `sectionCap`.
    static func arrange(_ entries: [Entry], queryIsEmpty: Bool) -> [Group] {
        if queryIsEmpty {
            var recent = entries.filter { [.agent, .person, .group, .server].contains($0.kind) && !$0.startsADM }
            recent.sort { $0.when > $1.when }
            if recent.count > recentCap { recent.removeLast(recent.count - recentCap) }
            return recent.isEmpty ? [] : [Group(section: .recent, rows: recent)]
        }
        var out: [Group] = []
        for section in Section.withQuery {
            var rows = entries.filter { $0.section == section }
            if rows.isEmpty { continue }
            rows.sort { a, b in
                if a.rank != b.rank { return a.rank < b.rank }
                return a.when > b.when
            }
            if rows.count > sectionCap { rows.removeLast(rows.count - sectionCap) }
            out.append(Group(section: section, rows: rows))
        }
        if let i = out.firstIndex(where: { group in group.section == .settings && group.rows.contains { row in row.isExactSetting } }) {
            out.insert(out.remove(at: i), at: 0)
        }
        return out
    }

    /// The rows in list order, flattened across sections.
    static func flatten(_ groups: [Group]) -> [Entry] { groups.flatMap(\.rows) }

    /// "Server · 3 channels · 12 members".
    static func serverSubtitle(channels: Int, members: Int) -> String {
        var s = "Server · \(channels) channel\(channels == 1 ? "" : "s")"
        if members > 0 { s += " · \(members) member\(members == 1 ? "" : "s")" }
        return s
    }

    /// "Group · 4 members".
    static func groupSubtitle(members: Int) -> String {
        "Group · \(members) member\(members == 1 ? "" : "s")"
    }

    /// The host of a link for its subtitle ("github.com"), the URL itself when it has none.
    static func linkHost(_ url: String) -> String {
        var rest = url
        for prefix in ["https://", "http://"] where rest.hasPrefix(prefix) {
            rest = String(rest.dropFirst(prefix.count))
            break
        }
        var host = rest.split(whereSeparator: { "/?#".contains($0) }).first.map(String.init) ?? rest
        if host.hasPrefix("www.") { host = String(host.dropFirst(4)) }
        return host.isEmpty ? url : host
    }

    /// Copy when a query finds nothing.
    static func emptyCopy(for query: String) -> String {
        "No results for \u{201C}\(query.trimmingCharacters(in: .whitespaces))\u{201D}"
    }
}
