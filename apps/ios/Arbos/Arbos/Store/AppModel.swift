// App state: the relay registry, one Loro replica per open room, and the
// view of them the screens read. Everything runs on the main actor; Loro
// frames are ~130 bytes every 120 ms, so there is nothing to move off it.

import Foundation
import Combine
import SwiftUI
import UIKit
import os
import ArbosFFI

/// Theme choice: System follows the device; Dark is the default for anyone who
/// never picked (D90, which replaced D81's System default). The order is the
/// Settings order (System first, then Light, then Dark).
enum Appearance: String, CaseIterable, Identifiable {
    case system, light, dark
    var id: String { rawValue }
    /// What an unset preference means (D90).
    static let `default`: Appearance = .dark
    var label: String {
        switch self {
        case .system: return "System"
        case .light: return "Light"
        case .dark: return "Dark"
        }
    }
}

enum Surface: String, CaseIterable, Identifiable {
    case matte, glass
    var id: String { rawValue }
    /// What an unset preference means: Matte (D90; Glass under the D10 Q3
    /// amendment before it).
    static let `default`: Surface = .matte
    var label: String {
        switch self {
        case .matte: return "Matte"
        case .glass: return "Glass"
        }
    }
}

/// Who may add me to a people group (`PATCH /me {group_add}` / `GET /me` → `privacy`).
enum GroupAddPolicy: String, CaseIterable, Identifiable {
    case anyone
    case peopleIKnow = "people_i_know"
    case nobody
    var id: String { rawValue }
    var label: String {
        switch self {
        case .anyone: return "Anyone"
        case .peopleIKnow: return "People I message"
        case .nobody: return "Nobody"
        }
    }
    var summary: String {
        switch self {
        case .anyone: return "Anyone"
        case .peopleIKnow: return "People I message"
        case .nobody: return "Nobody"
        }
    }
}

struct Notice: Identifiable, Equatable {
    let id = UUID()
    var text: String
}

/// A toast at the top of the screen (Jacob 2026-10-26, [feedback error-messages-ios-2026-10-26]):
/// an info notice (`error` nil: "Forwarded to Studio", 6 s) or a failure drawn by `ErrorNote`
/// (10 s; a touch holds it; × dismisses; Try again re-runs the action). `ToastHost` draws them.
struct Toast: Identifiable, Equatable {
    let id: UUID
    var text: String
    var error: UserError?
    var detailsOpen = false
    var held = false
    /// Since when a touch holds it (the hold has a cap).
    var heldSince: Date? = nil
    var hasRetry = false
    var at: Date
    /// How many times this same failure arrived while the toast stood (1 = once); 2 and up
    /// is a "×N" on the note instead of a second toast ([feedback toast-stack-ios-2026-10-27]).
    var count: Int = 1

    static let infoTTL: TimeInterval = 6
    static let errorTTL: TimeInterval = 10
    /// A held toast (touch on it, Details closed) is let go after this long.
    static let holdMax: TimeInterval = 60
    /// How many the slot shows at once; older ones fold into one "N more" line.
    static let visibleMax = 3
    var ttl: TimeInterval { error == nil ? Toast.infoTTL : Toast.errorTTL }

    /// Two toasts are the same failure when they read the same sentence.
    func sameAs(_ other: UserError) -> Bool { error?.sentence == other.sentence }
}

/// What the slot draws: the newest toasts, oldest first, and how many older ones are folded.
struct ToastStack: Equatable {
    var visible: [Toast]
    var folded: Int
}

/// Which surface owns which failures while it is up (a sheet with a form open): a failure for
/// one of its verbs is kept for that surface (`AppModel.claimedFailure`) instead of a toast.
struct FailureClaim: Equatable {
    var surface: String
    var verbs: [String]
}

/// Last successful home registry, kept in the on-device store so reopening the
/// app paints the agent list before the relay round-trip finishes.
private struct RegistryCache: Codable {
    var projects: [Project]
    var chats: [Id: [Chat]]
}

/// Keys in the store's `kv` table.
private enum StoreKey {
    static let registry = "registry"
    static let people = "people"
    static let directory = "directory"
    static let serversAvailable = "serversAvailable"
    static let me = "me"
    static let unread = "unread"
    static let lastChat = "lastChat"
}

private let pushLog = Logger(subsystem: "com.unarbos.arbos.ios", category: "pushes")

@MainActor
final class AppModel: ObservableObject {
    /// The hosted relay; self-hosters change it under Advanced.
    static let defaultRelayUrl = "https://relay.arbos.life"

    /// D90: Dark is the default; the key is only written when the user picks a
    /// theme in Settings, so an absent key means "never chose" → Dark, and an
    /// explicit "system", "dark" or "light" stays.
    @Published var appearanceSetting: String = UserDefaults.standard.string(forKey: "appearance") ?? Appearance.default.rawValue {
        didSet { UserDefaults.standard.set(appearanceSetting, forKey: "appearance") }
    }
    /// Sum of SessionRoom reconnect / state_timeout counters (About page),
    /// plus the most recent non-nil room `lastError`.
    var sharedRoomStats: (reconnects: Int, timeouts: Int, lastError: String?) {
        var reconnects = 0
        var timeouts = 0
        var lastError: String?
        for room in rooms.values {
            reconnects += room.reconnects
            timeouts += room.stateTimeouts
            if let err = room.lastError { lastError = err }
        }
        return (reconnects, timeouts, lastError)
    }
    /// Q6 text size factor (`TextScale.steps`), on-device only.
    @Published var textScaleSetting: Double = {
        let v = UserDefaults.standard.double(forKey: TextScale.key)
        return v > 0 ? v : 1
    }() {
        didSet { UserDefaults.standard.set(textScaleSetting, forKey: TextScale.key) }
    }
    var textScale: CGFloat { CGFloat(textScaleSetting) }

    /// Matte is the default (D90). The key is only written when the user picks a
    /// surface in Settings, so an absent key means "never chose" → Matte, and an
    /// explicit "glass" (the D10 Q3 era default, when saved) stays Glass.
    @Published var surfaceSetting: String = UserDefaults.standard.string(forKey: "surface") ?? Surface.default.rawValue {
        didSet { UserDefaults.standard.set(surfaceSetting, forKey: "surface") }
    }

    /// Who may add me to people groups (`anyone` | `people_i_know` | `nobody`).
    /// Synced from `GET /me` → `privacy.group_add`; written with `PATCH /me`.
    @Published var groupAddPolicy: String = GroupAddPolicy.anyone.rawValue

    @Published private(set) var credentials: DeviceCredentials?
    /// Self-serve sign-in (GitHub). `tokens` is the JWT pair; `credentials`
    /// mirrors its access token so every existing call path keeps working.
    @Published private(set) var tokens: AuthTokens?
    @Published private(set) var user: AuthUser?
    #if DEBUG
    /// Fixture only: the signed-in user (D79 initials, `myUserId` for reactions).
    func setFixtureUser(_ who: AuthUser) { user = who }
    #endif
    @Published private(set) var credits: Credits?
    /// A newer TestFlight build than the one running (continuous delivery, D85): the Home card
    /// and Settings → About offer one tap into TestFlight. `nil` when up to date or dismissed.
    @Published private(set) var availableUpdate: AvailableBuild?
    /// D89: the host is full and this account is in line for a machine (`/me` →
    /// `machine.waitlisted` / `machine.waitlist_position`). nil = not waitlisted.
    @Published var waitlistPosition: Int?
    /// People chats (D72): DMs I am a member of, newest activity first.
    @Published var peopleChats: [PeopleChat] = [] {
        didSet {
            #if DEBUG
            PerfCounters.bump("pub.peopleChats")
            #endif
        }
    }
    /// Chat ids with unread rows: the relay's per-user `unread` flag plus live rows from
    /// others that landed while the chat was off screen (never `head_seq > read_seq`, #1438).
    @Published var peopleUnread: Set<String> = []
    @Published private(set) var blocked: [Person] = []
    /// Every Arbos user the relay knows (members + invited), for the New Message sheet.
    /// Restored from the device store at launch and refreshed in the registry loop, so
    /// the sheet opens on its final list instead of a spinner that the list replaces.
    @Published var directory: [Person] = []
    private var directoryLoadedAt: Int64 = 0
    /// How long a loaded directory stays fresh before the next `loadDirectory` refetches.
    nonisolated static let directoryTtlMs: Int64 = 600_000
    /// The chat currently on screen; pushes for it never notify while the
    /// scene is active (Mac: `window_active`). Background / lock clears focus
    /// so a reply still pushes a banner.
    var visibleChatId: String? {
        didSet {
            guard visibleChatId != oldValue else { return }
            if let old = oldValue { rooms[old]?.focused = false }
            if let new = visibleChatId {
                rooms[new]?.focused = appActive
                PushManager.shared.clearDelivered(chatId: new)
            }
        }
    }
    /// Scene is `.active`. Focus and in-app banner suppression only apply then.
    private var appActive = true
    /// True once the scene has been `.background` since the last `.active`.
    /// Unlock goes background → inactive → active; the immediate previous phase
    /// on unlock is `.inactive`, so we cannot key the redial off that alone.
    private var wasBackgrounded = false

    /// Called from the root scene on every `scenePhase` change.
    /// Leaving `.active` drops `focused_chat` on the relay (and stops swallowing
    /// banners) so APNs can fire. Returning to `.active` after `.background`
    /// redials rooms ([feedback app-01M5MVNNNBZHJWSRCZ81S04C8E]).
    func noteScenePhase(_ phase: ScenePhase) {
        if phase == .background { wasBackgrounded = true }
        let active = phase == .active
        let resumeRooms = active && wasBackgrounded
        if active { wasBackgrounded = false }

        let becameActive = active && !appActive
        let becameInactive = !active && appActive
        guard becameActive || becameInactive else { return }
        appActive = active
        if let id = visibleChatId {
            rooms[id]?.focused = active
            // Rows that landed while inactive (e.g. mid-call) lit the unread flag
            // because isWatching was false — clear now that we are back on screen.
            if active {
                if isPeopleChat(id) { markPeopleRead(id, force: true) }
                else { markRead(chatId: id, force: true) }
            }
        }
        // Leaving the foreground is the last safe moment to write before a kill.
        if becameInactive { flushStore(all: true) }
        // Foreground again: clear the 403/404 backoff so listed rooms can dial,
        // create any that are missing, and — after a real background — redial
        // sockets iOS left half-dead.
        if becameActive {
            roomUnavailableFailures = [:]
            roomUnavailableAt = [:]
            let existing = Array(rooms.values)
            recreateListedRooms()
            if resumeRooms {
                for room in existing { room.resumeAfterForeground() }
            }
        }
    }

    /// Test / call-site alias: only flips the active bit (no background redial).
    func setAppActive(_ active: Bool) {
        noteScenePhase(active ? .active : .inactive)
    }

    /// Chat on screen *and* the app is in the foreground — the only case where
    /// a message notification is suppressed.
    private func isWatching(_ chatId: String) -> Bool {
        appActive && visibleChatId == chatId
    }
    /// Per-chat unread counts from `GET /me/unread` (empty on relays without it).
    @Published private(set) var unreadCounts: [String: Int] = [:]
    /// A chat a notification asked for before its list had loaded.
    private var pendingOpen: (chatId: String, projectId: String)?
    private var notifiedPushIds: Set<String> = []
    @Published private(set) var authMode: AuthMode = .github
    @Published private(set) var signingIn = false
    @Published private(set) var signInError: UserError?
    @Published var signInErrorDetailsOpen = false
    /// After a fresh GitHub sign-in, ask for display name + handle once (Settings can edit later).
    @Published var needsProfileSetup = false
    private let webAuth = WebAuth()
    private var refreshTask: Task<Bool, Never>?
    @Published var projects: [Project] = [] {
        didSet {
        }
    }
    @Published var chats: [Id: [Chat]] = [:] {
        didSet {
            #if DEBUG
            PerfCounters.bump("pub.chats")
            #endif
        }
    }
    @Published var messages: [Id: [Message]] = [:] {
        didSet { bumpVersions(from: oldValue, to: messages) }
    }
    /// Membership notices this device shows ahead of the relay's row (ARB-393), per chat.
    private(set) var optimisticNotices: [Id: [OptimisticNotice]] = [:]
    /// Bumped whenever a chat's list (`messages` or `channelRows`) is replaced; views
    /// watch this instead of comparing the whole message array on every body.
    @Published private(set) var messagesVersion: [Id: Int] = [:]
    /// One pass over a chat's list, kept until that list changes.
    private var activityCache: [Id: ChatActivity] = [:]

    /// A chat's scan results (streaming / open hand-off / awaiting) and the view's
    /// change detection must not cost a pass over the list per screen body: both key
    /// off the version. Arrays are COW, so unchanged chats share storage with the old
    /// dictionary and keep their version (arbonites-chat-lag, 2026-10-24).
    private func bumpVersions(from old: [Id: [Message]], to new: [Id: [Message]]) {
        #if DEBUG
        PerfCounters.bump("pub.messages")
        #endif
        for (id, list) in new where !Self.sameStorage(list, old[id]) {
            messagesVersion[id, default: 0] &+= 1
            activityCache[id] = nil
        }
        for id in old.keys where new[id] == nil {
            messagesVersion[id, default: 0] &+= 1
            activityCache[id] = nil
        }
    }

    /// Same array buffer (an unchanged value carried over by copy-on-write).
    private static func sameStorage(_ a: [Message], _ b: [Message]?) -> Bool {
        guard let b, a.count == b.count else { return false }
        if a.isEmpty { return true }
        return a.withUnsafeBufferPointer { pa in b.withUnsafeBufferPointer { pb in pa.baseAddress == pb.baseAddress } }
    }
    /// The agent's open background jobs per project, as the relay's activity record last
    /// listed them (duplex §2.4) — the tasks chip and sheet. No entry = no open job.
    @Published var jobs: [Id: [Job]] = [:]
    private var jobsPolledAt: [Id: Int64] = [:]
    private var jobsPolling: Set<Id> = []
    /// Job tabs per agent project (`JobTabs`): one `job:<id>` chat per job the agent started,
    /// kept until the user closes it; never on the relay.
    @Published var jobTabs: [Id: [Chat]] = [:]
    /// Job tabs the user closed: not re-opened by the next listing (a link re-opens them).
    var closedJobTabs: Set<String> = []
    /// Each job's exchange as last fetched; the tab's transcript is synthesised from it.
    @Published var rollouts: [String: JobRollout] = [:]
    /// Steers typed in a job tab the box has not echoed yet, by job id.
    var pendingSteers: [String: [Message]] = [:]
    private var rolloutPolledAt: [String: Int64] = [:]
    private var rolloutPolling: Set<String> = []
    /// How often the open chat's jobs are polled while one is live (the design's "chip
    /// updates on the next poll"; stop ≤ 3 s).
    static let jobsPollMs: Int64 = 1_500
    @Published var caughtUp: Set<Id> = [] {
        didSet {
            #if DEBUG
            PerfCounters.bump("pub.caughtUp")
            #endif
        }
    }
    @Published var link: Link = .connecting
    /// Per-room link, for the chat header's connection pill. The pill must not
    /// use the global `link`: any other room's brief reconnect was painting
    /// "Reconnecting…" on a live chat (an agent that just messaged looked broken
    /// on an empty transcript). `roomLinkSince` is when each state began.
    @Published private(set) var roomLinks: [Id: LinkState] = [:]
    private(set) var roomLinkSince: [Id: Date] = [:]
    /// The relay's HTTP reachability (the registry refresh): `.offline` /
    /// `.relayDown` while it fails, `.live` as soon as a call succeeds or any room is live.
    @Published private(set) var relayLink: LinkState = .live
    private(set) var relayLinkSince = Date.distantPast
    /// A link state must hold this long before the pill shows it: a 250 ms redial
    /// or a sub-second catch-up never flashes a pill.
    static let linkPillGrace: TimeInterval = 1.2
    @Published private(set) var models: [String] = []
    @Published var machine: Machine = .unknown
    @Published private(set) var machineBusy = false
    /// D82: the first-login screen while the relay creates the computer and first
    /// agent; nil for every ordinary launch. Driven by `FirstLogin.swift`.
    @Published var firstLogin: FirstLogin?
    /// Where the first assistant bubble (the greeting) landed, in global
    /// coordinates — the morph target of the first-login terminal.
    @Published var greetingBubbleFrame: CGRect?
    var firstLoginTask: Task<Void, Never>?
    /// Rig: `ARBOS_OPEN_SERVER` was applied (once, after the first login if there is one).
    var openServerHookDone = false
    /// Rig: the tab the channel page opens on (`ARBOS_OPEN_SERVER=…:page-settings`).
    var openServerHookTab: String?
    /// Rig: `ARBOS_SHOW` ran once for this launch. A chat's `onAppear` fires again when a
    /// screen pushed over it is popped; without this a `jobtab` hook re-pushed the tab the
    /// moment a back swipe popped it.
    var showHookDone = false
    /// `machine.always_on` as last reported by `GET /me` (nil = field absent).
    private var meAlwaysOn: Bool?
    /// Navigation: home → chat. Shared so menus anywhere can open a chat.
    @Published var path: [Route] = [] {
        didSet {
            // The open chat is loaded first next launch and evicted last.
            if case .chat(let chatId, let pid)? = path.last, pid != channelSlot, let store {
                store.touchDoc(chatId)
                store.put(StoreKey.lastChat, Data(chatId.utf8))
            }
        }
    }
    /// A chat screen is on top of the stack: the toast slot is its header's, not the root's.
    var chatIsUp: Bool {
        if case .chat? = path.last { return true }
        return false
    }
    /// True when the Debug fixture replaced the relay (screenshots).
    var isFixture = false
    /// Debug demo tours run once per process, whichever view instance starts them.
    var demoStarted = false

    enum Route: Hashable {
        case chat(chatId: Id, projectId: Id)
        /// D88: a server's channel list (the slid-in left pane); its channels open as
        /// `.chat(chatId: channelId, projectId: channelSlot)` on top of it.
        case server(serverId: Id)
    }

    // MARK: Servers (D88)

    /// Servers I belong to, from `GET /servers` (names, roles, channels, invite link).
    /// Empty when the relay has no servers (`GET /me/matrix` → 503 `servers_off`).
    @Published var servers: [Server] = []
    /// The relay answers `GET /servers`: the `+` sheet offers "Server" / "Join a server".
    /// A relay with servers off (503 `servers_off`) shows nothing server-related.
    @Published var serversAvailable = false
    /// Channels muted on this phone (channel page › Notifications); local, like the Mac's mute list.
    @Published var mutedChannelIds: Set<String> = Set(UserDefaults.standard.stringArray(forKey: "mutedChannelIds") ?? []) {
        didSet { UserDefaults.standard.set(Array(mutedChannelIds), forKey: "mutedChannelIds") }
    }
    /// People group chats muted on this phone until the given date (`distantFuture` = until unmuted).
    /// Timed mutes live here; the relay's `me.muted` stays true while muted so other devices match.
    @Published var peopleMutedUntil: [String: Date] = AppModel.loadPeopleMutedUntil() {
        didSet { AppModel.savePeopleMutedUntil(peopleMutedUntil) }
    }
    /// The shared Matrix client (crates/arbos-matrix through ArbosFFI): the rooms' data plane.
    var matrixClient: ArbosFFI.MatrixClient?
    /// Retains the UniFFI foreign listener for the client's lifetime.
    var matrixListener: MatrixBridgeListener?
    /// Latest room list from the client: previews, unread and last activity per server/channel.
    @Published var matrixSnapshot = ArbosFFI.MatrixSnapshot(spaces: [], channels: [])
    /// The client's link to the homeserver (`onConnection`); false shows "Reconnecting…" in a
    /// channel's header pill, from `matrixDisconnectedSince` plus the pill's grace.
    @Published var matrixConnected = true
    var matrixDisconnectedSince: Date?
    /// Decoded channel timelines by relay channel id — the rows ChatView renders for a channel.
    @Published var channelRows: [Id: [Message]] = [:] {
        didSet { bumpVersions(from: oldValue, to: channelRows) }
    }
    /// Channels whose room reached the beginning of its history (`loadOlder` → nil).
    var channelAtStart: Set<Id> = []
    /// Channels with an `openRoom` / `loadOlder` in flight.
    var channelLoading: Set<Id> = []
    /// `matrixStart` in flight (one at a time).
    var matrixStarting = false

    /// Who the call screen shows before anyone joins: the DM peer, or (for a
    /// people group) every other human plus the agents in that chat; for a
    /// project chat the project's agent (Calls Phase 2). A selective group
    /// start (`CallManager.ringIds`) narrows the group list to the people we named.
    func expectedCallPeople(_ c: Call) -> [CallPerson] {
        guard let chat = peopleChat(c.chatId) else {
            if !c.agents.isEmpty { return c.agents }
            guard let project = project(c.projectId) else { return [] }
            return [CallPerson(id: project.id, kind: "agent", name: project.name, handle: mentionSlug(project.name),
                               avatar: project.avatar, avatarColor: project.avatarColor, avatarUrl: nil)]
        }
        let me = user?.id
        if chat.isGroup {
            let others = chat.members.filter { $0.id != me }
            if let ring = CallManager.shared.ringIds, c.callId == CallManager.shared.call?.callId {
                return others.filter { ring.contains($0.id) }.map(CallPerson.from(member:))
            }
            return others.map(CallPerson.from(member:))
        }
        guard let other = chat.counterpart(me: me) else { return [] }
        return [CallPerson.from(member: other)]
    }

    /// Is this chat the one on screen (top of the navigation path)?
    func isShowing(chatId: Id) -> Bool {
        if case .chat(let id, _)? = path.last { return id == chatId }
        return false
    }

    func open(chat: Chat) {
        unarchive(chat.projectId)
        path = [.chat(chatId: chat.id, projectId: chat.projectId)]
    }

    /// Tapping an agent opens its one chat (D78).
    func open(project: Project) {
        unarchive(project.id)
        if let chat = singleChat(of: project.id) {
            open(chat: chat)
        } else {
            // No landing chat yet — open a local thread at once (Mac `new_tab`).
            _ = startNewChat(projectId: project.id)
        }
    }

    // MARK: Home archive (per user on the relay; UserDefaults keeps the offline copy)

    /// Agents hidden from the home list (Hidden chats) until unarchived or new activity.
    @Published private(set) var archivedProjectIds: Set<Id> = {
        Set(UserDefaults.standard.stringArray(forKey: "archivedProjects") ?? [])
    }()

    /// People DMs/groups hidden from home until unarchived or new activity.
    @Published private(set) var archivedPeopleChatIds: Set<Id> = {
        Set(UserDefaults.standard.stringArray(forKey: "archivedPeopleChats") ?? [])
    }()

    /// Servers hidden from home until unarchived.
    @Published private(set) var archivedServerIds: Set<Id> = {
        Set(UserDefaults.standard.stringArray(forKey: "archivedServers") ?? [])
    }()

    /// Rows this device just (un)archived: the relay list may still say the old state for
    /// a poll or two, so the reconcile skips them until the deadline.
    private var archiveLocalUntil: [String: Date] = [:]
    private static let archiveLocalGraceSeconds: TimeInterval = 20

    func isArchived(_ projectId: Id) -> Bool { archivedProjectIds.contains(projectId) }

    func isArchived(peopleChatId: Id) -> Bool { archivedPeopleChatIds.contains(peopleChatId) }

    func isArchived(serverId: Id) -> Bool { archivedServerIds.contains(serverId) }

    func archive(_ projectId: Id) { setArchived(projectId: projectId, true) }
    func unarchive(_ projectId: Id) { setArchived(projectId: projectId, false) }
    func archive(peopleChatId: Id) { setArchived(peopleChatId: peopleChatId, true) }
    func unarchive(peopleChatId: Id) { setArchived(peopleChatId: peopleChatId, false) }
    func archive(serverId: Id) { setArchived(serverId: serverId, true) }
    func unarchive(serverId: Id) { setArchived(serverId: serverId, false) }

    private func setArchived(projectId: Id, _ archived: Bool) {
        guard archivedProjectIds.contains(projectId) != archived else { return }
        if archived { archivedProjectIds.insert(projectId) } else { archivedProjectIds.remove(projectId) }
        UserDefaults.standard.set(Array(archivedProjectIds), forKey: "archivedProjects")
        // Archive clears a home pin — the row is hidden until unarchived.
        if archived { unpinHome(projectId: projectId) }
        archiveLocalUntil["p:" + projectId] = Date().addingTimeInterval(Self.archiveLocalGraceSeconds)
        guard credentials != nil, !isFixture else { return }
        Task {
            do { try await api.setProjectArchived(projectId, archived: archived) } catch let e as RelayError where e.status == 404 {
                // Older relay: the archive stays on this device.
            } catch { fail(archived ? "archive the chat" : "unarchive the chat", error) }
        }
    }

    private func setArchived(peopleChatId: Id, _ archived: Bool) {
        guard archivedPeopleChatIds.contains(peopleChatId) != archived else { return }
        if archived { archivedPeopleChatIds.insert(peopleChatId) } else { archivedPeopleChatIds.remove(peopleChatId) }
        UserDefaults.standard.set(Array(archivedPeopleChatIds), forKey: "archivedPeopleChats")
        if archived { unpinHome(peopleChatId: peopleChatId) }
        if let i = peopleChats.firstIndex(where: { $0.id == peopleChatId }) {
            peopleChats[i].archivedAt = archived ? nowMs() : nil
        }
        archiveLocalUntil["c:" + peopleChatId] = Date().addingTimeInterval(Self.archiveLocalGraceSeconds)
        guard credentials != nil, !isFixture else { return }
        Task {
            do { try await api.chatSettings(chatId: peopleChatId, archived: archived) } catch let e as RelayError where e.status == 404 {
                // Older relay: the archive stays on this device.
            } catch { fail(archived ? "archive the chat" : "unarchive the chat", error) }
        }
    }

    private func setArchived(serverId: Id, _ archived: Bool) {
        guard archivedServerIds.contains(serverId) != archived else { return }
        if archived { archivedServerIds.insert(serverId) } else { archivedServerIds.remove(serverId) }
        UserDefaults.standard.set(Array(archivedServerIds), forKey: "archivedServers")
        archiveLocalUntil["s:" + serverId] = Date().addingTimeInterval(Self.archiveLocalGraceSeconds)
        guard credentials != nil, !isFixture else { return }
        Task {
            do { try await api.setServerArchived(serverId, archived: archived) } catch let e as RelayError where e.status == 404 {
                // Older relay: the archive stays on this device.
            } catch { fail(archived ? "archive the chat" : "unarchive the chat", error) }
        }
    }

    /// The relay's per-user flags win over the offline copy, except for rows this device
    /// changed in the last few seconds (`archiveLocalUntil`).
    private func reconcileArchive(key prefix: String, relay: [(Id, Bool)], into set: inout Set<Id>, defaultsKey: String) {
        let now = Date()
        var next = set
        for (id, archived) in relay {
            if let until = archiveLocalUntil[prefix + id], until > now { continue }
            if archived { next.insert(id) } else { next.remove(id) }
        }
        guard next != set else { return }
        set = next
        UserDefaults.standard.set(Array(set), forKey: defaultsKey)
    }

    func reconcileArchivedProjects(_ list: [Project]) {
        guard !isFixture else { return }
        reconcileArchive(key: "p:", relay: list.map { ($0.id, $0.archivedAt != nil) }, into: &archivedProjectIds, defaultsKey: "archivedProjects")
    }

    func reconcileArchivedPeople(_ list: [PeopleChat]) {
        guard !isFixture else { return }
        reconcileArchive(key: "c:", relay: list.map { ($0.id, $0.archivedAt != nil) }, into: &archivedPeopleChatIds, defaultsKey: "archivedPeopleChats")
    }

    func reconcileArchivedServers(_ list: [Server]) {
        guard !isFixture else { return }
        reconcileArchive(key: "s:", relay: list.map { ($0.id, $0.archivedAt != nil) }, into: &archivedServerIds, defaultsKey: "archivedServers")
    }

    /// Another device of mine flipped a row (`space_archived` / `chat_archived` push).
    func applyRemoteArchive(spaceId: Id, kind: String, archived: Bool) {
        archiveLocalUntil.removeValue(forKey: (kind == "server" ? "s:" : "p:") + spaceId)
        if kind == "server" {
            reconcileArchive(key: "s:", relay: [(spaceId, archived)], into: &archivedServerIds, defaultsKey: "archivedServers")
        } else {
            reconcileArchive(key: "p:", relay: [(spaceId, archived)], into: &archivedProjectIds, defaultsKey: "archivedProjects")
            if archived { unpinHome(projectId: spaceId) }
        }
    }

    func applyRemoteArchive(peopleChatId: Id, archived: Bool) {
        archiveLocalUntil.removeValue(forKey: "c:" + peopleChatId)
        reconcileArchive(key: "c:", relay: [(peopleChatId, archived)], into: &archivedPeopleChatIds, defaultsKey: "archivedPeopleChats")
        if archived { unpinHome(peopleChatId: peopleChatId) }
    }

    /// An archived chat got a new inbound message: the row comes back to the list.
    func unarchiveOnActivity(chatId: Id) {
        if peopleChat(chatId) != nil {
            if archivedPeopleChatIds.contains(chatId) { unarchive(peopleChatId: chatId) }
            return
        }
        for (pid, list) in chats where list.contains(where: { $0.id == chatId }) {
            if archivedProjectIds.contains(pid) { unarchive(pid) }
            return
        }
    }

    // MARK: Home pins (device-local)

    /// How many home rows may sit pinned at once (agents + people chats).
    static let maxHomePins = 5

    /// Home-list keys (`p:<projectId>` / `c:<peopleChatId>`) kept at the top.
    /// Order is the pin strip: index 0 is the topmost row.
    @Published private(set) var pinnedHomeIds: [String] = {
        var seen = Set<String>()
        return (UserDefaults.standard.stringArray(forKey: "pinnedHome") ?? [])
            .filter { seen.insert($0).inserted }
    }()

    func isHomePinned(projectId: Id) -> Bool { pinnedHomeIds.contains("p:" + projectId) }
    func isHomePinned(peopleChatId: Id) -> Bool { pinnedHomeIds.contains("c:" + peopleChatId) }

    /// Position in the pin strip (0 = top), or `nil` when not pinned.
    func homePinIndex(projectId: Id) -> Int? { pinnedHomeIds.firstIndex(of: "p:" + projectId) }
    func homePinIndex(peopleChatId: Id) -> Int? { pinnedHomeIds.firstIndex(of: "c:" + peopleChatId) }

    func pinHome(projectId: Id) {
        unarchive(projectId)
        setHomePinned("p:" + projectId, true)
    }

    func unpinHome(projectId: Id) { setHomePinned("p:" + projectId, false) }

    func pinHome(peopleChatId: Id) {
        unarchive(peopleChatId: peopleChatId)
        setHomePinned("c:" + peopleChatId, true)
    }
    func unpinHome(peopleChatId: Id) { setHomePinned("c:" + peopleChatId, false) }

    /// Put a pinned home row first in the pin strip (long-press → Move to top).
    func moveHomePinToTop(projectId: Id) { moveHomePinToTop(key: "p:" + projectId) }
    func moveHomePinToTop(peopleChatId: Id) { moveHomePinToTop(key: "c:" + peopleChatId) }

    /// Move a pinned home row one step toward the top of the strip.
    func moveHomePinUp(projectId: Id) { moveHomePin(key: "p:" + projectId, delta: -1) }
    func moveHomePinUp(peopleChatId: Id) { moveHomePin(key: "c:" + peopleChatId, delta: -1) }

    /// Move a pinned home row one step toward the bottom of the strip.
    func moveHomePinDown(projectId: Id) { moveHomePin(key: "p:" + projectId, delta: 1) }
    func moveHomePinDown(peopleChatId: Id) { moveHomePin(key: "c:" + peopleChatId, delta: 1) }

    private func setHomePinned(_ key: String, _ pinned: Bool) {
        if pinned {
            guard !pinnedHomeIds.contains(key) else { return }
            // Cap at five so the top of Home stays a short favourites strip.
            guard pinnedHomeIds.count < Self.maxHomePins else {
                failRefused("pin the chat", "You can pin up to five chats.")
                return
            }
            // New pins land at the top; later Move to top / up / down adjusts order.
            pinnedHomeIds.insert(key, at: 0)
        } else {
            guard let i = pinnedHomeIds.firstIndex(of: key) else { return }
            pinnedHomeIds.remove(at: i)
        }
        UserDefaults.standard.set(pinnedHomeIds, forKey: "pinnedHome")
    }

    private func moveHomePinToTop(key: String) {
        guard let i = pinnedHomeIds.firstIndex(of: key), i > 0 else { return }
        pinnedHomeIds.remove(at: i)
        pinnedHomeIds.insert(key, at: 0)
        UserDefaults.standard.set(pinnedHomeIds, forKey: "pinnedHome")
    }

    private func moveHomePin(key: String, delta: Int) {
        guard let i = pinnedHomeIds.firstIndex(of: key) else { return }
        let j = i + delta
        guard pinnedHomeIds.indices.contains(j) else { return }
        pinnedHomeIds.swapAt(i, j)
        UserDefaults.standard.set(pinnedHomeIds, forKey: "pinnedHome")
    }

    @Published private(set) var machineError: String?
    /// Compatibility shim: setting it posts an **info** toast (something that worked or a plain
    /// status). A failure is `fail(_:_:)` — `ErrorUILintTests` keeps raw errors out of here.
    @Published var notice: Notice? {
        didSet {
            guard let n = notice else { return }
            notice = nil
            notify(n.text)
        }
    }
    /// The toast stack, newest last (`ToastHost`).
    @Published private(set) var toasts: [Toast] = []
    private var toastRetries: [UUID: () -> Void] = [:]
    private var toastTimers: [UUID: Task<Void, Never>] = [:]
    /// A surface with a form up takes its verbs' failures; the note it draws.
    @Published var failureClaim: FailureClaim? {
        didSet { if failureClaim != oldValue { claimedFailure = nil } }
    }
    @Published var claimedFailure: UserError?
    @Published var claimedFailureDetailsOpen = false
    /// Per chat: the last send / attachment / recording failure, drawn above the composer until
    /// Try again, a new send, or the next successful send.
    @Published private(set) var sendErrors: [Id: UserError] = [:]
    @Published var sendErrorDetailsOpen: Set<Id> = []
    @Published private(set) var pairingBusy = false
    @Published private(set) var pairingError: UserError?
    @Published var pairingErrorDetailsOpen = false
    @Published private(set) var registryError: UserError?
    @Published var registryErrorDetailsOpen = false
    /// Settings → Connectors (Google Gmail/Calendar, …). Empty until first refresh.
    @Published private(set) var connectors = ConnectorsView()
    @Published private(set) var connectorsBusy = false
    /// Settings → Secrets (D83). `nil` until the first `GET /secrets`.
    @Published var secrets: SecretsOverview?
    /// Settings → Devices (`GET /auth/devices`, Oct 23).
    @Published var devices: [AuthDevice] = []
    @Published var devicesLoaded = false
    @Published var devicesError: UserError?
    /// Agent profile → Secrets, per project.
    @Published var projectSecrets: [Id: ProjectSecrets] = [:]
    /// Standing shell approvals per agent (#1544), loaded when the agent pane opens.
    @Published var shellGrants: [Id: [ShellGrant]] = [:]
    /// Secret hand-off request cards: approval id → saving / saved(env) / failed(sentence).
    @Published var handoff: [String: HandoffState] = [:]
    /// Fixture only: the Vault page's item list (the relay answers this live).
    var fixtureVaultItems: [[String: Any]] = []
    @Published var secretsBusy = false
    /// Home / Settings → Goals (long-running objectives). Empty until first refresh.
    @Published var goals: [GoalItem] = []
    @Published var goalsBusy = false
    /// Open a goal detail from Home or Settings.
    @Published var selectedGoalId: String?

    private var api: RelayAPI
    /// The relay client, for feature extensions in other files.
    var relay: RelayAPI { api }
    private var docs: [Id: CoreChatDoc] = [:]
    /// Room connection AppModel drives (`SessionRoom` / shared client core).
    private var rooms: [Id: SessionRoom] = [:]
    /// Client-minted message ids whose push the relay has not `ACK`ed (S3),
    /// keyed by chat id so one room's ACK cannot leave another chat's bubble faint.
    private var unacked: [Id: Set<Id>] = [:]
    /// Rooms whose pushes were refused with `not_available` on the live connection.
    /// Cleared by that room's next ACK (not by another room's ACK).
    @Published private(set) var pushRefused: Set<Id> = []
    /// The relay's reason for each refused room, in plain words ([feedback blocked-state-ios-2026-10-26]);
    /// the chat-level note under the header draws it. Cleared with the room's next ACK.
    @Published private(set) var pushRefusalText: [Id: String] = [:]
    /// Rooms with a full re-read scheduled by `requestSnapshot` (one per coalesce window).
    private var snapshotPending: Set<Id> = []
    /// When each room's transcript was last re-read from its doc (`Date` seconds).
    private var snapshotLastAt: [Id: TimeInterval] = [:]
    /// A backfill delivers one row per frame and each import reports a list change.
    /// Re-reading the transcript at most this often keeps the paint moving and the
    /// decode cost linear in the backfill, not quadratic.
    static let snapshotCoalesceMs = 80
    /// While a room catches up (not yet `caughtUp`) every doc change — list or message
    /// touch — folds into one re-read per this window, and the home list's activity bump
    /// happens with that re-read, not per row. 72 rows replayed per row cost 11 s of
    /// blocked main thread on a 1 500-message group (iphone-open-scroll-block, 2026-10-24).
    static let catchUpCoalesceMs = 600
    /// Live rows: the first lands at once, the rest of a burst fold into one read per window.
    static let liveCoalesceMs = 150
    /// Lists at or above this size are decoded off the main actor (`emitSnapshot`).
    static let asyncSnapshotMinRows = 300
    /// Per-chat generation of the last requested snapshot; a slower background decode
    /// that finishes after a newer one is dropped.
    private var snapshotGeneration: [Id: Int] = [:]
    /// Rooms whose home-list activity bump waits for the next snapshot (catch-up).
    private var activityPending: Set<Id> = []
    private var registryTask: Task<Void, Never>?

    // MARK: On-device store (#929)

    /// SQLite under Application Support, one file per relay + device; nil in
    /// the fixture, before sign-in, or with `ARBOS_NO_STORE=1` (launch timing A/B).
    private var store: DeviceStore?
    /// Rooms whose doc or cursor changed since the last save.
    private var dirtyDocs: Set<Id> = []
    private var flushTask: Task<Void, Never>?
    /// Where the first populated home list came from: "store" or "relay" (launch log).
    private(set) var contentOrigin = "relay"
    /// Keys the store loader has handed to `ensureRoom` (or decided to skip).
    private var storeDocsLoaded = false
    @Published private(set) var storeUsage = DeviceStore.Usage()
    /// Set when the first home list came from disk and the relay is not live yet.
    var showingSavedCopy: Bool { contentOrigin == "store" && link != .live }
    private static let flushDelayNs: UInt64 = 1_500_000_000
    /// At most one PUT /chats/{id}/read per chat inside this window (latest value at the end).
    static let readPutThrottleSeconds: TimeInterval = 2
    /// Last seq/headSeq successfully handed to the relay for mark-read, per chat.
    private var lastReadPutValue: [Id: Int64] = [:]
    private var lastReadPutAt: [Id: Date] = [:]
    /// Newest value waiting for the throttle window to end.
    private var pendingReadPutValue: [Id: Int64] = [:]
    private var readPutTasks: [Id: Task<Void, Never>] = [:]
    /// Consecutive HTTP 403/404 endings per room key (cleared when the room reaches live).
    private var roomUnavailableFailures: [Id: Int] = [:]
    private var roomUnavailableAt: [Id: Date] = [:]
    /// When `ensureRoom` last created each room — prune skips keys younger than 60s.
    private var roomCreatedAt: [Id: Date] = [:]
    /// Cursor to hand back when recreating a room after unavailable (doc stays in `docs`).
    private var roomRestoredCursors: [Id: Int64] = [:]
    /// After this many unavailable endings in a row, skip recreate until backoff or foreground.
    nonisolated static let roomUnavailableRetryLimit = 3
    /// How long to wait after `roomUnavailableRetryLimit` before recreating again.
    nonisolated static let roomUnavailableBackoffSeconds: TimeInterval = 600
    /// Locally created chats are not pruned until the relay has had time to list them.
    nonisolated static let roomRecentGraceSeconds: TimeInterval = 60

    var appearance: Appearance { Appearance(rawValue: appearanceSetting) ?? .default }
    var surface: Surface { Surface(rawValue: surfaceSetting) ?? .default }
    var isPaired: Bool { credentials != nil }
    var relayUrl: String { api.endpoint.httpUrl }

    /// The model the avatar views resolve through (`AgentAvatar.shown`): the app has one; a
    /// test's model becomes current while it exists.
    private(set) weak static var current: AppModel?

    init() {
        LaunchClock.mark("app-init")
        // Drop the retired SessionRoom TestFlight switch so it does not linger.
        UserDefaults.standard.removeObject(forKey: "sharedRoomSession")
        #if DEBUG
        if let scale = DebugHooks.textScale, TextScale.steps.contains(where: { abs($0 - CGFloat(scale)) < 0.001 }) {
            textScaleSetting = scale
        }
        // Set here, in init, so the observers do not write the one-launch override to disk.
        if let a = DebugHooks.appearance { appearanceSetting = a.rawValue }
        if let s = DebugHooks.surface { surfaceSetting = s.rawValue }
        #endif
        var saved = UserDefaults.standard.string(forKey: "relayUrl") ?? AppModel.defaultRelayUrl
        var creds: DeviceCredentials?
        #if DEBUG
        // Simulator runs: `SIMCTL_CHILD_ARBOS_RELAY_URL` / `_TOKEN` skip the pairing screen.
        let env = ProcessInfo.processInfo.environment
        if let url = env["ARBOS_RELAY_URL"], !url.isEmpty { saved = url }
        if let tok = env["ARBOS_RELAY_TOKEN"], !tok.isEmpty {
            creds = DeviceCredentials(relayUrl: RelayEndpoint(saved).httpUrl, deviceId: env["ARBOS_DEVICE_ID"] ?? "ios:env", deviceToken: tok)
        }
        #endif
        #if DEBUG
        if env["ARBOS_FIXTURE"] == "1" {
            api = RelayAPI(endpoint: RelayEndpoint("http://fixture.invalid")) { "" }
            credentials = DeviceCredentials(relayUrl: "http://fixture.invalid", deviceId: "ios:fixture", deviceToken: "fixture")
            LinkIcons.shared.fetch = { host in LinkIcons.fixtureIcon(host: host) }
            loadFixture()
            return
        }
        #endif
        let endpoint = RelayEndpoint(saved)
        if !endpoint.httpUrl.isEmpty {
            UserDefaults.standard.set(endpoint.httpUrl, forKey: "relayUrl")
        }
        api = RelayAPI(endpoint: endpoint) { "" }
        // Site icons for the bubbles' named links come from whichever relay is current.
        LinkIcons.shared.fetch = { [weak self] host in await self?.api.linkIcon(host: host) }
        if creds == nil, let saved = Keychain.loadAuth(relayUrl: endpoint.httpUrl) {
            tokens = saved
            creds = DeviceCredentials(relayUrl: saved.relayUrl, deviceId: saved.deviceId, deviceToken: saved.accessToken)
        }
        if creds == nil { creds = Keychain.load(relayUrl: endpoint.httpUrl) }
        credentials = creds
        api.token = { [weak self] in self?.credentials?.deviceToken ?? "" }
        api.refresher = { [weak self] in await self?.refreshTokens() ?? false }
        if creds != nil {
            startAfterPairing()
        }
        #if DEBUG
        // Rig hook (D82 first-login recordings): `ARBOS_TEST_TOKENS=<file>` holds a
        // `/auth/token` response for a test account; taken as a fresh sign-in.
        if creds == nil, let path = env["ARBOS_TEST_TOKENS"], !path.isEmpty,
           let data = FileManager.default.contents(atPath: path),
           let v = try? JSONSerialization.jsonObject(with: data),
           let (toks, who) = try? RelayAPI.parseTokens(v, relayUrl: endpoint.httpUrl) {
            // Unsigned simulator builds have no Keychain: keep the rig session in memory.
            adopt(tokens: toks, user: who, persist: false)
        }
        #endif
        AppModel.current = self
        Task { await probeAuthMode() }
        PushManager.shared.visibleChatId = { [weak self] in
            guard let self, self.appActive else { return nil }
            return self.visibleChatId
        }
        PushManager.shared.isHiddenChat = { [weak self] id in self?.isHiddenChat(id) ?? false }
        PushManager.shared.isMutedChat = { [weak self] id in self?.isPeopleChatMuted(id) ?? false }
        NotificationCenter.default.addObserver(forName: PushManager.openChat, object: nil, queue: .main) { [weak self] n in
            guard let self, let cid = n.userInfo?["chat_id"] as? String else { return }
            let pid = (n.userInfo?["project_id"] as? String) ?? peopleSlot
            Task { @MainActor in self.openFromNotification(chatId: cid, projectId: pid) }
        }
        NotificationCenter.default.addObserver(forName: PushManager.remoteReceived, object: nil, queue: .main) { [weak self] n in
            // content-available on ARBOS_CALL wakes a backgrounded phone; report
            // CallKit here so it rings without the user opening the app first.
            let info = n.userInfo ?? [:]
            Task { @MainActor in
                let aps = info["aps"] as? [String: Any]
                let category = (aps?["category"] as? String) ?? (info["category"] as? String)
                let event = info["event"] as? String
                if category == PushManager.callCategory || event == "started" || event == "ended" || event == "missed" {
                    CallManager.shared.handleCallAlert(info)
                }
                // Silent clear-on-read push (`type: clear`, the APNs form of
                // `notifications_cleared`): same effect as the /me/pushes row, but it also
                // reaches a backgrounded phone.
                if (info["type"] as? String) == "clear", let cid = info["chat_id"] as? String, !cid.isEmpty {
                    let badge = (info["badge"] as? NSNumber)?.int64Value ?? -1
                    self?.applyRemoteClear(chatId: cid, relayTotal: badge)
                }
                await self?.refreshPeople(); await self?.refreshUnread()
            }
        }
    }

    /// True while a store is open but its docs have not been handed to `ensureRoom`
    /// yet — a notification open must not build an empty room that would drop the
    /// saved doc and pending offline sends.
    nonisolated static func shouldDeferRoomOpen(hasStore: Bool, storeDocsLoaded: Bool) -> Bool {
        hasStore && !storeDocsLoaded
    }

    /// Deep link from a notification: the people chat or the project chat,
    /// or remembered until the lists arrive. While store docs are still loading,
    /// only record `pendingOpen` (and set `path` so the screen can show) — do not
    /// call `ensureRoom` until `retryPendingOpen` after `loadStoreDocs`.
    func openFromNotification(chatId: String, projectId: String) {
        let deferRoom = Self.shouldDeferRoomOpen(hasStore: store != nil, storeDocsLoaded: storeDocsLoaded)
        if peopleChat(chatId) != nil || projectId == peopleSlot && peopleChats.contains(where: { $0.id == chatId }) {
            if !deferRoom {
                ensureRoom(projectId: peopleSlot, key: chatId, roomChat: chatId)
            }
            path = [.chat(chatId: chatId, projectId: peopleSlot)]
            pendingOpen = deferRoom ? (chatId, peopleSlot) : nil
        } else if let c = chat(chatId) {
            if !deferRoom {
                ensureRoom(projectId: c.projectId, key: c.id, roomChat: c.id)
            }
            path = [.chat(chatId: c.id, projectId: c.projectId)]
            pendingOpen = deferRoom ? (c.id, c.projectId) : nil
        } else if projectId != peopleSlot, project(projectId) != nil {
            if !deferRoom {
                ensureRoom(projectId: projectId, key: chatId, roomChat: chatId)
            }
            path = [.chat(chatId: chatId, projectId: projectId)]
            pendingOpen = deferRoom ? (chatId, projectId) : nil
        } else {
            pendingOpen = (chatId, projectId)
        }
    }

    /// What the chat header's connection pill says for `chatId`, or nil when the
    /// link is fine ([feedback connection-pill]). The room's own socket and the
    /// relay's HTTP poll each vote; the highest-priority verdict wins (offline >
    /// relay down > reconnecting > syncing). A state younger than `linkPillGrace`
    /// does not show; `linkPillRepaint` ticks the view once the grace is over.
    /// A channel's room is the Matrix client: its `onConnection` false is a reconnect.
    func linkPill(for chatId: Id, channel: Bool = false, now: Date = Date()) -> LinkState? {
        func settled(_ state: LinkState, _ since: Date) -> LinkState? {
            state != .live && now.timeIntervalSince(since) >= Self.linkPillGrace ? state : nil
        }
        var verdict: LinkState? = settled(relayLink, relayLinkSince)
        if channel {
            if !matrixConnected, let since = matrixDisconnectedSince, let s = settled(.reconnecting, since) {
                verdict = max(verdict ?? .live, s)
            }
        } else if let state = roomLinks[chatId], let since = roomLinkSince[chatId], let s = settled(state, since) {
            verdict = max(verdict ?? .live, s)
        }
        return verdict
    }

    /// Bumped once the grace period after a link change is over, so the pill
    /// appears (or stays away) without a user event.
    @Published private(set) var linkPillRepaint = 0

    func setRoomLink(_ key: Id, _ state: LinkState) {
        guard roomLinks[key] != state else { return }
        roomLinks[key] = state
        roomLinkSince[key] = Date()
        // A room that is live proves the relay is reachable: a verdict from an
        // older HTTP poll must not keep "Relay down" up.
        if state == .live, relayLink != .live { setRelayLink(.live) }
        if state != .live { scheduleLinkPillRepaint() }
    }

    func setRelayLink(_ state: LinkState) {
        guard relayLink != state else { return }
        relayLink = state
        relayLinkSince = Date()
        if state != .live { scheduleLinkPillRepaint() }
    }

    func scheduleLinkPillRepaint() {
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(Int(Self.linkPillGrace * 1000) + 50))
            self?.linkPillRepaint &+= 1
        }
    }

    private func retryPendingOpen() {
        guard let p = pendingOpen else { return }
        openFromNotification(chatId: p.chatId, projectId: p.projectId)
    }

    // MARK: Feedback

    /// JPEG the screenshot (≤15 MB per the contract) and post it with the note.
    func sendFeedback(note: String, image: UIImage?, projectId: Id?) async throws -> String? {
        var data: Data? = nil
        var mime: String? = nil
        var w = 0, h = 0
        if let image {
            var quality: CGFloat = 0.9
            var jpeg = image.jpegData(compressionQuality: quality)
            while let d = jpeg, d.count > 14_000_000, quality > 0.3 {
                quality -= 0.2
                jpeg = image.jpegData(compressionQuality: quality)
            }
            data = jpeg
            mime = "image/jpeg"
            w = Int(image.size.width * image.scale)
            h = Int(image.size.height * image.scale)
        }
        if isFixture {
            try? await Task.sleep(nanoseconds: 900_000_000)
            return "01FIXTURE0000000000FEEDBK"
        }
        return try await api.sendFeedback(note: note, image: data, mime: mime, width: w, height: h,
                                          appVersion: FeedbackCenter.appVersion, os: "iOS \(UIDevice.current.systemVersion)", projectId: projectId)
    }

    // MARK: Calls v1

    private func hookCalls() {
        let calls = CallManager.shared
        calls.api = api
        calls.isFixture = isFixture
        calls.deviceId = { [weak self] in self?.deviceId ?? "" }
        calls.me = { [weak self] in
            guard let self, let u = self.user else { return nil }
            return CallPerson(id: u.id, kind: "user", name: u.name, handle: u.login, avatar: nil, avatarColor: nil, avatarUrl: u.avatarUrl)
        }
        calls.writeSystemLine = { [weak self] chatId, projectId, text, callId in
            self?.writeSystemLine(chatId: chatId, projectId: projectId, text: text, callId: callId)
        }
        calls.calleePresence = { [weak self] chatId in
            guard let self, let chat = self.peopleChat(chatId), chat.kind == "dm", let other = chat.counterpart(me: self.user?.id) else { return nil }
            let seen = max(other.lastSeenAt ?? 0, self.lastSeen[other.id] ?? 0)
            return (other.online || (nowMs() - seen) < 90_000, seen > 0 ? seen : nil)
        }
        calls.calleeName = { [weak self] chatId in
            guard let self, let chat = self.peopleChat(chatId) else { return nil }
            // Groups name the chat, not the first other human (that looked like a DM to Kenobi).
            if chat.isGroup { return chat.title.isEmpty ? "Group" : chat.title }
            return chat.counterpart(me: self.user?.id)?.displayName
        }
        // Presence carries `in_call` for the chat we are talking in.
        callSink = calls.$call.receive(on: DispatchQueue.main).sink { [weak self] c in
            guard let self else { return }
            for (key, room) in self.rooms { room.inCall = (c?.chatId == key) ? c?.callId : nil }
        }
        guard !isFixture else { return }
        calls.prepareVoip()
        Task { await calls.sendVoipTokenIfNeeded(force: true) }
    }
    private var callSink: AnyCancellable?

    /// A system row in a chat doc (call start/end lines). Fixture: local append.
    func writeSystemLine(chatId: Id, projectId: Id, text: String, callId: String) {
        #if DEBUG
        if isFixture {
            let m = Message(id: newId(), chatId: chatId, role: .system, sender: "system", status: .done, turn: nil, decision: nil,
                            subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: nowMs(),
                            completedAt: nowMs(), pending: false, parts: [.text(id: newId(), text: text)], hiddenCount: 0)
            messages[chatId, default: []].append(m)
            return
        }
        #endif
        guard let doc = docs[chatId] else { return }
        do {
            try doc.insertSystemLine(id: newId(), text: text, deviceId: deviceId, callId: callId)
            doc.commit()
        } catch {
            fail("write the call line", error)
        }
    }

    /// D82 first login: the live doc of a chat (nil until its room exists).
    func liveDoc(_ chatId: Id) -> CoreChatDoc? { docs[chatId] }

    /// D82 first login: `POST /projects` for the account's first agent.
    func createFirstProject(name: String) async throws {
        _ = try await api.createProject(name: name)
    }

    /// D82 first login: `POST /machine/start` from the Retry block.
    func startMachineQuietly() async {
        try? await api.startMachine()
    }

    /// Opening a chat: is a call on there? (GET /chats/{id}/call)
    func refreshActiveCall(chatId: Id, projectId: Id?) async {
        guard credentials != nil, !isFixture else { return }
        if let c = try? await api.activeCall(chatId: chatId, projectId: projectId) {
            CallManager.shared.noteActive(c, chatId: chatId)
        } else {
            CallManager.shared.noteActive(nil, chatId: chatId)
        }
    }

    /// A peer's presence says `in_call`: fetch it for the Join bar.
    private func peerInCall(_ callId: String, chatId: Id) {
        guard CallManager.shared.activeByChat[chatId]?.callId != callId else { return }
        Task {
            if let c = try? await api.call(callId) {
                CallManager.shared.noteActive(c, chatId: chatId)
                #if DEBUG
                if DebugHooks.autoJoinCall, !c.ended, !CallManager.shared.isActive, visibleChatId == chatId {
                    await CallManager.shared.join(c, title: peopleChat(chatId)?.title ?? "Call")
                }
                #endif
            }
        }
    }

    // MARK: Push registration + badge

    private func hookPush() {
        hookCalls()
        guard let creds = credentials, !isFixture else { return }
        let api = self.api
        PushManager.shared.register = { token, env in
            let version = (Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "") + " (" + (Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "") + ")"
            do {
                let enabled = try await api.registerPush(deviceId: creds.deviceId, token: token, environment: env,
                                                         bundleId: Bundle.main.bundleIdentifier ?? "com.unarbos.arbos.ios", appVersion: version)
                await MainActor.run { PushManager.shared.relayPushEnabled = enabled }
            } catch {
                #if DEBUG
                print("push: register failed: \(error.localizedDescription)")
                #endif
                await MainActor.run { PushManager.shared.forgetRegistration() }
            }
        }
        Task { await PushManager.shared.refreshStatus(); await PushManager.shared.sendTokenIfNeeded(force: true) }
    }

    /// `GET /me/unread` when the relay has it; otherwise the local counts.
    func refreshUnread() async {
        guard credentials != nil, !isFixture else { applyBadge(); return }
        if let r = (try? await api.unread()) ?? nil {
            unreadCounts = r.chats
            saveUnreadCache()
        }
        applyBadge()
    }

    /// Everything unread: people chats by sequence, project chats by flag.
    var unreadTotal: Int {
        // Hidden plain chats (D78: not the agent's one chat) never badge.
        if !unreadCounts.isEmpty {
            let flagged = peopleUnread.union(chats.values.flatMap { $0 }.filter { $0.unread }.map(\.id))
            return Self.badgeTotal(remote: unreadCounts, flagged: flagged, hidden: { self.isHiddenChat($0) })
        }
        let people = peopleChats.reduce(0) { $0 + unreadCount(for: $1) }
        let projects = chats.values.flatMap { $0 }.filter { $0.unread && !isHiddenChat($0.id) }.count
        return people + projects
    }

    /// The app badge from the relay's per-chat counts (`GET /me/unread`) plus one for every
    /// chat a live row flagged before `/me/unread` caught up; hidden chats never count.
    nonisolated static func badgeTotal(remote: [String: Int], flagged: Set<String>, hidden: (String) -> Bool) -> Int {
        var total = remote.reduce(0) { $0 + (hidden($1.key) ? 0 : $1.value) }
        for id in flagged where !hidden(id) && (remote[id] ?? 0) == 0 {
            total += 1
        }
        return total
    }

    /// A clear from another device (`notifications_cleared`) applied to the local unread
    /// picture: the chat's remote count and local flag go, every other chat keeps its count.
    nonisolated static func clearingChat(_ chatId: String, remote: inout [String: Int], flagged: inout Set<String>) {
        if remote[chatId] != nil { remote[chatId] = 0 }
        flagged.remove(chatId)
    }

    /// The badge right after a clear: the smaller of the relay's total (every chat, the read
    /// one at 0 — what its `/me/unread` would say) and the local sum. Each side can only be
    /// too high: the local sum when a live row lit a flag `/me/unread` has not confirmed, the
    /// relay's when this device read a chat moments ago and its throttled `PUT /read` is
    /// still pending, or for hidden plain chats (D78) the relay counts and the badge never
    /// shows. A total the relay did not send (negative) falls back to the local sum.
    nonisolated static func badgeAfterClear(relayTotal: Int64, localTotal: Int) -> Int {
        relayTotal >= 0 ? Int(min(relayTotal, Int64(max(0, localTotal)))) : max(0, localTotal)
    }

    /// Unread rows of one agent chat for its dot: none for a hidden plain chat.
    func chatHasUnread(_ chat: Chat) -> Bool {
        if isHiddenChat(chat.id) { return false }
        // On screen: never keep a stale local/remote flag (same rule as people chats).
        if chat.id == visibleChatId { return false }
        if chat.unread { return true }
        return (unreadCounts[chat.id] ?? 0) > 0
    }

    func unreadCount(for chat: PeopleChat) -> Int {
        Self.peopleUnreadCount(
            remote: unreadCounts[chat.id],
            flagged: chat.unread || peopleUnread.contains(chat.id),
            watching: chat.id == visibleChatId
        )
    }

    /// #1438: one read cursor per user, kept by the relay. `remote` is `GET /me/unread`'s
    /// count for the chat (nil when it lists nothing for it); `flagged` is the relay's
    /// per-chat `unread` (same cursor) or a live row from someone else that landed while the
    /// chat was off screen; `watching` = on screen in the foreground. Row sequence numbers
    /// (`head_seq` / `read_seq`) are not consulted: they counted the user's own rows and
    /// system lines, which is what painted the false blue dots.
    nonisolated static func peopleUnreadCount(remote: Int?, flagged: Bool, watching: Bool) -> Int {
        if watching { return 0 }
        let local = flagged ? 1 : 0
        // Loro can light the flag before GET /me/unread catches up — never let a
        // stale remote 0 hide the blue circle / count.
        if let n = remote { return max(n, local) }
        return local
    }

    func applyBadge() {
        PushManager.shared.setBadge(unreadTotal)
    }

    // MARK: Sign in with GitHub (self-serve relay)

    func probeAuthMode() async {
        authMode = await api.authMode()
    }

    func signInWithGitHub() {
        guard !signingIn else { return }
        signingIn = true
        signInError = nil
        Task {
            defer { signingIn = false }
            do {
                guard let start = api.authStartURL(deviceName: UIDevice.current.name) else {
                    throw AuthError(message: "Bad relay address")
                }
                let code = try await webAuth.signIn(startURL: start)
                let (toks, who) = try await api.exchange(grantCode: code)
                adopt(tokens: toks, user: who)
                needsProfileSetup = true
            } catch {
                signInError = UserError.classify("sign in", error)
            }
        }
    }

    /// Settings / onboarding: save display name and handle via `PATCH /me`.
    @discardableResult
    func updateProfile(name: String, handle: String) async -> String? {
        let trimmedName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedHandle = handle.trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "@"))
        guard !trimmedHandle.isEmpty else { return "Choose a handle." }
        do {
            let who = try await api.patchMe(name: trimmedName, login: trimmedHandle)
            user = who
            saveAccountCache()
            needsProfileSetup = false
            return nil
        } catch let e as RelayError {
            return AuthError.humanize(e.type ?? "", e.message.isEmpty ? "Could not update profile" : e.message)
        } catch {
            return UserError.classify("save your profile", error).sentence
        }
    }

    /// Settings → Privacy: who may add me to groups (`PATCH /me {group_add}`).
    @discardableResult
    func updateGroupAddPolicy(_ policy: GroupAddPolicy) async -> String? {
        let previous = groupAddPolicy
        groupAddPolicy = policy.rawValue
        guard !isFixture else { return nil }
        do {
            _ = try await api.patchMe(groupAdd: policy.rawValue)
            return nil
        } catch let e as RelayError {
            groupAddPolicy = previous
            return AuthError.humanize(e.type ?? "", e.message.isEmpty ? "Could not update privacy" : e.message)
        } catch {
            groupAddPolicy = previous
            return UserError.classify("save the privacy setting", error).sentence
        }
    }

    private func adopt(tokens toks: AuthTokens, user who: AuthUser?, persist: Bool = true) {
        tokens = toks
        if let who { user = who }
        if persist {
            do { try Keychain.storeAuth(toks) } catch {
                fail("save your sign-in to the keychain", error)
            }
        }
        let wasPaired = credentials != nil
        credentials = DeviceCredentials(relayUrl: toks.relayUrl, deviceId: toks.deviceId, deviceToken: toks.accessToken)
        if !wasPaired {
            // A sign-in through the browser (not a token refresh): a possible first login.
            // Rig (Debug): `ARBOS_SKIP_FIRST_LOGIN=1` lands on the lists at once — a shot of an
            // existing account on a simulator whose test box has no agentd to greet it.
            if who != nil, !DebugHooks.skipFirstLogin { beginFirstLogin() }
            startAfterPairing()
        }
    }

    /// Single-flight refresh; false when the relay rejected the refresh token
    /// (the device is then signed out — a reused rotated token revokes it).
    func refreshTokens() async -> Bool {
        if let running = refreshTask { return await running.value }
        guard let current = tokens, !current.refreshToken.isEmpty else { return false }
        let task = Task<Bool, Never> { [weak self] in
            guard let self else { return false }
            do {
                let (next, who) = try await api.refresh(current)
                adopt(tokens: next, user: who)
                return true
            } catch {
                if let e = error as? RelayError, e.status == 401 || e.status == 403 {
                    signOut(remote: false)
                    signInError = UserError.of("stay signed in", .authExpired, "token refresh refused")
                }
                return false
            }
        }
        refreshTask = task
        let ok = await task.value
        refreshTask = nil
        return ok
    }

    /// `/me`; runs with every registry poll.
    func refreshAccount() async {
        guard credentials != nil, !isFixture else { return }
        if tokens?.isExpiringSoon == true { _ = await refreshTokens() }
        if let (who, cr, summary, features, privacy) = try? await api.me() {
            user = who
            credits = cr
            saveAccountCache()
            // D92 remote flag `features.leaf_avatars` (absent on an older relay = on).
            LeafAvatarsFlag.shared.set(features?.leaf_avatars ?? true)
            LeafAvatarsFlag.shared.setLottie(features?.leaf_lottie ?? true)
            // Voice mode kill switch (absent on an older relay = off).
            VoiceModeFlag.shared.set(features?.voice_mode ?? false)
            // Who may add me to groups (absent on an older relay = anyone).
            if let policy = privacy?.group_add, GroupAddPolicy(rawValue: policy) != nil {
                groupAddPolicy = policy
            }
            let position = summary.waitlisted ? Int(summary.waitlist_position ?? 1) : nil
            if position != waitlistPosition { waitlistPosition = position }
            meAlwaysOn = summary.always_on
            var m = machine
            m.held = summary.held
            if summary.always_on, !m.alwaysOn {
                m.alwaysOn = true
                m.state = m.coerced(m.state)
            }
            machine = m
        }
    }

    func signOut(remote: Bool = true) {
        let had = tokens
        if remote, let creds = credentials {
            let api = self.api
            Task { try? await api.unregisterPush(deviceId: creds.deviceId); try? await api.unregisterVoip(deviceId: creds.deviceId) }
        }
        CallManager.shared.forgetVoipRegistration()
        PushManager.shared.forgetRegistration()
        PushManager.shared.setBadge(0)
        if remote, had != nil { Task { await api.logout() } }
        if let had { Keychain.deleteAuth(relayUrl: had.relayUrl) }
        tokens = nil
        user = nil
        credits = nil
        needsProfileSetup = false
        unpair(forget: true)
    }

    var deviceId: String { credentials?.deviceId ?? "ios:unpaired" }

    // MARK: Pairing

    func setRelayUrl(_ raw: String) {
        let endpoint = RelayEndpoint(raw)
        UserDefaults.standard.set(endpoint.httpUrl, forKey: "relayUrl")
        guard endpoint != api.endpoint else { return }
        let previous = credentials
        unpair(forget: false)
        api = RelayAPI(endpoint: endpoint) { [weak self] in self?.credentials?.deviceToken ?? "" }
        api.refresher = { [weak self] in await self?.refreshTokens() ?? false }
        tokens = nil
        user = nil
        credits = nil
        Task { await probeAuthMode() }
        if let saved = Keychain.loadAuth(relayUrl: endpoint.httpUrl) {
            tokens = saved
            credentials = DeviceCredentials(relayUrl: saved.relayUrl, deviceId: saved.deviceId, deviceToken: saved.accessToken)
        } else {
            credentials = Keychain.load(relayUrl: endpoint.httpUrl)
        }
        if credentials != nil {
            startAfterPairing()
        } else if let previous, tokens == nil, previous.deviceToken.hasPrefix("adv_") {
            // A quick-tunnel URL changes while the relay (and this device's
            // token) stays the same: try the old token on the new address
            // before asking for a pairing code.
            Task { await adopt(previous, at: endpoint) }
        }
    }

    private func adopt(_ old: DeviceCredentials, at endpoint: RelayEndpoint) async {
        let probe = RelayAPI(endpoint: endpoint) { old.deviceToken }
        guard (try? await probe.registry()) != nil else { return }
        let creds = DeviceCredentials(relayUrl: endpoint.httpUrl, deviceId: old.deviceId, deviceToken: old.deviceToken)
        try? Keychain.store(creds)
        credentials = creds
        startAfterPairing()
    }


    // MARK: - toasts and failures (one element, one place — Jacob 2026-10-26)

    /// An info toast: something that worked or a plain status. Never a failure. The same text
    /// again while its toast stands restarts that toast instead of adding one.
    func notify(_ text: String) {
        if let i = toasts.firstIndex(where: { $0.error == nil && $0.text == text }) {
            toasts[i].count += 1
            toasts[i].at = Date()
            armToast(toasts[i].id, after: toasts[i].ttl)
            return
        }
        pushToast(Toast(id: UUID(), text: text, error: nil, at: Date()))
    }

    /// The newest `Toast.visibleMax` toasts (oldest first) and how many older ones are folded
    /// away above them (Jacob 2026-10-27: at most three visible, the rest a count).
    var toastStack: ToastStack {
        let folded = max(0, toasts.count - Toast.visibleMax)
        return ToastStack(visible: Array(toasts.dropFirst(folded)), folded: folded)
    }

    /// The "N more" line's ×: the whole slot goes.
    func dismissAllToasts() {
        for t in toasts { toastTimers[t.id]?.cancel(); toastTimers[t.id] = nil; toastRetries[t.id] = nil }
        toasts.removeAll()
    }

    /// Something the user asked for did not happen: `verb` is what they tried ("save the
    /// secret"), `error` whatever was thrown. Classified by `UserError`, drawn by `ErrorNote`
    /// — inline by the surface that claimed the verb, else as an error toast.
    func fail(_ verb: String, _ error: Error, retry: (() -> Void)? = nil) {
        fail(UserError.classify(verb, error), retry: retry)
    }

    /// A failure already classified. The same failure again while its toast stands (same
    /// sentence) bumps that toast's count and restarts its clock — one note per failure,
    /// however often it repeats (eight "Couldn't load your secrets. You're offline." toasts
    /// on the Mac, 2026-10-27, are the reason).
    func fail(_ error: UserError, retry: (() -> Void)? = nil) {
        if let claim = failureClaim, claim.verbs.contains(where: { error.verb.hasPrefix($0) }) {
            claimedFailure = error
            claimedFailureDetailsOpen = false
            return
        }
        if let i = toasts.firstIndex(where: { $0.sameAs(error) }) {
            toasts[i].count += 1
            toasts[i].error = error
            toasts[i].at = Date()
            if let retry { toastRetries[toasts[i].id] = retry; toasts[i].hasRetry = true }
            armToast(toasts[i].id, after: toasts[i].ttl)
            return
        }
        let toast = Toast(id: UUID(), text: error.sentence, error: error, hasRetry: retry != nil, at: Date())
        if let retry { toastRetries[toast.id] = retry }
        pushToast(toast)
    }

    /// A local refusal (the form is not complete): the words as the why.
    func failRefused(_ verb: String, _ why: String) {
        fail(UserError.refused(verb, why))
    }

    private func pushToast(_ toast: Toast) {
        toasts.append(toast)
        armToast(toast.id, after: toast.ttl)
    }

    private func armToast(_ id: UUID, after ttl: TimeInterval) {
        toastTimers[id]?.cancel()
        toastTimers[id] = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(ttl * 1_000_000_000))
            guard let self, !Task.isCancelled else { return }
            if let t = self.toasts.first(where: { $0.id == id }), t.held {
                // Details open: stays. A plain hold ends at the cap (a finger parked on the
                // slot, or a release that never arrived, cannot pin a toast forever).
                let since = t.heldSince ?? Date()
                let left = Toast.holdMax - Date().timeIntervalSince(since)
                if t.detailsOpen || left > 0 {
                    self.armToast(id, after: t.detailsOpen ? ttl : max(left, 0.5))
                    return
                }
            }
            self.dismissToast(id)
        }
    }

    func dismissToast(_ id: UUID) {
        toasts.removeAll { $0.id == id }
        toastRetries[id] = nil
        toastTimers[id]?.cancel()
        toastTimers[id] = nil
    }

    /// A touch on a toast holds it; letting go restarts its clock.
    func holdToast(_ id: UUID, held: Bool) {
        guard let i = toasts.firstIndex(where: { $0.id == id }) else { return }
        if held, !toasts[i].held { toasts[i].heldSince = Date() }
        toasts[i].held = held
        if !held { toasts[i].heldSince = nil; armToast(id, after: toasts[i].ttl) }
    }

    func setToastDetails(_ id: UUID, open: Bool) {
        guard let i = toasts.firstIndex(where: { $0.id == id }) else { return }
        toasts[i].detailsOpen = open
        if !toasts[i].held { toasts[i].heldSince = Date() }
        toasts[i].held = true
    }

    /// Try again on an error toast: re-run its action and drop the toast.
    func retryToast(_ id: UUID) {
        let retry = toastRetries[id]
        dismissToast(id)
        retry?()
    }

    /// 401 / 403 on a toast's Sign in: back to the sign-in screen.
    func signOutForReauth() {
        unpair(forget: true)
    }

    /// The composer's note for `chatId`.
    func setSendError(_ chatId: Id, _ error: UserError) {
        sendErrors[chatId] = error
        sendErrorDetailsOpen.remove(chatId)
    }

    func clearSendError(_ chatId: Id) {
        sendErrors[chatId] = nil
        sendErrorDetailsOpen.remove(chatId)
    }

    /// Try again on the composer's note: re-send the newest failed message in that chat (the
    /// note clears either way).
    func retrySendError(_ chatId: Id) {
        clearSendError(chatId)
        if let failed = messages[chatId]?.last(where: { $0.status == .error }) {
            resendFailed(chatId: chatId, messageId: failed.id)
        }
    }

    func pair(code: String) {
        guard !pairingBusy else { return }
        pairingBusy = true
        pairingError = nil
        Task {
            defer { pairingBusy = false }
            do {
                let creds = try await api.pair(code: code, name: UIDevice.current.name)
                do {
                    try Keychain.store(creds)
                } catch {
                    fail("save your sign-in to the keychain", error)
                }
                credentials = creds
                beginFirstLogin()
                startAfterPairing()
            } catch {
                pairingError = (error as? RelayError)?.isUnauthorized == true
                    ? UserError.refused("pair this iPhone", "That code was not accepted. Codes expire after 10 minutes.")
                    : UserError.classify("pair this iPhone", error)
            }
        }
    }

    func unpair(forget: Bool = true) {
        registryTask?.cancel()
        registryTask = nil
        for room in rooms.values { room.stop() }
        rooms.removeAll()
        docs.removeAll()
        projects = []
        chats = [:]
        messages = [:]
        caughtUp = []
        unacked = [:]
        peopleChats = []
        peopleUnread = []
        unreadCounts = [:]
        stopMatrix()
        servers = []
        serversAvailable = false
        directory = []
        directoryLoadedAt = 0
        dirtyDocs = []
        flushTask?.cancel()
        flushTask = nil
        storeDocsLoaded = false
        for t in readPutTasks.values { t.cancel() }
        readPutTasks = [:]
        lastReadPutValue = [:]
        lastReadPutAt = [:]
        pendingReadPutValue = [:]
        roomUnavailableFailures = [:]
        roomUnavailableAt = [:]
        roomCreatedAt = [:]
        roomRestoredCursors = [:]
        // Sign-out wipes everything this account had on the device.
        store?.close(wipe: true)
        store = nil
        storeUsage = DeviceStore.Usage()
        if let creds = credentials {
            UserDefaults.standard.removeObject(forKey: "registryCache.\(creds.relayUrl).\(creds.deviceId)")
            if forget { Keychain.delete(relayUrl: creds.relayUrl) }
        }
        credentials = nil
        link = .connecting
        roomLinks = [:]
        roomLinkSince = [:]
        relayLink = .live
        connectors = ConnectorsView()
        connectorsBusy = false
        endFirstLogin()
    }

    private func startAfterPairing() {
        link = .connecting
        roomLinks = [:]
        roomLinkSince = [:]
        relayLink = .live
        openStore()
        // Paint the last home list and every saved transcript before any network.
        if let store, projects.isEmpty, peopleChats.isEmpty {
            loadStoreKV(store)
        }
        if projects.isEmpty { resumeFirstLoginIfPending() }
        hookPush()
        Task {
            if let ids = try? await api.models(), !ids.isEmpty {
                models = ids
            }
        }
        registryTask?.cancel()
        registryTask = Task { [weak self] in
            // Saved docs first, so their rooms dial with a cursor instead of
            // the registry opening them empty (one checkpoint download each).
            if let self, let store = self.store { await self.loadStoreDocs(store) }
            self?.storeDocsLoaded = true
            // Notification opens deferred while docs loaded can ensureRoom now.
            self?.retryPendingOpen()
            while !Task.isCancelled {
                guard let self else { return }
                await self.refreshRegistry()
                await self.refreshAccount()
                await self.refreshPeople()
                // The New Message sheet's list, ahead of the first "+" (TTL-gated, off this loop).
                Task { await self.loadDirectory() }
                await self.refreshServers()
                await self.startMatrixIfNeeded()
                await self.drainPushes()
                await self.refreshUnread()
                self.retryPendingOpen()
                try? await Task.sleep(nanoseconds: 15_000_000_000)
            }
        }
    }

    // MARK: Registry

    func refreshRegistry() async {
        do {
            let result = try await api.registry()
            let items = result.projects
            registryError = nil
            let previousProjects = projects
            let previousChatsMap = chats
            var newProjects: [Project] = []
            var newChats: [Id: [Chat]] = [:]
            for item in items {
                var p = item.project
                if let prev = previousProjects.first(where: { $0.id == p.id }) {
                    p.modelNote = prev.modelNote
                    if p.state == prev.state { p.stateDetail = prev.stateDetail }
                }
                newProjects.append(p)
                var mine = item.chats
                let prevChats = previousChatsMap[p.id] ?? []
                for i in mine.indices {
                    if let prev = prevChats.first(where: { $0.id == mine[i].id }) {
                        mine[i].unread = prev.unread
                        mine[i].lastMessageAt = max(mine[i].lastMessageAt, prev.lastMessageAt)
                        if mine[i].title == "New chat" && prev.title != "New chat" {
                            mine[i].title = prev.title
                        }
                    }
                }
                newChats[p.id] = AppModel.sorted(mine)
            }
            let merged = Self.mergeSkippedProjects(
                fresh: newProjects,
                previous: previousProjects,
                previousChats: previousChatsMap,
                skippedIds: result.skippedIds
            )
            newProjects = merged.projects
            for (id, list) in merged.chats {
                newChats[id] = list
            }
            if !result.skippedIds.isEmpty {
                NSLog("registry: kept \(result.skippedIds.count) project(s) skipped on detail parse/decode")
            }
            if result.incomplete {
                NSLog("registry: list incomplete (unreadable item id); skipping room prune")
            }
            for i in newProjects.indices {
                if let prev = previousProjects.first(where: { $0.id == newProjects[i].id }) {
                    newProjects[i].workspace = prev.workspace
                }
            }
            projects = newProjects.sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
            chats = newChats
            reconcileAgentAvatars()
            reconcileArchivedProjects(newProjects)
            saveRegistryCache()
            setRelayLink(.live)
            await refreshMachine()
            for p in newProjects {
                ensureRoom(projectId: p.id, key: projectDocKey(p.id), roomChat: projectDocChatId)
            }
            for list in newChats.values {
                for c in list {
                    ensureRoom(projectId: c.projectId, key: c.id, roomChat: c.id)
                }
            }
            // Drop rooms the relay no longer lists (deleted projects/chats). People
            // rooms stay for refreshPeople. Skip keys created in the last 60s so a
            // chat minted locally is not pruned before the relay lists it. Skip the
            // whole prune when the list had an unreadable item (incomplete).
            if Self.shouldPrune(incomplete: result.incomplete) {
                let listedChatIds = Set(newChats.values.flatMap { $0.map(\.id) })
                let listedProjectIds = Set(newProjects.map(\.id))
                let peopleKeys = Set(rooms.compactMap { key, room in room.projectId == peopleSlot ? key : nil })
                    .union(peopleChats.map(\.id))
                let now = Date()
                let recentKeys = Set(roomCreatedAt.compactMap { key, at in
                    now.timeIntervalSince(at) < Self.roomRecentGraceSeconds ? key : nil
                })
                let doomed = Self.roomsToPrune(
                    roomKeys: Set(rooms.keys).union(docs.keys),
                    listedChatIds: listedChatIds,
                    listedProjectIds: listedProjectIds,
                    peopleKeys: peopleKeys,
                    recentKeys: recentKeys
                )
                for key in doomed { wipeRoom(key) }
            }
            firstLoginRegistryUpdated()
        } catch {
            if (error as? RelayError)?.isUnauthorized == true {
                // Token revoked or relay re-initialised: pair again.
                unpair(forget: true)
                pairingError = UserError.of("stay signed in", .authExpired, "This device was signed out by the relay. Pair again.")
                return
            }
            registryError = UserError.classify("load your chats", error)
            link = .reconnecting
            // Only a verdict about reachability goes to the pill; an odd error
            // (bad JSON, a 404) says nothing about the link.
            let fault = LinkState.fault(error)
            if fault == .offline || fault == .relayDown { setRelayLink(fault) }
        }
    }

    // MARK: On-device store (#929): open, load, save, flush

    private func openStore() {
        guard store == nil, let creds = credentials, !isFixture else { return }
        if ProcessInfo.processInfo.environment["ARBOS_NO_STORE"] == "1" { return }
        store = DeviceStore.open(relayUrl: creds.relayUrl, account: creds.deviceId)
        storeUsage = store?.usage() ?? DeviceStore.Usage()
    }

    /// Registry, people chats, account and unread counts: small JSON, read
    /// synchronously so the first frame already has the list.
    private func loadStoreKV(_ store: DeviceStore) {
        if let data = store.get(StoreKey.registry),
           let cached = try? JSONDecoder().decode(RegistryCache.self, from: data), !cached.projects.isEmpty {
            projects = cached.projects
            chats = cached.chats
        }
        if let data = store.get(StoreKey.people),
           let raw = try? JSONSerialization.jsonObject(with: data) as? [Any] {
            let list = raw.compactMap(PeopleChat.parse)
            if !list.isEmpty {
                peopleChats = list
                for c in list where c.unread { peopleUnread.insert(c.id) }
            }
        }
        if user == nil, let data = store.get(StoreKey.me), let who = try? JSONDecoder().decode(AuthUser.self, from: data) {
            user = who
        }
        if directory.isEmpty, let data = store.get(StoreKey.directory), let cached = AppModel.decodeDirectoryCache(data) {
            directory = cached
            AvatarImageCache.shared.warm(urls: cached.map(\.avatarUrl))
        }
        // The "+" sheet's New server row is decided before the first `GET /servers` answers.
        if let data = store.get(StoreKey.serversAvailable) {
            serversAvailable = data.first == 1
        }
        if let data = store.get(StoreKey.unread), let counts = try? JSONDecoder().decode([String: Int].self, from: data) {
            unreadCounts = counts
        }
        // One copy of each agent's avatar everywhere, before the first frame: the record and
        // the roster copies came back from two caches written at different times; the
        // newest wins and every picture URL is rebuilt on the public versioned route (a
        // pre-#1813 cache held the owner-only blob URL — Jacob's relaunch leaf, 2026-10-26).
        reconcileAgentAvatars()
        AvatarImageCache.shared.warm(urls: projects.compactMap(\.imageURL))
        if !projects.isEmpty || !peopleChats.isEmpty { contentOrigin = "store" }
        LaunchClock.mark("kv-loaded", detail: "projects=\(projects.count) people=\(peopleChats.count)")
    }

    /// Import every saved room off the main thread (last opened first), then
    /// hand each to `ensureRoom` with its cursor so it paints and dials at once.
    private func loadStoreDocs(_ store: DeviceStore) async {
        let saved = store.loadDocs()
        guard !saved.isEmpty else { return }
        let pending = store.loadPending()
        let started = Date()
        // Import and the Loro → Message conversion run off the main thread; the
        // main actor only receives finished docs and message arrays.
        let docs: [(DeviceStore.SavedDoc, CoreChatDoc, [Message]?)] = await Task.detached(priority: .userInitiated) {
            saved.compactMap { s in
                let doc = CoreChatDoc(key: s.key)
                do {
                    try doc.importUpdate(s.snapshot)
                } catch {
                    NSLog("store: snapshot for \(s.key) unreadable: \(error.localizedDescription)")
                    return nil
                }
                let messages: [Message]? = s.roomChat == projectDocChatId ? nil : doc.readAll()
                return (s, doc, messages)
            }
        }.value
        for (s, doc, messages) in docs {
            ensureRoom(projectId: s.projectId, key: s.key, roomChat: s.roomChat,
                       restored: (doc, s.cursor, pending[s.key] ?? [], messages))
        }
        LaunchClock.mark("store-docs", detail: "\(docs.count) docs in \(Int(Date().timeIntervalSince(started) * 1000)) ms")
    }

    private func saveRegistryCache() {
        guard let store else { return }
        guard !projects.isEmpty else {
            store.remove(StoreKey.registry)
            return
        }
        if let data = try? JSONEncoder().encode(RegistryCache(projects: projects, chats: chats)) {
            store.put(StoreKey.registry, data)
        }
    }

    private func savePeopleCache(_ raw: [Any]) {
        guard let store, let data = try? JSONSerialization.data(withJSONObject: raw) else { return }
        store.put(StoreKey.people, data)
    }

    /// Remember whether this relay runs servers, so the next launch's "+" sheet
    /// shows (or hides) New server on its first frame instead of after the first poll.
    func rememberServersAvailable(_ on: Bool) {
        store?.put(StoreKey.serversAvailable, Data([on ? 1 : 0]))
    }

    private func saveDirectoryCache() {
        guard let store else { return }
        if let data = AppModel.encodeDirectoryCache(directory) {
            store.put(StoreKey.directory, data)
        } else {
            store.remove(StoreKey.directory)
        }
    }

    /// The directory as the store keeps it; nil for an empty list (nothing to restore).
    nonisolated static func encodeDirectoryCache(_ list: [Person]) -> Data? {
        guard !list.isEmpty else { return nil }
        return try? JSONEncoder().encode(list)
    }

    /// nil when the bytes are not a non-empty list (an older store, or nothing saved).
    nonisolated static func decodeDirectoryCache(_ data: Data) -> [Person]? {
        guard let list = try? JSONDecoder().decode([Person].self, from: data), !list.isEmpty else { return nil }
        return list
    }

    private func saveAccountCache() {
        guard let store, let who = user, let data = try? JSONEncoder().encode(who) else { return }
        store.put(StoreKey.me, data)
    }

    private func saveUnreadCache() {
        guard let store, let data = try? JSONEncoder().encode(unreadCounts) else { return }
        store.put(StoreKey.unread, data)
    }

    /// A room's doc or cursor moved: save soon (coalesced per 1.5 s).
    private func markDirty(_ key: Id) {
        guard store != nil else { return }
        dirtyDocs.insert(key)
        guard flushTask == nil else { return }
        flushTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: AppModel.flushDelayNs)
            guard let self, !Task.isCancelled else { return }
            self.flushTask = nil
            self.flushStore(all: false)
        }
    }

    /// Write dirty rooms (or every room) to the store. The cursor is read
    /// before the export, so a saved snapshot always covers its cursor.
    func flushStore(all: Bool) {
        guard let store else { return }
        flushTask?.cancel()
        flushTask = nil
        let keys = all ? Set(docs.keys) : dirtyDocs
        dirtyDocs = []
        for key in keys {
            guard let doc = docs[key], let room = rooms[key], !doc.isEmpty else { continue }
            let cursor = room.cursor
            guard let bytes = try? doc.exportSnapshot() else { continue }
            store.saveDoc(key: key, projectId: room.projectId, roomChat: room.roomChat, cursor: cursor, snapshot: bytes)
        }
        store.enforceCaps()
        if all { store.flush() }
        if !keys.isEmpty { storeUsage = store.usage() }
    }

    private func forgetDoc(_ key: Id) {
        dirtyDocs.remove(key)
        store?.deleteDoc(key)
    }

    static func sorted(_ chats: [Chat]) -> [Chat] {
        chats.sorted { a, b in
            if a.isPinned != b.isPinned { return a.isPinned }
            return a.lastMessageAt > b.lastMessageAt
        }
    }

    private func projectDocKey(_ projectId: Id) -> String { "proj:\(projectId)" }

    /// Keys to drop after a successful registry refresh: not a listed project chat,
    /// not a listed project's doc, not a people room, and not created in the last 60s.
    nonisolated static func roomsToPrune(
        roomKeys: Set<String>,
        listedChatIds: Set<String>,
        listedProjectIds: Set<String>,
        peopleKeys: Set<String>,
        recentKeys: Set<String>
    ) -> Set<String> {
        let listedProjectDocs = Set(listedProjectIds.map { "proj:\($0)" })
        return Set(roomKeys.filter { key in
            if recentKeys.contains(key) { return false }
            if peopleKeys.contains(key) { return false }
            if listedChatIds.contains(key) { return false }
            if listedProjectDocs.contains(key) { return false }
            return true
        })
    }

    /// False when the registry list had an item with no readable id — pruning
    /// would wipe rooms for projects we never got a chance to list.
    nonisolated static func shouldPrune(incomplete: Bool) -> Bool {
        !incomplete
    }

    /// Keep previous projects (and their chats) whose detail failed to parse;
    /// drop anything that is neither in `fresh` nor in `skippedIds`.
    nonisolated static func mergeSkippedProjects(
        fresh: [Project],
        previous: [Project],
        previousChats: [Id: [Chat]],
        skippedIds: Set<String>
    ) -> (projects: [Project], chats: [Id: [Chat]]) {
        var projects = fresh
        var chats: [Id: [Chat]] = [:]
        let freshIds = Set(fresh.map(\.id))
        for id in skippedIds where !freshIds.contains(id) {
            guard let kept = previous.first(where: { $0.id == id }) else { continue }
            projects.append(kept)
            if let list = previousChats[id] {
                chats[id] = list
            }
        }
        return (projects, chats)
    }

    /// Whether to dial a room again after consecutive HTTP 403/404 endings.
    /// True below the retry limit; false at the limit until `backoff` seconds pass.
    nonisolated static func shouldRecreateRoom(
        failures: Int,
        lastFailureAt: Date?,
        now: Date,
        limit: Int = AppModel.roomUnavailableRetryLimit,
        backoff: TimeInterval = AppModel.roomUnavailableBackoffSeconds
    ) -> Bool {
        if failures < limit { return true }
        guard let lastFailureAt else { return true }
        return now.timeIntervalSince(lastFailureAt) >= backoff
    }

    // MARK: Rooms

    /// `restored`: a doc imported from the on-device store with the relay
    /// cursor it was saved at and the pushes the relay never ACKed.
    #if DEBUG
    /// Perf rig (iphone-open-scroll-block): the fixture group as a real room restored from
    /// an exported checkpoint, then the exported rows imported on the main actor one by
    /// one — the same `onChange` → `requestSnapshot` → `emitSnapshot` path a live socket
    /// drives. `rooms` > 1 adds clones under other ids so several chats catch up at once.
    func perfReplay(checkpointPath: String, rowsPath: String, rooms: Int, gapMs: Int, afterMs: Int) {
        guard let ckpt = FileManager.default.contents(atPath: checkpointPath),
              let rowsText = try? String(contentsOfFile: rowsPath, encoding: .utf8) else {
            NSLog("perf replay: missing files")
            return
        }
        let rowBytes: [Data] = rowsText.split(separator: "\n").compactMap { line in
            guard let bar = line.firstIndex(of: "|") else { return nil }
            return Data(hexString: String(line[line.index(after: bar)...]))
        }
        var keys: [Id] = ["fx-group-launch"]
        for i in 1..<max(1, rooms) { keys.append("fx-replay-\(i)") }
        var docs: [(Id, CoreChatDoc)] = []
        for key in keys {
            guard let doc = try? CoreChatDoc(key: key, snapshot: ckpt) else { continue }
            if key != "fx-group-launch", let group = peopleChats.first(where: { $0.id == "fx-group-launch" }) {
                var clone = group
                clone.id = key
                clone.title = "Replay \(key)"
                peopleChats.append(clone)
            }
            // Store-restore shape: the doc and its decoded list land before the room starts.
            ensureRoom(projectId: peopleSlot, key: key, roomChat: key, restored: (doc, 0, [], doc.readAll()))
            // No relay in the rig: the socket would redial forever and flap the link
            // (each flap publishes `caughtUp`). The doc's change handler stays.
            self.rooms[key]?.stop()
            caughtUp.remove(key)
            docs.append((key, doc))
        }
        LaunchClock.mark("replay-restored", detail: "rooms=\(docs.count) rows=\(rowBytes.count) ckpt=\(ckpt.count)B")
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(afterMs))
            // `ARBOS_PERF_REPLAY_LIVE=1`: the rooms count as live (rows arriving while the
            // reader scrolls) instead of catching up.
            for (key, _) in docs { if DebugHooks.perfReplayLive { caughtUp.insert(key) } else { caughtUp.remove(key) } }
            PerfLog.mark("phase replay begin", ms: 0)
            let started = Date()
            for (ix, bytes) in rowBytes.enumerated() {
                for (_, doc) in docs {
                    let t0 = Date()
                    _ = try? doc.importUpdateReportingPending(bytes)
                    PerfCounters.time("import", ms: Date().timeIntervalSince(t0) * 1000)
                }
                if gapMs > 0 { try? await Task.sleep(for: .milliseconds(gapMs)) } else { await Task.yield() }
                if ix == rowBytes.count - 1 {
                    for (key, _) in docs {
                        caughtUp.insert(key)
                        #if DEBUG
                        PerfCounters.bump("emit.L1838")
                        #endif
                        emitSnapshot(key)
                    }
                }
            }
            PerfLog.mark("phase replay end", ms: Date().timeIntervalSince(started) * 1000)
        }
    }

    /// Test seam (iphone-group-send-instant): a fixture people chat gets a real chat doc
    /// seeded with its fixture lines and a room that is stopped at once, so `sendPeople`
    /// writes the way a live chat does — doc, push queue, change events — with no relay.
    func fixtureAttachDoc(chatId: Id) {
        guard docs[chatId] == nil else { return }
        let doc = CoreChatDoc(key: chatId)
        for m in messages[chatId] ?? [] where m.role == .user || m.role == .system {
            let author = CoreChatDoc.Author(sender: m.sender, handle: m.senderHandle, name: m.senderName, avatar: m.senderAvatar)
            if m.role == .system {
                try? doc.insertSystemLine(id: m.id, text: m.plainText, deviceId: "ios:fixture")
            } else {
                try? doc.insertUserMessage(id: m.id, text: m.plainText, deviceId: "ios:fixture", author: author, replyTo: m.replyTo)
            }
        }
        doc.commit()
        ensureRoom(projectId: peopleSlot, key: chatId, roomChat: chatId, restored: (doc, 0, [], doc.readAll()))
        rooms[chatId]?.stop()
        caughtUp.insert(chatId)
    }
    #endif

    private func ensureRoom(projectId: Id, key: Id, roomChat: String,
                            restored: (doc: CoreChatDoc, cursor: Int64, pending: [(String, Data)], messages: [Message]?)? = nil) {
        guard rooms[key] == nil else { return }
        guard Self.shouldRecreateRoom(
            failures: roomUnavailableFailures[key] ?? 0,
            lastFailureAt: roomUnavailableAt[key],
            now: Date()
        ) else { return }
        // After an unavailable drop the doc/messages stay; reuse them with the
        // saved cursor so history is not fetched again.
        let effective: (doc: CoreChatDoc, cursor: Int64, pending: [(String, Data)], messages: [Message]?)?
        if let restored {
            effective = restored
        } else if let kept = docs[key] {
            let cursor = roomRestoredCursors.removeValue(forKey: key) ?? 0
            let pending = store?.loadPending()[key] ?? []
            effective = (kept, cursor, pending, messages[key])
        } else {
            effective = nil
        }
        let doc = effective?.doc ?? CoreChatDoc(key: key)
        let isProjectDoc = roomChat == projectDocChatId
        docs[key] = doc
        doc.onChange { [weak self] touched in
            Task { @MainActor in
                guard let self else { return }
                #if DEBUG
                if SendTrace.traces(key) {
                    SendTrace.mark("doc-change", detail: "list=\(touched.listChanged) msgs=\(touched.messages.count) caughtUp=\(self.caughtUp.contains(key))")
                }
                #endif
                self.markDirty(key)
                if isProjectDoc {
                    self.applyProjectDoc(projectId)
                } else if !self.caughtUp.contains(key), touched.listChanged || !touched.messages.isEmpty {
                    // Catching up: the relay streams one row per frame. One re-read per
                    // window for all of them (list or message touches alike), and the
                    // home-list bump rides along instead of re-sorting per row.
                    self.activityPending.insert(key)
                    self.requestSnapshot(key, windowMs: Self.catchUpCoalesceMs, deferAlways: true)
                } else if touched.listChanged {
                    // Live: a burst of rows (agents answering each other) folds into one
                    // re-read per short window too; the home-list bump and the unread
                    // light ride with that read (`applySnapshot`) instead of per row.
                    self.activityPending.insert(key)
                    self.requestSnapshot(key, windowMs: Self.liveCoalesceMs)
                } else if !touched.messages.isEmpty {
                    self.emitMessages(key, indexes: touched.messages)
                    // Same kick project chats get via `chats` in markActivity — people
                    // chats need it too so ChatView rebuilds while the thread is open
                    // (streaming agent replies only touch message indexes).
                    self.markActivity(chatId: key)
                } else if self.messages[key] == nil, let doc = self.docs[key], doc.count > 0 {
                    // Checkpoint / bulk import sometimes reports no list path —
                    // still paint so home search can see the transcript.
                    #if DEBUG
                    PerfCounters.bump("emit.L1897")
                    #endif
                    self.emitSnapshot(key)
                }
            }
        }
        let room = SessionRoom(projectId: projectId, key: key, roomChat: roomChat, doc: doc, api: api,
                               cursor: effective?.cursor ?? 0, pending: effective?.pending ?? []) { [weak self] in
            self?.deviceId ?? "ios:unpaired"
        }
        room.onCursorMoved = { [weak self] in self?.markDirty(key) }
        room.onPendingChanged = { [weak self] batches in self?.store?.savePending(key: key, batches: batches) }
        room.onLink = { [weak self] l in
            guard let self else { return }
            self.link = l
            // Backfill rows after a reconnect must not light unread; only live
            // rows after the next catch-up do (Loro is ahead of /me/unread).
            if l != .live { self.caughtUp.remove(key) }
            // Real live (not the fake .live on 403/404, which skips onLink).
            if l == .live {
                self.roomUnavailableFailures[key] = nil
                self.roomUnavailableAt[key] = nil
            }
        }
        room.onLinkState = { [weak self] state in self?.setRoomLink(key, state) }
        room.presenceUser = { [weak self] in self?.user.map { ($0.id, $0.login) } }
        room.onBoxState = { [weak self] state, detail in
            self?.setProjectState(projectId, state, detail: detail)
        }
        room.onDocPaint = { [weak self] in
            guard let self, !isProjectDoc else { return }
            #if DEBUG
            PerfCounters.bump("emit.L1926")
            #endif
            self.emitSnapshot(key)
        }
        room.onCaughtUp = { [weak self] in
            guard let self else { return }
            if isProjectDoc {
                self.applyProjectDoc(projectId)
            } else {
                // Live before the final read, so that read also bumps the home list.
                self.caughtUp.insert(key)
                #if DEBUG
                PerfCounters.bump("emit.L1935")
                #endif
                self.emitSnapshot(key)
                self.firstLoginChatSynced(key)
            }
        }
        room.onAcked = { [weak self] in
            // Project-doc writes are not user bubbles; clearing a global set here
            // used to leave people-chat messages stuck at pending opacity.
            guard let self, !isProjectDoc else { return }
            if self.pushRefused.remove(key) != nil {
                self.pushRefusalText.removeValue(forKey: key)
            }
            guard self.unacked[key] != nil else { return }
            self.unacked[key] = nil
            #if DEBUG
            PerfCounters.bump("emit.L1945")
            #endif
            self.emitSnapshot(key)
        }
        if !isProjectDoc {
            room.newestMessageId = { [weak self] in self?.messages[key]?.last?.id }
            room.onPresence = { [weak self] header in
                self?.notePresence(header)
                self?.noteTyping(header, chatId: key)
                if let callId = header["in_call"] as? String { self?.peerInCall(callId, chatId: key) }
            }
        }
        room.onUnavailable = { [weak self] in
            Task { @MainActor in self?.handleRoomUnavailable(key: key) }
        }
        room.onPushRefused = { [weak self, weak room] in
            guard let self, !isProjectDoc else { return }
            // The relay's reason, in plain words ([feedback blocked-state-ios-2026-10-26]):
            // "<name> has blocked you in this chat." / "Couldn't post: you've blocked <name>…" /
            // the mute line / the generic delivery line. A refusal that names a reason is taken
            // even while the room already stood refused, so the words follow the relay's. The
            // chat-level note under the header draws it (`ChatView.chatNote`), not a toast.
            let why = room?.lastRefusal
            let fresh = self.pushRefused.insert(key).inserted
            if fresh || why?.reason != nil {
                let text = why?.sentence ?? BlockedState.refusalSentence(reason: nil, name: nil, handle: nil)
                self.pushRefusalText[key] = text
            }
        }
        rooms[key] = room
        roomCreatedAt[key] = Date()
        if let effective {
            // Paint from disk now; the room's backfill only adds what is new.
            if isProjectDoc {
                applyProjectDoc(projectId)
            } else if let list = effective.messages {
                messages[key] = list
            } else {
                #if DEBUG
                PerfCounters.bump("emit.L1966")
                #endif
                emitSnapshot(key)
            }
        }
        room.start()
    }

    /// HTTP 403/404: drop the dead room (keep doc/messages/store), count the
    /// failure, then refresh lists so a still-listed chat gets a fresh room.
    private func handleRoomUnavailable(key: Id) {
        guard let room = rooms[key] else { return }
        let projectId = room.projectId
        roomRestoredCursors[key] = room.cursor
        room.stop()
        rooms[key] = nil
        roomLinks[key] = nil
        roomLinkSince[key] = nil
        roomCreatedAt[key] = nil
        caughtUp.remove(key)
        roomUnavailableFailures[key, default: 0] += 1
        roomUnavailableAt[key] = Date()
        Task {
            if projectId == peopleSlot {
                await refreshPeople()
                // Unlisted: refreshPeople only wipes live people rooms; the kept
                // doc would otherwise sit forever with no room to prune.
                if peopleChat(key) == nil, rooms[key] == nil { wipeRoom(key) }
            } else {
                await refreshRegistry()
            }
        }
    }

    /// Stop and clear a room that the relay no longer lists (docs and store too).
    private func wipeRoom(_ key: Id) {
        rooms[key]?.stop()
        rooms[key] = nil
        docs[key] = nil
        messages[key] = nil
        roomLinks[key] = nil
        roomLinkSince[key] = nil
        unacked[key] = nil
        pushRefused.remove(key)
        pushRefusalText[key] = nil
        roomCreatedAt[key] = nil
        roomRestoredCursors[key] = nil
        roomUnavailableFailures[key] = nil
        roomUnavailableAt[key] = nil
        forgetDoc(key)
    }

    /// Re-dial every listed chat after the unavailable backoff was cleared.
    private func recreateListedRooms() {
        for p in projects {
            ensureRoom(projectId: p.id, key: projectDocKey(p.id), roomChat: projectDocChatId)
        }
        for list in chats.values {
            for c in list {
                ensureRoom(projectId: c.projectId, key: c.id, roomChat: c.id)
            }
        }
        for c in peopleChats {
            ensureRoom(projectId: peopleSlot, key: c.id, roomChat: c.id)
        }
    }

    /// A list change from the doc: re-read now when the room was quiet, else once
    /// at the end of the coalesce window. Direct `emitSnapshot` callers (catch-up,
    /// checkpoint paint, ACK) still read at once and clear a pending request.
    /// `deferAlways`: never read at once (a catch-up's first row must not cost a
    /// full re-read when the next 70 arrive within the window); the paint the
    /// reader sees meanwhile is the store / checkpoint one.
    private func requestSnapshot(_ chatId: Id, windowMs: Int = AppModel.snapshotCoalesceMs, deferAlways: Bool = false) {
        #if DEBUG
        if SendTrace.traces(chatId) {
            let since = Date().timeIntervalSince1970 - (snapshotLastAt[chatId] ?? 0)
            SendTrace.mark("snapshot-request", detail: "window=\(windowMs) deferAlways=\(deferAlways) alreadyPending=\(snapshotPending.contains(chatId)) sinceLast=\(Int(since * 1000))ms")
        }
        #endif
        guard !snapshotPending.contains(chatId) else { return }
        let window = Double(windowMs) / 1000
        let since = Date().timeIntervalSince1970 - (snapshotLastAt[chatId] ?? 0)
        guard since < window || deferAlways else {
            #if DEBUG
            PerfCounters.bump("emit.L2040")
            #endif
            emitSnapshot(chatId)
            return
        }
        snapshotPending.insert(chatId)
        let delayMs = deferAlways ? windowMs : max(1, Int((window - since) * 1000))
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(delayMs))
            guard let self, self.snapshotPending.contains(chatId) else { return }
            #if DEBUG
            PerfCounters.bump("emit.L2048")
            #endif
            self.emitSnapshot(chatId)
        }
    }

    private func emitSnapshot(_ chatId: Id) {
        snapshotPending.remove(chatId)
        snapshotLastAt[chatId] = Date().timeIntervalSince1970
        guard let doc = docs[chatId] else { return }
        let gen = (snapshotGeneration[chatId] ?? 0) &+ 1
        snapshotGeneration[chatId] = gen
        #if DEBUG
        if SendTrace.traces(chatId) {
            SendTrace.mark("snapshot-read", detail: "rows=\(doc.count) async=\(doc.count >= Self.asyncSnapshotMinRows)", once: false)
        }
        #endif
        // Short lists decode inline (tests and the fixture read `messages` right after);
        // a long group's 1 MB of rows is read and parsed off the main actor — the FFI doc
        // is thread-safe — and applied when it is back, unless a newer read superseded it.
        if doc.count < Self.asyncSnapshotMinRows {
            applySnapshot(chatId, list: doc.readAll())
            return
        }
        Task.detached(priority: .userInitiated) { [weak self] in
            #if DEBUG
            let t0 = Date()
            #endif
            let list = doc.readAll()
            #if DEBUG
            PerfCounters.time("readAll.bg", ms: Date().timeIntervalSince(t0) * 1000)
            #endif
            await MainActor.run {
                guard let self, self.snapshotGeneration[chatId] == gen else { return }
                self.applySnapshot(chatId, list: list)
            }
        }
    }

    private func applySnapshot(_ chatId: Id, list: [Message]) {
        #if DEBUG
        let t0 = Date()
        defer { PerfCounters.time("applySnapshot", ms: Date().timeIntervalSince(t0) * 1000) }
        #endif
        var list = list
        let pendingIds = unacked[chatId] ?? []
        for i in list.indices where list[i].role == .user {
            list[i].pending = pendingIds.contains(list[i].id)
        }
        foldOptimisticNotices(chatId: chatId, into: &list)
        // Reassign the whole dictionary so `@Published` always fires. Subscript
        // writes alone have left ChatView's `@State` rows stale for live inbound
        // agent replies in an open people chat (desktop had the line; iPhone did not).
        var next = messages
        next[chatId] = list
        messages = next
        #if DEBUG
        if SendTrace.traces(chatId) {
            SendTrace.markIfPresent("messages-has-row", ids: list.map(\.id), detail: "via=snapshot rows=\(list.count)")
        }
        #endif
        // The home list re-sorts once the room is live, not per catch-up snapshot.
        if caughtUp.contains(chatId), activityPending.remove(chatId) != nil {
            markActivity(chatId: chatId)
            // Loro is live; /me/pushes and /me/unread can lag — light the
            // blue circle and drain the push queue as soon as a new row lands.
            noteLiveUnread(chatId: chatId)
        }
    }

    private func emitMessages(_ chatId: Id, indexes: Set<Int>) {
        guard let doc = docs[chatId] else { return }
        var list = messages[chatId] ?? []
        var needSnapshot = false
        let pendingIds = unacked[chatId] ?? []
        for ix in indexes.sorted() {
            guard var m = doc.readMessage(ix) else { continue }
            if m.role == .user { m.pending = pendingIds.contains(m.id) }
            if ix < list.count, list[ix].id == m.id {
                list[ix] = m
            } else if ix == list.count {
                list.append(m)
            } else {
                needSnapshot = true
            }
        }
        if needSnapshot {
            #if DEBUG
            PerfCounters.bump("emit.L2119")
            #endif
            emitSnapshot(chatId)
        } else {
            var next = messages
            next[chatId] = list
            messages = next
            #if DEBUG
            if SendTrace.traces(chatId) {
                SendTrace.markIfPresent("messages-has-row", ids: list.map(\.id), detail: "via=indexes rows=\(list.count)")
            }
            #endif
        }
    }

    private func markUnacked(chatId: Id, messageId: Id) {
        unacked[chatId, default: []].insert(messageId)
    }

    private func clearUnacked(chatId: Id, messageId: Id) {
        guard var set = unacked[chatId] else { return }
        set.remove(messageId)
        unacked[chatId] = set.isEmpty ? nil : set
    }

    private func markActivity(chatId: Id) {
        for (pid, list) in chats {
            if let i = list.firstIndex(where: { $0.id == chatId }) {
                var updated = list
                updated[i].lastMessageAt = nowMs()
                chats[pid] = AppModel.sorted(updated)
                return
            }
        }
        // People chats: mirror the project-chat `chats` write. Without this,
        // live inbound rows while watching only touch `messages` — and when that
        // publish is missed, ChatView keeps stale `@State` rows (`noteLiveUnread`
        // no-ops while watching). Same bump local sends already do in commitPeople.
        if let i = peopleChats.firstIndex(where: { $0.id == chatId }) {
            var next = peopleChats
            next[i].lastActivityAt = nowMs()
            next.sort { $0.lastActivityAt > $1.lastActivityAt }
            peopleChats = next
        }
    }

    /// A live Loro row from someone else landed. Chat on screen in the foreground: the
    /// user has seen it, so the per-user read cursor on the relay advances (#1438) and
    /// nothing lights. Otherwise: show the blue circle immediately and drain
    /// `/me/pushes` so the banner does not wait for the 15 s registry poll (notify /
    /// APNs can lag the row). Own rows and system lines do neither.
    private func noteLiveUnread(chatId: Id) {
        guard !isHiddenChat(chatId) else { return }
        if let last = messages[chatId]?.last {
            if last.pending { return }
            if last.role == .system { return }
            if let me = myUserId, last.sender == me { return }
            // Hidden for me (someone I blocked): no dot, no banner.
            if hidesMessage(last, chatId: chatId) { return }
            // Project-chat echoes from this device use sender "user".
            if peopleChat(chatId) == nil, last.role == .user, last.sender == "user" || last.sender.isEmpty {
                return
            }
        }
        if isWatching(chatId) {
            if peopleChat(chatId) != nil { markPeopleRead(chatId, force: true) } else { markRead(chatId: chatId, force: true) }
            return
        }
        var changed = false
        if peopleChat(chatId) != nil {
            if !peopleUnread.contains(chatId) {
                peopleUnread.insert(chatId)
                changed = true
            }
            if let i = peopleChats.firstIndex(where: { $0.id == chatId }) {
                peopleChats[i].lastActivityAt = nowMs()
                if peopleChats[i].headSeq <= peopleChats[i].readSeq {
                    peopleChats[i].headSeq = peopleChats[i].readSeq + 1
                    changed = true
                }
            }
        } else {
            for (pid, list) in chats {
                if let i = list.firstIndex(where: { $0.id == chatId }), !list[i].unread {
                    var updated = list
                    updated[i].unread = true
                    chats[pid] = updated
                    changed = true
                    break
                }
            }
        }
        if changed {
            applyBadge()
            // An archived row comes back when someone writes in it.
            unarchiveOnActivity(chatId: chatId)
        }
        Task { await drainPushes() }
    }

    /// Project doc (crdt-sync §6.3): `settings.modelNote`, plus the D70
    /// mirrors `status.workspace {state, since}` and `status.box {state, since}`.
    /// The machine itself comes from `GET /machine`; the doc only refines the
    /// workspace dot between polls.
    private func applyProjectDoc(_ projectId: Id) {
        guard let doc = docs[projectDocKey(projectId)] else { return }
        let settings = doc.mapValue("settings")
        let note = settings["modelNote"] as? String ?? ""
        let status = doc.mapValue("status")
        let workspace = WorkspaceState.from((status["workspace"] as? [String: Any])?["state"] as? String)
        if let i = projects.firstIndex(where: { $0.id == projectId }) {
            if projects[i].modelNote != note { projects[i].modelNote = note }
            if workspace != .unknown, projects[i].workspace != workspace { projects[i].workspace = workspace }
        }
    }

    private func setProjectState(_ projectId: Id, _ state: BoxState, detail: String?) {
        guard let i = projects.firstIndex(where: { $0.id == projectId }) else { return }
        if projects[i].state == state && projects[i].stateDetail == detail { return }
        if projects[i].state != state { projects[i].stateSince = nowMs() }
        projects[i].state = state
        projects[i].stateDetail = detail
        // Room STATUS frames report the user's box (D70): the machine moved too.
        let shown = machine.coerced(state)
        if machine.reported, machine.state != shown, state != .error {
            machine.state = shown
            machine.since = nowMs()
        }
    }

    /// `GET /machine`: state, since, spec, spend and the workspace states.
    func refreshMachine() async {
        do {
            var m = try await api.machine()
            // `/me` may carry `always_on` on relays whose `/machine` does not.
            if meAlwaysOn == true, !m.alwaysOn {
                m.alwaysOn = true
                m.state = m.coerced(m.state)
            }
            machineError = nil
            if m != machine { machine = m }
            for i in projects.indices {
                if let w = m.workspaces[projects[i].id], projects[i].workspace != w { projects[i].workspace = w }
            }
        } catch {
            if (error as? RelayError)?.status == 404 {
                machineError = "This relay has no machine API yet"
            } else {
                machineError = UserError.classify("reach your machine", error).sentence
            }
        }
    }

    /// Machine-level error text (relay error or the machine's own state detail).
    var machineDetail: String? {
        if machine.state == .error {
            return projects.first { $0.state == .error }?.stateDetail ?? machineError
        }
        return nil
    }

    // MARK: Machine control (D70: one call for the whole box)

    func startMachine() {
        guard !machineBusy else { return }
        machineBusy = true
        machine.state = machine.coerced(.waking)
        machine.since = nowMs()
        Task {
            defer { machineBusy = false }
            do {
                try await api.startMachine()
            } catch {
                fail("start the machine", error)
                machine.state = .error
                machineError = UserError.classify("start the machine", error).sentence
            }
            await refreshMachine()
        }
    }

    func stopMachine() {
        guard !machineBusy else { return }
        machineBusy = true
        Task {
            defer { machineBusy = false }
            do {
                try await api.stopMachine()
                machine.state = .stoppedByUser
                machine.since = nowMs()
            } catch {
                fail("stop the machine", error)
            }
            await refreshMachine()
        }
    }

    /// Always-on boxes (D73): the one control is a restart. The state stays
    /// awake throughout; `machineBusy` is the only progress signal.
    func restartMachine() {
        guard !machineBusy else { return }
        machineBusy = true
        Task {
            defer { machineBusy = false }
            do {
                try await api.restartMachine()
                machine.since = nowMs()
            } catch {
                fail("restart the machine", error)
                machine.state = .error
                machineError = UserError.classify("restart the machine", error).sentence
            }
            await refreshMachine()
        }
    }

    // MARK: Connectors (F1)

    /// Load the relay's connector catalog + connected accounts for Settings.
    /// Failures stay quiet — opening Settings must not alert on a transient TLS
    /// blip. Connect / Disconnect still set `notice` themselves.
    func refreshConnectors() {
        guard credentials != nil, !isFixture else { return }
        Task {
            do {
                connectors = try await api.connectors()
            } catch let e as RelayError where e.status == 404 {
                // Older relays have no connectors route.
                connectors = ConnectorsView(loaded: true)
            } catch {
                // Same as refreshConnectorsOnce: leave prior state, no alert.
            }
        }
    }

    /// Start OAuth in the system auth sheet, or store a pasted token. The relay finishes
    /// the HTTPS callback, then redirects to `arbos://connectors/callback` so the sheet
    /// closes and we land back in Settings. Falls back to Safari if the sheet cannot
    /// start. Errors go through `notice` — Settings shows them itself (the root alert
    /// sits under the sheet and looks like a silent fail).
    func connectConnector(provider: String, token: String? = nil) {
        guard !connectorsBusy, credentials != nil, !isFixture else { return }
        connectorsBusy = true
        Task { @MainActor in
            do {
                if let token, !token.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    _ = try await api.connectConnector(provider: provider, token: token)
                    connectorsBusy = false
                    await refreshConnectorsOnce()
                    if connectors.connector(for: provider)?.status != .connected {
                        let err = connectors.connector(for: provider)?.lastError ?? ""
                        fail(UserError.refused("connect \(provider)", err.isEmpty ? "The token was not accepted." : err))
                    }
                    return
                }
                guard let urlStr = try await api.connectConnector(provider: provider),
                      let url = URL(string: urlStr) else {
                    connectorsBusy = false
                    fail(UserError.fromText("connect \(provider)", "the relay sent no sign-in URL"))
                    await refreshConnectorsOnce()
                    return
                }
                let web = WebAuth()
                do {
                    let callback = try await web.open(url)
                    connectorsBusy = false
                    let items = URLComponents(url: callback, resolvingAgainstBaseURL: false)?.queryItems ?? []
                    let ok = items.first(where: { $0.name == "ok" })?.value != "0"
                    await refreshConnectorsOnce()
                    if connectors.connector(for: provider)?.status == .connected { return }
                    if !ok {
                        let err = connectors.connector(for: provider)?.lastError ?? ""
                        fail(UserError.refused("connect \(provider)", err.isEmpty ? "The sign-in did not finish." : err))
                        return
                    }
                    // Token write can lag the redirect by a beat.
                    for wait in [1, 2, 4] as [UInt64] {
                        try? await Task.sleep(nanoseconds: wait * 1_000_000_000)
                        await refreshConnectorsOnce()
                        if connectors.connector(for: provider)?.status == .connected { break }
                    }
                } catch let e as AuthError where e.message == "Sign-in cancelled" {
                    connectorsBusy = false
                    await refreshConnectorsOnce()
                } catch let e as AuthError where e.message == "Could not open the sign-in sheet" {
                    // Rare: fall back to Safari and poll like Mac.
                    let opened = await UIApplication.shared.open(url)
                    connectorsBusy = false
                    guard opened else {
                        fail(UserError.fromText("connect \(provider)", "could not open the sign-in page"))
                        return
                    }
                    for wait in [0, 4, 8, 15, 30, 60] as [UInt64] {
                        if wait > 0 { try? await Task.sleep(nanoseconds: wait * 1_000_000_000) }
                        do {
                            connectors = try await api.connectors()
                        } catch { break }
                        if connectors.connector(for: provider)?.status == .connected { break }
                    }
                } catch {
                    connectorsBusy = false
                    fail(Self.connectError(provider: provider, error: error))
                    await refreshConnectorsOnce()
                }
            } catch {
                connectorsBusy = false
                fail(Self.connectError(provider: provider, error: error))
                await refreshConnectorsOnce()
            }
        }
    }

    /// Drop relay env-var names from the Settings alert when Google OAuth is missing.
    private static func connectError(provider: String, error: Error) -> UserError {
        let raw = error.localizedDescription
        if raw.localizedCaseInsensitiveContains("OAuth client not configured") {
            return UserError.refused("connect \(provider)", "The OAuth client is not configured on the relay.")
        }
        return UserError.classify("connect \(provider)", error)
    }

    func disconnectConnector(id: Id) {
        guard !connectorsBusy, credentials != nil, !isFixture else { return }
        connectorsBusy = true
        var next = connectors
        next.items.removeAll { $0.id == id }
        connectors = next
        Task {
            defer { connectorsBusy = false }
            do {
                try await api.disconnectConnector(id: id)
            } catch {
                fail("disconnect it", error)
            }
            await refreshConnectorsOnce()
        }
    }

    private func refreshConnectorsOnce() async {
        guard credentials != nil, !isFixture else { return }
        do {
            connectors = try await api.connectors()
        } catch let e as RelayError where e.status == 404 {
            connectors = ConnectorsView(loaded: true)
        } catch {}
    }

    // MARK: People chats (D72)

    var myUserId: String? { user.map { "user:\($0.id)" } }

    func peopleChat(_ id: String) -> PeopleChat? { peopleChats.first { $0.id == id } }

    func isPeopleChat(_ id: String) -> Bool { peopleChat(id) != nil }

    func refreshPeople() async {
        guard credentials != nil, !isFixture else { return }
        do {
            let raw = try await api.peopleChatsJSON()
            let list = raw.compactMap(PeopleChat.parse)
            peopleChats = list
            reconcileAgentAvatars()
            reconcileArchivedPeople(list)
            for c in list where c.unread { peopleUnread.insert(c.id) }
            for c in list where !c.unread && visibleChatId != c.id {
                // Loro can mark unread before the people list's head_seq catches up;
                // keep the blue circle while the latest row is still incoming.
                if peopleUnread.contains(c.id), let last = messages[c.id]?.last,
                   !last.pending, last.role != .system,
                   myUserId == nil || last.sender != myUserId {
                    continue
                }
                peopleUnread.remove(c.id)
            }
            for c in list { ensureRoom(projectId: peopleSlot, key: c.id, roomChat: c.id) }
            // Drop rooms/docs for people chats the relay no longer lists so a
            // restored chat cannot reconnect forever on HTTP 403.
            let listed = Set(list.map(\.id))
            let now = Date()
            for key in Set(rooms.keys).union(docs.keys) {
                guard !listed.contains(key) else { continue }
                guard rooms[key]?.projectId == peopleSlot else { continue }
                if let at = roomCreatedAt[key], now.timeIntervalSince(at) < Self.roomRecentGraceSeconds {
                    continue
                }
                wipeRoom(key)
            }
            savePeopleCache(raw)
        } catch {
            // Relays without people chats (older) answer 404; stay quiet.
        }
    }

    /// The relay's people search (a handle-prefix lookup) for the new-chat sheet;
    /// the sheet itself lists the directory through the shared matcher
    /// (`PeoplePicker.people`) and adds these on top. The fixture answers from
    /// the directory through the same matcher.
    func searchPeople(_ q: String) async -> [Person] {
        if isFixture {
            return PeoplePicker.people(directory: directory, remote: [], me: nil, query: q)
        }
        return (try? await api.searchUsers(q)) ?? []
    }

    /// `GET /users` (members and invited people except me), cached 10 min.
    /// Older relays without the route: the union of one-letter searches.
    func loadDirectory(force: Bool = false) async {
        guard credentials != nil, !isFixture else { return }
        if !force, !AppModel.directoryNeedsRefresh(loadedAt: directoryLoadedAt, now: nowMs(), isEmpty: directory.isEmpty) { return }
        directoryLoadedAt = nowMs()
        if let list = try? await api.users() {
            setDirectory(list.filter { $0.id != user?.id })
            return
        }
        var seen: [String: Person] = [:]
        await withTaskGroup(of: [Person].self) { group in
            for ch in "abcdefghijklmnopqrstuvwxyz0123456789" {
                group.addTask { [api] in (try? await api.searchUsers(String(ch))) ?? [] }
            }
            for await list in group {
                for p in list where p.status != "github" && p.id != user?.id { seen[p.id] = p }
            }
        }
        setDirectory(seen.values.sorted { $0.handle.lowercased() < $1.handle.lowercased() })
    }

    /// A refresh is due when nothing is loaded yet or the TTL has passed. A restored
    /// (non-empty) directory with no load this launch refetches at once, in the background.
    nonisolated static func directoryNeedsRefresh(loadedAt: Int64, now: Int64, isEmpty: Bool) -> Bool {
        isEmpty || loadedAt == 0 || now - loadedAt >= directoryTtlMs
    }

    /// Publish a fresh directory only when it differs (an unchanged list must not
    /// repaint an open sheet), persist it, and warm the avatars it shows.
    private func setDirectory(_ list: [Person]) {
        if let changed = AppModel.directoryUpdate(current: directory, fresh: list) {
            var t = Transaction()
            t.disablesAnimations = true
            withTransaction(t) { directory = changed }
            saveDirectoryCache()
        }
        AvatarImageCache.shared.warm(urls: list.map(\.avatarUrl))
    }

    /// The list to publish after a refresh: nil when it equals what is shown.
    nonisolated static func directoryUpdate(current: [Person], fresh: [Person]) -> [Person]? {
        fresh == current ? nil : fresh
    }

    /// People group: `POST /chats {kind: group, members}`. Relays that still
    /// return 501 for that route: open a DM with the first person, then add the
    /// rest (`POST /chats/{id}/members` turns a DM into a group on the third person).
    /// Optional `agentIds` are added afterwards (`POST /chats/{id}/agents`) so the
    /// group can start with people and your agents in one step.
    /// People, agents, or both — never nobody. Agents ride in the create body
    /// (`agent_ids`); whatever the relay did not add (an older relay) is added
    /// one by one afterwards.
    func createGroup(handles: [String], agentIds: [Id] = [], title: String? = nil, leaf: LeafPick? = nil, picture: DraftPicture? = nil) async -> Bool {
        let cleaned = handles
            .map { $0.trimmingCharacters(in: CharacterSet(charactersIn: "@ ")) }
            .filter { !$0.isEmpty }
        guard !cleaned.isEmpty || !agentIds.isEmpty else {
            failRefused("create the group", "Pick at least one person or agent.")
            return false
        }
        do {
            var chat: PeopleChat
            do {
                chat = try await api.createGroup(handles: cleaned, agentIds: agentIds, title: title, leaf: leaf)
            } catch let e as RelayError where !cleaned.isEmpty && (e.status == 404 || e.status == 405 || e.status == 501) {
                chat = try await createGroupViaDm(handles: cleaned)
            }
            if let picture, let jpeg = await draftJpeg(picture) {
                let key = "group-avatar-\(Int(Date().timeIntervalSince1970 * 1000)).\(PictureMaster.ext(jpeg))"
                if (try? await api.putBlob(projectId: "\(peopleSlot):\(chat.id)", key: key, mime: PictureMaster.mime(jpeg), data: jpeg)) != nil,
                   let patched = try? await api.patchPeopleChat(chat.id, imageKey: key) {
                    chat = patched
                }
            }
            for pid in agentIds where !chat.hasAgent(pid) {
                do {
                    chat = try await api.addAgent(chatId: chat.id, projectId: pid)
                } catch {
                    fail("add the agent to the new group", error)
                }
            }
            if let i = peopleChats.firstIndex(where: { $0.id == chat.id }) {
                peopleChats[i] = chat
            } else {
                peopleChats.insert(chat, at: 0)
            }
            ensureRoom(projectId: peopleSlot, key: chat.id, roomChat: chat.id)
            path = [.chat(chatId: chat.id, projectId: peopleSlot)]
            return true
        } catch {
            fail("create the group", error)
            return false
        }
    }

    /// Fallback when `POST /chats {kind: group}` is not on the relay yet.
    private func createGroupViaDm(handles: [String]) async throws -> PeopleChat {
        var chat = try await api.openDM(handle: handles[0])
        for handle in handles.dropFirst() {
            chat = try await api.addMember(chatId: chat.id, handle: handle)
        }
        return chat
    }

    /// Agent profile: rename / recolour a project (`PATCH /projects/{id}`).
    // MARK: Camera vision (§11)

    func setVision(_ projectId: Id, enabled: Bool) {
        if let i = projects.firstIndex(where: { $0.id == projectId }) { projects[i].visionEnabled = enabled }
        guard !isFixture else { return }
        Task {
            do {
                if let kept = try await api.setProjectVision(projectId, enabled: enabled),
                   let i = projects.firstIndex(where: { $0.id == projectId }) { projects[i].visionEnabled = kept }
            } catch {
                fail("save the camera setting", error)
                await refreshRegistry()
            }
        }
    }

    // MARK: Call voice (§10.3)

    @Published private(set) var voiceCatalog: VoiceCatalog?

    /// `GET /voices` once per launch (the relay caches it for an hour anyway). Nothing is
    /// fetched while voice mode is switched off on the relay (no picker to feed).
    /// The call-voice catalog for the agent's Voice row and `VoicePickerView`. It is
    /// independent of the voice-mode kill switch (spoken replies): the row picks the
    /// voice used in calls, so the list must load while voice mode is off.
    func loadVoices() async {
        guard voiceCatalog == nil else { return }
        if isFixture {
            voiceCatalog = AppModel.fixtureVoiceCatalog
            return
        }
        voiceCatalog = try? await api.voices()
    }

    /// The Mac fixture's five voices (`fixture_voices` in arbos-mac), so the rig's picker
    /// shows rows instead of the relay error.
    static let fixtureVoiceCatalog = VoiceCatalog(
        defaultId: "ara",
        voices: [
            Voice(id: "ara", name: "Ara", description: "Warm and clear, a touch of gravel. The current default."),
            Voice(id: "rex", name: "Rex", description: "Low, steady, unhurried."),
            Voice(id: "sol", name: "Sol", description: "Bright and quick; good for short answers."),
            Voice(id: "ivy", name: "Ivy", description: "Soft, close to the mic."),
            Voice(id: "ode", name: "Ode", description: "Even and neutral, reads long text well."),
        ]
    )

    /// Save on tap: show it at once, PATCH, then read back what the relay kept.
    func setVoice(_ projectId: Id, _ voice: String?) {
        if let i = projects.firstIndex(where: { $0.id == projectId }) { projects[i].voice = voice }
        guard !isFixture else { return }
        Task {
            do {
                let kept = try await api.setProjectVoice(projectId, voice: voice)
                if let i = projects.firstIndex(where: { $0.id == projectId }) { projects[i].voice = kept }
            } catch {
                fail("save the voice", error)
                await refreshRegistry()
            }
        }
    }

    // MARK: Agent pane — routines and the box report ([feedback iphone-agent-pane-tabs-2026-10-25])

    /// Each owned agent's routines, as last listed (the Details tab's Routines card).
    @Published private(set) var routines: [Id: [Routine]] = [:]
    /// The box as `GET /machine/debug` last reported it (the Computer tab, the Coding row).
    @Published private(set) var boxReport: BoxReport?

    /// `GET /projects/{id}/routines` when an owned agent's pane opens.
    func refreshRoutines(_ projectId: Id) async {
        if isFixture {
            #if DEBUG
            routines[projectId] = fixtureRoutines(projectId)
            #endif
            return
        }
        guard let list = try? await api.routines(projectId) else { return }
        routines[projectId] = list
    }

    /// The row's switch: flip at once, PATCH, read back on failure.
    func setRoutineEnabled(_ projectId: Id, routineId: Id, enabled: Bool) {
        if var list = routines[projectId], let i = list.firstIndex(where: { $0.id == routineId }) {
            list[i].enabled = enabled
            if !enabled { list[i].nextRunAt = nil }
            routines[projectId] = list
        }
        guard !isFixture else { return }
        Task {
            do {
                try await api.setRoutineEnabled(projectId, routineId: routineId, enabled: enabled)
            } catch {
                fail("save the routine", error)
            }
            await refreshRoutines(projectId)
        }
    }

    /// `GET /machine/debug` when the pane opens and when the Computer tab is picked. A relay
    /// without the route (404) leaves the last report alone; any other failure is shown as
    /// "The machine did not answer" with the reason.
    func refreshBoxReport() async {
        if isFixture {
            #if DEBUG
            boxReport = fixtureBoxReport()
            #endif
            return
        }
        do {
            boxReport = try await api.machineDebug()
        } catch let error as RelayError where error.status == 404 {
            return
        } catch {
            var r = BoxReport(debug: [String: Any](), now: nowMs())
            r.state = machine.state.rawValue
            r.unreachableReason = UserError.classify("reach the machine", error).why
            boxReport = r
        }
    }

    func updateProject(_ projectId: Id, name: String? = nil, color: String? = nil, icon: String? = nil, avatar: String? = nil, avatarColor: String? = nil, description: String? = nil) {
        if let i = projects.firstIndex(where: { $0.id == projectId }) {
            if let description { projects[i].description = description }
            if let name { projects[i].name = name }
            if let color { projects[i].color = color }
            if let icon { projects[i].icon = icon }
            if let avatar { projects[i].avatar = avatar }
            // "mono" is kept as an explicit pick; nil would fall back to the coloured default (D74).
            if let avatarColor { projects[i].avatarColor = avatarColor }
        }
        guard !isFixture else { return }
        Task {
            do { try await api.patchProject(projectId, name: name, color: color, icon: icon, avatar: avatar, avatarColor: avatarColor, description: description) }
            catch { fail("save the file", error) }
        }
    }

    // MARK: Avatar colour (D79)

    /// D92: the owner's leaf and disc colour picks (nil = automatic). Optimistic, then PATCH.
    func pickAvatarLeaf(_ projectId: Id, leaf: Int?, color: Int?) {
        // Optimistic, into every copy (header, rosters, chips) at once; the relay's stamp
        // replaces the local one when it answers.
        let local = (newestAgentAvatar(projectId)?.version ?? 0) + 1
        applyAgentAvatar(projectId, AgentAvatarFacts(imageKey: nil, avatarLeaf: leaf, avatarLeafColor: color, version: local), force: true)
        guard !isFixture else { return }
        Task {
            do {
                let stamped = try await api.setProjectAvatarLeaf(projectId, leaf: leaf, color: color)
                if stamped > 0 {
                    applyAgentAvatar(projectId, AgentAvatarFacts(imageKey: nil, avatarLeaf: leaf, avatarLeafColor: color, version: stamped), force: true)
                }
            } catch {
                fail("save the picture", error)
                await refreshRegistry()
            }
        }
    }

    /// Open (or create) the DM with a handle and navigate to it.
    func openDM(handle: String, navigate: Bool = true) async -> PeopleChat? {
        #if DEBUG
        if isFixture { return fixtureOpenDM(handle: handle, navigate: navigate) }
        #endif
        do {
            let chat = try await api.openDM(handle: handle)
            if !peopleChats.contains(where: { $0.id == chat.id }) { peopleChats.insert(chat, at: 0) }
            ensureRoom(projectId: peopleSlot, key: chat.id, roomChat: chat.id)
            if navigate { path = [.chat(chatId: chat.id, projectId: peopleSlot)] }
            return chat
        } catch {
            fail((error as? RelayError)?.type == "not_available"
                 ? UserError.refused("open the chat", "You can't message this person.")
                 : UserError.classify("open the chat", error))
            return nil
        }
    }

    func open(peopleChat chat: PeopleChat) {
        unarchive(peopleChatId: chat.id)
        path = [.chat(chatId: chat.id, projectId: peopleSlot)]
    }

    /// Send into a people chat: author fields + mentions resolved from `@slug`s.
    /// `address` is an optional chip target (`agent:…` / `user:…` / `"everyone"`) when the
    /// text itself has no `@` mention — so a follow-up can keep talking to one agent, or
    /// "hey everyone" can reach every agent, without typing `@`.
    /// `chipMentions` are extra agent member ids from a multi-select on the conversation
    /// selector (any combination of agents that should all reply).
    /// `chipTokens` (#1596) are the composer's committed mention chips, lower-case handle →
    /// member id: an `@handle` in the text that has a chip means exactly that member.
    func sendPeople(chatId: String, text: String, replyTo: String?, address: String? = nil,
                    chipMentions: [String] = [], attachments: [PendingAttachment] = [],
                    forwardedFrom: String? = nil, chipTokens: [String: String] = [:],
                    replyRef: ReplyRef? = nil) {
        guard let doc = docs[chatId], let chat = peopleChat(chatId), let me = user else { return }
        var mentions = AppModel.mentions(in: text, members: chat.members, senderHandle: me.login, chipTokens: chipTokens)
        // Leading bare agent names ("Bob?", "hey Bob …", "Arbos what is your name") count as
        // addressing that agent — same as typing @bob — so the box wakes without `@`.
        // Resolve agents before people so a human display-name collision (e.g. @unarbos
        // named Arbos vs agent @arbos) does not steal the address.
        let leadingAgents = AppModel.namedAgents(in: text, members: chat.members)
        for agent in leadingAgents where !mentions.contains(agent) {
            mentions.append(agent)
        }
        // Leading bare person names ("Allan", "Allan?", "Yo Allan …") count as addressing
        // that person — so a sticky "Message @bob" chip does not wake the agent. When an
        // agent handle/slug already matched, only person @handles count (not display names).
        let leadingPeople = leadingAgents.isEmpty
            ? AppModel.namedPeople(in: text, members: chat.members)
            : AppModel.namedPeopleByHandle(in: text, members: chat.members)
        for person in leadingPeople where !mentions.contains(person) {
            mentions.append(person)
        }
        // Conversation-selector combinations: every selected agent is mentioned so each box
        // wakes (agentd allows several replies on a shared people `send`).
        for agent in chipMentions where agent.hasPrefix("agent:") && !mentions.contains(agent) {
            mentions.append(agent)
        }
        // Agent @mentions, bare agent names, and "everyone"/"agents" phrases count as an
        // address. A leading person name must not keep a sticky agent chip — otherwise
        // "Message @bob" + "Allan?" wakes bob. A person @handle mid-line must not block the
        // chip (bob still answers about @jo). "everyone" always expands to every agent even
        // when a leftover `@agent` chip already named one.
        let addressedAgent = mentions.contains { $0.hasPrefix("agent:") }
        if let address {
            if address == "everyone" {
                mentions.append(contentsOf: chat.agents.map(\.member).filter { !mentions.contains($0) })
            } else if !addressedAgent {
                if address.hasPrefix("agent:"), !leadingPeople.isEmpty {
                    // Text opens by naming a person; leave the sticky agent out of mentions.
                } else if chat.members.contains(where: { $0.member == address }), !mentions.contains(address) {
                    mentions.append(address)
                }
            }
        }
        // In an agent-only DM, carry on with the agent who last spoke when nothing addresses
        // an agent and the message is not aimed at a person. A DM with a human peer (Allan +
        // bob) must not inherit that sticky target — otherwise "New test" / "Don't respond bob"
        // still wakes bob. A person chip / @handle / bare name must not also ping the last
        // agent — otherwise "Message Allan" still wakes bob. In a group, leave the message
        // unaddressed (unless a chip / @agent set one) so the box decision model can choose
        // which agent replies from chat context.
        var reply = replyTo
        let addressedPerson = mentions.contains { $0.hasPrefix("user:") }
        let hasHumanPeer = chat.humans.contains { $0.id != me.id }
        if chat.kind != "group", !hasHumanPeer, !mentions.contains(where: { $0.hasPrefix("agent:") }),
           !addressedPerson, reply == nil,
           let last = messages(of: chatId).last(where: { $0.role == .assistant }) {
            reply = last.id
            if let h = last.senderHandle?.lowercased(),
               let agent = chat.agents.first(where: {
                   mentionSlug($0.project?.name ?? "").lowercased() == h || $0.handle.lowercased() == h
               }) {
                mentions.append(agent.member)
            }
        }
        let author = CoreChatDoc.Author(sender: "user:\(me.id)", handle: me.login, name: me.name, avatar: me.avatarUrl)
        let messageId = newId()
        // Text-only: write on this turn (same as project `write`) so the push leaves
        // before we return. Deferring every send into a Task left bubbles faint when
        // the room ACK raced a still-queued commit.
        // A cross-chat reply never carries an in-chat `replyTo` as well.
        if replyRef != nil { reply = nil }
        if attachments.isEmpty {
            commitPeople(chatId: chatId, doc: doc, messageId: messageId, text: text, author: author,
                         mentions: mentions, reply: reply, attachments: [], forwardedFrom: forwardedFrom,
                         replyRef: replyRef)
            return
        }
        markUnacked(chatId: chatId, messageId: messageId)
        Task {
            var uploaded: [Attachment] = []
            for a in attachments {
                let key = AppModel.blobKey(for: a.name)
                do {
                    _ = try await api.putBlob(projectId: "\(peopleSlot):\(chatId)", key: key, mime: a.mime, data: a.data)
                    uploaded.append(Attachment(name: a.name, mime: a.mime, blobRef: key, kind: Self.attachmentKind(mime: a.mime, name: a.name)))
                } catch {
                    setSendError(chatId, UserError.classify("upload the attachment", error))
                }
            }
            commitPeople(chatId: chatId, doc: doc, messageId: messageId, text: text, author: author,
                         mentions: mentions, reply: reply, attachments: uploaded, alreadyUnacked: true,
                         replyRef: replyRef)
        }
    }

    /// Insert + pushCommand + commit for a people chat. `alreadyUnacked` when the caller
    /// reserved the id before an attachment upload.
    private func commitPeople(chatId: String, doc: CoreChatDoc, messageId: Id, text: String,
                              author: CoreChatDoc.Author, mentions: [String], reply: String?,
                              attachments: [Attachment], alreadyUnacked: Bool = false,
                              forwardedFrom: String? = nil, replyRef: ReplyRef? = nil) {
        if !alreadyUnacked { markUnacked(chatId: chatId, messageId: messageId) }
        // Rig (#1596): `ARBOS_TRACE_SENDS=1` prints what the row carries.
        if DebugHooks.traceSends {
            print("[arbos send] text=\(text.debugDescription) mentions=\(mentions)")
        }
        do {
            try doc.insertUserMessage(id: messageId, text: text, deviceId: deviceId, attachments: attachments,
                                      author: author, mentions: mentions, replyTo: reply, forwardedFrom: forwardedFrom,
                                      replyRef: replyRef)
            try doc.pushCommand("send", payload: ["messageId": messageId], device: deviceId)
        } catch {
            clearUnacked(chatId: chatId, messageId: messageId)
            setSendError(chatId, UserError.classify("send your message", error))
            return
        }
        #if DEBUG
        SendTrace.message(messageId, chatId: chatId)
        #endif
        // Bubble at once — same as channels / optimistic new chats. onChange still
        // reconciles; live coalesce + async full re-reads must not gate the send.
        paintLocalUserMessage(chatId: chatId, messageId: messageId, text: text,
                              attachments: attachments, replyTo: reply, forwardedFrom: forwardedFrom,
                              author: author, mentions: mentions, doc: doc, replyRef: replyRef)
        doc.commit()
        #if DEBUG
        if SendTrace.traces(chatId) { SendTrace.mark("commit") }
        #endif
        announce(chatId: chatId, projectId: nil, messageId: messageId, text: text, replyRef: replyRef)
        if let i = peopleChats.firstIndex(where: { $0.id == chatId }) { peopleChats[i].lastActivityAt = nowMs() }
        peopleChats.sort { $0.lastActivityAt > $1.lastActivityAt }
    }

    /// `@slug` tokens that name a member: agents by their mention slug, people by handle.
    /// `@everyone` / `@all` and a leading "hey everyone" (etc.) address every agent in the chat.
    /// Soft-deleted agents (no nested `project`) are skipped — they cannot wake or reply.
    /// `@slug` → member. Typed resolution (#1538): when a person and an agent share a slug, the
    /// slug means the AGENT if it is the sender's own handle (nobody @-mentions themselves) and
    /// the person otherwise; the autocomplete labels both so the typist can pick.
    static func mentions(in text: String, members: [ChatMember], senderHandle: String? = nil,
                         chipTokens: [String: String] = [:]) -> [String] {
        var out: [String] = []
        let agents = members.filter { $0.isAgent && $0.project != nil }
        let mine = senderHandle?.lowercased()
        func appendAgents() {
            for a in agents where !out.contains(a.member) { out.append(a.member) }
        }
        let words = text.split(whereSeparator: { $0.isWhitespace })
        for t in words where t.hasPrefix("@") {
            let slug = t.dropFirst().trimmingCharacters(in: CharacterSet(charactersIn: ".,!?:;")).lowercased()
            if slug == "everyone" || slug == "all" || slug == "agents" {
                appendAgents()
                continue
            }
            // `@here` (Jacob, 2026-10-24): everyone in this chat — its people but me, and its
            // agents — as member ids, which is what every agent's gate reads (rule 3), so they wake.
            if slug == "here" {
                for m in members where !out.contains(m.member) {
                    if !m.isAgent, let mine, m.handle.lowercased() == mine { continue }
                    if m.isAgent, m.project == nil { continue }
                    out.append(m.member)
                }
                continue
            }
            // A committed chip says who this is (#1596) — no tie rule needed.
            if let member = chipTokens[slug] {
                if !out.contains(member) { out.append(member) }
                continue
            }
            if slug == "arbos", agents.count == 1, let a = agents.first {
                if !out.contains(a.member) { out.append(a.member) }
                continue
            }
            let hits = members.filter {
                $0.handle.lowercased() == slug
                    || ($0.isAgent && $0.project != nil && mentionSlug($0.project?.name ?? "") == slug)
            }
            let preferAgent = slug == mine
            if let m = hits.first(where: { $0.isAgent == preferAgent }) ?? hits.first {
                if !out.contains(m.member) { out.append(m.member) }
            }
        }
        // "hey agents" / @everyone always names every agent — even when a chip already put
        // one `@agent` in the draft (otherwise only that one would wake).
        if AppModel.addressesEveryone(text) {
            appendAgents()
        }
        return out
    }

    /// "hey everyone" / "hey agents" / "can the agents…" at the start (no `@` required) —
    /// addresses every agent in the chat.
    static func addressesEveryone(_ text: String) -> Bool {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        // `@here` / `@agents` chips (collective mentions, 2026-10-24) address every agent.
        if trimmed.split(whereSeparator: { $0.isWhitespace }).contains(where: {
            let t = $0.trimmingCharacters(in: CharacterSet(charactersIn: ".,!?:;"))
            return t == "@here" || t == "@agents"
        }) {
            return true
        }
        for g in [
            "hey everyone", "hi everyone", "hello everyone", "yo everyone", "ok everyone", "okay everyone",
            "hey agents", "hi agents", "hello agents", "yo agents", "ok agents", "okay agents",
            "all agents", "the agents", "can the agents", "can agents",
        ] {
            if trimmed == g { return true }
            for sep in [" ", ",", "!", "?", ":", "—", "-"] {
                if trimmed.hasPrefix(g + sep) { return true }
            }
        }
        return false
    }

    /// Human members addressed at the start of the text without `@` — "Allan", "Allan?",
    /// "Allan. we are…", "Yo Allan …". Mid-line names ("ask Allan later") do not count, so a
    /// sticky agent chip still receives "can you see @jo?".
    static func namedPeople(in text: String, members: [ChatMember]) -> [String] {
        namedPeople(in: text, members: members, allowDisplayName: true)
    }

    /// Humans addressed by `@handle` only (not display/first name). A person's display name
    /// that collides with an agent slug must not block the agent — see `namedAgents`.
    static func namedPeopleByHandle(in text: String, members: [ChatMember]) -> [String] {
        namedPeople(in: text, members: members, allowDisplayName: false)
    }

    private static func namedPeople(in text: String, members: [ChatMember], allowDisplayName: Bool) -> [String] {
        let humans = members.filter { !$0.isAgent }
        guard !humans.isEmpty else { return [] }
        var lower = text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !lower.isEmpty else { return [] }
        for g in ["yo ", "hey ", "hi ", "hello ", "ok ", "okay "] {
            if lower.hasPrefix(g) { lower = String(lower.dropFirst(g.count)); break }
        }
        let first = lower.split(whereSeparator: { $0.isWhitespace }).first.map(String.init) ?? ""
        let token = first.trimmingCharacters(in: CharacterSet(charactersIn: ".,!?:;"))
        guard token.count >= 2 else { return [] }
        var out: [String] = []
        for h in humans {
            var needles: [String] = [h.handle.lowercased()]
            if allowDisplayName, let name = h.name?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty {
                let n = name.lowercased()
                needles.append(n)
                if let firstName = n.split(whereSeparator: { $0.isWhitespace }).first, firstName.count >= 2 {
                    needles.append(String(firstName))
                }
            }
            if Set(needles).contains(token), !out.contains(h.member) {
                out.append(h.member)
            }
        }
        return out
    }

    /// Agents addressed at the start of the text without `@` — "Bob", "Bob?", "hey Bob …".
    /// A leading person `@handle` wins when the same token could match a human; a person's
    /// display name alone does not beat an agent handle/slug (so "Arbos what is your name"
    /// wakes @arbos even when a human in the chat is also named Arbos). Mid-line names
    /// ("ask Bob later") do not count. Sole-agent bare "arbos" matches like `@arbos`.
    static func namedAgents(in text: String, members: [ChatMember]) -> [String] {
        // Person @handle still wins; display-name-only collisions do not suppress the agent.
        if !namedPeopleByHandle(in: text, members: members).isEmpty { return [] }
        let agents = members.filter { $0.isAgent && $0.project != nil }
        guard !agents.isEmpty else { return [] }
        var lower = text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !lower.isEmpty else { return [] }
        for g in ["yo ", "hey ", "hi ", "hello ", "ok ", "okay "] {
            if lower.hasPrefix(g) { lower = String(lower.dropFirst(g.count)); break }
        }
        let first = lower.split(whereSeparator: { $0.isWhitespace }).first.map(String.init) ?? ""
        let token = first.trimmingCharacters(in: CharacterSet(charactersIn: ".,!?:;"))
        guard token.count >= 2 else { return [] }
        var out: [String] = []
        for a in agents {
            var needles: [String] = [a.handle.lowercased()]
            let projectName = a.project?.name.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            if !projectName.isEmpty {
                let n = projectName.lowercased()
                needles.append(n)
                needles.append(mentionSlug(projectName))
                if let firstName = n.split(whereSeparator: { $0.isWhitespace }).first, firstName.count >= 2 {
                    needles.append(String(firstName))
                }
            }
            if let name = a.name?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty {
                let n = name.lowercased()
                needles.append(n)
                if let firstName = n.split(whereSeparator: { $0.isWhitespace }).first, firstName.count >= 2 {
                    needles.append(String(firstName))
                }
            }
            if Set(needles).contains(token), !out.contains(a.member) {
                out.append(a.member)
            }
        }
        // Sole-agent bare "arbos" — same alias as `@arbos` in `mentions(in:)`.
        if out.isEmpty, token == "arbos", agents.count == 1, let a = agents.first, !out.contains(a.member) {
            out.append(a.member)
        }
        return out
    }

    /// Mark a people chat read up to the relay head (cross-launch cursor).
    /// Local unread/badge clear is immediate; the PUT is skipped when unchanged
    /// and throttled to one per chat per `readPutThrottleSeconds`.
    /// Pass `force` to re-PUT the same cursor (e.g. onAppear / foreground) so the
    /// relay clears unread events recorded after the last PUT.
    func markPeopleRead(_ chatId: String, force: Bool = false) {
        let hadRemote = (unreadCounts[chatId] ?? 0) > 0
        let hadLocalFlag = peopleUnread.contains(chatId)
        peopleUnread.remove(chatId)
        if unreadCounts[chatId] != nil { unreadCounts[chatId] = 0 }
        PushManager.shared.clearDelivered(chatId: chatId)
        defer { applyBadge() }
        guard let chat = peopleChat(chatId) else { return }
        let wasUnread = chat.headSeq > chat.readSeq || chat.unread
        if wasUnread {
            if let i = peopleChats.firstIndex(where: { $0.id == chatId }) {
                peopleChats[i].readSeq = chat.headSeq
                peopleChats[i].unread = false
            }
        }
        scheduleReadPut(
            chatId: chatId,
            value: chat.headSeq,
            force: force || hadRemote || hadLocalFlag || wasUnread
        )
    }

    /// `notifications_cleared`: I read this chat on another device. The thread's delivered
    /// notifications leave Notification Center, the chat stops being unread here (no `PUT
    /// /read` — the relay already has the cursor), and the badge drops to the relay's total
    /// (`/me/unread` re-syncs the per-chat counts on the next poll).
    func applyRemoteClear(chatId: Id, relayTotal: Int64) {
        PushManager.shared.clearDelivered(chatId: chatId)
        var remote = unreadCounts
        var flagged = peopleUnread
        Self.clearingChat(chatId, remote: &remote, flagged: &flagged)
        unreadCounts = remote
        peopleUnread = flagged
        if let i = peopleChats.firstIndex(where: { $0.id == chatId }) {
            peopleChats[i].unread = false
            peopleChats[i].readSeq = max(peopleChats[i].readSeq, peopleChats[i].headSeq)
        }
        for (pid, list) in chats {
            if let i = list.firstIndex(where: { $0.id == chatId }), list[i].unread {
                var updated = list
                updated[i].unread = false
                chats[pid] = updated
            }
        }
        PushManager.shared.setBadge(Self.badgeAfterClear(relayTotal: relayTotal, localTotal: unreadTotal))
    }

    func addAgent(chatId: String, projectId: Id) {
        showMembershipNotice(chatId: chatId, names: projects.filter { $0.id == projectId }.map(\.name),
                             subjects: [MembershipNotice.Subject(kind: "agent", id: projectId)])
        Task {
            do {
                let chat = try await api.addAgent(chatId: chatId, projectId: projectId)
                if let i = peopleChats.firstIndex(where: { $0.id == chatId }) { peopleChats[i] = chat }
            } catch {
                withdrawOptimisticNotices(chatId: chatId)
                fail((error as? RelayError)?.type == "own_project_only"
                     ? UserError.refused("add the agent", "You can only add your own agents.")
                     : UserError.classify("add the agent", error))
            }
        }
    }

    /// The adding phone's own "<me> added <names>" line, on screen the moment Add is tapped
    /// (ARB-393). The relay writes the row of record; `foldOptimisticNotices` swaps this copy
    /// out when that row lands.
    ///
    /// `subjects`: who was added, by id, when every one of them is known — then the line is
    /// worded from the live roster and matched to the relay's row by facts; nil (someone is
    /// known by handle only) keeps the words-only line.
    func showMembershipNotice(chatId: Id, names: [String], subjects: [MembershipNotice.Subject]? = nil) {
        let name = user?.name?.trimmingCharacters(in: .whitespaces) ?? ""
        let who = name.isEmpty ? (user?.login ?? "") : name
        guard !who.isEmpty, let text = OptimisticNotice.addedLine(who: who, names: names) else { return }
        var facts: MembershipNotice? = nil
        if let me = user?.id, let subjects, !subjects.isEmpty {
            facts = MembershipNotice(kind: "added", actor: me, subjects: subjects)
        }
        let n = OptimisticNotice(text: text, now: nowMs(), notice: facts)
        optimisticNotices[chatId, default: []].append(n)
        var next = messages
        next[chatId, default: []].append(n.message(chatId: chatId))
        messages = next
    }

    /// A fresh relay snapshot replaces the chat's list: keep the local notices it has not
    /// confirmed yet, drop the ones it has (or that are past their TTL).
    private func foldOptimisticNotices(chatId: Id, into list: inout [Message]) {
        guard var notices = optimisticNotices[chatId] else { return }
        OptimisticNotice.reconcile(&list, notices: &notices, chatId: chatId, now: nowMs())
        if notices.isEmpty { optimisticNotices[chatId] = nil } else { optimisticNotices[chatId] = notices }
    }

    /// The add failed: nothing was added, so no local notice may claim it was.
    private func withdrawOptimisticNotices(chatId: Id) {
        guard optimisticNotices.removeValue(forKey: chatId) != nil else { return }
        var next = messages
        next[chatId]?.removeAll { $0.id.hasPrefix(OptimisticNotice.idPrefix) }
        messages = next
    }

    /// Names the relay will use in its notice: a person by name, else @handle; an agent by its
    /// project name.
    private func membershipNames(handles: [String], agentIds: [Id]) -> [String] {
        let people: [String] = handles.map { (h: String) -> String in
            let n = personName(handle: h)?.trimmingCharacters(in: .whitespaces) ?? ""
            return n.isEmpty ? "@\(h)" : n
        }
        let agents: [String] = agentIds.compactMap { (id: Id) -> String? in projects.first { $0.id == id }?.name }
        return people + agents
    }

    /// The added members by id, or nil when a handle is not known here (then the line cannot
    /// carry facts and stays words-only).
    private func membershipSubjects(handles: [String], agentIds: [Id]) -> [MembershipNotice.Subject]? {
        var out: [MembershipNotice.Subject] = []
        for h in handles {
            guard let id = personId(handle: h) else { return nil }
            out.append(MembershipNotice.Subject(kind: "user", id: id))
        }
        out += agentIds.map { MembershipNotice.Subject(kind: "agent", id: $0) }
        return out
    }

    func addPerson(chatId: String, handle: String) {
        Task {
            do {
                let chat = try await api.addMember(chatId: chatId, handle: handle)
                if let i = peopleChats.firstIndex(where: { $0.id == chatId }) { peopleChats[i] = chat }
            } catch {
                let e = error as? RelayError
                let said: String? = e?.type == "already_member" ? "@\(handle) is already in this chat."
                    : e?.type == "not_available" ? "You can't add this person."
                    : (e?.status == 404 || e?.status == 405 || e?.status == 501) ? "Adding people is not on this relay yet."
                    : nil
                fail(said.map { UserError.refused("add @\(handle)", $0) } ?? UserError.classify("add @\(handle)", error))
            }
        }
    }

    /// Group profile › Invite links › New link (any member).
    func createGroupInvite(_ chatId: Id, expires: Bool, singleUse: Bool) async -> ServerInviteLink? {
        do {
            return try await api.createGroupInvite(chatId, expires: expires, maxUses: singleUse ? 1 : nil)
        } catch {
            fail("make an invite link", error)
            return nil
        }
    }

    /// Group profile › Invite links › list.
    func groupInvites(_ chatId: Id) async -> [ServerInviteLink] {
        (try? await api.groupInvites(chatId)) ?? []
    }

    /// Group profile › Invite links › revoke.
    func revokeGroupInvite(_ chatId: Id, inviteId: String) async -> Bool {
        do {
            try await api.revokeGroupInvite(chatId, inviteId: inviteId)
            return true
        } catch {
            fail("revoke the link", error)
            return false
        }
    }

    /// `arbos://join-group/<code>` (the site's group invite page): join and open the chat.
    func joinGroupFromLink(code: String) async {
        let code = code.trimmingCharacters(in: CharacterSet(charactersIn: "/ "))
        guard !code.isEmpty else { return }
        do {
            let chat = try await api.joinGroup(code: code)
            if let i = peopleChats.firstIndex(where: { $0.id == chat.id }) {
                peopleChats[i] = chat
            } else {
                peopleChats.insert(chat, at: 0)
            }
            ensureRoom(projectId: peopleSlot, key: chat.id, roomChat: chat.id)
            open(peopleChat: chat)
        } catch {
            let type = (error as? RelayError)?.type
            let said: String? = type == "not_available" ? "You can't join this group."
                : type == "not_found" || (error as? RelayError)?.status == 404 ? "That invite link is not valid."
                : nil
            fail(said.map { UserError.refused("join the group", $0) } ?? UserError.classify("join the group", error))
        }
    }

    /// The group pane's Add members: every pick goes to THIS chat — people by
    /// `POST /chats/{id}/members`, agents by `POST /chats/{id}/agents`. All are
    /// tried; the first failure is reported once and the call returns false.
    func addMembers(chatId: String, handles: [String], agentIds: [Id]) async -> Bool {
        showMembershipNotice(chatId: chatId, names: membershipNames(handles: handles, agentIds: agentIds),
                             subjects: membershipSubjects(handles: handles, agentIds: agentIds))
        // One call for the batch (#1349): the relay adds everyone and writes one
        // "<me> added A, B and C" line. A relay that only knows `{handle}` answers 4xx;
        // then every add is tried on its own as before.
        do {
            let chat = try await api.addMembers(chatId: chatId, handles: handles, agentIds: agentIds)
            if let i = peopleChats.firstIndex(where: { $0.id == chatId }) { peopleChats[i] = chat }
            return true
        } catch let e as RelayError where [400, 404, 415, 422].contains(e.status) {
            // fall through to the per-item adds
        } catch {
            withdrawOptimisticNotices(chatId: chatId)
            fail("add members", error)
            return false
        }
        var failure: UserError?
        for handle in handles {
            do {
                let chat = try await api.addMember(chatId: chatId, handle: handle)
                if let i = peopleChats.firstIndex(where: { $0.id == chatId }) { peopleChats[i] = chat }
            } catch {
                let e = error as? RelayError
                failure = failure ?? (e?.type == "already_member" ? UserError.refused("add @\(handle)", "@\(handle) is already in this chat.")
                                      : e?.type == "not_available" ? UserError.refused("add @\(handle)", "You can't add @\(handle).")
                                      : UserError.classify("add @\(handle)", error))
            }
        }
        for projectId in agentIds {
            do {
                let chat = try await api.addAgent(chatId: chatId, projectId: projectId)
                if let i = peopleChats.firstIndex(where: { $0.id == chatId }) { peopleChats[i] = chat }
            } catch {
                failure = failure ?? ((error as? RelayError)?.type == "own_project_only"
                                      ? UserError.refused("add the agent", "You can only add your own agents.")
                                      : UserError.classify("add the agent", error))
            }
        }
        if let failure {
            withdrawOptimisticNotices(chatId: chatId)
            fail(failure)
            return false
        }
        return true
    }

    /// Owner/admin removes (or bans) a person from a people chat
    /// (`DELETE /chats/{id}/members/{userId}[?ban=1]`); the relay writes the
    /// "<me> removed @handle" line (#1349). Ban keeps them from rejoining.
    func removeMember(chatId: String, userId: String, ban: Bool = false) {
        Task {
            do {
                try await api.removeMember(chatId: chatId, userId: userId, ban: ban)
                if let i = peopleChats.firstIndex(where: { $0.id == chatId }) {
                    if ban, let chat = try? await api.peopleChat(chatId) {
                        peopleChats[i] = chat
                    } else {
                        peopleChats[i].members.removeAll { !$0.isAgent && $0.id == userId }
                    }
                }
                // The relay writes "<me> removed @handle" into the chat (#1349).
            } catch {
                let e = error as? RelayError
                let verb = ban ? "ban them" : "remove them"
                fail(e?.type == "owner_only"
                     ? UserError.refused(verb, "Only an owner or admin can manage members.")
                     : UserError.classify(verb, error))
            }
        }
    }

    /// Owner/admin clears a ban (`POST …/members/{userId}/unban`) so the person can be added again.
    func unbanMember(chatId: String, userId: String) {
        Task {
            do {
                let chat = try await api.unbanMember(chatId: chatId, userId: userId)
                if let i = peopleChats.firstIndex(where: { $0.id == chatId }) {
                    peopleChats[i] = chat
                }
            } catch {
                let e = error as? RelayError
                fail(e?.type == "owner_only"
                     ? UserError.refused("unban them", "Only an owner or an admin with mute and ban rights can unban.")
                     : UserError.classify("unban them", error))
            }
        }
    }

    /// Owner/admin mutes (or unmutes) a member from posting in a people group.
    /// Pass `duration` when muting to choose how long the mute lasts.
    func setMemberSilenced(
        chatId: String,
        userId: String,
        silenced: Bool,
        duration: MemberMuteDuration? = nil
    ) {
        Task {
            do {
                let until = silenced ? duration?.silencedUntilMs : nil
                let chat = try await api.setMemberSilenced(
                    chatId: chatId, userId: userId, silenced: silenced, silencedUntilMs: until
                )
                if let i = peopleChats.firstIndex(where: { $0.id == chatId }) {
                    peopleChats[i] = chat
                }
            } catch {
                let e = error as? RelayError
                fail(e?.type == "owner_only"
                     ? UserError.refused("mute them", "Only an owner or an admin with mute rights can mute members.")
                     : UserError.classify("mute them", error))
            }
        }
    }

    /// Owner promotes or demotes a group member (`admin` / `member`).
    func setMemberRole(chatId: String, userId: String, role: String) {
        Task {
            do {
                let chat = try await api.setMemberRole(chatId: chatId, userId: userId, role: role)
                if let i = peopleChats.firstIndex(where: { $0.id == chatId }) {
                    peopleChats[i] = chat
                }
            } catch {
                let e = error as? RelayError
                fail(e?.type == "owner_only"
                     ? UserError.refused("change the role", "Only the owner can change admin roles.")
                     : UserError.classify("change the role", error))
            }
        }
    }

    /// Owner sets whether an admin may mute and ban members.
    func setMemberCanMuteBan(chatId: String, userId: String, canMuteBan: Bool) {
        Task {
            do {
                let chat = try await api.setMemberCanMuteBan(
                    chatId: chatId, userId: userId, canMuteBan: canMuteBan
                )
                if let i = peopleChats.firstIndex(where: { $0.id == chatId }) {
                    peopleChats[i] = chat
                }
            } catch {
                let e = error as? RelayError
                fail(e?.type == "owner_only"
                     ? UserError.refused("change the mute and ban rights", "Only the owner can set mute and ban rights.")
                     : UserError.classify("change the mute and ban rights", error))
            }
        }
    }

    func removeAgent(chatId: String, projectId: Id) {
        Task {
            do {
                try await api.removeAgent(chatId: chatId, projectId: projectId)
                if let i = peopleChats.firstIndex(where: { $0.id == chatId }) {
                    peopleChats[i].members.removeAll { $0.isAgent && $0.id == projectId }
                }
            } catch { fail("remove the agent", error) }
        }
    }

    func setReplyPolicy(chatId: String, projectId: Id, policy: String) {
        if let i = peopleChats.firstIndex(where: { $0.id == chatId }),
           let j = peopleChats[i].members.firstIndex(where: { $0.isAgent && $0.id == projectId }) {
            peopleChats[i].members[j].replyPolicy = policy
        }
        Task { try? await api.setReplyPolicy(chatId: chatId, projectId: projectId, policy: policy) }
    }

    func leavePeopleChat(_ chatId: String) {
        peopleChats.removeAll { $0.id == chatId }
        rooms[chatId]?.stop(); rooms[chatId] = nil; docs[chatId] = nil; messages[chatId] = nil
        roomLinks[chatId] = nil
        unacked[chatId] = nil
        forgetDoc(chatId)
        path = []
        Task { try? await api.leaveChat(chatId) }
    }

    /// Rename a people group and/or set its picture blob key or description (`PATCH /chats/{id}`).
    func patchPeopleChat(_ chatId: String, title: String? = nil, imageKey: String? = nil, description: String? = nil) async -> Bool {
        do {
            let chat = try await api.patchPeopleChat(chatId, title: title, imageKey: imageKey, description: description)
            if let i = peopleChats.firstIndex(where: { $0.id == chatId }) {
                peopleChats[i] = chat
            } else {
                peopleChats.insert(chat, at: 0)
            }
            return true
        } catch {
            fail("update the group", error)
            return false
        }
    }

    /// D92 amendment: the agent's picture as the avatar loader fetches it (the newest copy's).
    func projectImageURL(_ project: Project) -> URL? {
        let facts = newestAgentAvatar(project.id) ?? AgentAvatarFacts(project)
        return agentPictureURL(id: project.id, imageKey: facts.imageKey, version: facts.version).flatMap(URL.init(string:))
    }

    /// D92 amendment: upload an agent picture into the project's own blob slot and set it.
    func setProjectImage(projectId: Id, jpeg: Data) async -> Bool {
        let key = "avatar-\(Int(Date().timeIntervalSince1970 * 1000)).\(PictureMaster.ext(jpeg))"
        if isFixture {
            return await setProjectImageKey(projectId: projectId, key: key)
        }
        do {
            _ = try await api.putBlob(projectId: projectId, key: key, mime: PictureMaster.mime(jpeg), data: jpeg)
            return await setProjectImageKey(projectId: projectId, key: key)
        } catch {
            fail("upload the picture", error)
            return false
        }
    }

    /// D92 amendment: a blob the relay already holds (the Generate tab's answer) becomes the picture.
    func setProjectImageKey(projectId: Id, key: String) async -> Bool {
        // Optimistic, into every copy at once (the header used to keep its old URL while the
        // working row already drew the new picture — Jacob, 2026-10-25); the relay's stamp
        // replaces the local one when it answers.
        let base = newestAgentAvatar(projectId)
        let local = (base?.version ?? 0) + 1
        applyAgentAvatar(projectId, AgentAvatarFacts(imageKey: key, avatarLeaf: base?.avatarLeaf, avatarLeafColor: base?.avatarLeafColor, version: local), force: true)
        guard !isFixture else { return true }
        do {
            let stamped = try await api.setProjectImageKey(projectId, key: key)
            if stamped > 0 {
                applyAgentAvatar(projectId, AgentAvatarFacts(imageKey: key, avatarLeaf: base?.avatarLeaf, avatarLeafColor: base?.avatarLeafColor, version: stamped), force: true)
            }
            return true
        } catch {
            fail("save the picture", error)
            await refreshRegistry()
            return false
        }
    }

    /// D92 amendment: the Generate tab — the relay paints one picture and answers with its key.
    func generateProjectAvatar(projectId: Id, prompt: String) async throws -> String {
        if isFixture { return try await Self.fixtureGenerate(prompt) }
        return try await api.generateProjectAvatar(projectId, prompt: prompt)
    }

    /// The same for a people group's picture (any member) — the key lands in `people:{chat}`.
    func generateGroupAvatar(chatId: String, prompt: String) async throws -> String {
        if isFixture { return try await Self.fixtureGenerate(prompt) }
        return try await api.generateGroupAvatar(chatId, prompt: prompt)
    }

    /// Set a group picture from a key the relay already holds (the Generate tab's answer).
    func setGroupImageKey(chatId: String, key: String) async -> Bool {
        if isFixture {
            if let i = peopleChats.firstIndex(where: { $0.id == chatId }) { peopleChats[i].imageKey = key }
            return true
        }
        return await patchPeopleChat(chatId, imageKey: key)
    }

    /// D92 amendment: a group's leaf picture (nil = initials); drops the uploaded picture.
    func setGroupLeaf(chatId: String, pick: LeafPick?) async -> Bool {
        if let i = peopleChats.firstIndex(where: { $0.id == chatId }) {
            peopleChats[i].avatarLeaf = pick.map { Int($0.leaf) }
            peopleChats[i].avatarLeafColor = pick.map { Int($0.color) }
            peopleChats[i].imageKey = nil
        }
        guard !isFixture else { return true }
        do {
            let chat = try await api.setPeopleChatLeaf(chatId, pick: pick)
            if let i = peopleChats.firstIndex(where: { $0.id == chatId }) { peopleChats[i] = chat }
            return true
        } catch {
            fail("save the group picture", error)
            return false
        }
    }

    /// Upload a group picture to `people:{chatId}` and patch `image_key`.
    func setGroupImage(chatId: String, jpeg: Data) async -> Bool {
        let key = "group-avatar-\(Int(Date().timeIntervalSince1970 * 1000)).\(PictureMaster.ext(jpeg))"
        do {
            _ = try await api.putBlob(projectId: "\(peopleSlot):\(chatId)", key: key, mime: PictureMaster.mime(jpeg), data: jpeg)
            return await patchPeopleChat(chatId, imageKey: key)
        } catch {
            fail("upload the group picture", error)
            return false
        }
    }

    /// Settings profile picture: `PUT /me/avatar` with the master (PNG or JPEG), then update the local user.
    func setUserAvatar(jpeg: Data) async -> Bool {
        do {
            let url = try await api.putMeAvatar(jpeg: jpeg)
            if var who = user {
                who.avatarUrl = url
                user = who
                saveAccountCache()
            }
            return true
        } catch {
            fail("save your picture", error)
            return false
        }
    }

    /// The fixture's "generation": a key named after the prompt after 1.2 s. Rig (error-state
    /// shots): a prompt with "timeout" fails with the relay's 504 sentence after 2 s, one with
    /// "slow" paints 25 s first (the progress state).
    static func fixtureGenerate(_ prompt: String) async throws -> String {
        let lower = prompt.lowercased()
        if lower.contains("slow") { try? await Task.sleep(nanoseconds: 25_000_000_000) }
        if lower.contains("timeout") {
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            throw RelayError(status: 504, message: "Couldn't generate the image. The server took too long.", type: "avatar_timeout",
                             detail: "gpt-image-1-mini: operation timed out after 40 s (attempt 2 of 2); relay budget 75 s")
        }
        try? await Task.sleep(nanoseconds: 1_200_000_000)
        return "fixture-gen-\(lower.replacingOccurrences(of: " ", with: "-"))"
    }

    /// The avatar picker's Generate tab for my own picture: the relay paints one picture into
    /// my `users:{id}` slot and answers with its key (not yet set).
    func generateMyAvatar(prompt: String) async throws -> String {
        if isFixture { return try await Self.fixtureGenerate(prompt) }
        return try await api.generateMyAvatar(prompt: prompt)
    }

    /// The generated preview becomes my picture (`PUT /me/avatar {image_key}`), then the local
    /// user carries the new URL like after an upload.
    func setUserAvatarKey(_ key: String) async -> Bool {
        if isFixture {
            if var who = user {
                who.avatarUrl = "fixture://avatar/me-\(key)"
                user = who
            }
            return true
        }
        do {
            let url = try await api.putMeAvatarKey(key)
            if var who = user {
                who.avatarUrl = url
                user = who
                saveAccountCache()
            }
            return true
        } catch {
            fail("save your picture", error)
            return false
        }
    }

    /// A picture uploaded with `setUserAvatar` (the relay serves it under
    /// `/avatars/`); GitHub pictures come from GitHub's host. Same rule as the Mac.
    var hasCustomAvatar: Bool {
        user?.avatarUrl?.contains("/avatars/") == true
    }

    /// Settings profile picture: `DELETE /me/avatar` — back to the GitHub picture.
    func removeUserAvatar() async -> Bool {
        do {
            user = try await api.deleteMeAvatar()
            saveAccountCache()
            return true
        } catch {
            fail("remove your picture", error)
            return false
        }
    }

    /// The chat list's Delete on a people group, the Mac's rule (2026-10-20):
    /// the owner deletes it for everyone; a member leaves and the group stays.
    func ownsGroup(_ c: PeopleChat) -> Bool {
        guard c.isGroup else { return true }
        if c.myRole == "owner" || c.myRole == "admin" { return true }
        if let me = user?.id, c.createdBy == me { return true }
        return c.humans.first { $0.id == user?.id }?.role == "owner"
    }

    func deleteOrLeavePeopleChat(_ c: PeopleChat) {
        if ownsGroup(c) { deletePeopleChat(c.id) } else { leavePeopleChat(c.id) }
    }

    func deletePeopleChat(_ chatId: String) {
        peopleChats.removeAll { $0.id == chatId }
        rooms[chatId]?.stop(); rooms[chatId] = nil; docs[chatId] = nil; messages[chatId] = nil
        roomLinks[chatId] = nil
        unacked[chatId] = nil
        forgetDoc(chatId)
        if archivedPeopleChatIds.contains(chatId) {
            archivedPeopleChatIds.remove(chatId)
            UserDefaults.standard.set(Array(archivedPeopleChatIds), forKey: "archivedPeopleChats")
        }
        unpinHome(peopleChatId: chatId)
        path = []
        Task {
            do { try await api.deleteChat(chatId) } catch { fail("delete it", error) }
        }
    }

    func block(_ member: ChatMember) { block(handle: member.handle) }

    func block(handle: String) {
        Task {
            do { try await api.block(handle: handle); blocked = (try? await api.blocks()) ?? blocked }
            catch { fail("block them", error) }
        }
    }

    // MARK: Profile pages

    /// The profile page the root view presents (person, group or agent).
    @Published var profile: ProfileTarget?
    /// People pushed over `profile` (a member tapped on a group's or server's
    /// page): the sheet is a NavigationStack, the back chevron pops them.
    @Published var profilePath: [ProfileTarget] = []
    /// A chat the next ChatView should open with its search bar up (profile "search").
    @Published var pendingSearchChat: String?
    /// A message the next ChatView should scroll to and tint (a search result opened
    /// from the bottom bar, [feedback iphone-bottom-search-bar-2026-10-25]).
    @Published var pendingReveal: MessageReveal?
    /// The Settings page the next Settings sheet lands on (a Settings row opened
    /// from search, [feedback search-settings-ios-2026-10-26]): Home presents the
    /// sheet when this is set and clears it when the sheet closes.
    @Published var pendingSettingsLanding: String?
    /// The agent pane's settings row to drop open once the pane shows (a search
    /// result for "voice" / "model" / "reasoning").
    @Published var pendingAgentSetting: GlobalSearch.AgentSetting?
    /// App-wide search: message text, attachments and links per loaded chat, kept
    /// current lazily against `messagesVersion` (`GlobalSearch`).
    var searchIndex = SearchIndex()
    /// Unsent composer text kept per chat while the ChatView is off-screen (Mac `Composer.drafts`).
    private var composerDrafts: [Id: String] = [:]
    /// Last presence time per user id, from room presence frames this session.
    @Published private(set) var lastSeen: [String: Int64] = [:]
    /// Who is typing per chat (handle → until ms), from room presence frames
    /// with `typing`; the presence row over the composer reads it
    /// ([feedback typing-indicator-left-2026-10-23], Mac `AppState.typing`).
    @Published var typing: [Id: [String: Int64]] = [:]
    /// Last time we told the room we are typing, per chat (Mac `typing_sent_at`).
    private var typingSentAt: [Id: Int64] = [:]

    /// Draft text for a chat, or `""` when none.
    func composerDraft(for chatId: Id) -> String {
        composerDrafts[chatId] ?? ""
    }

    /// Remember (or clear) the unsent composer text for a chat.
    func setComposerDraft(_ text: String, for chatId: Id) {
        if text.isEmpty {
            composerDrafts.removeValue(forKey: chatId)
        } else {
            composerDrafts[chatId] = text
        }
    }

    func showProfile(_ target: ProfileTarget) {
        if let root = profile, case .channel = root, case .server = target {
            // The channel page's server row pushes the server's page (back returns).
            if profilePath.last != target { profilePath.append(target) }
            return
        }
        if let root = profile, root.pushesPeople, case .user = target {
            // A person opened from a page that is already up is pushed, so
            // back returns to that page with its tab and scroll untouched.
            let top = profilePath.last ?? root
            if top != target { profilePath.append(target) }
            return
        }
        profilePath = []
        profile = target
    }

    /// A sender avatar was tapped: resolve the handle through the chat's members.
    /// Stable user id for a handle seen in a chat (members first, then the directory); nil when unknown.
    func personId(handle: String) -> String? {
        if let me = user, me.login.caseInsensitiveCompare(handle) == .orderedSame { return me.id }
        for c in peopleChats {
            if let m = c.members.first(where: { !$0.isAgent && $0.handle.caseInsensitiveCompare(handle) == .orderedSame }) { return m.id }
        }
        return directory.first { $0.handle.caseInsensitiveCompare(handle) == .orderedSame }?.id
    }

    func personName(handle: String) -> String? {
        if let me = user, me.login.caseInsensitiveCompare(handle) == .orderedSame { return me.name }
        for c in peopleChats {
            if let m = c.members.first(where: { !$0.isAgent && $0.handle.caseInsensitiveCompare(handle) == .orderedSame }) { return m.name }
        }
        return directory.first { $0.handle.caseInsensitiveCompare(handle) == .orderedSame }?.name
    }

    /// The senders whose rows `chat` hides for me ([feedback blocked-state-ios-2026-10-26],
    /// Jacob 2026-10-26: a group block hides the blocked person's messages from the blocker
    /// alone): the relay's `blockedSenders` hint plus every member on my block list, as
    /// `user:<id>`. Empty for a DM (its composer bar says Blocked instead).
    func hiddenSenders(in chat: PeopleChat) -> Set<String> {
        guard chat.isGroup else { return [] }
        var out = Set(chat.blockedSenders)
        for m in chat.humans where BlockedState.isBlocked(id: m.id, handle: m.handle, in: blocked) {
            out.insert(m.id.hasPrefix("user:") ? m.id : "user:\(m.id)")
        }
        return out
    }

    /// Tests: the relay's block list as `GET /blocks` would hand it over.
    func setBlockedForTests(_ list: [Person]) { blocked = list }

    /// Is this message hidden for me — from someone I blocked, in a people group?
    func hidesMessage(_ m: Message, chatId: Id) -> Bool {
        guard m.role == .user, let chat = peopleChat(chatId) else { return false }
        return hiddenSenders(in: chat).contains(m.sender)
    }

    /// The one action behind every profile target in a chat — a sender name, the
    /// avatar beside a bubble, an @mention chip, the header pill, a status-row
    /// picture ([feedback profile-pane-toggle-ios-2026-10-26], `ProfileToggle`):
    /// open the sheet on `target`; dismiss it when that same person or agent is
    /// already up, whichever target opened it; swap it in place for someone else.
    func toggleProfile(_ target: ProfileTarget) {
        guard let next = ProfileToggle.next(showing: profilePath.last ?? profile, tapped: target) else {
            profilePath = []
            profile = nil
            return
        }
        profilePath = []
        profile = next
    }

    /// True while the sheet's top page is `target`'s subject (the label then
    /// says "Close profile").
    func profileShows(_ target: ProfileTarget) -> Bool {
        (profilePath.last ?? profile)?.sameSubject(as: target) ?? false
    }

    /// The page a sender's handle opens: the chat's member, else the directory
    /// entry, else a bare person from the row's own fields.
    func personTarget(handle: String, avatarUrl: String?, chatId: String) -> ProfileTarget {
        if let m = peopleChat(chatId)?.members.first(where: { $0.handle == handle && !$0.isAgent }) {
            return .user(ProfilePerson(m))
        } else if let p = directory.first(where: { $0.handle == handle }) {
            return .user(ProfilePerson(p))
        } else {
            return .user(ProfilePerson(id: handle, handle: handle, name: nil, avatarUrl: avatarUrl))
        }
    }

    func openPersonProfile(handle: String, avatarUrl: String?, chatId: String) {
        toggleProfile(personTarget(handle: handle, avatarUrl: avatarUrl, chatId: chatId))
    }

    /// An agent that is not in my project list: the project carried by a
    /// people-chat member row (someone else's agent in a DM).
    func foreignAgent(_ projectId: Id) -> Project? {
        for c in peopleChats {
            if let a = c.agents.first(where: { $0.id == projectId }) {
                // Through the shared identity (#1592): the same name, leaf and picture every
                // other surface draws for this agent.
                return agent(member: a, chatId: c.id)
            }
        }
        return nil
    }

    func notePresence(_ header: [String: Any]) {
        guard let user = header["user"] as? String else { return }
        let at = (header["at"] as? NSNumber)?.int64Value ?? nowMs()
        if (lastSeen[user] ?? 0) < at { lastSeen[user] = at }
    }

    /// A presence frame's `typing` flag for `chatId`: on → the handle types for
    /// 6 s (the sender refreshes every 3 s), off → gone at once.
    func noteTyping(_ header: [String: Any], chatId: Id) {
        guard let handle = header["handle"] as? String, !handle.isEmpty,
              header["typing"] != nil else { return }
        // `DocJSON.bool` accepts Bool and NSNumber; a bare `as? Bool` missed live frames.
        let active = DocJSON.bool(header["typing"])
        if handle == user?.login { return }
        var chat = typing[chatId] ?? [:]
        if active {
            chat[handle] = nowMs() + 6_000
        } else {
            chat[handle] = nil
        }
        typing[chatId] = chat.isEmpty ? nil : chat
    }

    /// Composer keystroke in a people chat (DM or group): at most one presence
    /// typing ping per 3 s so peers see who is typing ([feedback
    /// app-01M5N7WSA1VCYHE9BE3CX4V0R6], Mac `AppState::typed_in`).
    func typedIn(chatId: Id) {
        guard peopleChat(chatId) != nil else { return }
        let now = nowMs()
        let last = typingSentAt[chatId] ?? 0
        guard now - last > 3_000 else { return }
        typingSentAt[chatId] = now
        rooms[chatId]?.setTyping(true)
    }

    /// Handles typing in `chatId` right now, the most recently seen first.
    func typingHandles(in chatId: Id) -> [String] {
        let now = nowMs()
        return (typing[chatId] ?? [:])
            .filter { $0.value > now }
            .sorted { $0.value > $1.value }
            .map(\.key)
    }

    /// `PUT /chats/{id}/settings {muted}` per member; local flag updated first.
    func setMuted(chatId: String, muted: Bool) {
        if let i = peopleChats.firstIndex(where: { $0.id == chatId }) { peopleChats[i].muted = muted }
        guard !isFixture else { return }
        Task { try? await api.chatSettings(chatId: chatId, muted: muted) }
    }

    private static let peopleMutedUntilKey = "peopleMutedUntil"

    private static func loadPeopleMutedUntil() -> [String: Date] {
        let raw = UserDefaults.standard.dictionary(forKey: peopleMutedUntilKey) as? [String: Double] ?? [:]
        return Dictionary(uniqueKeysWithValues: raw.map { ($0.key, Date(timeIntervalSince1970: $0.value)) })
    }

    private static func savePeopleMutedUntil(_ map: [String: Date]) {
        let raw = Dictionary(uniqueKeysWithValues: map.map { ($0.key, $0.value.timeIntervalSince1970) })
        UserDefaults.standard.set(raw, forKey: peopleMutedUntilKey)
    }

    /// True while this people chat should not show banners (timed local mute or relay `me.muted`).
    func isPeopleChatMuted(_ chatId: String) -> Bool {
        if let until = peopleMutedUntil[chatId] {
            if until > Date() { return true }
            // Timed mute ended: clear local expiry and the relay flag.
            peopleMutedUntil.removeValue(forKey: chatId)
            if peopleChat(chatId)?.muted == true { setMuted(chatId: chatId, muted: false) }
            return false
        }
        return peopleChat(chatId)?.muted == true
    }

    /// Mute a people group for a chosen length of time (1 hour, 8 hours, or until unmuted).
    func mutePeopleChat(_ chatId: String, for duration: ChatMuteDuration) {
        peopleMutedUntil[chatId] = duration.endDate
        setMuted(chatId: chatId, muted: true)
    }

    /// Clear a people-chat mute (local expiry and relay flag).
    func unmutePeopleChat(_ chatId: String) {
        peopleMutedUntil.removeValue(forKey: chatId)
        setMuted(chatId: chatId, muted: false)
    }

    /// Subtitle under Mute / Unmute on the group info page.
    func peopleMuteSubtitle(_ chatId: String) -> String {
        guard isPeopleChatMuted(chatId) else {
            return "No banners for a while, or until you turn them back on"
        }
        if let until = peopleMutedUntil[chatId], until.timeIntervalSinceNow < 60 * 60 * 24 * 365 {
            let f = DateFormatter()
            f.doesRelativeDateFormatting = true
            f.dateStyle = .short
            f.timeStyle = .short
            return "Muted until \(f.string(from: until))"
        }
        return "Muted until you turn it back on"
    }

    /// `GET /users/{id}` for a fresh presence line on a profile page.
    func fetchUser(_ id: String) async -> Person? {
        guard credentials != nil, !isFixture else { return nil }
        return (try? await api.user(id)) ?? nil
    }

    /// Projects of mine (or shared with me) that this person is a member of.
    func sharedProjects(with person: ProfilePerson) async -> [Project] {
        guard !isFixture else { return projects.filter { $0.shared } }
        let id = personId(handle: person.handle) ?? person.id
        let handle = person.handle
        var out: [Project] = []
        for p in projects {
            if let oh = p.ownerHandle, oh.caseInsensitiveCompare(handle) == .orderedSame {
                out.append(p); continue
            }
            if let oid = p.ownerId, oid == id || oid == person.id {
                out.append(p); continue
            }
            let members = await projectMembers(p.id)
            if members.contains(where: {
                $0.id == id || $0.id == person.id
                    || $0.handle.caseInsensitiveCompare(handle) == .orderedSame
            }) { out.append(p) }
        }
        return out
    }

    func unblock(_ person: Person) {
        Task {
            do { try await api.unblock(userId: person.id) }
            catch let e as RelayError where e.status == 404 {
                // Already not blocked on the relay; the list below says so.
            }
            catch { fail("unblock them", error) }
            await refreshBlocks()
        }
    }

    func refreshBlocks() async {
        if let list = try? await api.blocks() { blocked = list }
    }

    // Projects together (step 3)
    func projectMembers(_ projectId: Id) async -> [ProjectMember] {
        (try? await api.projectMembers(projectId)) ?? []
    }

    func inviteToProject(_ projectId: Id, handle: String) async -> [ProjectMember]? {
        do { return try await api.addProjectMember(projectId, handle: handle) }
        catch {
            fail((error as? RelayError)?.type == "already_member"
                 ? UserError.refused("invite @\(handle)", "@\(handle) is already a member.")
                 : UserError.classify("invite @\(handle)", error))
            return nil
        }
    }

    func removeProjectMember(_ projectId: Id, userId: String) async {
        do { try await api.removeProjectMember(projectId, userId: userId) }
        catch { fail("remove them", error) }
    }

    /// `/me/pushes`: unread flags, chat list refreshes and local notifications.
    func drainPushes() async {
        guard credentials != nil, !isFixture, let items = try? await api.pushes(), !items.isEmpty else { return }
        var needChats = false
        var needServers = false
        for p in items where !notifiedPushIds.contains(p.id) {
            notifiedPushIds.insert(p.id)
            pushLog.notice("push id=\(p.id, privacy: .public) event=\(String(describing: p.event), privacy: .public)")
            switch p.event {
            case .peopleMessage(let payload):
                let cid = payload.chat_id
                // Since 2026-10-26 the relay queues this for an agent's reply too (people chats
                // and, with `project_id`, the agent's own project chats); before, only a
                // person's row did, so an @agent answer never showed a banner here.
                let projectId = payload.project_id ?? peopleSlot
                let inPeople = payload.project_id == nil
                // Agent (or person) reached out in a people chat we have not listed yet —
                // refresh so open-from-notification can dial the room.
                if inPeople, peopleChat(cid) == nil { needChats = true }
                if !isWatching(cid) {
                    if inPeople {
                        peopleUnread.insert(cid)
                        unarchiveOnActivity(chatId: cid)
                    }
                    // Muted group: keep the unread flag, skip the local banner.
                    if !inPeople || !isPeopleChatMuted(cid) {
                        let sender = payload.sender
                        let from = sender.handle.isEmpty ? (payload.from_handle.isEmpty ? "someone" : payload.from_handle) : sender.handle
                        // §7c sender{} first; the chat's member list fills any gap.
                        let member = peopleChat(cid)?.members.first { $0.handle == from }
                        let agent = member?.isAgent == true ? member?.project : nil
                        let kind = sender.kind.isEmpty ? (agent != nil ? "agent" : "user") : sender.kind
                        let body = (payload.text ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
                        let info = MessagePush.userInfo(
                            chatId: cid, projectId: projectId, title: sender.name.isEmpty ? (member?.displayName ?? "@\(from)") : sender.name,
                            body: body.isEmpty ? "New message" : body,
                            senderKind: kind, senderId: sender.id.isEmpty ? (member?.id ?? from) : sender.id,
                            senderName: sender.name.isEmpty ? (member?.displayName ?? from) : sender.name, senderHandle: from,
                            avatar: sender.avatar,
                            avatarColor: sender.avatar_color,
                            avatarUrl: sender.avatar_url ?? member?.avatarUrl)
                        Task { await PushManager.shared.postMessage(id: p.id, info: info) }
                    }
                    applyBadge()
                }
            case .peopleChatCreated(let payload):
                needChats = true
                if !payload.by_handle.isEmpty {
                    PushManager.post(id: p.id, title: "@\(payload.by_handle)", body: "Started a chat with you", chatId: payload.chat_id)
                }
            case .peopleChatUpdated, .peopleAgentAdded, .peopleAgentRemoved, .peopleMemberAdded, .peopleMemberRemoved:
                needChats = true
            case .callStarted(let started):
                let c = Call(dto: started.call)
                if c.chatId != CallManager.shared.call?.chatId {
                    CallManager.shared.ring(c, from: CallPerson(dto: started.sender), viaVoip: false)
                }
            case .callMissed(let missed):
                CallManager.shared.missedCall(
                    callId: missed.call_id,
                    chatId: missed.chat_id,
                    projectId: missed.project_id.isEmpty ? peopleSlot : missed.project_id,
                    from: CallPerson(dto: missed.from)
                )
            case .callEnded(let ended):
                CallManager.shared.remoteEnded(
                    callId: ended.call_id,
                    chatId: ended.chat_id,
                    durationS: Int(ended.duration_s),
                    writerDevice: ended.writer_device
                )
            case .peopleProjectShared(let payload):
                if !payload.by_handle.isEmpty {
                    PushManager.post(id: p.id, title: "@\(payload.by_handle)", body: "Shared a project with you", chatId: "")
                }
                await refreshRegistry()
            case .projectMemberRemoved:
                await refreshRegistry()
            case .secretRequest(let payload):
                // D83: an agent wants a secret; the card is in its chat, Settings badges too.
                let agent = payload.project_name.isEmpty ? "An agent" : payload.project_name
                let host = payload.host.flatMap { $0.isEmpty ? nil : " at \($0)" } ?? ""
                PushManager.post(id: p.id, title: agent, body: "\(agent) wants to use \(payload.name)\(host)", chatId: "")
                await refreshSecrets()
            case .secretsPaused:
                await refreshSecrets()
            case .messageRedacted(let payload):
                // Secret hand-off: the row itself arrives through room sync (and the store's
                // next snapshot is shallow); here the delivered notifications that may still
                // show the pasted key go, and the search index follows the message list.
                PushManager.shared.clearDelivered(chatId: payload.chat_id)
                syncSearchIndex()
            case .secretExpiring(let payload):
                PushManager.post(
                    id: p.id, title: "Secrets",
                    body: payload.expired ? "\(payload.name) expired. Agents lost it." : "\(payload.name) expires in 3 days.",
                    chatId: ""
                )
                await refreshSecrets()
            case .feedbackCapture:
                await answerFeedbackCapture(p)
            case .releasePublished(let payload):
                // A build is installable (TestFlight processed it): refresh the Update card now.
                if payload.platform == "ios" { await checkForUpdate() }
            case .serverUpdated, .serverMemberJoined, .serverMemberRemoved, .serverDeleted:
                // D88: the relay is the control plane for servers — re-read the list.
                needServers = true
            case .profileUpdated(let payload):
                // Someone's picture changed: mine on another device (re-read /me) or a person I
                // share a chat or a server with (their card comes with the lists).
                if payload.user_id == user?.id { await refreshAccount() }
                needChats = true
                needServers = true
            case .agentProfileUpdated(let payload):
                // An agent's avatar changed (the agents' `profile_updated`): the new picture and
                // stamp land in every copy now; the re-reads bring the leaf picks and the rest.
                applyAgentAvatarPush(projectId: payload.project_id, imageKey: payload.image_key, version: Int(payload.avatar_version))
                await refreshRegistry()
                needChats = true
            case .chatArchived(let payload):
                // Another device of mine archived / unarchived a people chat.
                if peopleChat(payload.chat_id) != nil {
                    applyRemoteArchive(peopleChatId: payload.chat_id, archived: payload.archived_at != nil)
                }
            case .spaceArchived(let payload):
                applyRemoteArchive(spaceId: payload.space_id, kind: payload.kind, archived: payload.archived_at != nil)
            case .channelMessage:
                // Server channel banner: push-server-channels-ios-2026-10-26 (next item).
                break
            case .peopleChatDeleted, .peopleInviteAccepted, .machineReady:
                break
            case .notificationsCleared(let payload):
                applyRemoteClear(chatId: payload.chat_id, relayTotal: payload.badge)
            }
        }
        if needChats { await refreshPeople() }
        if needServers { await refreshServers() }
        if notifiedPushIds.count > 2000 { notifiedPushIds.removeAll() }
    }

    /// `feedback_capture`: the user asked their agent to file feedback; the agent wants a picture
    /// of the screen they are looking at. Only the app in the foreground answers with an image
    /// (the same key-window capture as the shake sheet); a stale or background request is skipped.
    private func answerFeedbackCapture(_ p: PushItem) async {
        guard case .feedbackCapture(let payload) = p.event else { return }
        let requestId = payload.request_id
        guard !requestId.isEmpty else { return }
        let expires = payload.expires_at
        if expires > 0, Int64(Date().timeIntervalSince1970 * 1000) > expires { return }
        let screen = visibleChatId.map { "chat:\($0)" } ?? "home"
        var image: Data?
        var size = (w: 0, h: 0)
        var skipped: String?
        if appActive, let shot = FeedbackCenter.captureScreen() {
            image = shot.jpegData(compressionQuality: 0.85)
            size = (Int(shot.size.width * shot.scale), Int(shot.size.height * shot.scale))
        } else {
            skipped = appActive ? "failed: window capture returned nothing" : "background"
        }
        do {
            try await api.sendFeedbackCapture(requestId: requestId, image: image, mime: "image/jpeg", width: size.w, height: size.h,
                                              appVersion: FeedbackCenter.appVersion, os: "iOS \(UIDevice.current.systemVersion)",
                                              device: FeedbackCenter.deviceLine, screen: screen, skipped: skipped)
            if image != nil { notice = Notice(text: "Screenshot added to your feedback") }
        } catch {
            pushLog.notice("feedback capture upload failed: \(error.localizedDescription, privacy: .public)")
        }
    }

    // MARK: Lookups

    /// The relay's HTTP origin (`https://relay.arbos.life`), for URLs built outside this file.
    var relayHttpUrl: String { api.endpoint.httpUrl }

    /// Relay blob URL with the device token as a query parameter (the relay
    /// accepts `?token=` on every route), so AVPlayer and Safari can open it.
    func blobURL(projectId: Id, key: String) -> URL? {
        let base = api.endpoint.httpUrl
        guard !base.isEmpty else { return nil }
        // Build via URLComponents so a people-slot `:` and JWT characters stay valid;
        // leave the token value un-encoded (`=` padding) — the relay does not decode.
        guard var comps = URLComponents(string: base) else { return nil }
        comps.percentEncodedPath = "/blob/\(Self.blobPathSegment(projectId))/\(Self.blobPathSegment(key))"
        let token = credentials?.deviceToken ?? ""
        comps.percentEncodedQuery = token.isEmpty ? nil : "token=\(token)"
        return comps.url
    }

    /// Path segment for `/blob/{project}/{key}`: encode spaces etc., keep `:` for `people:{chat}`.
    private static func blobPathSegment(_ raw: String) -> String {
        var allowed = CharacterSet.urlPathAllowed
        allowed.insert(":")
        return raw.addingPercentEncoding(withAllowedCharacters: allowed) ?? raw
    }

    var authHeaders: [String: String] {
        ["Authorization": "Bearer \(credentials?.deviceToken ?? "")"]
    }

    // MARK: One chat per agent (D78)

    /// The chat each agent opens into, chosen once per agent on this device and
    /// then fixed: the chat the user last used here before D78 (migrated from the
    /// old per-agent "last opened" store), else the relay's `primary_chat_id`
    /// (see `primaryChat`). Falls back to the primary if that chat is gone.
    /// Other plain chats stay on the box and relay as history (memory tree,
    /// search); the iPhone never lists, badges or notifies them.
    private var singleChatIds: [Id: Id] {
        get { (UserDefaults.standard.dictionary(forKey: "singleChatByProject") as? [String: String]) ?? [:] }
        set { UserDefaults.standard.set(newValue, forKey: "singleChatByProject") }
    }

    private func setSingleChat(_ chatId: Id, of projectId: Id) {
        var m = singleChatIds
        m[projectId] = chatId
        singleChatIds = m
    }

    func singleChat(of projectId: Id) -> Chat? {
        let plain = chats(of: projectId).filter { !$0.isGroup && !$0.isSystem }
        if let id = singleChatIds[projectId], let c = plain.first(where: { $0.id == id }) { return c }
        // Migration from multi-chat: the chat last used on this device wins once.
        let legacy = (UserDefaults.standard.dictionary(forKey: "lastOpenedChat") as? [String: String]) ?? [:]
        if let id = legacy[projectId], let c = plain.first(where: { $0.id == id }) {
            setSingleChat(c.id, of: projectId)
            return c
        }
        guard let c = primaryChat(of: projectId) else { return nil }
        if !c.isGroup, !c.isSystem { setSingleChat(c.id, of: projectId) }
        return c
    }

    /// A plain chat of an agent that is not its one chat: history only.
    func isHiddenChat(_ chatId: Id) -> Bool {
        guard let c = chat(chatId), !c.isGroup, !c.isSystem else { return false }
        return singleChat(of: c.projectId)?.id != c.id
    }

    /// The one chat an agent shows. Rule: the relay's `primary_chat` when it
    /// sends one; else the chat with the most messages, ties → the earliest
    /// created (ULID order); before any messages have loaded, the pinned
    /// ("main") chat, else the first.
    func primaryChat(of projectId: Id) -> Chat? {
        let all = chats(of: projectId)
        // Agent-hosted group chats are their own rows on Home and the "projects"
        // system chat is in the menu; the primary is a plain thread (§9b).
        let plain = all.filter { !$0.isGroup && !$0.isSystem }
        let list = plain.isEmpty ? all : plain
        guard !list.isEmpty else { return nil }
        if let pid = project(projectId)?.primaryChatId, let c = list.first(where: { $0.id == pid }) { return c }
        let counted = list.map { ($0, messages(of: $0.id).count) }
        if counted.contains(where: { $0.1 > 0 }) {
            return counted.max { a, b in a.1 != b.1 ? a.1 < b.1 : a.0.id > b.0.id }?.0
        }
        return list.first { $0.isPinned } ?? list.min { $0.id < $1.id }
    }


    func project(_ id: Id) -> Project? { projects.first { $0.id == id } }

    func chat(_ id: Id) -> Chat? {
        for list in chats.values {
            if let c = list.first(where: { $0.id == id }) { return c }
        }
        return nil
    }

    func chats(of projectId: Id) -> [Chat] { chats[projectId] ?? [] }

    /// Room docs first; a server channel (D88) reads its Matrix timeline.
    func messages(of chatId: Id) -> [Message] { messages[chatId] ?? channelRows[chatId] ?? [] }

    /// Add or remove an emoji reaction on a message. Multiple different emojis
    /// can be active on the same message; tapping the same emoji again clears it.
    func toggleReaction(chatId: Id, messageId: Id, emoji: String) {
        guard let me = myUserId, !emoji.isEmpty else { return }
        #if DEBUG
        if isFixture {
            guard var list = messages[chatId], let i = list.firstIndex(where: { $0.id == messageId }) else { return }
            list[i].reactions = MessageReaction.toggling(list[i].reactions, emoji: emoji, sender: me)
            messages[chatId] = list
            Haptics.pick()
            return
        }
        #endif
        guard let doc = docs[chatId] else { return }
        do {
            try doc.toggleReaction(messageId: messageId, emoji: emoji, sender: me)
            doc.commit()
            Haptics.pick()
        } catch {
            fail("update the reaction", error)
        }
    }

    /// Delete a message for everyone. The relay decides (`DELETE /room/…/messages/{id}`,
    /// 403 `not_yours` otherwise) and writes the deletion into the doc; it comes back as a
    /// relay-signed row. No push notification. In a people group, this device also writes a
    /// hidden admin audit line for the Activity screen before asking the relay.
    func deleteMessage(chatId: Id, messageId: Id, projectId: Id? = nil) {
        guard !messageId.isEmpty else { return }
        let audit = groupDeletionAuditLine(chatId: chatId)
        #if DEBUG
        if isFixture {
            guard var list = messages[chatId] else { return }
            list.removeAll { $0.id == messageId }
            if let audit {
                let m = Message(id: newId(), chatId: chatId, role: .system, sender: "system", status: .done, turn: nil, decision: nil,
                                subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: nowMs(),
                                completedAt: nowMs(), pending: false, parts: [.text(id: newId(), text: audit)], hiddenCount: 0)
                list.append(m)
            }
            messages[chatId] = list
            clearUnacked(chatId: chatId, messageId: messageId)
            Haptics.pick()
            return
        }
        #endif
        if let audit, let doc = docs[chatId] {
            do {
                try doc.insertSystemLine(id: newId(), text: audit, deviceId: deviceId)
                doc.commit()
            } catch {
                // Activity can miss this delete; the relay delete still runs.
            }
        }
        let room: String
        if peopleChat(chatId) != nil {
            room = "people/\(chatId)"
        } else if let projectId, projectId != peopleSlot {
            room = "\(projectId)/\(chatId)"
        } else {
            room = "people/\(chatId)"
        }
        Task {
            do {
                try await api.deleteMessage(room: room, messageId: messageId)
                clearUnacked(chatId: chatId, messageId: messageId)
                Haptics.pick()
            } catch {
                let e = error as? RelayError
                fail(e?.status == 403
                     ? UserError.refused("delete the message", "You can only delete your own messages.")
                     : UserError.classify("delete the message", error))
            }
        }
    }

    /// Edit a message's text in the shared chat doc (own messages). No new send
    /// command — peers see the updated body via room sync only.
    func editMessage(chatId: Id, messageId: Id, text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !messageId.isEmpty, !trimmed.isEmpty else { return }
        #if DEBUG
        if isFixture {
            guard var list = messages[chatId],
                  let idx = list.firstIndex(where: { $0.id == messageId })
            else { return }
            var parts = list[idx].parts
            if let t = parts.lastIndex(where: {
                if case .text = $0 { return true } else { return false }
            }) {
                if case .text(let id, _) = parts[t] {
                    parts[t] = .text(id: id, text: trimmed)
                }
            } else {
                parts.append(.text(id: "\(messageId)-t", text: trimmed))
            }
            list[idx].parts = parts
            messages[chatId] = list
            Haptics.pick()
            return
        }
        #endif
        guard let doc = docs[chatId] else { return }
        do {
            try doc.editMessage(messageId: messageId, text: trimmed)
            doc.commit()
            Haptics.pick()
        } catch {
            fail("edit the message", error)
        }
    }

    /// Text for a people-group deletion audit, or nil when this chat is not a group.
    private func groupDeletionAuditLine(chatId: Id) -> String? {
        guard let people = peopleChat(chatId), people.isGroup else { return nil }
        let who: String = {
            if let name = user?.name?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty {
                return name
            }
            if let login = user?.login, !login.isEmpty { return "@\(login)" }
            return "Someone"
        }()
        return GroupActivity.deletionNotice(who: who)
    }

    /// Paint any room doc that already has rows but no published transcript yet
    /// (checkpoint landed, catch-up still in flight). Home search reads
    /// `messages`, so this must run when the search field becomes active.
    func paintTranscriptsForSearch() {
        for (key, doc) in docs {
            guard rooms[key]?.roomChat != projectDocChatId else { continue }
            if messages[key] == nil, doc.count > 0 {
                #if DEBUG
                PerfCounters.bump("emit.L4066")
                #endif
                emitSnapshot(key)
            }
        }
    }

    /// Every loaded user / assistant / project message with prose in a chat
    /// (home search; system lines never count). Falls back to a one-shot doc
    /// read when the published array is still nil.
    func searchableMessages(of chatId: Id) -> [Message] {
        let list: [Message]
        if let published = messages[chatId] {
            list = published
        } else if let doc = docs[chatId], doc.count > 0 {
            list = doc.readAll()
        } else {
            list = []
        }
        return list.filter { $0.role != .system && !$0.plainText.isEmpty }
    }

    /// Every loaded message of a chat for the app-wide search index — text or
    /// attachments; system lines never count (`SearchStore`).
    func indexableMessages(of chatId: Id) -> [Message] {
        let list: [Message]
        if let published = messages[chatId] {
            list = published
        } else if let rows = channelRows[chatId] {
            list = rows
        } else if let doc = docs[chatId], doc.count > 0 {
            list = doc.readAll()
        } else {
            list = []
        }
        return list.filter { $0.role != .system }
    }

    /// S8: the last assistant message asked a question and nothing was sent since.
    func chatNeedsAnswer(_ chatId: Id) -> Bool {
        guard let last = messages(of: chatId).last, last.role == .assistant, !last.isStreaming else { return false }
        // A choice card is answered from its rows, so it does not switch the composer.
        if let q = last.openQuestion { return !q.hasChoices }
        return last.askUserText != nil
    }

    /// The running turn's current activity for the working line: the
    /// message's `activity`, else the latest working/step status part with a
    /// real message; nil → "<name> is working".
    func currentActivity(_ chatId: Id) -> String? {
        // The newest live row that has words: a short main turn streaming with no activity
        // yet must not blank a job's "Reading … · 2 tasks" under it (duplex §2.4).
        for m in messages(of: chatId).reversed() where m.role == .assistant && m.isStreaming {
            if let words = Self.activityWords(m) { return words }
        }
        return nil
    }

    private static func activityWords(_ m: Message) -> String? {
        if let a = m.activity?.trimmingCharacters(in: .whitespacesAndNewlines), !a.isEmpty { return a }
        for part in m.parts.reversed() {
            if case .status(_, let code, let text) = part, code == .working || code == .step {
                let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
                // "Working…", "Working.", "thinking" → fall through to "<name> is working".
                let bare = t.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: ".…"))
                if !bare.isEmpty, bare != "working", bare != "thinking" { return t }
            }
        }
        return nil
    }

    /// A turn is running when a recent assistant message still streams and
    /// no newer user line has arrived after it. A message left `streaming`
    /// by a box that died (or a leftover stream under a later user message)
    /// must not pin the indicator forever — every streaming row uses the
    /// same 15-minute freshness window (including the last assistant message).
    /// What the working-state checks need from a chat's list, gathered in one pass
    /// and cached until the list changes. The time windows are applied at read time.
    struct ChatActivity {
        /// `createdAt` of the newest assistant row still streaming with no user line after it.
        var liveStreamAt: Int64?
        /// `createdAt` of the newest job row (`job` set) still streaming, user lines or not.
        var liveJobAt: Int64?
        /// Open hand-off rows: (streaming, createdAt, completedAt ?? createdAt).
        var openHandoffs: [(streaming: Bool, createdAt: Int64, finishedAt: Int64)] = []
        /// `createdAt` of the last row when it is the user's.
        var lastUserAt: Int64?
    }

    private func activity(_ chatId: Id) -> ChatActivity {
        if let hit = activityCache[chatId] { return hit }
        let list = messages(of: chatId)
        var out = ChatActivity()
        var userAfter = false
        for m in list.reversed() {
            if out.liveStreamAt == nil {
                if m.role == .user { userAfter = true }
                if m.role == .assistant, m.isStreaming, !userAfter { out.liveStreamAt = m.createdAt }
            }
            if out.liveJobAt == nil, m.role == .assistant, m.job != nil, m.isStreaming { out.liveJobAt = m.createdAt }
            if m.role == .assistant, m.subagentId != nil || m.subagentName != nil,
               m.turn == .new || m.turn == .spawn || m.turn == .wake,
               (m.handoffMessages ?? 1) < 2 {
                out.openHandoffs.append((m.isStreaming, m.createdAt, m.completedAt ?? m.createdAt))
            }
        }
        if let last = list.last, last.role == .user { out.lastUserAt = last.createdAt }
        activityCache[chatId] = out
        return out
    }

    func isStreaming(_ chatId: Id) -> Bool {
        let recent = nowMs() - 15 * 60 * 1000
        if let at = activity(chatId).liveStreamAt, at >= recent { return true }
        // A background job's row streams until the box finishes it, whatever the user
        // typed since (duplex §2.3).
        return activity(chatId).liveJobAt.map { $0 >= recent } ?? false
    }

    func isStreaming(project projectId: Id) -> Bool {
        chats(of: projectId).contains { isStreaming($0.id) || hasOpenSubagent($0.id) }
    }

    /// A subagent hand-off is still open: the brief went out (`handoffMessages` 1)
    /// and the result has not come back (2). The spawn/wake row may already be
    /// `done` while the child still runs, so `isStreaming` alone misses it —
    /// show the faded working line until the hand-off closes.
    func hasOpenSubagent(_ chatId: Id) -> Bool {
        let now = nowMs()
        let recent = now - 15 * 60 * 1000
        return activity(chatId).openHandoffs.contains { h in
            // Streaming hand-off rows (the normal case while the child runs).
            // Same freshness window as `isStreaming` so a dead box cannot pin forever.
            if h.streaming { return h.createdAt >= recent }
            // Brief race after `finish_message` before `handoffMessages` flips to 2.
            return now - h.finishedAt < 60_000
        }
    }

    /// Visual cheat: the last row is the user's and no assistant turn has
    /// started yet — show the working line immediately so a simple first
    /// question does not sit on a dead transcript while the box wakes.
    /// Agent project chats only (a human DM has no agent to "work").
    func isAwaitingReply(_ chatId: Id) -> Bool {
        if isStreaming(chatId) || hasOpenSubagent(chatId) { return false }
        if peopleChat(chatId) != nil { return false }
        // Nobody "works" in a server channel; a sent line is just a line.
        if channel(chatId) != nil { return false }
        guard let at = activity(chatId).lastUserAt else { return false }
        // Short bridge until the agent opens a streaming row (cold wake).
        // ChatView ticks this once a second so it can expire cleanly.
        return nowMs() - at < 15_000
    }

    // MARK: Commands (crdt-sync §6.2: the client writes rows and commands)

    /// An attachment staged in the composer, uploaded on send.
    struct PendingAttachment: Identifiable, Equatable {
        let id = UUID()
        var name: String
        var mime: String
        var data: Data
    }

    /// Retry on one of my bubbles the relay refused (`status == .error`): the failed row
    /// goes and its text + reply-to go out again as a new message through the normal send
    /// path (attachments were picked at send time and are not re-read).
    func resendFailed(chatId: Id, messageId: Id) {
        guard var list = messages[chatId],
              let ix = list.firstIndex(where: { $0.id == messageId && $0.status == .error }) else { return }
        let failed = list.remove(at: ix)
        messages[chatId] = list
        let text = failed.plainText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        send(chatId: chatId, text: text, replyTo: failed.replyTo)
    }

    /// `replyTo` = the message this send answers (reply-to); it lands on the row as `replyTo`
    /// so every client draws the quote header and agentd puts the quoted message in the turn.
    func send(chatId: Id, text: String, attachments: [PendingAttachment] = [], replyTo: Id? = nil,
              forwardedFrom: String? = nil) {
        // A new send clears the composer's note (the next failure brings its own).
        clearSendError(chatId)
        if JobTabs.isJobChat(chatId) {
            steerFromJobTab(chatId: chatId, text: text)
            return
        }
        if attachments.isEmpty {
            write(chatId: chatId, text: text, command: "send", attachments: [], replyTo: replyTo,
                  forwardedFrom: forwardedFrom)
            return
        }
        guard let chat = chat(chatId) else { return }
        if isFixture {
            write(chatId: chatId, text: text, command: "send", attachments: attachments.map {
                Attachment(name: $0.name, mime: $0.mime, blobRef: nil, kind: Self.attachmentKind(mime: $0.mime, name: $0.name))
            }, replyTo: replyTo)
            return
        }
        Task {
            var uploaded: [Attachment] = []
            for a in attachments {
                let key = AppModel.blobKey(for: a.name)
                let kind = Self.attachmentKind(mime: a.mime, name: a.name)
                do {
                    _ = try await api.putBlob(projectId: chat.projectId, key: key, mime: a.mime, data: a.data)
                    uploaded.append(Attachment(name: a.name, mime: a.mime, blobRef: key, kind: kind))
                } catch {
                    setSendError(chatId, UserError.classify("upload the attachment", error))
                    uploaded.append(Attachment(name: a.name, mime: a.mime, blobRef: nil, kind: kind))
                }
            }
            write(chatId: chatId, text: text, command: "send", attachments: uploaded, replyTo: replyTo)
        }
    }

    /// Attachment `kind` hint written into the doc (matches agentd / Mac).
    static func attachmentKind(mime: String, name: String) -> String? {
        let probe = Attachment(name: name, mime: mime, blobRef: nil)
        if probe.isImage { return "image" }
        if mime.lowercased().hasPrefix("audio/") { return "audio" }
        if mime.lowercased().hasPrefix("video/") { return "video" }
        return nil
    }

    /// Voice memo → text through the relay. The audio is only an STT input; callers send
    /// the transcript (or put it in the draft) and do not attach the file. `nil` when the
    /// relay cannot transcribe; the user sees why.
    func transcribe(projectId: Id?, memo: PendingAttachment) async -> String? {
        if isFixture { return nil }
        do {
            let text = try await api.transcribe(projectId: projectId, data: memo.data, mime: memo.mime)
            return text.isEmpty ? nil : text
        } catch {
            if let chat = visibleChatId {
                setSendError(chat, UserError.classify("transcribe your memo", error))
            } else {
                fail("transcribe your memo", error)
            }
            return nil
        }
    }

    /// Blob object name for `PUT /blob/{project}/{key}`. The relay's `safe_key` only
    /// allows ASCII alnum plus `-_.:` and caps length at 128 — match Mac/`agentd`.
    static func blobKey(for name: String) -> String {
        let prefix = "att-\(newId().lowercased())-"
        let budget = max(0, 128 - prefix.count)
        return prefix + String(safeName(name).prefix(budget))
    }

    /// Strip to relay-safe ASCII (Mac `safe_name`). Swift `isLetter`/`isNumber` keep
    /// Unicode (e.g. locale photo titles like `2026年10月15日.jpg`) which the relay
    /// rejects with HTTP 400 `bad key`.
    static func safeName(_ name: String) -> String {
        let cleaned = String(name.map { c -> Character in
            if c.isASCII && (c.isLetter || c.isNumber || c == "." || c == "-" || c == "_") {
                return c
            }
            return "_"
        })
        return cleaned.isEmpty ? "file" : cleaned
    }

    /// Typed answer from the composer (S8). When the open question is a `question` part
    /// the phone cannot draw as rows (text, date, form, or plain), the answer carries its
    /// `questionId` so agentd flips the part to answered.
    func answer(chatId: Id, text: String) {
        var extra: [String: Any] = [:]
        if let q = messages(of: chatId).last?.openQuestion, !q.hasChoices {
            extra = ["questionId": q.questionId, "value": text]
            markQuestionAnswered(chatId: chatId, questionId: q.questionId, answer: [text])
        }
        write(chatId: chatId, text: text, command: "answer", attachments: [], extra: extra)
    }

    /// Answer from a question card row: the label becomes the user's bubble at once
    /// (optimistic, same words as the Mac), the card flips to answered, and the structured
    /// value travels in the `answer` command for agentd to render for the model.
    func answerQuestion(chatId: Id, questionId: String, value: Any, label: String) {
        let ids: [String] = (value as? [String]) ?? [(value as? String) ?? ""]
        if let approvalId = questionId.stripping(prefix: "approval:") {
            decideApproval(chatId: chatId, approvalId: approvalId, choice: ids.first ?? "deny")
            return
        }
        markQuestionAnswered(chatId: chatId, questionId: questionId, answer: ids)
        write(chatId: chatId, text: "Answer: \(label)", command: "answer", attachments: [],
              extra: ["questionId": questionId, "value": value])
    }

    /// An approval card (S18) answered on the phone: for a secret request the relay row is the
    /// authority (`POST /secrets/requests/{id}/decide`), then the doc command `approve` /
    /// `reject` flips the card for agentd, exactly as the Mac does with `vaultRequestId`.
    /// Secret hand-off request cards: where this device's save stands, by approval id.
    enum HandoffState: Equatable {
        case saving
        /// Saved; the vault name.
        case saved(String)
        /// Drawn by the card's `ErrorNote` ("question-card-error").
        case failed(UserError)
    }

    /// Secret hand-off request card → Save: the value leaves this device once, to
    /// `POST /secrets` (source `chat`, the agent granted); the card is then answered with
    /// the saved secret's id and names — never the value — so the agent learns only the name.
    func saveSecretFromCard(chatId: Id, approvalId: String, projectId: Id, name: String, value: String, purpose: String) {
        handoff[approvalId] = .saving
        Task { @MainActor in
            do {
                let body: [String: Any] = [
                    "name": name, "kind": "key", "value": value, "note": purpose,
                    "agents": [["project_id": projectId, "note": purpose]],
                    "source": ["kind": "chat", "project_id": projectId, "chat_id": chatId],
                ]
                let saved = try await api.createSecret(body)
                markQuestionAnswered(chatId: chatId, questionId: "approval:\(approvalId)", answer: ["saved"])
                if let doc = docs[chatId] {
                    let payload: [String: Any] = ["approvalId": approvalId,
                                                  "secret": ["id": saved.id, "name": saved.name, "env": saved.env]]
                    try? doc.pushCommand("approve", payload: payload, device: deviceId)
                    doc.commit()
                }
                handoff[approvalId] = .saved(saved.env)
                await refreshSecrets()
            } catch {
                handoff[approvalId] = .failed(UserError.classify("save the secret", error))
            }
        }
    }

    func decideApproval(chatId: Id, approvalId: String, choice: String) {
        let questionId = "approval:\(approvalId)"
        var secretRequestId: String?
        for m in messages[chatId] ?? [] {
            for p in m.parts {
                if case .question(_, let q) = p, q.questionId == questionId { secretRequestId = q.secretRequestId }
            }
        }
        markQuestionAnswered(chatId: chatId, questionId: questionId, answer: [choice])
        let approve = choice != "deny" && choice != "reject"
        if let rid = secretRequestId {
            Task { @MainActor in
                do {
                    try await api.decideSecretRequest(rid, decision: choice)
                    await refreshSecrets()
                } catch {
                    fail("save the secret", error)
                }
            }
        }
        guard let doc = docs[chatId] else { return }
        var payload: [String: Any] = ["approvalId": approvalId]
        if !approve { payload["reason"] = "denied on iPhone" }
        // #1544: "grant:<path class>" is a destructive-shell card's standing option.
        if let grant = choice.stripping(prefix: "grant:"), !grant.isEmpty { payload["grant"] = grant }
        try? doc.pushCommand(approve ? "approve" : "reject", payload: payload, device: deviceId)
        doc.commit()
    }

    /// #1544: the agent pane opened — load its standing shell approvals.
    func refreshShellGrants(_ projectId: Id) async {
        guard credentials != nil, !isFixture else { return }
        if let v = try? await relay.shellGrants(projectId) {
            shellGrants[projectId] = v
        }
    }

    /// Settings › Shell approvals: switch a grant off. The row goes at once; the relay's list
    /// confirms it.
    func revokeShellGrant(projectId: Id, grantId: String) {
        shellGrants[projectId]?.removeAll { $0.id == grantId }
        guard !isFixture else { return }
        Task { @MainActor in
            do {
                try await relay.revokeShellGrant(projectId, grantId: grantId)
            } catch {
                fail("change the shell approvals", error)
            }
            await refreshShellGrants(projectId)
        }
    }

    private func markQuestionAnswered(chatId: Id, questionId: String, answer: [String]) {
        guard var list = messages[chatId] else { return }
        for i in list.indices {
            for j in list[i].parts.indices {
                guard case .question(let id, var q) = list[i].parts[j], q.questionId == questionId else { continue }
                q.open = false
                q.answer = answer
                list[i].parts[j] = .question(id: id, question: q)
            }
        }
        messages[chatId] = list
    }

    private func write(chatId: Id, text: String, command: String, attachments: [Attachment],
                       extra: [String: Any] = [:], messageId presetId: Id? = nil, replyTo: Id? = nil,
                       forwardedFrom: String? = nil) {
        guard let chat = chat(chatId) else {
            #if DEBUG
            if isFixture { fixtureAppend(chatId: chatId, text: text, attachments: attachments, replyTo: replyTo) }
            #endif
            return
        }
        // Optimistic new chat: the relay id (and room doc) may not exist yet.
        // Echo the bubble now and flush once `finalizeNewChat` swaps the id.
        guard let doc = docs[chatId] else {
            enqueueOptimisticSend(chatId: chatId, chat: chat, text: text, command: command,
                                  attachments: attachments, extra: extra, replyTo: replyTo,
                                  forwardedFrom: forwardedFrom)
            return
        }
        let messageId = presetId ?? newId()
        let author = projectAuthor(projectId: chat.projectId)
        markUnacked(chatId: chatId, messageId: messageId)
        do {
            try doc.insertUserMessage(id: messageId, text: text, deviceId: deviceId, attachments: attachments,
                                      author: author, replyTo: replyTo,
                                      forwardedFrom: forwardedFrom)
            try doc.pushCommand(command, payload: extra.merging(["messageId": messageId]) { _, new in new }, device: deviceId)
            if !chat.isPinned && chat.title == "New chat" {
                let title = AppModel.title(from: text)
                try doc.pushCommand("retitle", payload: ["title": title], device: deviceId)
                retitleLocally(chatId: chatId, title: title)
            }
        } catch {
            clearUnacked(chatId: chatId, messageId: messageId)
            setSendError(chatId, UserError.classify("send your message", error))
            return
        }
        #if DEBUG
        SendTrace.message(messageId, chatId: chatId)
        #endif
        // Bubble at once — same as channels / optimistic new chats. onChange still
        // reconciles; live coalesce + async full re-reads must not gate the send.
        paintLocalUserMessage(chatId: chatId, messageId: messageId, text: text,
                              attachments: attachments, replyTo: replyTo, forwardedFrom: forwardedFrom,
                              author: author, doc: doc)
        doc.commit()
        #if DEBUG
        if SendTrace.traces(chatId) { SendTrace.mark("commit") }
        #endif
        markActivity(chatId: chatId)
        if command == "send" { announce(chatId: chatId, projectId: chat.projectId, messageId: messageId, text: text) }
        if let p = project(chat.projectId), p.state.isAsleep || p.state == .error {
            wake(projectId: p.id, reason: "send")
        }
    }

    /// Append a just-written user row to `messages` so the bubble appears in the
    /// same turn as Send — before onChange's coalesced / async snapshot lands.
    /// Without this, a send is on the wire at once but the chat stays blank until
    /// a scroll forces a layout pass (feedback app-01M5S3V2823WNP081K5FT16ANQ).
    ///
    /// The row is read back from the doc (`doc`, the writer that just appended it),
    /// so it is byte-for-byte what the next snapshot will carry — same `createdAt`,
    /// same part ids — and that snapshot changes nothing the view can see. The
    /// hand-built row below is the fallback for a doc that cannot be read yet.
    private func paintLocalUserMessage(
        chatId: Id, messageId: Id, text: String, attachments: [Attachment],
        replyTo: Id? = nil, forwardedFrom: String? = nil,
        author: CoreChatDoc.Author? = nil, mentions: [String] = [], doc: CoreChatDoc? = nil,
        replyRef: ReplyRef? = nil
    ) {
        if messages[chatId]?.contains(where: { $0.id == messageId }) == true { return }
        if let doc, doc.count > 0, var row = doc.readMessage(doc.count - 1), row.id == messageId {
            row.pending = true
            var next = messages
            next[chatId, default: []].append(row)
            messages = next
            #if DEBUG
            if SendTrace.traces(chatId) { SendTrace.mark("messages-has-row", detail: "via=optimistic-readback rows=\(next[chatId]?.count ?? 0)") }
            #endif
            return
        }
        var parts: [Part] = attachments.map { .attachment(id: newId(), attachment: $0) }
        if !text.isEmpty {
            parts.append(.text(id: "\(messageId)-t", text: text))
        }
        if parts.isEmpty {
            parts = [.text(id: "\(messageId)-t", text: text)]
        }
        var m = Message(
            id: messageId, chatId: chatId, role: .user,
            sender: author?.sender ?? "user",
            status: .done, turn: nil, decision: nil, subagentId: nil, subagentName: nil,
            inReplyTo: nil, usage: [:], model: nil,
            createdAt: nowMs(), completedAt: nowMs(), pending: true,
            parts: parts, hiddenCount: 0
        )
        m.replyTo = replyTo
        m.forwardedFrom = forwardedFrom
        m.replyRef = replyRef
        m.senderHandle = author?.handle
        m.senderName = author?.name
        m.senderAvatar = author?.avatar
        m.mentions = mentions
        var next = messages
        next[chatId, default: []].append(m)
        messages = next
        #if DEBUG
        if SendTrace.traces(chatId) { SendTrace.mark("messages-has-row", detail: "via=optimistic rows=\(next[chatId]?.count ?? 0)") }
        #endif
    }

    /// Author fields for a row in a project chat (API §3/§7, same as the Mac): always the
    /// handle and display name, so the agent addresses the writer by name; `sender: user:<id>`
    /// in a project someone shared with me, so a guest's words are never read as the owner's
    /// (#1580). A solo project keeps the plain `user` the box expects.
    private func projectAuthor(projectId: Id) -> CoreChatDoc.Author? {
        guard let me = user else { return nil }
        let shared = project(projectId)?.shared == true
        return CoreChatDoc.Author(sender: shared ? "user:\(me.id)" : "user", handle: me.login, name: me.name, avatar: nil)
    }

    /// A send typed before the optimistic new-chat's relay id landed.
    private struct OptimisticSend {
        var messageId: Id
        var text: String
        var command: String
        var attachments: [Attachment]
        var extra: [String: Any]
        var replyTo: Id?
        var forwardedFrom: String?
    }
    private var optimisticSends: [Id: [OptimisticSend]] = [:]
    /// Local chat ids still waiting on `POST /projects/{id}/chats`.
    private var optimisticChatIds: Set<Id> = []
    /// Local → real chat id once the relay create lands (for awaiters).
    private var optimisticRemap: [Id: Id] = [:]

    /// True when we already published an empty transcript for this chat
    /// (optimistic new chat) — ChatView shows the empty hint, not a spinner.
    func hasKnownEmptyTranscript(_ chatId: Id) -> Bool {
        messages[chatId]?.isEmpty == true
    }

    private func enqueueOptimisticSend(chatId: Id, chat: Chat, text: String, command: String,
                                       attachments: [Attachment], extra: [String: Any], replyTo: Id? = nil,
                                       forwardedFrom: String? = nil) {
        guard optimisticChatIds.contains(chatId) else {
            setSendError(chatId, UserError.fromText("send your message", "chat is not ready yet"))
            return
        }
        let messageId = newId()
        markUnacked(chatId: chatId, messageId: messageId)
        var m = Message(id: messageId, chatId: chatId, role: .user, sender: "user", status: .done, turn: nil,
                        decision: nil, subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil,
                        createdAt: nowMs(), completedAt: nowMs(), pending: true,
                        parts: attachments.map { .attachment(id: newId(), attachment: $0) }
                            + (text.isEmpty ? [] : [.text(id: "\(messageId)-t", text: text)]),
                        hiddenCount: 0)
        m.replyTo = replyTo
        m.forwardedFrom = forwardedFrom
        // Keep attachment-only rows valid when text is empty.
        if m.parts.isEmpty { m.parts = [.text(id: "\(messageId)-t", text: text)] }
        messages[chatId, default: []].append(m)
        if !chat.isPinned && chat.title == "New chat" {
            retitleLocally(chatId: chatId, title: AppModel.title(from: text))
        }
        optimisticSends[chatId, default: []].append(
            OptimisticSend(messageId: messageId, text: text, command: command,
                           attachments: attachments, extra: extra, replyTo: replyTo, forwardedFrom: forwardedFrom)
        )
        markActivity(chatId: chatId)
        if let p = project(chat.projectId), p.state.isAsleep || p.state == .error {
            wake(projectId: p.id, reason: "send")
        }
    }

    /// §7c: tell the relay about the row we just pushed so other members'
    /// phones get a real preview instead of "New message". Best effort.
    private func announce(chatId: Id, projectId: Id?, messageId: String, text: String, replyRef: ReplyRef? = nil) {
        guard !isFixture else { return }
        let api = self.api
        Task { try? await api.notifyMessage(chatId: chatId, projectId: projectId, messageId: messageId, text: text, replyRef: replyRef) }
    }

    // MARK: Reply in DM (#1836)

    /// The cross-chat reply to arm on the DM that `replyInDM` opens next; the ChatView of
    /// that DM takes it on appear (Mac `pending_dm_reply`).
    @Published var pendingDMReply: (chatId: Id, ref: ReplyRef)?

    /// The cross-chat reference for `message` in `chatId`: its address plus a snapshot of
    /// who wrote it and what it said (crdt-sync §6.2 `replyRef`).
    func replyRef(for message: Message, in chatId: Id, projectId: Id?) -> ReplyRef {
        let people = projectId == nil || projectId == peopleSlot
        let mine = message.sender == "user" || message.sender == myUserId
        let (name, handle): (String, String)
        if mine, let me = user {
            (name, handle) = (me.name ?? me.login, me.login)
        } else if message.role == .user {
            (name, handle) = (message.senderName ?? "", message.senderHandle ?? "")
        } else {
            let agentName = projectId.flatMap { project($0)?.name } ?? message.senderName ?? ""
            (name, handle) = (agentName, message.senderHandle ?? "")
        }
        var text = message.plainText
        if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            for part in message.parts {
                if case .attachment(_, let a) = part { text = a.name; break }
            }
        }
        return ReplyRef(chatId: chatId, projectId: people ? "people" : (projectId ?? "people"),
                        messageId: message.id, chatTitle: chatDisplayTitle(chatId, projectId: projectId),
                        sender: message.sender, senderName: name, senderHandle: handle,
                        text: ReplyRef.snapshot(text))
    }

    /// What a quote header calls `chatId`: the DM peer's name, the group's title, or the
    /// project (agent) name.
    func chatDisplayTitle(_ chatId: Id, projectId: Id?) -> String {
        if let pc = peopleChat(chatId) {
            if pc.kind == "dm", let peer = pc.humans.first(where: { $0.id != user?.id }) {
                return peer.name ?? "@\(peer.handle)"
            }
            return pc.title
        }
        if let projectId, let p = project(projectId) { return p.name }
        return chat(chatId)?.title ?? ""
    }

    /// Is `chatId` one of this user's chats (a people chat, or a chat of an own or shared
    /// project)? Only then can a cross-chat quote open its original.
    func isMember(ofChat chatId: Id) -> Bool {
        if peopleChat(chatId) != nil { return true }
        return chats.values.contains { $0.contains { $0.id == chatId } }
    }

    /// Bubble menu → Reply in DM: open (or create) the DM with the person who wrote
    /// `message` and arm its composer with a cross-chat reply to it.
    func replyInDM(to message: Message, in chatId: Id, projectId: Id?) {
        guard let handle = message.senderHandle, !handle.isEmpty,
              message.sender.hasPrefix("user:"), message.sender != myUserId
        else { return }
        let ref = replyRef(for: message, in: chatId, projectId: projectId)
        Task { @MainActor in
            guard let dm = await openDM(handle: handle) else { return }
            pendingDMReply = (dm.id, ref)
        }
    }

    /// Tap on a cross-chat quote: open the source chat on the original. The ChatView
    /// scrolls to and tints `pendingReveal` on appear. `false` when not a member.
    @discardableResult
    func reveal(_ r: ReplyRef) -> Bool {
        guard isMember(ofChat: r.chatId) else { return false }
        pendingReveal = MessageReveal(chatId: r.chatId, messageId: r.messageId)
        if peopleChat(r.chatId) != nil {
            path = [.chat(chatId: r.chatId, projectId: peopleSlot)]
        } else if let pid = chats.first(where: { $0.value.contains { $0.id == r.chatId } })?.key {
            path = [.chat(chatId: r.chatId, projectId: pid)]
        } else {
            return false
        }
        return true
    }


    func stop(chatId: Id) {
        if let jobId = JobTabs.jobId(chatId) {
            // A job tab's Stop halts that job only (`stop { jobId }`).
            if let parent = jobParentChat(jobId) { stopJob(chatId: parent, jobId: jobId) }
            return
        }
        forgetJobs(chatId: chatId)
        guard let doc = docs[chatId] else { return }
        try? doc.pushCommand("stop", payload: [:], device: deviceId)
        doc.commit()
    }

    // MARK: Tasks chip and sheet (duplex §2.4, D7)

    /// The open background jobs of `chatId`, newest step first as the relay lists them.
    /// Empty when the relay predates `activity.jobs[]` or nothing runs.
    func openJobs(_ chatId: Id) -> [Job] {
        if peopleChat(chatId) != nil {
            // A group / DM: the jobs any agent in it started from this chat.
            return jobs.values.flatMap { $0 }.filter { $0.chatId == chatId }
        }
        guard let chat = chat(chatId), let list = jobs[chat.projectId] else { return [] }
        return list.filter { $0.chatId == nil || $0.chatId == chatId }
    }

    /// The agent projects whose listings the open chat needs: its own agent, or every agent
    /// in a group / DM.
    private func jobsProjects(for chatId: Id) -> [Id] {
        if let pc = peopleChat(chatId) {
            return pc.members.filter { $0.isAgent }.map(\.id).filter { project($0) != nil }
        }
        if let chat = chat(chatId) { return [chat.projectId] }
        return []
    }

    // MARK: Job tabs (`JobTabs`)

    /// The project a job belongs to: the one whose listing or tabs hold it.
    func jobProject(_ jobId: String) -> Id? {
        if let hit = jobs.first(where: { $0.value.contains { $0.id == jobId } }) { return hit.key }
        let chat = JobTabs.chatId(jobId)
        return jobTabs.first { $0.value.contains { $0.id == chat } }?.key
    }

    /// The tab entry behind a `job:<id>` chat id.
    func jobTab(_ chatId: Id) -> Chat? {
        for list in jobTabs.values {
            if let c = list.first(where: { $0.id == chatId }) { return c }
        }
        return nil
    }

    /// The agent's job tabs, after its chats.
    func jobTabs(of projectId: Id) -> [Chat] { jobTabs[projectId] ?? [] }

    private func jobRow(_ jobId: String) -> Job? { jobs.values.flatMap { $0 }.first { $0.id == jobId } }

    /// The chat the job was asked in: the document's word, else the chip's.
    private func jobParentChat(_ jobId: String) -> Id? {
        rollouts[jobId]?.chatId ?? jobRow(jobId)?.chatId
    }

    /// The listing changed: every job the agent runs has a tab (unless the user closed it);
    /// tabs of finished jobs stay until closed.
    private func syncJobTabs(projectId: Id) {
        var tabs = jobTabs[projectId] ?? []
        for job in jobs[projectId] ?? [] where !closedJobTabs.contains(job.id) {
            let id = JobTabs.chatId(job.id)
            if let i = tabs.firstIndex(where: { $0.id == id }) {
                if !job.title.trimmingCharacters(in: .whitespaces).isEmpty { tabs[i].title = job.title }
            } else {
                tabs.append(JobTabs.tab(projectId: projectId, jobId: job.id, title: job.title, at: nowMs()))
            }
        }
        if tabs != (jobTabs[projectId] ?? []) { jobTabs[projectId] = tabs }
    }

    /// Open the tab of `jobId`: the agent's own chat first (so back lands there — from a
    /// group this is the jump to the agent's space), then the job's screen. A closed tab comes back.
    func openJobTab(_ jobId: String) {
        guard let projectId = jobProject(jobId) else { return }
        closedJobTabs.remove(jobId)
        let chatId = JobTabs.chatId(jobId)
        if jobTab(chatId) == nil {
            let title = jobRow(jobId)?.title ?? rollouts[jobId]?.title ?? ""
            jobTabs[projectId, default: []].append(JobTabs.tab(projectId: projectId, jobId: jobId, title: title, at: nowMs()))
        }
        unarchive(projectId)
        var next: [Route] = []
        if let main = singleChat(of: projectId) ?? chats(of: projectId).first {
            next.append(.chat(chatId: main.id, projectId: projectId))
        }
        next.append(.chat(chatId: chatId, projectId: projectId))
        path = next
        pollRolloutIfNeeded(chatId: chatId, force: true)
    }

    /// The status line's link: the newest open job of the chat.
    func openNewestJobTab(from chatId: Id) {
        if let job = openJobs(chatId).first { openJobTab(job.id) }
    }

    /// Close a job tab: back to the agent's chat; the job itself goes on.
    func closeJobTab(_ chatId: Id) {
        guard let jobId = JobTabs.jobId(chatId), let projectId = jobProject(jobId) else { return }
        jobTabs[projectId]?.removeAll { $0.id == chatId }
        closedJobTabs.insert(jobId)
        messages.removeValue(forKey: chatId)
        pendingSteers.removeValue(forKey: jobId)
        if isShowing(chatId: chatId) { path.removeLast() }
    }

    /// The open job tab polls its document each tick while the job runs (the whole document
    /// first, then only the entries past the newest one held).
    func pollRolloutIfNeeded(chatId: Id, force: Bool = false) {
        guard let jobId = JobTabs.jobId(chatId), let projectId = jobProject(jobId) else { return }
        if isFixture {
            if rollouts[jobId] == nil || rollouts[jobId]?.isRunning == true { fixtureAdvanceRollout(jobId) }
            return
        }
        if let held = rollouts[jobId], held.unavailable || !held.isRunning, !force { return }
        let now = nowMs()
        guard force || now - (rolloutPolledAt[jobId] ?? 0) >= AppModel.jobsPollMs, !rolloutPolling.contains(jobId) else { return }
        rolloutPolledAt[jobId] = now
        rolloutPolling.insert(jobId)
        let after = rollouts[jobId].flatMap { $0.entries.isEmpty ? nil : $0.nextSeq - 1 }
        let api = self.api
        Task { @MainActor [weak self] in
            defer { self?.rolloutPolling.remove(jobId) }
            guard let self else { return }
            do {
                if let doc = try await api.jobRollout(projectId: projectId, jobId: jobId, after: after) {
                    applyRollout(jobId: jobId, doc)
                } else {
                    var ph = rollouts[jobId] ?? JobRollout.placeholder(
                        jobRow(jobId) ?? Job(id: jobId, title: "", state: .running, step: "", elapsedS: 0), parentName: "")
                    ph.unavailable = true
                    applyRollout(jobId: jobId, ph, replace: true)
                }
            } catch {
                // Keep the last document on a blip.
            }
        }
    }

    /// A job document arrived: the tab's transcript is rebuilt from it.
    func applyRollout(jobId: String, _ doc: JobRollout, replace: Bool = false) {
        if replace || rollouts[jobId] == nil { rollouts[jobId] = doc } else { rollouts[jobId]?.merge(doc) }
        guard let held = rollouts[jobId] else { return }
        let chatId = JobTabs.chatId(jobId)
        let list = JobTabs.messages(held, pending: pendingSteers[jobId] ?? [])
        if let projectId = jobProject(jobId), var tabs = jobTabs[projectId], let i = tabs.firstIndex(where: { $0.id == chatId }) {
            if !held.title.trimmingCharacters(in: .whitespaces).isEmpty { tabs[i].title = held.title }
            if let last = list.map(\.createdAt).max() { tabs[i].lastMessageAt = max(tabs[i].lastMessageAt, last) }
            if tabs != jobTabs[projectId] { jobTabs[projectId] = tabs }
        }
        messages[chatId] = list
    }

    /// Typing in a job tab steers the job: the words go to the parent chat as a reply under
    /// the job's ask (the Steer button's wire) and show in the tab at once.
    private func steerFromJobTab(chatId: Id, text: String) {
        guard let jobId = JobTabs.jobId(chatId) else { return }
        let ask = rollouts[jobId]?.askMessageId ?? jobRow(jobId)?.askMessageId
        guard let parent = jobParentChat(jobId), let ask else {
            failRefused("steer the task", "This task cannot be steered from here yet — reply to your request in the chat instead.")
            return
        }
        let messageId = newId()
        let echo = JobTabs.typedSteer(chat: chatId, messageId: messageId, text: text, ask: ask)
        pendingSteers[jobId, default: []].append(echo)
        messages[chatId, default: []].append(echo)
        // The parent chat carries the real message: a reply under the ask (its own echo
        // comes from `write`).
        write(chatId: parent, text: text, command: "send", attachments: [], messageId: messageId, replyTo: ask)
    }

    /// Fixture: the scripted exchange of a seeded job, advanced by the clock so a tab streams.
    private func fixtureAdvanceRollout(_ jobId: String) {
        guard let job = jobRow(jobId) ?? rollouts[jobId].map({ Job(id: $0.id, title: $0.title, state: $0.state, step: $0.step, elapsedS: $0.elapsedS, askMessageId: $0.askMessageId, chatId: $0.chatId) }) else { return }
        let start = nowMs() - Int64(job.elapsedS) * 1000 - 1
        let (brief, script, result): (String, [(Int64, RolloutKind, String, Int64)], String) = {
            switch job.title {
            case "Searching for surf schools":
                return ("Find the 3 best-reviewed surf schools near Santa Teresa, compare group and private lesson prices, and say which is the best value.",
                        [(3, .text, "On it — pulling ratings and current prices for the top schools around Santa Teresa.", 0),
                         (6, .tool, "Searched the web for “surf schools Santa Teresa reviews 2026”", 2100),
                         (9, .tool, "Read tripadvisor.com — Santa Teresa surf schools", 4800),
                         (15, .tool, "Read bluemysticsurf.com/prices", 3000),
                         (19, .tool, "Reading delsoulsurf.com", 2600),
                         (28, .text, "Three schools have enough reviews to compare; checking the lesson pages.", 0),
                         (40, .tool, "Comparing prices across the three", 1800)],
                        "Blue Mystic is the best value for single lessons ($60–70 group, $90 private); Del Soul if you take above five; Monkey Adventures runs $75 / $115.")
            case "Fixing the flaky tests":
                return ("The relay's websocket tests fail one run in five on CI. Find the race and fix it on a branch.",
                        [(4, .text, "Reading the failing runs first.", 0), (8, .tool, "Read .github/workflows/ci.yml", 1200),
                         (12, .tool, "Ran cargo test -p arbos-relay ws:: (3×)", 48_000), (70, .tool, "Read crates/arbos-relay/src/ws.rs", 2100),
                         (80, .text, "The ping timer races the close frame; adding the join before the shutdown.", 0),
                         (100, .tool, "Running cargo test -p arbos-relay (1 of 3)", 45_000)],
                        "Fixed: the ping timer raced the close frame. Three green runs on fix/ws-close-race.")
            default:
                return ("Rename the config module to settings across the crate.", [], "")
            }
        }()
        let done = DebugHooks.fixtureRolloutDone && job.title == "Searching for surf schools"
        let now = nowMs()
        var entries = [RolloutEntry(seq: 1, at: start, kind: .prompt, text: brief)]
        var seq: Int64 = 2
        var calls = 0
        for (offset, kind, words, dur) in script {
            let at = start + offset * 1000
            if !done, at > now { break }
            let finished = done || at + dur <= now
            if kind == .tool { calls += 1 }
            entries.append(RolloutEntry(seq: seq, at: at, kind: kind, text: kind == .tool ? nil : words,
                                        label: kind == .tool ? words : nil, name: kind == .tool ? "tool" : nil,
                                        status: kind == .tool ? (finished ? "done" : "running") : nil,
                                        durationMs: kind == .tool && finished ? dur : nil))
            seq += 1
        }
        if done {
            entries.append(RolloutEntry(seq: seq, at: start + 372_000, kind: .result, text: result, status: "done", durationMs: 372_000))
            seq += 1
        }
        let doc = JobRollout(id: job.id, title: job.title, kind: "research", state: job.state, step: job.step,
                             elapsedS: done ? 372 : Int((now - start) / 1000), startedAt: start, endedAt: done ? start + 372_000 : nil,
                             chatId: job.chatId, askMessageId: job.askMessageId,
                             parentProjectId: jobProject(jobId) ?? "", parentName: project(jobProject(jobId) ?? "")?.name ?? "",
                             brief: brief, entries: entries, toolCalls: calls, nextSeq: seq)
        applyRollout(jobId: jobId, doc, replace: true)
    }

    /// A background job's row still runs (duplex §2.3): jobs are independent of the turn,
    /// so a newer user line does not end one — only the box finishing the row does, or
    /// the freshness window (a dead box).
    func hasLiveJob(_ chatId: Id) -> Bool {
        let recent = nowMs() - 15 * 60 * 1000
        return messages(of: chatId).contains { $0.role == .assistant && $0.job != nil && $0.isStreaming && $0.createdAt >= recent }
    }

    /// Poll `GET /projects/{id}` for the open chat's jobs while a job row is live or the relay
    /// listed one last time; called from the chat view's one-second tick. Transport errors keep
    /// the last answer rather than blinking the list away.
    func pollJobsIfNeeded(chatId: Id) {
        guard !isFixture else { return }
        let projects = jobsProjects(for: chatId)
        guard !projects.isEmpty else { return }
        let listed = projects.contains { !(jobs[$0] ?? []).isEmpty }
        guard listed || hasLiveJob(chatId) else { return }
        let now = nowMs()
        for projectId in projects {
            guard now - (jobsPolledAt[projectId] ?? 0) >= AppModel.jobsPollMs, !jobsPolling.contains(projectId) else { continue }
            jobsPolledAt[projectId] = now
            jobsPolling.insert(projectId)
            let api = self.api
            Task { @MainActor [weak self] in
                defer { self?.jobsPolling.remove(projectId) }
                guard let fresh = try? await api.projectJobs(projectId: projectId) else { return }
                self?.applyJobs(projectId: projectId, fresh)
            }
        }
    }

    func applyJobs(projectId: Id, _ fresh: [Job]) {
        if fresh.isEmpty {
            if jobs[projectId] != nil { jobs.removeValue(forKey: projectId) }
        } else if jobs[projectId] != fresh {
            jobs[projectId] = fresh
        }
        syncJobTabs(projectId: projectId)
    }

    /// `stop { jobId }`: that job only. The row leaves the sheet at once; the next poll is
    /// the box's word.
    func stopJob(chatId: Id, jobId: String) {
        if let chat = chat(chatId), var list = jobs[chat.projectId] {
            list.removeAll { $0.id == jobId }
            applyJobs(projectId: chat.projectId, list)
        }
        guard let doc = docs[chatId] else { return }
        let messageId = messages(of: chatId).last { $0.role == .user }?.id ?? ""
        try? doc.pushCommand("stop", payload: ["jobId": jobId, "messageId": messageId], device: deviceId)
        doc.commit()
    }

    /// The Stop button / Stop all halts every job of the chat (D7): forget them at once.
    private func forgetJobs(chatId: Id) {
        guard let chat = chat(chatId), let list = jobs[chat.projectId] else { return }
        applyJobs(projectId: chat.projectId, list.filter { $0.chatId != nil && $0.chatId != chatId })
    }

    func retry(chatId: Id, messageId: Id) {
        guard let doc = docs[chatId] else { return }
        try? doc.pushCommand("retry", payload: ["messageId": messageId], device: deviceId)
        doc.commit()
    }

    private func retitleLocally(chatId: Id, title: String) {
        for (pid, list) in chats {
            if let i = list.firstIndex(where: { $0.id == chatId }) {
                var updated = list
                updated[i].title = title
                chats[pid] = updated
                return
            }
        }
    }

    static func title(from text: String) -> String {
        let line = text.split(separator: "\n", omittingEmptySubsequences: true).first.map(String.init) ?? ""
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        if trimmed.isEmpty { return "New chat" }
        if trimmed.count > 34 { return String(trimmed.prefix(34)) + "…" }
        return trimmed
    }

    /// A ChatGPT-style subject for a chat: the auto-title from the first message,
    /// when it is more than the placeholder or the agent's own name. Nil → the
    /// row should keep showing the agent name.
    static func chatSubject(_ title: String, agentName: String) -> String? {
        let t = title.trimmingCharacters(in: .whitespacesAndNewlines)
        if t.isEmpty || t == "New chat" || t == "Main" { return nil }
        if t.caseInsensitiveCompare(agentName) == .orderedSame { return nil }
        return t
    }

    /// Subject of this agent's one chat on this device, when it has one.
    func chatSubject(of projectId: Id) -> String? {
        guard let project = project(projectId),
              let chat = singleChat(of: projectId) else { return nil }
        return AppModel.chatSubject(chat.title, agentName: project.name)
    }

    func wake(projectId: Id, reason: String) {
        rooms[projectDocKey(projectId)]?.wake(reason: reason)
        setProjectState(projectId, .waking, detail: nil)
        machine.state = machine.coerced(.waking)
        machine.since = nowMs()
        Task {
            do {
                try await api.wake(projectId: projectId)
            } catch {
                fail("wake the machine", error)
                setProjectState(projectId, .error, detail: UserError.classify("wake the machine", error).sentence)
                machine.state = .error
            }
            await refreshMachine()
        }
    }

    /// Open a new chat at once (Mac `new_tab`): local id + empty transcript,
    /// no spinner. The relay create runs in the background and the id is
    /// swapped when it lands. Pre-wakes a sleeping box so the first question
    /// does not wait on a cold start.
    @discardableResult
    func startNewChat(projectId: Id) -> Chat {
        let localId = newId()
        let chat = Chat(id: localId, projectId: projectId, title: "New chat",
                        isPinned: false, unread: false, lastMessageAt: nowMs())
        var list = chats[projectId] ?? []
        // Right after the pinned "Main" chat, same strip order as Mac.
        let at = min((list.firstIndex(where: { $0.isPinned }).map { $0 + 1 } ?? 0), list.count)
        list.insert(chat, at: at)
        chats[projectId] = list
        messages[localId] = []
        caughtUp.insert(localId)
        optimisticChatIds.insert(localId)
        if let p = project(projectId), p.state.isAsleep || p.state == .error {
            wake(projectId: projectId, reason: "new-chat")
        }
        open(chat: chat)
        Task { await finalizeNewChat(localId: localId, projectId: projectId) }
        return chat
    }

    /// Returns the new chat once the relay has created it (legacy awaiters).
    func newChat(projectId: Id) async -> Chat? {
        let local = startNewChat(projectId: projectId)
        let localId = local.id
        for _ in 0..<100 {
            if let realId = optimisticRemap[localId], let c = chat(realId) {
                optimisticRemap.removeValue(forKey: localId)
                return c
            }
            if !optimisticChatIds.contains(localId), optimisticRemap[localId] == nil {
                return nil // abandoned
            }
            try? await Task.sleep(nanoseconds: 50_000_000)
        }
        return optimisticRemap[localId].flatMap(chat) ?? chat(localId)
    }

    private func finalizeNewChat(localId: Id, projectId: Id) async {
        do {
            let real = try await api.createChat(projectId: projectId)
            adoptOptimisticChat(localId: localId, real: real)
        } catch {
            abandonOptimisticChat(localId: localId, projectId: projectId,
                                  reason: UserError.classify("start the chat", error).raw)
        }
    }

    private func adoptOptimisticChat(localId: Id, real: Chat) {
        optimisticChatIds.remove(localId)
        optimisticRemap[localId] = real.id
        var list = chats[real.projectId] ?? []
        if let i = list.firstIndex(where: { $0.id == localId }) {
            // Keep any title the user already typed into.
            var adopted = real
            if list[i].title != "New chat" { adopted.title = list[i].title }
            list[i] = adopted
        } else if !list.contains(where: { $0.id == real.id }) {
            list.append(real)
        }
        chats[real.projectId] = AppModel.sorted(list)

        // Copy under the real id first; keep the local key until path swaps so
        // an in-flight ChatView does not flash an empty transcript mid-adopt.
        let pending = messages[localId] ?? []
        messages[real.id] = pending.map { m in
            var copy = m
            copy.chatId = real.id
            return copy
        }
        if let u = unacked[localId] { unacked[real.id] = u }
        caughtUp.insert(real.id)
        if singleChatIds[real.projectId] == localId { setSingleChat(real.id, of: real.projectId) }

        // Room (and doc) before the navigation swap so a remounted ChatView can send.
        ensureRoom(projectId: real.projectId, key: real.id, roomChat: real.id)

        let queued = optimisticSends.removeValue(forKey: localId) ?? []
        // Keep the echoed bubbles on screen; write them into the real doc with
        // the same ids so emitSnapshot replaces them in place (no flicker).
        for item in queued {
            write(chatId: real.id, text: item.text, command: item.command,
                  attachments: item.attachments, extra: item.extra, messageId: item.messageId, replyTo: item.replyTo,
                  forwardedFrom: item.forwardedFrom)
        }

        // Swap the navigation id for the real one, then drop the local mirrors.
        if case .chat(let id, let pid)? = path.last, id == localId {
            path[path.count - 1] = .chat(chatId: real.id, projectId: pid)
        }
        if visibleChatId == localId { visibleChatId = real.id }
        messages[localId] = nil
        unacked[localId] = nil
        caughtUp.remove(localId)
    }

    private func abandonOptimisticChat(localId: Id, projectId: Id, reason: String) {
        optimisticChatIds.remove(localId)
        optimisticSends[localId] = nil
        unacked[localId] = nil
        messages[localId] = nil
        caughtUp.remove(localId)
        chats[projectId] = (chats[projectId] ?? []).filter { $0.id != localId }
        if case .chat(let id, _)? = path.last, id == localId {
            path.removeLast()
        }
        if visibleChatId == localId { visibleChatId = nil }
        fail(UserError.fromText("start the chat", reason))
    }

    /// Whether to PUT /read now, skip (same value already sent), or wait out the
    /// throttle window and send the latest value then.
    enum ReadPutPlan: Equatable {
        case send
        case skip
        case delay(seconds: TimeInterval)
    }

    /// Pure helper for mark-read PUT coalescing: unchanged → skip (unless `force`);
    /// new or forced value after the window → send; inside the window → delay.
    nonisolated static func readPutPlan(
        lastSent: Int64?,
        lastSentAt: Date?,
        value: Int64,
        now: Date,
        force: Bool = false
    ) -> ReadPutPlan {
        if lastSent == value && !force { return .skip }
        if let lastSentAt {
            let elapsed = now.timeIntervalSince(lastSentAt)
            if elapsed < readPutThrottleSeconds {
                return .delay(seconds: readPutThrottleSeconds - elapsed)
            }
        }
        return .send
    }

    /// Queue or send PUT /chats/{id}/read for `value`, using `readPutPlan`.
    /// With `force`, an unchanged value still sends (outside the window) or stays
    /// pending through a delay (inside it) so the relay can clear late unread events.
    private func scheduleReadPut(chatId: Id, value: Int64, force: Bool = false, now: Date = Date()) {
        switch Self.readPutPlan(
            lastSent: lastReadPutValue[chatId],
            lastSentAt: lastReadPutAt[chatId],
            value: value,
            now: now,
            force: force
        ) {
        case .skip:
            return
        case .send:
            readPutTasks[chatId]?.cancel()
            readPutTasks[chatId] = nil
            pendingReadPutValue[chatId] = nil
            performReadPut(chatId: chatId, value: value, at: now)
        case .delay(let seconds):
            // Forced re-PUT of the same cursor still keeps a pending send.
            pendingReadPutValue[chatId] = value
            guard readPutTasks[chatId] == nil else { return }
            readPutTasks[chatId] = Task { [weak self] in
                let ms = max(1, Int((seconds * 1000).rounded()))
                try? await Task.sleep(for: .milliseconds(ms))
                guard !Task.isCancelled, let self else { return }
                self.readPutTasks[chatId] = nil
                let latest = self.pendingReadPutValue[chatId] ?? value
                self.pendingReadPutValue[chatId] = nil
                self.performReadPut(chatId: chatId, value: latest, at: Date())
            }
        }
    }

    private func performReadPut(chatId: Id, value: Int64, at: Date) {
        lastReadPutValue[chatId] = value
        lastReadPutAt[chatId] = at
        let api = self.api
        Task { try? await api.markRead(chatId: chatId, seq: value) }
    }

    /// Clear a project chat's unread dot. Local unread/badge clear is immediate;
    /// the PUT is skipped when unchanged and throttled to one per chat per
    /// `readPutThrottleSeconds` (notify can still record events while on screen).
    /// Pass `force` to re-PUT the same cursor (e.g. onAppear / foreground) so the
    /// relay clears unread events recorded after the last PUT.
    func markRead(chatId: Id, force: Bool = false) {
        if JobTabs.isJobChat(chatId) { return }
        let hadRemote = (unreadCounts[chatId] ?? 0) > 0
        if unreadCounts[chatId] != nil { unreadCounts[chatId] = 0 }
        PushManager.shared.clearDelivered(chatId: chatId)
        var hadLocal = false
        for (pid, list) in chats {
            if let i = list.firstIndex(where: { $0.id == chatId }), list[i].unread {
                var updated = list
                updated[i].unread = false
                chats[pid] = updated
                hadLocal = true
            }
        }
        if hadRemote || hadLocal { applyBadge() }
        // §7c: PUT /chats/{id}/read takes project chat ids too; the relay
        // clears the user's events for it and badges their other devices.
        if !isFixture, chat(chatId) != nil {
            let seq = Int64(messages(of: chatId).count)
            scheduleReadPut(chatId: chatId, value: seq, force: force || hadRemote || hadLocal)
        }
    }

    func setModel(projectId: Id, model: String, effort: Effort) {
        if let doc = docs[projectDocKey(projectId)] {
            try? doc.setModelSettings(model: model, effort: effort)
            doc.commit()
        }
        if let i = projects.firstIndex(where: { $0.id == projectId }) {
            projects[i].model = model
            projects[i].effort = effort
        }
        Task {
            do {
                try await api.setModel(projectId: projectId, model: model, effort: effort)
            } catch {
                fail("change the model", error)
            }
        }
    }

    func createProject(name: String, avatarColorIndex: Int? = nil, leaf: LeafPick? = nil, picture: DraftPicture? = nil) {
        Task {
            do {
                let id = try await api.createProject(name: name, avatarColorIndex: avatarColorIndex, leaf: leaf)
                // The Create sheet's picture goes into the new agent's own slot first, so the
                // list refresh already shows it.
                if let picture, let id, let jpeg = await draftJpeg(picture) {
                    let key = "avatar-\(Int(Date().timeIntervalSince1970 * 1000)).\(PictureMaster.ext(jpeg))"
                    if (try? await api.putBlob(projectId: id, key: key, mime: PictureMaster.mime(jpeg), data: jpeg)) != nil {
                        _ = try? await api.setProjectImageKey(id, key: key)
                    }
                }
                await refreshRegistry()
            } catch {
                fail("create the agent", error)
            }
        }
    }

    // MARK: Create sheet pictures (Jacob, 2026-10-25)

    /// Generate before the entity exists: into my own slot (`POST /me/avatar/generate`, #1789).
    func generateDraftAvatar(prompt: String) async throws -> String {
        if isFixture { return try await Self.fixtureGenerate(prompt) }
        do {
            return try await api.generateMyAvatar(prompt: prompt)
        } catch let e as RelayError where e.status == 404 {
            // A relay without the own-slot route yet: say so instead of "HTTP 404".
            throw RelayError(status: 404, message: "Generate is not on this relay yet; upload a picture instead.")
        }
    }

    /// Where a draft's generated picture is fetched from: `users:{me}`.
    func draftPictureURL(key: String) -> URL? {
        guard let me = user?.id else { return nil }
        return blobURL(projectId: "users:\(me)", key: key)
    }

    /// The Create sheet's picture as JPEG bytes for the new entity's slot: the photo as chosen, or
    /// the generated picture read back from my slot. nil when it cannot be read (a notice says so;
    /// the entity keeps its neutral initial).
    func draftJpeg(_ picture: DraftPicture) async -> Data? {
        switch picture {
        case .jpeg(let data):
            return data
        case .generated(let key):
            guard let me = user?.id else { return nil }
            do {
                return try await api.blobData(projectId: "users:\(me)", key: key)
            } catch {
                fail("keep the generated picture", error)
                return nil
            }
        }
    }

    /// Grok Bot "Duplicate": a new top-level agent with the same profile, look,
    /// model, instructions, and workspace context (files + memory) — not the
    /// chat transcript. Own agents only.
    func duplicateProject(_ project: Project) {
        guard !project.shared else { return }
        let name = uniqueCopyName(of: project.name)
        let modelId = project.model
        let effort = project.effort
        let sourceId = project.id
        Task {
            do {
                // Do not pass `models` on create: a partial map would drop the
                // relay's default subagent/decision slots. Set the talking
                // model after the copy exists. `sourceProjectId` asks the
                // relay to copy the source agent's project folder and memory.
                _ = try await api.createProject(
                    name: name,
                    leaf: LeafArt.isNeutral(project.avatarLeaf) ? LeafArt.neutralPick : LeafArt.pick(project),
                    color: project.color,
                    description: project.description,
                    instructions: project.instructions,
                    sourceProjectId: sourceId
                )
                await refreshRegistry()
                if let copy = projects.first(where: { $0.name == name && !$0.shared }) {
                    if copy.model != modelId || copy.effort != effort {
                        setModel(projectId: copy.id, model: modelId, effort: effort)
                    }
                    open(project: copy)
                }
            } catch {
                fail("duplicate the agent", error)
            }
        }
    }

    /// Soft-delete an owned agent from every list (`DELETE /projects/{id}`).
    func deleteProject(_ projectId: Id) {
        guard let p = project(projectId), !p.shared else { return }
        if path.contains(where: {
            if case .chat(_, let pid) = $0 { return pid == projectId }
            return false
        }) {
            path.removeAll()
        }
        // Stop this project's chat rooms and its project-doc room at once so
        // they do not keep dialing after the lists drop them.
        var keys = Set((chats[projectId] ?? []).map(\.id))
        keys.insert(projectDocKey(projectId))
        for (key, room) in rooms where room.projectId == projectId {
            keys.insert(key)
        }
        for key in keys { wipeRoom(key) }
        projects.removeAll { $0.id == projectId }
        chats[projectId] = nil
        if archivedProjectIds.contains(projectId) {
            archivedProjectIds.remove(projectId)
            UserDefaults.standard.set(Array(archivedProjectIds), forKey: "archivedProjects")
        }
        unpinHome(projectId: projectId)
        Task {
            do {
                try await api.deleteProject(projectId)
                await refreshRegistry()
            } catch {
                fail("delete it", error)
                await refreshRegistry()
            }
        }
    }

    /// "`Name` copy", then "`Name` copy 2"… so the relay's unique-name check passes.
    private func uniqueCopyName(of name: String) -> String {
        let owned = Set(projects.filter { !$0.shared }.map(\.name))
        let base = "\(name) copy"
        if !owned.contains(base) { return base }
        for i in 2...99 {
            let candidate = "\(name) copy \(i)"
            if !owned.contains(candidate) { return candidate }
        }
        return "\(name) copy \(String(UUID().uuidString.prefix(4)))"
    }
}

// MARK: - App updates (continuous delivery, D85)

/// A TestFlight build newer than the one running, as the relay reports it (`GET /releases/current`,
/// fed by ios.yml after the build is distributed to the tester group).
struct AvailableBuild: Equatable {
    let version: String
    let build: String
    let url: String

    /// TestFlight's own page for Arbos; the public invite link is the fallback when TestFlight
    /// is not installed.
    static let testFlightAppURL = URL(string: "itms-beta://beta.itunes.apple.com/v1/app/6812503407")!
    static let testFlightInviteURL = URL(string: "https://testflight.apple.com/join/ec5fWCuJ")!
}

extension AppModel {
    /// The build number this app runs (`CFBundleVersion`, UTC `YYYYMMDDHHMM…` from ios.yml).
    static var runningBuild: String {
        Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "0"
    }

    private static let dismissedBuildKey = "update.dismissedBuild"

    /// Numeric build comparison; a non-numeric build (Xcode's "1") never counts as newer.
    static func isNewerBuild(_ candidate: String, than running: String) -> Bool {
        guard let c = UInt64(candidate), let r = UInt64(running) else { return false }
        return c > r
    }

    /// `GET /releases/current` → the iOS row; sets `availableUpdate` when its build is newer than
    /// ours and the user did not dismiss that build. Called when the app becomes active and on
    /// the relay's `release_published` push.
    func checkForUpdate() async {
        guard credentials != nil, !isFixture else { return }
        guard let v = try? await api.json("GET", "/releases/current"),
              JSONSerialization.isValidJSONObject(v),
              let data = try? JSONSerialization.data(withJSONObject: v),
              let dto = try? JSONDecoder().decode(ReleasesDto.self, from: data) else { return }
        guard let ios = dto.releases.first(where: { $0.platform == "ios" }),
              !ios.build.isEmpty else { return }
        let dismissed = UserDefaults.standard.string(forKey: Self.dismissedBuildKey)
        if Self.isNewerBuild(ios.build, than: Self.runningBuild), dismissed != ios.build {
            let next = AvailableBuild(version: ios.version, build: ios.build, url: ios.url)
            if availableUpdate != next { availableUpdate = next }
        } else if availableUpdate != nil {
            availableUpdate = nil
        }
    }

    /// "Update": open TestFlight on Arbos (the invite link when TestFlight is missing).
    func openTestFlight() {
        Task { @MainActor in
            let opened = await UIApplication.shared.open(AvailableBuild.testFlightAppURL)
            if !opened { _ = await UIApplication.shared.open(AvailableBuild.testFlightInviteURL) }
        }
    }

    /// "Later": hide the card for this build; the next build shows it again.
    func dismissUpdate() {
        if let b = availableUpdate?.build { UserDefaults.standard.set(b, forKey: Self.dismissedBuildKey) }
        availableUpdate = nil
    }
}
