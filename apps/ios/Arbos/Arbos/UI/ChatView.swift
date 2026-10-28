// One conversation, Grok Bot layout: floating glass header (back circle,
// avatar + name pill), messages scrolling underneath with a top
// fade, "<Project> is working" while a turn runs, and the composer ("+",
// "Ask <Project>" field with mic, white call button) floating above the
// keyboard.

import ArbosFFI
import PhotosUI
import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// Long-press → Forward payload for the chat picker sheet.
private struct PendingForward: Identifiable, Equatable {
    let id = UUID()
    let text: String
    let from: String
}

/// Long-press → View thread payload for the reply-chain sheet.
private struct PendingThread: Identifiable, Equatable {
    var id: Id { root.id }
    let root: Message
    let replies: [Message]
}


struct ChatView: View {
    @EnvironmentObject private var model: AppModel
    let chatId: Id
    let projectId: Id

    var body: some View {
        ChatSessionHost(model: model, chatId: chatId, projectId: projectId)
    }

    /// Photos leave the phone as JPEG or PNG no wider than 2048 px: the models behind the
    /// agent read those; a 12 MB HEIC straight from the camera they do not. Videos and
    /// anything that is not an image pass through unchanged.
    static func imageForUpload(_ data: Data, type: UTType?) -> (data: Data, mime: String, ext: String) {
        let original = (data: data, mime: type?.preferredMIMEType ?? "image/jpeg", ext: type?.preferredFilenameExtension ?? "jpg")
        if let type, type.conforms(to: .movie) || type.conforms(to: .video) { return original }
        guard let image = UIImage(data: data) else { return original }
        let longest = max(image.size.width, image.size.height) * image.scale
        let isWebFormat = type.map { $0.conforms(to: .jpeg) || $0.conforms(to: .png) } ?? false
        if isWebFormat && longest <= 2048 && data.count <= 4 * 1024 * 1024 { return original }
        let scale = min(1, 2048 / max(longest, 1))
        let size = CGSize(width: (image.size.width * scale).rounded(), height: (image.size.height * scale).rounded())
        let format = UIGraphicsImageRendererFormat.default()
        format.scale = 1
        let drawn = UIGraphicsImageRenderer(size: size, format: format).image { _ in image.draw(in: CGRect(origin: .zero, size: size)) }
        guard let jpeg = drawn.jpegData(compressionQuality: 0.85) else { return original }
        return (jpeg, "image/jpeg", "jpg")
    }

    /// Fixed-locale stamp for photo attachment names (relay blob keys must be ASCII).
    static let photoStamp: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd HH.mm.ss"
        return f
    }()
}

/// Forwards only chat-scoped `AppModel` publishes into `ChatSession`. Unrelated
/// fields (credits, devices, goals, …) no longer rebuild the open chat.
///
/// `tick` is handed to the session as a stored input on purpose: SwiftUI re-runs a
/// child's body only when its inputs differ, and `model` / `chatId` / `projectId`
/// never do. Re-rendering this host alone left `ChatSession.body` — and with it
/// `.onChange(of: messagesVersion[chatId])`, which rebuilds the rows — unevaluated,
/// so a sent bubble sat in `messages` until some unrelated state flipped (a scroll,
/// the keyboard, a presence tick, the next inbound row): Jacob's 5-second own
/// message in groups, Doug's "appears when I scroll" (#1720), measured per hop in
/// iphone-group-send-instant-2026-10-25.
private struct ChatSessionHost: View {
    let model: AppModel
    let chatId: Id
    let projectId: Id
    @State private var tick = 0

    var body: some View {
        #if DEBUG
        let _ = SendTrace.recent ? SendTrace.mark("host-body", detail: "tick=\(tick)", once: false) : ()
        #endif
        ChatSession(model: model, chatId: chatId, projectId: projectId, tick: tick)
            .onReceive(model.$messages) { _ in tick &+= 1 }
            .onReceive(model.$messagesVersion) { _ in tick &+= 1 }
            .onReceive(model.$channelRows) { _ in tick &+= 1 }
            .onReceive(model.$caughtUp) { _ in tick &+= 1 }
            .onReceive(model.$typing) { _ in tick &+= 1 }
            .onReceive(model.$peopleChats) { _ in tick &+= 1 }
            .onReceive(model.$machine) { _ in tick &+= 1 }
            .onReceive(model.$roomLinks) { _ in tick &+= 1 }
            .onReceive(model.$relayLink) { _ in tick &+= 1 }
            .onReceive(model.$linkPillRepaint) { _ in tick &+= 1 }
            .onReceive(model.$projects) { _ in tick &+= 1 }
            .onReceive(model.$directory) { _ in tick &+= 1 }
            .onReceive(model.$servers) { _ in tick &+= 1 }
            .onReceive(model.$pendingSearchChat) { _ in tick &+= 1 }
            .onReceive(model.$user) { _ in tick &+= 1 }
            .onReceive(model.$appearanceSetting) { _ in tick &+= 1 }
            .onReceive(model.$surfaceSetting) { _ in tick &+= 1 }
            .onReceive(model.$textScaleSetting) { _ in tick &+= 1 }
            .onReceive(model.$firstLogin) { _ in tick &+= 1 }
            .onReceive(model.$chats) { _ in tick &+= 1 }
            .onReceive(model.$mutedChannelIds) { _ in tick &+= 1 }
            .onReceive(model.$models) { _ in tick &+= 1 }
            .onReceive(model.$notice) { _ in tick &+= 1 }
            .onReceive(model.$lastSeen) { _ in tick &+= 1 }
            .onReceive(model.$jobs) { _ in tick &+= 1 }
    }
}

/// One open conversation. Holds `model` as a plain reference (not
/// `@EnvironmentObject`) so only `ChatSessionHost`'s filtered publishes rebuild it.
/// Composer draft lives in `ComposerDraft` / `ComposerField` — keystrokes do not
/// re-evaluate this view (arbonites-chat-lag + feedback app-01M5NNJ001NHHYBHXTASM5NXBZ).
struct ChatSession: View {
    let model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let chatId: Id
    let projectId: Id
    /// The host's publish counter. Not read in `body`; it is here so a forwarded
    /// publish changes this view's inputs and SwiftUI evaluates the body (and its
    /// `onChange` modifiers) again — see `ChatSessionHost`.
    let tick: Int

    /// The composer text lives in its own observable so a keystroke re-renders the
    /// field, not this screen. Read it as `composerDraft.text` for actions; the
    /// field observes it for display.
    @State private var composerDraft = ComposerDraft()
    /// `AppModel.addressesEveryone(text)` for the current draft, updated only when it flips.
    @State private var draftAddressesEveryone = false
    @State private var rows: [Row] = []
    /// The message list the current `rows` were built from (last id / count checks).
    @State private var rowsSource: [Message] = []
    /// Bumped by every `refreshRows`; with the window bounds it identifies `shown`
    /// without comparing the row array (that compare ran once per row per change).
    @State private var rowsVersion = 0
    /// Message ids that `rows` / `messageById` currently mirror — used to skip a full
    /// `Transcript.rows` rebuild on streaming deltas and single appends.
    @State private var refreshIds: [Id] = []
    /// id → message for the rows on screen (menus, search, replies); rebuilt with `rows`.
    @State private var messageById: [Id: Message] = [:]
    /// How many of the newest rows are in the view tree. The transcript is a plain
    /// VStack (see `transcript`), so every row it holds is built and laid out at once;
    /// a long group chat put thousands in it and the open took seconds (long-chat-perf,
    /// 2026-10-23). Cap stays modest so finger-scrolling stays smooth; older rows join
    /// as the user nears the top (`growWindow`).
    @State private var windowCount = ChatSession.windowInitial
    /// Same initial window on iOS 17 — PreferenceKey geometry near the top grows it
    /// (feedback app-01M5NNJ001NHHYBHXTASM5NXBZ). iOS 18+ uses onScrollGeometryChange.
    static let windowInitial: Int = 80
    /// Rows added per step. Small on purpose: a step builds and lays out this many
    /// bubbles in one frame (≈4 ms each in a Debug simulator build).
    static let windowStep = 30
    /// Most rows the view tree holds while reading history. Past this, each step
    /// that adds older rows at the top also drops the same number of newest rows
    /// below the viewport, and the first row on screen is re-pinned, so the cost
    /// of a page of history stays flat however far back the reader goes
    /// (arbonites-chat-lag: 0.3–0.8 s frames and 600 MB at 600+ rows).
    static let windowMax = 360
    /// Newest rows dropped below the viewport while reading history; 0 when the
    /// window is the tail of the transcript.
    @State private var windowSkipEnd = 0
    @State private var showModelPicker = false
    /// Rig: the media viewer opened by `ARBOS_SHOW=media…` (media-viewer-2026-10-23).
    @State private var rigViewer: MediaViewerState?
    @State private var rigViewerForward = false
    @State private var showSettings = false
    /// Rig: `ARBOS_SHOW=agentsecrets` presents the agent's Secrets page (D83).
    @State private var showAgentSecrets = false
    @State private var sentOnce = false
    @State private var transcribing = false
    @State private var pending: [AppModel.PendingAttachment] = []
    @State private var photoItem: PhotosPickerItem?
    @State private var showPhotos = false
    @State private var showFiles = false
    @State private var showAttach = false
    @State private var scroller: ScrollViewProxy?
    /// Scrolled more than ~120 pt above the end: show Grok's round chevron
    /// (or the new-messages pill when rows landed below).
    @State private var awayFromBottom = false
    /// Follow the end as rows stream / grow. Cleared when the user scrolls up;
    /// restored when they return near the end or tap the chevron / pill (Mac stick pin).
    @State private var pinnedToBottom = true
    /// When this chat screen appeared (the history window does not grow in its first second).
    @State private var appearedAt = Date()
    /// Rows that arrived while the reader was away from the end — drives the
    /// "New messages" pill. Cleared when they re-pin or jump to the latest.
    @State private var newBelowCount = 0
    @State private var demoRan = false
    @State private var demoQueue: [String] = []
    @StateObject private var recorder = MemoRecorder()
    /// Finger is down on the mic (hold-to-talk or click-to-toggle).
    @State private var memoHoldActive = false
    /// Second tap while recording: stop on press and ignore the matching release.
    @State private var memoIgnoreRelease = false
    /// Hold ended before mic permission returned; finish once recording starts.
    @State private var memoFinishWhenPermission = false
    /// When the current mic press began (for hold-vs-tap).
    @State private var memoPressStartedAt: Date?
    /// Hold-to-talk: finger slid left far enough to discard on release.
    @State private var memoCancelArmed = false
    /// Hold-to-talk: horizontal drag (≤ 0) while the mic is pressed.
    @State private var memoDragX: CGFloat = 0
    /// The field (a `UITextView`, #1596) follows this both ways.
    @State private var composerFocused = false

    /// Hold-to-talk: slide the mic this far left to discard instead of send.
    private static let memoCancelDistance: CGFloat = 56

    @State private var showProfile = false
    /// Unblock asks first ([feedback blocked-state-ios-2026-10-26]).
    @State private var confirmUnblock = false
    @State private var showSearch = false
    @State private var searchText = ""
    /// Index into `searchMatches` for the up/down controls; −1 = none yet
    /// ([feedback app-01M5W4AVG30C4S2TPJ7T30CVJR]).
    @State private var searchMatchIndex = -1
    /// Group chats: pick members to call, or call everyone.
    @State private var showGroupCallPicker = false
    @FocusState private var searchFocused: Bool
    @ObservedObject private var calls = CallManager.shared
    /// iMessage-style: drag the transcript left to peek message times (0…`RowView.timeRevealWidth`).
    /// Driven by the transcript's one swipe recognizer (`TranscriptSwipeHost`).
    @State private var timeReveal: CGFloat = 0
    /// The reply swipe in progress: which row follows the finger and how far (same recognizer).
    @State private var replySwipe = ReplySwipe()
    /// Row frames in the transcript space, for the recognizer's row-by-location lookup.
    @State private var rowFrames = RowFrameIndex()
    /// Long-press → Forward: the text to send and who wrote it (Forward sheet).
    @State private var pendingForward: PendingForward?
    /// The tasks sheet (duplex §2.4): open while the user looks at the agent's jobs.
    @State private var showTasks = false
    @State private var pendingThread: PendingThread?

    /// Rows after the in-chat search filter (profile "search").
    private var visibleRows: [Row] {
        let q = searchText.trimmingCharacters(in: .whitespaces)
        guard showSearch, !q.isEmpty else { return rows }
        return rows.filter { m_text($0).localizedCaseInsensitiveContains(q) }
    }

    /// Matching rows for the Search-in-chat up/down buttons (oldest first).
    /// Same filter as `visibleRows` while a query is active.
    private var searchMatches: [Row] {
        let q = searchText.trimmingCharacters(in: .whitespaces)
        guard showSearch, !q.isEmpty else { return [] }
        return visibleRows
    }

    private var canStepSearchMatch: Bool { !searchMatches.isEmpty }

    static func shownStamp(_ rowsVersion: Int, _ windowCount: Int, _ windowSkipEnd: Int, _ query: String, _ searching: Bool) -> Int {
        var h = Hasher()
        h.combine(rowsVersion); h.combine(windowCount); h.combine(windowSkipEnd)
        if searching { h.combine(query) }
        return h.finalize()
    }

    /// The newest `windowCount` of `visibleRows`: what the VStack actually holds.
    private var windowedRows: [Row] {
        let all = visibleRows
        let kept = windowSkipEnd > 0 && windowSkipEnd < all.count ? all.dropLast(windowSkipEnd) : all[...]
        return kept.count > windowCount ? Array(kept.suffix(windowCount)) : Array(kept)
    }

    /// Nearing the top: bring the next older slice into the view tree. The scroll view is
    /// anchored at the bottom, so the rows on screen stay put while the content grows above.
    /// Never inside the caller's animation: an animated change to the row list makes
    /// SwiftUI diff the whole window per frame.
    private func growWindow() {
        let all = visibleRows
        let available = all.count - windowSkipEnd
        guard windowCount < available else { return }
        let step = min(Self.windowStep, available - windowCount)
        let firstShown = windowedRows.first?.id
        var t = Transaction()
        t.disablesAnimations = true
        withTransaction(t) {
            if windowCount + step > Self.windowMax {
                // Slide: the oldest shown rows stay, the same number of newest rows leave.
                windowSkipEnd += step
            } else {
                windowCount += step
            }
        }
        if windowCount + step > Self.windowMax, let firstShown, let proxy = scroller {
            // Rows left below the viewport and the bottom anchor would shift what is on
            // screen; put the row that was at the top back where it was.
            DispatchQueue.main.async { proxy.scrollTo(firstShown, anchor: .top) }
        }
    }

    /// Back at the tail (a send, the new-messages pill, settle): the window is the
    /// newest rows again.
    private func resetWindowToTail() {
        guard windowSkipEnd > 0 || windowCount > Self.windowMax else { return }
        var t = Transaction()
        t.disablesAnimations = true
        withTransaction(t) {
            windowSkipEnd = 0
            windowCount = min(windowCount, Self.windowMax)
        }
    }

    /// Make sure the row for `messageId` is in the window (quote taps into older history).
    /// Returns true when the window had to grow (the caller scrolls after the next layout).
    private func includeInWindow(messageId: Id) -> Bool {
        let all = visibleRows
        guard let ix = all.firstIndex(where: { $0.messageId == messageId }) else { return false }
        let needed = all.count - ix + Self.windowStep / 2
        guard needed > windowCount || windowSkipEnd > 0 else { return false }
        var t = Transaction()
        t.disablesAnimations = true
        withTransaction(t) {
            windowSkipEnd = 0
            windowCount = min(all.count, max(windowCount, needed))
        }
        return true
    }

    /// Who the CallKit card names: the other person in a DM, else the group/project.
    private var callTitle: String {
        if let people, people.isGroup {
            return people.title.isEmpty ? "Group" : people.title
        }
        if let c = counterpart { return c.displayName }
        return project?.name ?? people?.title ?? name
    }

    /// Header tap: DM → the other person; group / shared project → the group page; own agent → its page;
    /// server channel → the channel's page (#1317; the server's page is the channel list's header).
    private func openHeaderProfile() {
        model.toggleProfile(headerTarget)
    }

    /// What the header pill opens — and, tapped again while that page is up,
    /// closes (`ProfileToggle`, [feedback profile-pane-toggle-ios-2026-10-26]).
    private var headerTarget: ProfileTarget {
        if let channel {
            return .channel(channelId: channel.id)
        } else if let people {
            if !people.isGroup, let other = counterpart {
                return .user(ProfilePerson(other))
            }
            return .group(chatId: chatId, projectId: nil)
        } else if project?.shared == true {
            return .group(chatId: chatId, projectId: projectId)
        }
        return .agent(projectId: projectId)
    }

    /// The header pill's accessible label: "Open <name>'s profile" / "Close profile".
    private var headerLabel: String {
        let who: String
        switch headerTarget {
        case .user(let p): who = p.displayName
        case .group: who = people?.title.isEmpty == false ? (people?.title ?? name) : name
        case .agent: who = name
        case .channel: who = channel?.name ?? name
        case .server: who = name
        }
        return ProfileToggle.label(name: who, showing: model.profileShows(headerTarget))
    }

    /// Hardware ⌘F toggles the bar ([feedback iphone-chat-search-cmd-f-toggle-2026-10-27]):
    /// closed → open and focus the field; open → close it like ✕ / Esc.
    private func openSearchFromKeyboard() {
        let effect = ChatSearchShortcut.apply(open: showSearch)
        if !effect.open {
            closeSearch()
            return
        }
        if effect.animate {
            withAnimation(.easeOut(duration: 0.18)) { showSearch = effect.open }
        } else {
            showSearch = effect.open
        }
        if effect.focus {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { searchFocused = true }
        }
    }

    /// Esc or the ✕: the bar goes, the filter with it.
    private func closeSearch() {
        showSearch = false
        searchText = ""
        searchMatchIndex = -1
        searchFocused = false
    }

    /// Hardware-keyboard keys for the bar ([feedback iphone-chat-search-cmd-f-2026-10-26]).
    /// Zero-size buttons carry the shortcuts: the header's disc toggles on tap, but the
    /// Find key must only ever open or refocus the bar, and Esc closes it like the Mac's.
    private var searchKeyboardShortcuts: some View {
        ZStack {
            Button(action: openSearchFromKeyboard) { EmptyView() }
                .keyboardShortcut(ChatSearchShortcut.key, modifiers: ChatSearchShortcut.modifiers)
            if showSearch {
                Button(action: closeSearch) { EmptyView() }
                    .keyboardShortcut(.cancelAction)
            }
        }
        .frame(width: 0, height: 0)
        .opacity(0)
        .accessibilityHidden(true)
    }

    /// Profile "search" tile: raise the in-chat search bar once for this chat.
    private func openPendingSearchIfNeeded() {
        guard model.pendingSearchChat == chatId else { return }
        model.pendingSearchChat = nil
        showSearch = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { searchFocused = true }
    }

    /// Up (`delta` −1) / down (`delta` +1): jump to the previous or next row that
    /// contains the search keyword, wrapping at the ends. The first press lands on
    /// the newest (up) or oldest (down) match (`searchMatchIndex` starts at −1).
    private func stepSearchMatch(by delta: Int) {
        let matches = searchMatches
        guard !matches.isEmpty else { return }
        let next: Int
        if searchMatchIndex < 0 || searchMatchIndex >= matches.count {
            next = delta < 0 ? matches.count - 1 : 0
        } else {
            next = (searchMatchIndex + delta + matches.count) % matches.count
        }
        searchMatchIndex = next
        pinnedToBottom = false
        Haptics.pick()
        revealQuoted(matches[next].messageId)
    }

    /// Reset the match cursor when the query changes (next up/down picks fresh).
    private func syncSearchMatchIndex() {
        searchMatchIndex = -1
        if highlightedMessage != nil,
           !searchMatches.contains(where: { $0.messageId == highlightedMessage }) {
            highlightedMessage = nil
        }
    }

    /// Reply in DM landed here: arm the composer with the cross-chat reply (#1836).
    private func takePendingDMReplyIfNeeded() {
        guard let p = model.pendingDMReply, p.chatId == chatId else { return }
        model.pendingDMReply = nil
        replyTo = nil
        replyRef = p.ref
        composerFocused = true
    }

    /// A result from Home's search bar — or a cross-chat quote tap (Reply in DM, #1836) —
    /// opened this chat at a message: once the rows hold it, scroll there and tint it
    /// (the quote-tap path, `revealQuoted`). The appear settle scrolls to the end first;
    /// the jump follows a beat later.
    private func revealPendingIfNeeded() {
        guard let reveal = model.pendingReveal, reveal.chatId == chatId,
              rows.contains(where: { $0.messageId == reveal.messageId && $0.role != .system }) else { return }
        model.pendingReveal = nil
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) {
            pinnedToBottom = false
            revealQuoted(reveal.messageId)
        }
    }

    /// Tokenised blob URL for a custom group picture, if the chat has one.
    private func groupImageURL(_ people: PeopleChat) -> String? {
        guard let key = people.imageKey, !key.isEmpty else { return nil }
        return model.blobURL(projectId: "\(peopleSlot):\(people.id)", key: key)?.absoluteString
    }

    /// A call in this chat we are not in (the one we are in shows as the in-call pill instead).
    private var hasCallBar: Bool {
        guard let active = calls.activeByChat[chatId], !active.ended else { return false }
        return calls.call?.callId != active.callId
    }
    /// Our own call, collapsed into the header bar of this chat (voice; video floats as PiP).
    private var hasInCallBar: Bool {
        calls.isActive && !calls.isPresented && calls.call?.kind == .voice && calls.call?.chatId == chatId
    }
    @State private var chatNoteDetailsOpen = false
    private var headerExtra: CGFloat { (hasCallBar ? 52 : 0) + (hasInCallBar ? 56 : 0) + (showSearch ? Controls.height + 8 : 0) }
    /// Member chips under the header — off on iPhone (feedback ABDgl8tFlqeUlwP4DihfYhk).
    private var showsMemberChips: Bool { false }

    private func startCall(_ kind: CallKind, ring: [String]? = nil) {
        Task { await calls.start(chatId: chatId, projectId: isPeople ? nil : projectId, kind: kind, title: callTitle, ring: ring) }
    }

    /// Groups open the picker; DMs and agent chats start at once.
    private func requestCall(_ kind: CallKind) {
        if people?.isGroup == true {
            showGroupCallPicker = true
        } else {
            startCall(kind)
        }
    }
    @State private var replyTo: Message?
    /// Reply in DM (#1836): the message in another chat the next send here answers
    /// (its address + snapshot, written as `replyRef`). Exclusive with `replyTo`.
    @State private var replyRef: ReplyRef?
    /// Long-press → Edit: the own message whose text is in the composer.
    @State private var editing: Message?
    /// Drives the connection pill's dot pulse while it is reconnecting / syncing.
    @State private var pulsePhase = false
    /// The original a quote tap scrolled to; tinted until the fade timer clears it.
    @State private var highlightedMessage: Id?
    /// The bubble whose long-press actions (reaction capsule + menu) are open.
    @State private var actionsRow: Row?
    /// The composer had the keyboard up when the bubble was pressed (`openActions`).
    @State private var actionsRestoresKeyboard = false
    @State private var mentionQuery: String?
    /// Chip-picked addressees in a people chat: `agent:…` and/or `user:…` member ids.
    /// Empty means no chip: agent-only DMs may still infer the last agent; human-peer DMs stay
    /// with the person until a chip / `@` / reply picks an agent. Several agent chips can be
    /// on at once so that combination all reply (and "hey agents" / every agent chip → everyone).
    @State private var talkTo: Set<String> = []

    private var project: Project? { model.project(projectId) }
    private var chat: Chat? { model.chat(chatId) ?? model.jobTab(chatId) }
    /// A background job's tab (`JobTabs`): the job's exchange as a chat screen.
    private var isJobTab: Bool { JobTabs.isJobChat(chatId) }
    private var people: PeopleChat? { model.peopleChat(chatId) }
    private var isPeople: Bool { people != nil }
    /// D88: a server channel rendered with this same view (`projectId == channelSlot`).
    private var channel: ServerChannel? { projectId == channelSlot ? model.channel(chatId) : nil }
    private var isChannel: Bool { projectId == channelSlot }
    private var channelServer: Server? { channel.flatMap { model.server($0.serverId) } }
    private var counterpart: ChatMember? { people?.counterpart(me: model.user?.id) }
    /// The DM's other person while they are on my block list ([feedback
    /// blocked-state-ios-2026-10-26]): the header tag and the composer bar follow it.
    private var blockedPeer: ChatMember? {
        guard let people, !people.isGroup || people.kind == "dm", let other = counterpart else { return nil }
        return BlockedState.isBlocked(id: other.id, handle: other.handle, in: model.blocked) ? other : nil
    }
    private var messages: [Message] { model.messages(of: chatId) }
    private var needsAnswer: Bool { !isChannel && model.chatNeedsAnswer(chatId) }
    /// True while a turn runs, a subagent hand-off is still open, or right
    /// after the user sends (before the agent opens a streaming row) — visual
    /// cheat so a simple first reply does not sit on a dead transcript.
    private var streaming: Bool {
        model.isStreaming(chatId) || model.hasOpenSubagent(chatId) || model.isAwaitingReply(chatId)
    }
    private var name: String {
        if let channel { return channel.name }
        if let people { return counterpart.map { "@\($0.handle)" } ?? people.title }
        if let chat, chat.isGroup, !chat.title.isEmpty { return chat.title }
        if let chat, chat.isSystem { return "\(project?.name ?? "Arbos") · Projects" }
        if isJobTab, let chat { return chat.title }
        return project?.name ?? "Arbos"
    }
    /// Everyone on the presence row over the composer, newest first: people
    /// typing (room presence) and agents working (a fresh stream or an open
    /// hand-off, one entry per agent) — [feedback typing-indicator-left-2026-10-23].
    private var presenceParticipants: [PresenceParticipant] {
        var out: [PresenceParticipant] = []
        for handle in model.typingHandles(in: chatId) {
            let m = people?.members.first {
                !$0.isAgent && $0.handle.caseInsensitiveCompare(handle) == .orderedSame
            }
            out.append(PresenceParticipant(
                id: "typing:\(handle)",
                name: m?.name.flatMap { $0.isEmpty ? nil : $0 } ?? "@\(handle)",
                activity: .typing,
                picture: AnyView(PersonAvatar(url: m?.avatarUrl, handle: handle, size: PresenceRow.avatar,
                                              name: m?.name, id: m?.id, colorIndex: m?.avatarColorIndex)),
                target: model.personTarget(handle: handle, avatarUrl: m?.avatarUrl, chatId: chatId)
            ))
        }
        guard streaming else { return out }
        if isPeople {
            let recent = nowMs() - 15 * 60 * 1000
            var seen = Set<String>()
            for m in messages.reversed() where m.role == .assistant && m.isStreaming && m.createdAt >= recent {
                // The same question the bubble asks (#1592): the row's sender, handle and
                // name through `AppModel.agent` — one identity, so the row's picture is the
                // bubble's disc and its words the bubble's caption, never the raw handle.
                let p = model.agent(sender: m.sender, handle: m.senderHandle, name: m.senderName, chatId: chatId)
                guard seen.insert(p.id).inserted else { continue }
                out.append(PresenceParticipant(
                    id: "working:\(p.id)", name: p.name, activity: .working,
                    picture: AnyView(AgentAvatar(project: p, theme: theme, size: PresenceRow.avatar, working: true)),
                    target: .agent(projectId: p.id)
                ))
            }
        } else if let project {
            let p = model.agent(id: project.id, name: project.name)
            out.append(PresenceParticipant(
                id: "working:\(p.id)", name: p.name, activity: .working,
                picture: AnyView(AgentAvatar(project: p, theme: theme, size: PresenceRow.avatar, working: true)),
                target: .agent(projectId: project.id)
            ))
        }
        return out
    }

    /// Agent member ids currently selected on the conversation chips.
    private var talkToAgents: [String] {
        talkTo.filter { $0.hasPrefix("agent:") }.sorted()
    }

    /// True when every agent in the chat is selected (or the draft says "hey agents").
    private var talkingToEveryone: Bool {
        if draftAddressesEveryone { return true }
        guard let agents = people?.agents, agents.count > 1 else { return false }
        return talkToAgents.count == agents.count
    }

    /// Who the next people-chat message is for — chip pick, reply-to, or (agent-only DM) the
    /// agent who last spoke. A DM with a human peer (Allan + bob) leaves the addressee unset so
    /// plain lines stay with the person; tap the agent chip or `@` to talk to the agent. Groups
    /// leave the addressee unset so the decision model on the box can choose who replies, or none.
    /// When several agent chips are on, this is nil (see `talkToAgents` / `sendAddress`).
    private var effectiveTalkTo: String? {
        if talkingToEveryone { return "everyone" }
        if talkToAgents.count > 1 { return nil }
        if let one = talkTo.first { return one }
        if let r = replyTo, r.role == .assistant, let member = agentMember(for: r) {
            return member.member
        }
        // Agent-only DMs keep the last-agent follow-up; a human peer must not inherit @bob.
        let hasHumanPeer = people?.humans.contains { $0.id != model.user?.id } == true
        if people?.kind != "group", !hasHumanPeer,
           let last = messages.last(where: { $0.role == .assistant }),
           let member = agentMember(for: last) {
            return member.member
        }
        return nil
    }

    /// Composer prompt that names the addressee so it is obvious who will answer.
    /// With a picture or video staged, the field is the description bar for that
    /// send — one message, not a follow-up text bubble.
    private func composerPrompt(for draft: String) -> String {
        if pending.contains(where: Self.isCaptionablePending) { return "Add a description…" }
        if transcribing { return "Transcribing…" }
        if let channel { return "Message #\(channel.name)" }
        if isJobTab { return "Steer this task" }
        if !isPeople { return needsAnswer ? "Answer \(name)" : "Ask \(name)" }
        if talkingToEveryone { return "Message everyone" }
        if talkToAgents.count > 1 { return "Message \(talkToAgents.count) agents" }
        // Bare agent or person name in the draft ("Bob?", "Allan?") overrides a sticky chip.
        // Agent handle/slug wins over a human display-name collision ("Arbos" → @arbos).
        if let people {
            let namedAgents = AppModel.namedAgents(in: draft, members: people.members)
            if let member = people.members.first(where: { namedAgents.contains($0.member) }) {
                return "Message \(member.displayName)"
            }
            let namedPeople = AppModel.namedPeople(in: draft, members: people.members)
            if let member = people.members.first(where: { namedPeople.contains($0.member) }) {
                return "Message \(member.displayName)"
            }
        }
        if let member = people?.members.first(where: { $0.member == effectiveTalkTo }) {
            let slug = member.isAgent ? mentionSlug(member.project?.name ?? member.handle) : member.handle
            return "Message @\(slug)"
        }
        // No chip pick: match the header name ("Message Kenobi"), not "@handle".
        if let people {
            return "Message \(counterpart?.displayName ?? people.title)"
        }
        return "Message \(name)"
    }

    private func agentMember(for message: Message) -> ChatMember? {
        guard let h = message.senderHandle?.lowercased() else { return nil }
        return people?.agents.first {
            mentionSlug($0.project?.name ?? "").lowercased() == h || $0.handle.lowercased() == h
        }
    }

    /// Rebuild transcript rows. Membership lines ("Jacob added Home, hjkb and Doug",
    /// "Alice left") are system messages the relay writes into the doc (#1349).
    /// Streaming deltas and single appends rebuild only the tail (feedback
    /// app-01M5NNJ001NHHYBHXTASM5NXBZ); everything else still does a full pass.
    private func refreshRows() {
        // A channel is a people-style transcript: my lines right, everyone else by name.
        let slot = isPeople ? "\(peopleSlot):\(chatId)" : isChannel ? "\(channelSlot):\(chatId)" : projectId
        var agentNames: [String: String] = [:]
        for m in people?.agents ?? [] {
            let id = m.member.hasPrefix("agent:") ? String(m.member.dropFirst(6)) : m.member
            if let name = m.project?.name ?? m.name, !name.isEmpty { agentNames[id] = name }
        }
        // People by user id → current name (handle when unnamed); me from the live `/me`.
        var memberNames: [String: String] = [:]
        for m in people?.humans ?? [] {
            let name = m.name?.trimmingCharacters(in: .whitespaces) ?? ""
            memberNames[m.id] = name.isEmpty ? "@\(m.handle)" : name
        }
        if let who = model.user {
            let name = who.name?.trimmingCharacters(in: .whitespaces) ?? ""
            memberNames[who.id] = name.isEmpty ? "@\(who.login)" : name
        }
        let list = messages
        #if DEBUG
        let refreshStarted = Date()
        defer {
            PerfCounters.time("rows()", ms: Date().timeIntervalSince(refreshStarted) * 1000)
            if SendTrace.traces(chatId) {
                SendTrace.markIfPresent("rows-has-row", ids: rows.map(\.messageId), detail: "rows=\(rows.count) window=\(windowCount) skipEnd=\(windowSkipEnd)")
            }
        }
        #endif
        let me = isChannel ? model.channelMe : model.myUserId
        // Sender names inside the bubbles: groups and channels only — never a 1:1 chat.
        let multiParty = isChannel || (people?.isGroup ?? false)
        let peopleFlag = isPeople || isChannel
        // People I blocked: their rows fold into one line for me (groups only; a DM with
        // them has the composer bar) — [feedback blocked-state-ios-2026-10-26].
        let hiddenSenders = people.map { model.hiddenSenders(in: $0) } ?? []

        // Incremental: same ids and only the last message's content changed (streaming
        // token) — rebuild only the tail. A middle edit (reaction, etc.) falls through.
        if list.count == refreshIds.count, list.count > 0,
           zip(list, refreshIds).allSatisfy({ $0.id == $1 }) {
            let last = list[list.count - 1]
            let prefixSame = list.dropLast().allSatisfy { messageById[$0.id] == $0 }
            if prefixSame {
                if messageById[last.id] == last { return }
                var byId = messageById
                byId[last.id] = last
                let cut = rows.firstIndex(where: { $0.messageId == last.id }).map { ix -> Int in
                    if ix > 0, rows[ix - 1].id == "\(last.id)#date" { return ix - 1 }
                    return ix
                } ?? rows.count
                let prev = list.count >= 2 ? list[list.count - 2] : nil
                let slice = prev.map { [$0, last] } ?? [last]
                let rebuilt = Transcript.rows(slice, byId: byId, projectId: slot, me: me,
                                              people: peopleFlag, agentNames: agentNames, multiParty: multiParty, memberNames: memberNames, hiddenSenders: hiddenSenders)
                let newTail: [Row]
                if let prev {
                    let start = rebuilt.firstIndex(where: { $0.messageId == last.id }).map { ix -> Int in
                        if ix > 0, rebuilt[ix - 1].id == "\(last.id)#date" { return ix - 1 }
                        return ix
                    } ?? rebuilt.count
                    newTail = Array(rebuilt[start...])
                } else {
                    newTail = rebuilt
                }
                rows = Array(rows.prefix(cut)) + newTail
                messageById = byId
                rowsVersion &+= 1
                return
            }
        }

        // Incremental append: one new message at the end.
        if list.count == refreshIds.count + 1, list.count > 0,
           zip(list.dropLast(), refreshIds).allSatisfy({ $0.id == $1 }) {
            let last = list[list.count - 1]
            var byId = messageById
            byId[last.id] = last
            let prev = list.count >= 2 ? list[list.count - 2] : nil
            let slice = prev.map { [$0, last] } ?? [last]
            let rebuilt = Transcript.rows(slice, byId: byId, projectId: slot, me: me,
                                          people: peopleFlag, agentNames: agentNames, multiParty: multiParty, memberNames: memberNames, hiddenSenders: hiddenSenders)
            let newTail: [Row]
            if let prev {
                let start = rebuilt.firstIndex(where: { $0.messageId == last.id }).map { ix -> Int in
                    if ix > 0, rebuilt[ix - 1].id == "\(last.id)#date" { return ix - 1 }
                    return ix
                } ?? rebuilt.count
                newTail = Array(rebuilt[start...])
            } else {
                newTail = rebuilt
            }
            if windowSkipEnd == 0, windowCount < rows.count + newTail.count {
                windowCount = min(rows.count + newTail.count, windowCount + newTail.count)
            } else if windowSkipEnd > 0 {
                windowSkipEnd += newTail.count
            }
            rows = rows + newTail
            messageById = byId
            refreshIds = list.map(\.id)
            rowsVersion &+= 1
            return
        }

        let byId = Transcript.index(list)
        let next = Transcript.rows(list, byId: byId, projectId: slot, me: me,
                                   people: peopleFlag, agentNames: agentNames, multiParty: multiParty, memberNames: memberNames, hiddenSenders: hiddenSenders)
        // Rows appended after the last one we had (new messages) join the window, so the
        // oldest row on screen does not fall out of the view tree when a new one lands.
        // History that arrives above it — the first snapshot, a backfill, an older
        // channel page — does not widen the window.
        if windowSkipEnd == 0, windowCount < next.count, let lastId = rows.last?.id,
           let j = next.lastIndex(where: { $0.id == lastId }) {
            let appended = next.count - 1 - j
            if appended > 0 { windowCount = min(next.count, windowCount + appended) }
        } else if windowSkipEnd > 0, next.count > rows.count {
            // Reading history: rows that arrive below stay outside the window (the pill
            // counts them) and must not push the shown slice.
            windowSkipEnd += next.count - rows.count
        }
        messageById = byId
        rows = next
        refreshIds = list.map(\.id)
        rowsVersion &+= 1
    }

    var body: some View {
        #if DEBUG
        let _ = PerfCounters.bump("chatBody")
        let _ = DebugHooks.perfChanges ? Self._printChanges() : ()
        let _ = SendTrace.recent ? SendTrace.mark("session-body", detail: "tick=\(tick) version=\(model.messagesVersion[chatId] ?? 0) rows=\(rows.count) msgs=\(messages.count)", once: false) : ()
        #endif
        // Header stays outside GeometryReader (same pattern as ServerChannelsView /
        // HiddenChatsView). Nesting it inside the reader let iOS 26/27 vertically
        // centre the glass bar over the transcript (feedback app-01M5P98RB40WKA70R70DMDN97E).
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            GeometryReader { geo in
                transcript(width: geo.size.width, height: geo.size.height)
                    // A tap on a mention chip (`arbos://mention/<kind>/<id>`, MentionChips.swift)
                    // opens that entity's page; every other link goes to the system.
                    .environment(\.openURL, OpenURLAction { url in
                        // A tap on a hidden `||spoiler||` (`arbos://spoiler/<key>`) shows it for
                        // this viewer; nothing leaves the device.
                        if let key = SpoilerState.key(from: url) {
                            SpoilerState.shared.toggle(key)
                            return .handled
                        }
                        guard let target = MentionTable.target(from: url, model: model) else { return .systemAction }
                        // A chip is a profile target like the sender name beside it: open,
                        // dismiss on the same subject, swap for someone else (`ProfileToggle`).
                        model.toggleProfile(target)
                        return .handled
                    })
            }
            headerBlur
            // Hug content height: a horizontal ScrollView (member chips) inside this
            // stack is flexible on the vertical axis, so without fixedSize the VStack
            // fills the proposed height and iOS 27 lays the back button / title mid-screen
            // with the chips over the composer ([feedback APuu1qHpbKmmpqZeUusEi0Q]).
            VStack(spacing: 8) {
                header
                linkPill.padding(.top, -5)
                // The toast slot (Jacob 2026-10-27, [feedback toast-stack-ios-2026-10-27]): just
                // below the pill (and the connection pill), hanging over the transcript without
                // taking space — a zero-height anchor with the host as its overlay.
                Color.clear.frame(height: 0)
                    .overlay(alignment: .top) {
                        ToastHost().environmentObject(model)
                    }
                    .zIndex(2)
                // The chat-level note (Jacob 2026-10-26): pinned under the header pill, 16 inset —
                // the pushes refused for this chat, or the machine in error. One element.
                chatNote.padding(.horizontal, 16)
                if let people, showsMemberChips { memberChips(people).padding(.horizontal, 16) }
                if showSearch {
                    HStack(spacing: 8) {
                        Image(systemName: "magnifyingglass").font(.system(size: Controls.glyph, weight: .medium)).foregroundStyle(theme.muted)
                        TextField("", text: $searchText, prompt: Text("Search in chat").foregroundStyle(theme.placeholder))
                            .font(theme.font(Theme.body)).foregroundStyle(theme.text)
                            .focused($searchFocused)
                            .autocorrectionDisabled()
                        // Up/down: step between the same keyword's matches ([feedback app-01M5W4AVG30C4S2TPJ7T30CVJR]).
                        Button { stepSearchMatch(by: -1) } label: {
                            Image(systemName: "chevron.up")
                                .font(.system(size: 13, weight: .semibold))
                                .foregroundStyle(canStepSearchMatch ? theme.text : theme.muted)
                        }
                        .buttonStyle(.plain)
                        .disabled(!canStepSearchMatch)
                        .accessibilityLabel("Previous match")
                        .accessibilityIdentifier("chat-search-prev")
                        Button { stepSearchMatch(by: 1) } label: {
                            Image(systemName: "chevron.down")
                                .font(.system(size: 13, weight: .semibold))
                                .foregroundStyle(canStepSearchMatch ? theme.text : theme.muted)
                        }
                        .buttonStyle(.plain)
                        .disabled(!canStepSearchMatch)
                        .accessibilityLabel("Next match")
                        .accessibilityIdentifier("chat-search-next")
                        Button { closeSearch() } label: {
                            Image(systemName: "xmark.circle.fill").foregroundStyle(theme.muted)
                        }
                        .buttonStyle(.plain)
                    }
                    // The family's field (48, capsule, text at 17) in the header's glass
                    // (Jacob 2026-10-25 22:53; sized 2026-10-26).
                    .padding(.horizontal, Controls.textInset).frame(height: Controls.height)
                    .background {
                        HeaderGlassBackground(
                            shape: Capsule(),
                            material: ChatHeaderGlass.searchBarMaterial(surface: theme.surface),
                            tinted: true)
                    }
                    .padding(.horizontal, Controls.inset)
                }
                if hasInCallBar, let c = calls.call {
                    InCallOverlay(expected: model.expectedCallPeople(c), inline: true)
                        .frame(height: 48)
                }
                if hasCallBar, let active = calls.activeByChat[chatId] {
                    CallBar(call: active) {
                        Task { await calls.join(active, title: callTitle) }
                    }
                    .padding(.horizontal, 16)
                }
            }
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .top)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .overlay(alignment: .bottom) {
            if newBelowCount > 0 {
                newMessagesPill
                    .padding(.bottom, 10)
                    .transition(.opacity.combined(with: .scale(scale: 0.92)))
            }
        }
        // Trailing-only overlay so the hit target is just the disc (not a full-width
        // bar). UIKit tap inside the disc still works while the transcript decelerates.
        .overlay(alignment: .bottomTrailing) {
            if newBelowCount == 0, awayFromBottom {
                scrollToBottomButton
                    .padding(.trailing, composerSideInset)
                    .padding(.bottom, 10)
                    .transition(.opacity.combined(with: .scale(scale: 0.8)))
            }
        }
        .animation(.easeOut(duration: 0.2), value: awayFromBottom)
        .animation(.easeOut(duration: 0.2), value: newBelowCount > 0)
        .environment(\.composerInset, composerSideInset)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 0) {
                // The composer's note (Jacob 2026-10-26): the last send / attachment / recording
                // failure in this chat, directly above the composer at its width; Try again
                // re-sends the newest failed message.
                if let error = model.sendErrors[chatId] {
                    ErrorNote(id: "composer-error", error: error,
                              detailsOpen: Binding(get: { model.sendErrorDetailsOpen.contains(chatId) },
                                                   set: { if $0 { model.sendErrorDetailsOpen.insert(chatId) } else { model.sendErrorDetailsOpen.remove(chatId) } }),
                              onTryAgain: { model.retrySendError(chatId) })
                        .padding(.horizontal, composerSideInset)
                        .padding(.bottom, 10)
                }
                presenceRow
                    .environment(\.composerInset, composerSideInset)
                    .animation(.easeOut(duration: 0.25), value: composerFocused)
                Group {
                    if isChannel, !model.channelCanPost(chatId) {
                        // Rule 9: a restricted channel I cannot post in — one quiet line, no composer.
                        Text("Only admins post here")
                            .font(theme.font(Theme.small))
                            .foregroundStyle(theme.muted)
                            .frame(maxWidth: .infinity)
                            .frame(height: Theme.composerHeight)
                            .accessibilityIdentifier("channel-read-only")
                    } else if isPeople, people?.isSilencedNow == true {
                        // Group admin muted me from posting — one quiet line, no composer.
                        Text("You've been muted in this group")
                            .font(theme.font(Theme.small))
                            .foregroundStyle(theme.muted)
                            .frame(maxWidth: .infinity)
                            .frame(height: Theme.composerHeight)
                            .accessibilityIdentifier("group-silenced")
                    } else if let peer = blockedPeer {
                        // A DM with someone I blocked has no input ([feedback
                        // blocked-state-ios-2026-10-26]): "You blocked X · Unblock" in its place.
                        BlockedComposerBar(name: peer.displayName) { confirmUnblock = true }
                    } else {
                        composer
                    }
                }
                // Grok: 31 pt side inset with the keyboard down, 19 pt with it up.
                .padding(.horizontal, composerSideInset)
                .animation(.easeOut(duration: 0.25), value: composerFocused)
            }
            .padding(.top, Theme.chatBarTopPad)
            // Grok: 18 pt between the field and the keyboard, 2 pt above the home indicator.
            .padding(.bottom, composerFocused ? Theme.composerBottomKeyboard : Theme.composerBottom)
            .background(alignment: .top) {
                LinearGradient(colors: [theme.bg.opacity(0), theme.bg], startPoint: .top, endPoint: .bottom)
                    .frame(height: 24)
                    .offset(y: -24)
            }
            .background(theme.bg)
            // A drag that starts here selects text or slides the mic — never "back".
            .backSwipeComposerBar()
        }
        .overlayPreferenceValue(RowFrameKey.self) { anchors in
            GeometryReader { geo in
                if let row = actionsRow, let anchor = anchors[row.id] {
                    let frame = geo[anchor]
                    // This reader ignores the safe area, so its own insets read 0; the
                    // overlay needs the real status-bar / home-indicator insets to fit.
                    let safeArea = KeyboardFrameObserver.windowSafeArea
                    MessageActionsOverlay(
                        row: row,
                        frame: frame,
                        width: geo.size.width,
                        height: geo.size.height,
                        safeTop: max(geo.safeAreaInsets.top, safeArea.top),
                        safeBottom: max(geo.safeAreaInsets.bottom, safeArea.bottom),
                        held: heldReactions(row),
                        items: MessageMenu.items(forMessage: true,
                                                canEdit: canEditMessage(row),
                                                canDelete: canDelete(row),
                                                hasReplies: row.replyCount > 0,
                                                hasPreview: Transcript.linkPreviewsEnabled && messageById[row.messageId]?.linkPreview != nil),
                        onReact: { emoji in
                            closeActions()
                            model.toggleReaction(chatId: chatId, messageId: row.messageId, emoji: emoji)
                        },
                        onMenu: { item in
                            closeActions(after: item)
                            runMessageMenu(item, row: row)
                        },
                        onDismiss: { closeActions() },
                        bubble: {
                            // showReactionSenders: holding the message lists who
                            // used each emoji under the held bubble (chip hold
                            // does the same one-emoji list in the transcript).
                            RowView(model: model, theme: theme, composerInset: composerSideInset, row: row, chatId: chatId, width: geo.size.width,
                                    people: isPeople || isChannel, timeReveal: 0,
                                    mentions: MentionTable.Cache.shared.table(model: model, theme: theme),
                                    showReactionSenders: !row.reactions.isEmpty)
                                .padding(.horizontal, 16)
                        }
                    )
                }
            }
            .ignoresSafeArea()
        }
        // Custom back chevron; swipe right (edge pop or anywhere outside the composer
        // bar) goes back one level — `BackSwipe.swift`, shared by every chat screen.
        .backSwipeToPop()
        .fullScreenCover(item: $rigViewer) { v in
            MediaViewer(album: v.items, index: v.index, onClose: { rigViewer = nil }, openForward: rigViewerForward)
                .environmentObject(model)
                .environment(\.theme, theme)
        }
        .sheet(isPresented: $showModelPicker, onDismiss: demoDismissed) {
            if let project {
                ModelPickerView(project: project).environmentObject(model).environment(\.theme, theme)
            }
        }
        .sheet(isPresented: $showAgentSecrets) {
            NavigationStack { AgentSecretsPage(projectId: projectId) }
                .environmentObject(model).environment(\.theme, theme)
                .presentationBackground(theme.bg)
                .preferredColorScheme(theme.isDark ? .dark : .light)
        }
        .sheet(isPresented: $showSettings) {
            SettingsView().environmentObject(model).environment(\.theme, theme)
        }
        .sheet(isPresented: $showGroupCallPicker) {
            if let people {
                GroupCallPickerSheet(people: people, meId: model.user?.id) { ring in
                    startCall(.voice, ring: ring)
                }
                .environment(\.theme, theme)
            }
        }
        .confirmationDialog(BlockedState.confirmTitle(handle: blockedPeer?.handle ?? counterpart?.handle ?? ""), isPresented: $confirmUnblock, titleVisibility: .visible) {
            Button(BlockedState.unblock) {
                if let peer = blockedPeer, let entry = BlockRow.entry(in: model.blocked, id: peer.id, handle: peer.handle) { model.unblock(entry) }
            }
        } message: { Text(BlockedState.confirmNote) }
        .sheet(isPresented: $showProfile) {
            AgentEditSheet(projectId: projectId).environmentObject(model).environment(\.theme, theme)
        }
        .sheet(item: $pendingForward) { pending in
            ForwardSheet(fromName: pending.from) { target in
                pendingForward = nil
                model.forwardText(pending.text, from: pending.from, to: target)
                model.notice = Notice(text: "Forwarded to \(target.title)")
            }
            .environmentObject(model)
            .environment(\.theme, theme)
            .presentationDetents([.medium, .large])
            .presentationDragIndicator(.visible)
        }
        .sheet(item: $pendingThread) { pending in
            MessageThreadSheet(root: pending.root, replies: pending.replies)
                .environment(\.theme, theme)
        }
        .sheet(isPresented: $showTasks) {
            TasksSheet(jobs: model.openJobs(chatId),
                       onSteer: { job in steerJob(job) },
                       onStop: { job in model.stopJob(chatId: chatId, jobId: job.id) },
                       onStopAll: { showTasks = false; stop() },
                       onOpen: { job in showTasks = false; model.openJobTab(job.id) })
                .environment(\.theme, theme)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
                .presentationBackground(theme.bg)
                .preferredColorScheme(theme.isDark ? .dark : .light)
        }
        .task(id: chatId) {
            // Polls the open chat's jobs while one is live (`AppModel.pollJobsIfNeeded`);
            // cancelled with the view. The sheet lists live work only: nothing open → it
            // closes itself.
            if isJobTab { model.pollRolloutIfNeeded(chatId: chatId, force: true) }
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(AppModel.jobsPollMs))
                guard !Task.isCancelled else { break }
                model.pollJobsIfNeeded(chatId: chatId)
                if isJobTab { model.pollRolloutIfNeeded(chatId: chatId) }
                if showTasks, model.openJobs(chatId).isEmpty { showTasks = false }
            }
        }
        .task {
            guard DebugHooks.liveDemo, !model.demoStarted, DebugHooks.openChat == chatId || DebugHooks.openChat == nil, let text = DebugHooks.autoSend else { return }
            model.demoStarted = true
            while !model.caughtUp.contains(chatId) { try? await Task.sleep(nanoseconds: 200_000_000) }
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            composerFocused = true
            try? await Task.sleep(nanoseconds: 800_000_000)
            for ch in text { composerDraft.text.append(ch); try? await Task.sleep(nanoseconds: 45_000_000) }
            try? await Task.sleep(nanoseconds: 600_000_000)
            send()
            try? await Task.sleep(nanoseconds: 400_000_000)
            composerFocused = false
            // Wait for the turn: streaming starts, then ends (cap 45 s).
            var waited = 0
            while !streaming && waited < 150 { try? await Task.sleep(nanoseconds: 200_000_000); waited += 1 }
            while streaming && waited < 225 { try? await Task.sleep(nanoseconds: 200_000_000); waited += 1 }
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            if rows.count > 8 {
                withAnimation(.easeInOut(duration: 1.2)) { scroller?.scrollTo(rows[rows.count - 8].id, anchor: .top) }
                try? await Task.sleep(nanoseconds: 2_400_000_000)
                withAnimation(.easeInOut(duration: 1.0)) { scroller?.scrollTo("bottom", anchor: .bottom) }
                try? await Task.sleep(nanoseconds: 1_600_000_000)
            }
            demoQueue = ["model", "threads", "attach", "voice", "settings"]
            demoPresentNext()
        }
        .task {
            guard DebugHooks.demo, model.isFixture, !model.demoStarted else { return }
            model.demoStarted = true
            try? await Task.sleep(nanoseconds: 1_800_000_000)
            composerFocused = true
            try? await Task.sleep(nanoseconds: 900_000_000)
            for ch in "Also keep your replies short, please." {
                composerDraft.text.append(ch)
                try? await Task.sleep(nanoseconds: 55_000_000)
            }
            try? await Task.sleep(nanoseconds: 500_000_000)
            send()
            try? await Task.sleep(nanoseconds: 300_000_000)
            composerFocused = false
            try? await Task.sleep(nanoseconds: 7_500_000_000)
            dismiss()
        }
        .task(id: calls.activeByChat[chatId]?.callId) {
            // While the Join bar is up, re-check the relay so an ended call does not stick.
            guard !isChannel, calls.activeByChat[chatId] != nil else { return }
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 4_000_000_000)
                guard !Task.isCancelled, calls.activeByChat[chatId] != nil else { return }
                await model.refreshActiveCall(chatId: chatId, projectId: isPeople ? nil : projectId)
            }
        }
        .onAppear {
            appearedAt = Date()
            LaunchClock.mark("chat-appear", detail: "messages=\(messages.count)")
            // Force: re-PUT even if the cursor is unchanged so the relay clears
            // unread events recorded after the last PUT (other apps / badge).
            model.markRead(chatId: chatId, force: true)
            model.visibleChatId = chatId
            // Restore unsent text left when the user backed out of this thread.
            let saved = DebugHooks.composerDraft ?? model.composerDraft(for: chatId)
            composerDraft.onChange = { [chatId] d in
                // Keep the draft while the view is still up so a process kill mid-type
                // is not required — leaving the thread saves on disappear too.
                model.setComposerDraft(d, for: chatId)
                // People chats (DM + group): tell the room we are typing so peers
                // see names on the presence row ([feedback app-01M5N7WSA1VCYHE9BE3CX4V0R6]).
                if isPeople, !d.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    model.typedIn(chatId: chatId)
                }
                let everyone = AppModel.addressesEveryone(d)
                if everyone != draftAddressesEveryone { draftAddressesEveryone = everyone }
                // `@handle` + a separator typed right after it: a chip, no picker (behaviour 5).
                if isPeople, let m = MentionDraft.autoResolve(text: d, caret: composerDraft.caret, mentions: composerDraft.mentions,
                                                              members: people?.members ?? [], me: model.user?.login) {
                    var next = composerDraft.mentions
                    next.append(m)
                    next.sort { $0.range.lowerBound < $1.range.lowerBound }
                    composerDraft.mentions = next
                }
                updateMentionQuery()
            }
            composerDraft.onCaretOrMentions = { updateMentionQuery() }
            if composerDraft.text.isEmpty, !saved.isEmpty {
                composerDraft.text = saved
                composerDraft.caret = (saved as NSString).length
                composerDraft.mentions = MentionDraft.resolveAll(text: saved, mentions: [], members: people?.members ?? [], me: model.user?.login, collectives: isPeople)
                if DebugHooks.composerDraft != nil { composerFocused = true }
            }
            if isPeople { model.markPeopleRead(chatId, force: true) }
            if isChannel {
                // The newest page now, live rows through the listener from here on.
                model.openChannel(chatId)
            } else {
                Task { await model.refreshActiveCall(chatId: chatId, projectId: isPeople ? nil : projectId) }
            }
            openPendingSearchIfNeeded()
            takePendingDMReplyIfNeeded()
            revealPendingIfNeeded()
            // Rig: `ARBOS_SHOW=chatsearch` raises the Search-in-chat bar (header-glass shots).
            if DebugHooks.show == "chatsearch" { showSearch = true }
            if let link = DebugHooks.link {
                model.setRoomLink(chatId, link)
                if let ms = DebugHooks.linkClearsMs {
                    DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(ms)) { model.setRoomLink(chatId, .live) }
                }
            }
            #if DEBUG
            let drive = DebugHooks.perfDrive
            if !drive.isEmpty { Task { @MainActor in await runPerfDrive(drive) } }
            #endif
            if let show = DebugHooks.show, !model.showHookDone {
                model.showHookDone = true
                DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(DebugHooks.showDelayMs)) {
                    switch show {
                    case "keyboard": composerFocused = true
                    case "settings": showSettings = true
                    case "agentsecrets": showAgentSecrets = true
                    case "voice": startCall(.voice)
                    case "model": showModelPicker = true
                    // `agentvoice | agentlabel | agentrename`: the agent pane, which opens the Voice
                    // picker / edits the label / opens the rename alert on appear.
                    case "profile", "agentvoice", "agentlabel", "agentrename": openHeaderProfile()
                    // `avatarpicker[-generate|-upload]`: the agent pane, which opens the sheet on that tab.
                    case let s where s.hasPrefix("avatarpicker"): openHeaderProfile()
                    case "edit": showProfile = true
                    // `tasks`: the tasks sheet over the presence row (needs `ARBOS_FIXTURE_JOBS`).
                    case "tasks": if !model.openJobs(chatId).isEmpty { showTasks = true }
                    // `jobtab`: the newest job's tab (needs `ARBOS_FIXTURE_JOBS`).
                    case "jobtab": model.openNewestJobTab(from: chatId)
                    case "userprofile": if let c = counterpart { model.showProfile(.user(ProfilePerson(c))) }
                    // `addmembers`: the group pane opens its Add members sheet on appear.
                    case "groupprofile", "addmembers", "removemember": model.showProfile(.group(chatId: chatId, projectId: isPeople ? nil : projectId))
                    case let s where s.hasPrefix("groupavatar"): model.showProfile(.group(chatId: chatId, projectId: isPeople ? nil : projectId))
                    case "member":
                        // The group page, then its first other member pushed over it (rig).
                        openHeaderProfile()
                        if let other = people?.humans.first(where: { $0.id != model.user?.id }) {
                            DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { model.showProfile(.user(ProfilePerson(other))) }
                        }
                    case "members", "addperson": openHeaderProfile()
                    case "foreignagent", "foreignagent-owner":
                        // Someone else's agent in this group (needs `ARBOS_FIXTURE_FOREIGN_AGENT=1`):
                        // its page; `-owner` then follows its "Run by …" link
                        // ([feedback agent-pane-run-by-link-ios-2026-10-25]).
                        if let agent = people?.agents.first(where: { $0.ownerId != model.user?.id }), let foreign = model.foreignAgent(agent.id) {
                            model.showProfile(.agent(projectId: foreign.id))
                            if show == "foreignagent-owner", let owner = agent.ownerHandle {
                                DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
                                    model.showProfile(.user(ProfilePerson(id: agent.ownerId ?? owner, handle: owner, name: foreign.ownerName, avatarUrl: foreign.ownerAvatarUrl)))
                                }
                            }
                        }
                    // `agentprofile`: the first roster agent's profile hero (a foreign agent's
                    // picture — the avatar-flip rig).
                    case "agentprofile": if let a = people?.agents.first { model.showProfile(.agent(projectId: a.id)) }
                    case "feedback": FeedbackCenter.shared.open(screen: "chat", chatId: chatId, projectId: isPeople ? nil : projectId)
                    case "call":
                        #if DEBUG
                        if let url = DebugHooks.callUrl, let token = DebugHooks.callToken {
                            Task { await calls.debugJoin(url: url, token: token, kind: DebugHooks.callKind == "video" ? .video : .voice,
                                                         chatId: chatId, projectId: isPeople ? peopleSlot : projectId) }
                        } else {
                            startCall(DebugHooks.callKind == "video" ? .video : .voice)
                        }
                        #else
                        startCall(.voice)
                        #endif
                    case "recording": recorder.start()
                    case "scrollup":
                        if let first = windowedRows.first { withAnimation { scroller?.scrollTo(first.id, anchor: .top) } }
                    case "media", "media-forward", "media-file":
                        // Rig (media-viewer-2026-10-23): the viewer on this chat's album at
                        // ARBOS_MEDIA_INDEX (chat order, default 0), with the Forward sheet
                        // up for `media-forward`; `media-file` opens the chat's first file.
                        let slot = isPeople ? "\(peopleSlot):\(chatId)" : projectId
                        if show == "media-file" {
                            let file = messages.lazy.flatMap { m in m.parts.compactMap { part -> MediaItem? in
                                guard case .attachment(let id, let a) = part, MediaAlbum.kind(of: a) == .file else { return nil }
                                return MediaAlbum.item(id: "\(m.id)#\(id)", message: m, attachment: a, kind: .file, slot: slot,
                                                       projectName: { model.project($0)?.name })
                            } }.first
                            if let file { rigViewer = MediaViewerState(items: [file], index: 0) }
                        } else {
                            let album = MediaAlbum.items(messages: messages, slot: slot, projectName: { model.project($0)?.name })
                            let ix = Int(ProcessInfo.processInfo.environment["ARBOS_MEDIA_INDEX"] ?? "") ?? 0
                            if !album.isEmpty { rigViewer = MediaViewerState(items: album, index: min(ix, album.count - 1)) }
                        }
                        rigViewerForward = show == "media-forward"
                    case "pending":
                        pending = [.init(name: "Q3 board deck v5.pdf", mime: "application/pdf", data: Data()),
                                   .init(name: "Photo Oct 15.jpg", mime: "image/jpeg", data: Data())]
                        composerFocused = true
                    case "reply":
                        // Rig: the composer's reply preview on the newest agent message.
                        replyTo = messages.last { $0.role == .assistant && !$0.isStreaming }
                        composerFocused = true
                    case "reply-long":
                        // Rig: the preview on a multi-paragraph markdown reply (the shape of
                        // Jacob's Oct 23 screenshot; fixture only).
                        #if DEBUG
                        if model.isFixture {
                            model.fixtureAppendAssistant(chatId: chatId, text: AppModel.longReview)
                            replyTo = model.messages[chatId]?.last { $0.role == .assistant }
                            composerFocused = true
                        }
                        #endif
                    case "code-block":
                        // Rig: a finished reply with a fenced ```rust block, for the code
                        // card's Copy pill shots (fixture only).
                        #if DEBUG
                        if model.isFixture {
                            model.fixtureAppendAssistant(chatId: chatId, text: AppModel.codeReply)
                        }
                        #endif
                    case "table", "table-top":
                        // Rig: Jacob's Lima bookstore turn — a GFM table of linked stores
                        // between two prose bubbles; `table-top` stops after the table so a
                        // phone-height shot shows it (fixture only).
                        #if DEBUG
                        if model.isFixture {
                            model.fixtureLimaReply(chatId: chatId, bubbles: show == "table-top" ? 2 : 3)
                        }
                        #endif
                    case "links":
                        // Rig: Jacob's New York Post turn — seven linked headlines as
                        // bullets, each with its site icon (fixture only).
                        #if DEBUG
                        if model.isFixture {
                            model.fixtureNYPostReply(chatId: chatId)
                        }
                        #endif
                    case "math":
                        // Rig: Jacob's bubble-sort turn — a Python code card, then prose
                        // with inline `$O(n^2)$` math (fixture only).
                        #if DEBUG
                        if model.isFixture {
                            model.fixtureBubbleSortReply(chatId: chatId)
                        }
                        #endif
                    case "agent-message", "agent-message-received":
                        // Rig: agent-to-agent messaging rows (fixture only) — the sender's
                        // hand-off + confirm + relayed reply, or the receiver's labelled row.
                        #if DEBUG
                        if model.isFixture {
                            model.fixtureAgentMessage(chatId: chatId, received: show == "agent-message-received")
                        }
                        #endif
                    case "forward":
                        // Rig: the Forward sheet for the newest message bubble.
                        if let row = rows.last(where: { isMessageRow($0) }) { beginForward(row) }
                    case "forwarded":
                        // Rig: a sent bubble forwarded from Sam Rivera (fixture only).
                        #if DEBUG
                        if model.isFixture {
                            model.fixtureAppend(chatId: chatId,
                                                text: "Sending in a sec. Also: can we add your Arbos agent here so it can pull the numbers while we talk?",
                                                attachments: [], forwardedFrom: "Sam Rivera")
                        }
                        #endif
                    case "actions":
                        // Rig: the long-press actions (capsule + menu) on the newest bubble.
                        if let row = rows.last(where: { isMessageRow($0) }) { openActions(row) }
                    case "keyboard-actions":
                        // Rig (iphone-bubble-menu-keyboard): the composer keyboard is up, then the
                        // newest bubble is long-pressed — the overlay must clear the keyboard.
                        composerFocused = true
                        DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(1200)) {
                            if let row = rows.last(where: { isMessageRow($0) }) { openActions(row) }
                        }
                    case "reply-dm":
                        // Rig (#1836): the composer armed with a cross-chat reply, as Reply in DM leaves it.
                        replyRef = ReplyRef(chatId: "fx-group-launch", projectId: "people", messageId: "g1", chatTitle: "Launch week",
                                            sender: "user:fx-u3", senderName: "Mira Kaplan", senderHandle: "mira-k",
                                            text: "Launch checklist is here: https://notion.so/arbos/launch-week")
                        composerFocused = true
                    case "replied":
                        // Rig: a sent bubble quoting the newest agent message (fixture only).
                        #if DEBUG
                        if model.isFixture, let m = messages.last(where: { $0.role == .assistant && !$0.isStreaming }) {
                            model.fixtureAppend(chatId: chatId, text: "Does that cover the #general channel too?",
                                                attachments: [], replyTo: m.id)
                        }
                        #endif
                    default: break
                    }
                }
            }
        }
        .onDisappear {
            // Keep the typed draft so coming back to this thread restores it.
            model.setComposerDraft(composerDraft.text, for: chatId)
            // Catch notifies that landed after the last onChange mark-read while we were still in the chat
            // (including rows that arrived while the app was inactive on a call — isWatching was false).
            if isPeople { model.markPeopleRead(chatId) }
            else { model.markRead(chatId: chatId) }
            if model.visibleChatId == chatId { model.visibleChatId = nil }
        }
        // Profile "search" sets pendingSearchChat while this chat is already on screen
        // (sheet over us), so onAppear alone never sees it — watch the published flag.
        .onChange(of: model.pendingSearchChat) { _, _ in openPendingSearchIfNeeded() }
        .onChange(of: model.pendingDMReply?.chatId) { _, _ in takePendingDMReplyIfNeeded() }
        // A bottom-bar search result or a quote tap: the rows may land after appear (room backfill).
        .onChange(of: model.pendingReveal) { _, _ in revealPendingIfNeeded() }
        .onChange(of: rows.count) { _, _ in revealPendingIfNeeded() }
        .onChange(of: searchText) { _, _ in syncSearchMatchIndex() }
        // The per-chat version, not the array: comparing 1 000+ messages on every
        // body (each keystroke, presence tick, publish) was a measurable share of
        // the Arbonites lag (arbonites-chat-lag, 2026-10-24).
        .onChange(of: model.messagesVersion[chatId] ?? 0, initial: true) { _, new in
            #if DEBUG
            if SendTrace.traces(chatId) { SendTrace.mark("version-onchange", detail: "version=\(new)", once: false) }
            #endif
            let old = rowsSource
            let new = messages
            let initial = old.isEmpty && rows.isEmpty
            rowsSource = new
            refreshRows()
            // Streaming tokens reassign `messages` with the same last id — do not
            // mark-read on every delta. Initial call (`old == new`) and a new last
            // row still mark; AppModel also skips/throttles the PUT.
            if old.last?.id != new.last?.id || initial {
                model.markRead(chatId: chatId)
                if isPeople { model.markPeopleRead(chatId) }
            }
            // A channel's read marker moves when a new row lands while it is on screen.
            if isChannel, old.last?.id != new.last?.id { model.markChannelRead(chatId) }
        }
        // Adding a person or agent updates membership without a new message — rebuild so "@handle joined" appears.
        // A member's rename re-words the membership lines that name them, so the key carries names.
        .onChange(of: people?.members.map { "\($0.id):\($0.joinedAt):\($0.name ?? "")" } ?? []) { _, _ in
            refreshRows()
        }
        .onChange(of: model.user?.name) { _, _ in refreshRows() }
        .onChange(of: model.caughtUp.contains(chatId), initial: true) { _, ready in
            if ready, !sentOnce, !DebugHooks.liveDemo, let text = DebugHooks.autoSend, DebugHooks.openChat == chatId {
                // Several messages separated by " ||| " go out a few seconds apart
                // (multitasking checks: the composer never waits for a reply).
                let parts = text.components(separatedBy: " ||| ").filter { !$0.isEmpty }
                composerDraft.text = parts.first ?? text
                send()
                if parts.count > 1 {
                    Task {
                        for p in parts.dropFirst() {
                            try? await Task.sleep(nanoseconds: 3_000_000_000)
                            composerDraft.text = p
                            send()
                        }
                    }
                }
            }
        }
    }

    /// Demo tour: present the next sheet; each one auto-dismisses and its
    /// onDismiss schedules the next, so presentations never overlap.
    private func demoPresentNext() {
        guard !demoQueue.isEmpty else { return }
        let step = demoQueue.removeFirst()
        let hold: Double = step == "voice" ? 3.5 : 3.0
        switch step {
        case "model": showModelPicker = true
        case "attach": showAttach = true
        case "voice": startCall(.voice)
        case "settings": showSettings = true
        default: break
        }
        guard step != "settings" else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + hold) {
            switch step {
            case "model": showModelPicker = false
            case "attach": showAttach = false
            case "voice": break
            default: break
            }
        }
    }

    private func demoDismissed() {
        guard DebugHooks.liveDemo, !demoQueue.isEmpty else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) { demoPresentNext() }
    }

    // MARK: Header (floating glass)

    /// Strip the messages scroll under (ref-02): the shared `ListTopFade` from the physical
    /// top edge — frosted on Glass, solid on Matte — plus whatever the header stacks
    /// (member chips, the Search-in-chat bar, call strips).
    private var headerBlur: some View {
        ListTopFade(screen: .chat, extra: (showsMemberChips ? 38 : 0) + headerExtra)
    }

    private var headerKind: ChatHeaderKind {
        if channel != nil { return .channel }
        if let people { return people.isGroup ? .group : .dm }
        return .agent
    }

    /// Liquid Glass on Glass (frost under iOS 26), solid on Matte — one material for the
    /// back disc, the title pill, the search disc and the Search-in-chat bar.
    private var headerMaterial: HeaderMaterial {
        ChatHeaderGlass.materials(kind: headerKind, surface: theme.surface).first?.1
            ?? ChatHeaderGlass.material(surface: theme.surface)
    }

    private var header: some View {
        HStack {
            Button { dismiss() } label: {
                GlassCircle(material: headerMaterial) {
                    Image(systemName: "chevron.left")
                        .font(theme.font(Controls.glyph, weight: .semibold))
                        .foregroundStyle(theme.text)
                }
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Back")
            // Names the open chat for the UI tests (which screen is on top after a swipe).
            .accessibilityIdentifier("chat-back:\(chatId)")
            Spacer()
            GlassPill(material: headerMaterial) {
                HStack(spacing: 6) {
                    if let channel {
                        // Rule 3: the unified pill — the server's disc, the `#` glyph and the
                        // name (never "#name": the glyph shows it, #1317).
                        HStack(spacing: Theme.headerPillGap) {
                            if let s = channelServer {
                                ServerAvatar(server: s, size: Controls.avatar)
                            }
                            HStack(spacing: 5) {
                                Text("#")
                                    .font(.system(size: Theme.body, weight: .semibold))
                                    .foregroundStyle(theme.muted)
                                Text(channel.name)
                                    .font(.system(size: Theme.body, weight: .semibold))
                                    .foregroundStyle(theme.text)
                                    .lineLimit(1)
                            }
                        }
                        .contentShape(Rectangle())
                        .onTapGesture { openHeaderProfile() }
                        .accessibilityAddTraits(.isButton)
                        .accessibilityLabel(headerLabel)
                        .accessibilityHint("Opens the channel details")
                    } else if let people {
                        // One pill for every chat (2026-10-20): avatar + name,
                        // no member cluster, counts or agent lines.
                        HStack(spacing: Theme.headerPillGap) {
                            if people.isGroup, let url = groupImageURL(people) {
                                // Custom group picture.
                                PersonAvatar(url: url, handle: people.title, size: Controls.avatar, name: people.title, id: people.id)
                                Text(people.title)
                                    .font(.system(size: Theme.body, weight: .semibold))
                                    .foregroundStyle(theme.text)
                                    .lineLimit(1)
                            } else if people.isGroup, people.isNeutralAvatar {
                                NeutralAvatar(name: people.title, size: Controls.avatar)
                                Text(people.title)
                                    .font(.system(size: Theme.body, weight: .semibold))
                                    .foregroundStyle(theme.text)
                                    .lineLimit(1)
                            } else if people.isGroup, let leaf = people.leafPick, LeafAvatarsFlag.shared.on {
                                // D92 amendment: the leaf picked on the Create screen.
                                LeafAvatar(pick: leaf, size: Controls.avatar)
                                Text(people.title)
                                    .font(.system(size: Theme.body, weight: .semibold))
                                    .foregroundStyle(theme.text)
                                    .lineLimit(1)
                            } else if let other = counterpart, !people.isGroup || people.kind == "dm" {
                                // DM peer (including a DM that gained agents — still their face).
                                PersonAvatar(url: other.avatarUrl, handle: other.handle, size: Controls.avatar, name: other.name, id: other.id, colorIndex: other.avatarColorIndex)
                                Text(other.displayName)
                                    .font(.system(size: Theme.body, weight: .semibold))
                                    .foregroundStyle(theme.text)
                                    .lineLimit(1)
                                // Blocked ([feedback blocked-state-ios-2026-10-26]): the tag after the name.
                                if blockedPeer != nil { BlockedTag() }
                            } else {
                                // Named multi-human group with no picture: title initials.
                                PersonAvatar(url: nil, handle: people.title, size: Controls.avatar, name: people.title, id: people.id)
                                Text(people.title)
                                    .font(.system(size: Theme.body, weight: .semibold))
                                    .foregroundStyle(theme.text)
                                    .lineLimit(1)
                            }
                        }
                        .contentShape(Rectangle())
                        .onTapGesture { openHeaderProfile() }
                        .accessibilityAddTraits(.isButton)
                        .accessibilityLabel(headerLabel)
                        .accessibilityHint(people.isGroup ? "Opens group info" : "Opens the profile")
                    } else {
                        HStack(spacing: Theme.headerPillGap) {
                            if let project {
                                AgentAvatar(project: project, theme: theme, size: Controls.avatar, dot: machineDotColor(model.machine.state, theme: theme), working: streaming)
                            }
                            Text(name)
                                .font(.system(size: Theme.body, weight: .semibold))
                                .foregroundStyle(theme.text)
                                .lineLimit(1)
                        }
                        .contentShape(Rectangle())
                        .onTapGesture { openHeaderProfile() }
                        .accessibilityAddTraits(.isButton)
                        .accessibilityLabel(headerLabel)
                        .accessibilityHint("Opens the agent profile")
                    }
                }
                .padding(.leading, Controls.avatarInset)
                .padding(.trailing, Controls.textInset)
                .frame(height: Controls.height)
                .contentShape(Capsule())
            }
            Spacer()
            // Same-size circle as the back button keeps the title pill centred.
            // Call lives on the composer (phone when idle); a header phone here
            // duplicated it in groups (#1625).
            Button {
                if showSearch {
                    closeSearch()
                } else {
                    withAnimation(.easeOut(duration: 0.18)) { showSearch = true }
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { searchFocused = true }
                }
            } label: {
                GlassCircle(material: headerMaterial) {
                    Image(systemName: "magnifyingglass")
                        .font(theme.font(Controls.glyph, weight: .medium))
                        .foregroundStyle(theme.text)
                }
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Search in chat")
            .accessibilityIdentifier("chat-search-button")
            .background { searchKeyboardShortcuts }
        }
        .controlBar()
    }

    private var composerSideInset: CGFloat {
        composerFocused ? Theme.composerInsetKeyboard : Theme.composerInset
    }

    /// Jump to the latest row and resume stick-to-bottom.
    ///
    /// From deep in history — the window slid past the tail (`windowSkipEnd > 0`) —
    /// the window is re-cut to the newest `windowInitial` rows, the slice a chat
    /// opens on, and the scroll lands at once: rebuilding the full 360-row window
    /// and then animating through it took ~0.75 s (Debug simulator) on a 1 700-row
    /// group ([feedback jump-to-latest-instant-ios-2026-10-25]; the Mac's ↓ had the
    /// same shape of bug). Within the tail window the short animated hop stays.
    private func jumpToLatest() {
        pinnedToBottom = true
        awayFromBottom = false
        newBelowCount = 0
        let landing = Self.windowAfterJump(count: windowCount, skipEnd: windowSkipEnd)
        if landing.count == windowCount, landing.skipEnd == windowSkipEnd {
            withAnimation(.easeOut(duration: 0.3)) { scroller?.scrollTo("bottom", anchor: .bottom) }
            return
        }
        var t = Transaction()
        t.disablesAnimations = true
        withTransaction(t) {
            windowSkipEnd = landing.skipEnd
            windowCount = landing.count
        }
        // The first scrollTo runs before layout knows the re-cut content's size.
        scroller?.scrollTo("bottom", anchor: .bottom)
        DispatchQueue.main.async { scroller?.scrollTo("bottom", anchor: .bottom) }
    }

    /// The window ↓ lands on. Unchanged while the window still ends at the tail
    /// (a short animated hop over rows that are already built); once it has slid
    /// into history, the newest `windowInitial` rows — one opening page to build,
    /// not `windowMax` bubbles — and the reader grows it again by scrolling up.
    static func windowAfterJump(count: Int, skipEnd: Int) -> (count: Int, skipEnd: Int) {
        skipEnd > 0 ? (windowInitial, 0) : (min(count, windowMax), 0)
    }

    /// After a send: stick to the end now and again once the optimistic row has height.
    private func scrollAfterSend() {
        scroller?.scrollTo("bottom", anchor: .bottom)
        DispatchQueue.main.async { scroller?.scrollTo("bottom", anchor: .bottom) }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) {
            withAnimation(.easeOut(duration: 0.2)) { scroller?.scrollTo("bottom", anchor: .bottom) }
        }
    }

    /// Grok's scroll-to-bottom control: a 36 pt field-coloured circle with a
    /// chevron, right-aligned with the composer, 18 pt above it.
    /// UIKit `UIControl` (not SwiftUI `Button`) so a tap still fires while the
    /// transcript is decelerating — SwiftUI button gestures lose to UIScrollView.
    private var scrollToBottomButton: some View {
        ZStack {
            Circle().fill(theme.field) // control-size: exception transcriptJumpChip
            Circle().strokeBorder(theme.fieldBorder, lineWidth: 1) // control-size: exception transcriptJumpChip
            Image(systemName: "chevron.down")
                .font(theme.font(15, weight: .medium))
                .foregroundStyle(theme.text)
            ScrollInterruptTap(action: jumpToLatest)
                .frame(width: 44, height: 44)
        }
        .frame(width: Theme.voiceButton, height: Theme.voiceButton)
        .contentShape(Circle().inset(by: -(44 - Theme.voiceButton) / 2))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Scroll to bottom")
        .accessibilityAddTraits(.isButton)
        .accessibilityAction { jumpToLatest() }
    }

    /// Shown when rows land below while the reader is up in history — tap jumps
    /// to the end (iMessage / Slack style; Grok's bare chevron stays for scroll-up alone).
    private var newMessagesPill: some View {
        HStack(spacing: 6) {
            Image(systemName: "chevron.down")
                .font(theme.font(13, weight: .semibold))
            Text(newBelowCount == 1 ? "New message" : "New messages")
                .font(theme.font(Theme.small, weight: .semibold))
        }
        .foregroundStyle(theme.text)
        .padding(.horizontal, 14)
        .frame(height: Theme.voiceButton)
        .background(Capsule().fill(theme.field)) // control-size: exception transcriptJumpChip
        .overlay(Capsule().strokeBorder(theme.fieldBorder, lineWidth: 1)) // control-size: exception transcriptJumpChip
        .contentShape(Capsule().inset(by: -4))
        .overlay { ScrollInterruptTap(action: jumpToLatest) }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("New messages")
        .accessibilityHint("Scrolls to the latest message")
        .accessibilityIdentifier("chat-jump-to-latest")
        .accessibilityAddTraits(.isButton)
        .accessibilityAction { jumpToLatest() }
    }

    /// The transcript's swipe recognizer (`TranscriptSwipeHost`): the reply swipe on a row and
    /// the iMessage time peek, both decided on the first points so a vertical pan is the
    /// scroll view's alone. A nested table scrolling sideways undoes a peek in progress
    /// ([feedback ACQD4mUCTPxH7z3Y7YO8Vjw]).
    private func handleTranscriptSwipe(_ event: TranscriptSwipeEvent) {
        var still = Transaction()
        still.disablesAnimations = true
        switch event {
        case .replyChanged(let row, let dx):
            let reveal = BubbleSwipe.reveal(for: dx)
            let armed = BubbleSwipe.triggers(reveal: reveal)
            if armed, !replySwipe.armed { Haptics.pick() }
            withTransaction(still) { replySwipe = ReplySwipe(rowId: row, reveal: reveal, armed: armed) }
        case .replyEnded(let row, let fire):
            withAnimation(.spring(response: 0.34, dampingFraction: 0.86)) { replySwipe = ReplySwipe() }
            if fire, let target = windowedRows.first(where: { $0.id == row }) { beginReply(to: target) }
        case .timeChanged(let dx):
            if NestedHorizontalScroll.claiming {
                if timeReveal != 0 { withTransaction(still) { timeReveal = 0 } }
                return
            }
            // iMessage feel: 1:1 up to the reveal width, then rubber-band
            // (a fifth of the excess, at most 24 pt) instead of a hard stop.
            let raw = max(0, -dx)
            let w = RowView.timeRevealWidth
            let reveal = raw <= w ? raw : w + min(24, (raw - w) * 0.2)
            withTransaction(still) { timeReveal = reveal }
        case .timeEnded:
            withAnimation(.spring(response: 0.34, dampingFraction: 0.86)) { timeReveal = 0 }
        }
    }

    // MARK: Transcript

    private func transcript(width: CGFloat, height: CGFloat) -> some View {
        ScrollViewReader { proxy in
            ScrollView {
                // VStack (not LazyVStack): a growing vertical TextField resizes the
                // bottom safe-area inset, and LazyVStack can drop every visible row
                // for a white transcript until the next layout pass.
                VStack(alignment: .leading, spacing: 0) {
                    // iOS 17 window growth: report this top edge's position in the scroll space.
                    Color.clear.frame(height: 0)
                        .background(
                            GeometryReader { geo in
                                Color.clear.preference(
                                    key: ChatScrollTopKey.self,
                                    value: geo.frame(in: .named("arbosChatScroll")).minY
                                )
                            }
                        )
                    Color.clear.frame(height: (showsMemberChips ? 104 : 64) + headerExtra).onAppear { scroller = proxy }
                    #if DEBUG
                    if DebugHooks.perfTouch {
                        ScrollPerfProbe().frame(height: 0)
                    }
                    #endif
                    if let banner = machineBannerText {
                        // Grok (Mac refs 69/70): a quiet line while the machine wakes. The error
                        // state is the pinned note under the header (`chatNote`).
                        HStack(spacing: 6) {
                            ProgressView().controlSize(.mini).tint(theme.muted)
                            Text(banner)
                        }
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                        .frame(maxWidth: .infinity)
                        .padding(.bottom, 16)
                    }
                    if !model.caughtUp.contains(chatId) && messages.isEmpty && !model.isFixture {
                        // Known-empty (optimistic new chat seeds `messages[id] = []`):
                        // show the empty hint at once — no spinner flash while the
                        // room catches up on a thread we already know has no rows.
                        if model.hasKnownEmptyTranscript(chatId) {
                            if !isPeople, !isChannel { emptyHint }
                        } else {
                            HStack { Spacer(); ProgressView().tint(theme.muted); Spacer() }.padding(.top, 60)
                        }
                    } else if rows.isEmpty, !isPeople, !isChannel {
                        // People chats and channels stay blank (iMessage-style); the agent
                        // empty copy would say "Ask @handle anything…" which is wrong here.
                        emptyHint
                    }
                    let shown = windowedRows
                    // One MentionTable for every bubble (Mac builds Mentions once per
                    // sync). Per-row rebuilds were the main group-chat lag on iPhone.
                    let mentions = MentionTable.Cache.shared.table(model: model, theme: theme)
                    // Equatable host: composer keystrokes re-evaluate ChatView but
                    // skip the ForEach when the transcript inputs are unchanged.
                    TranscriptRows(
                        model: model,
                        theme: theme,
                        composerInset: composerSideInset,
                        people: isPeople || isChannel,
                        mentions: mentions,
                        shown: shown,
                        shownStamp: Self.shownStamp(rowsVersion, windowCount, windowSkipEnd, searchText, showSearch),
                        chatId: chatId,
                        width: width,
                        timeReveal: timeReveal,
                        replySwipe: replySwipe,
                        rowFrames: rowFrames,
                        highlightedMessage: highlightedMessage,
                        rowsCount: rows.count,
                        messagesCount: messages.count,
                        canReply: { isMessageRow($0) },
                        onQuoteTap: { revealQuoted($0) },
                        onMenu: { runMessageMenu($0, row: $1) },
                        actionsRowId: actionsRow?.id,
                        onLongPress: { row in openActions(row) }
                    )
                    .equatable()
                    // Who is working is the presence row over the composer
                    // (`PresenceRow`), not a transcript row.
                    // Clears the 24 pt fade above the composer, so the last row
                    // is never under the chat bar; the status row rises into it
                    // (`Theme.presenceLift`).
                    Color.clear.frame(height: Theme.transcriptTail).id("bottom")
                }
                .padding(.horizontal, 16)
                // The rows report their frames in this space; the one swipe recognizer for
                // reply (right, on a row) and time peek (left) lives on the scroll view.
                .coordinateSpace(name: TranscriptSpace.name)
                .background {
                    if ChatGestures.replySwipe || ChatGestures.timeReveal {
                        TranscriptSwipeHost(frames: rowFrames, onEvent: handleTranscriptSwipe)
                    }
                }
                // The avatar column follows the `+` as the composer inset animates with the keyboard.
                .animation(.easeOut(duration: 0.25), value: composerFocused)
                // Short threads: fill the viewport and pin rows to the bottom so
                // two bubbles cannot float mid-screen (scroll-anchor alone was
                // centering them on iOS 26). Empty-state copy keeps its own
                // top padding; tall threads are unchanged.
                .frame(
                    minHeight: (visibleRows.isEmpty && !streaming) ? 0 : height,
                    alignment: .bottom
                )
                // Fill the short-thread frame so taps on empty chrome hit this view.
                .contentShape(Rectangle())
            }
            .defaultScrollAnchor(.bottom)
            .scrollDismissesKeyboard(.interactively)
            // Tap empty chat chrome to dismiss the keyboard and free the transcript.
            .onTapGesture {
                composerFocused = false
                searchFocused = false
            }
            .modifier(AwayFromBottomReporter(away: $awayFromBottom, pinned: $pinnedToBottom) {
                // Streaming / new-row growth while pinned: re-stick without animation so
                // each delta glides with layout instead of fighting a scroll animation.
                proxy.scrollTo("bottom", anchor: .bottom)
            })
            // Nearing the top: the next older slice of rows joins the view tree; a channel
            // also asks the server for one older page.
            .modifier(NearTopReporter(enabled: true) {
                // The first layout reports offset 0 before the bottom anchor applies; a
                // grow step there (30 more bubbles) landed inside the open and cost a
                // 200–300 ms frame on a long group. History waits a beat after appear.
                guard Date().timeIntervalSince(appearedAt) > 1.0 else { return }
                if windowCount + windowSkipEnd < visibleRows.count {
                    growWindow()
                } else if isChannel {
                    model.loadOlderChannel(chatId)
                }
            })
            // Top fade is `headerBlur` over the transcript — a ScrollView `.mask`
            // forced an offscreen pass of every row on each scroll frame and made
            // long / group chats feel sticky (feedback app-01M5MQ2GFB1681YXNJMZV4EK7J).
            .onAppear { settleToBottom(proxy) }
            // `rows.last` (not only its id): a streaming reply keeps the same id while its
            // text grows; pinning must follow that growth too (#1444 / lazy window #1399).
            .onChange(of: rows.last) { old, new in
                if old == nil {
                    settleToBottom(proxy)
                } else if pinnedToBottom {
                    // Stick only while the reader is following the end.
                    if old?.id != new?.id {
                        withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo("bottom", anchor: .bottom) }
                    } else {
                        proxy.scrollTo("bottom", anchor: .bottom)
                    }
                } else if let new, old?.id != new.id {
                    // Reading history: a newer end row (not an older-page prepend) → pill.
                    if let oldId = old?.id, let ix = rows.lastIndex(where: { $0.id == oldId }) {
                        newBelowCount += max(0, rows.count - 1 - ix)
                    } else {
                        newBelowCount += 1
                    }
                }
            }
            .onChange(of: rows.count) { old, new in
                // The first snapshot can land as many rows at once after the
                // view is up (tap-open before the room's backfill, debug hook).
                if old == 0, new > 0 { settleToBottom(proxy) }
            }
            .onChange(of: model.caughtUp.contains(chatId)) { _, ready in
                if ready { settleToBottom(proxy) }
            }
            .onChange(of: streaming) { _, _ in
                guard pinnedToBottom else { return }
                withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo("bottom", anchor: .bottom) }
            }
            .onChange(of: composerFocused) { _, focused in
                guard focused else { return }
                pinnedToBottom = true
                awayFromBottom = false
                newBelowCount = 0
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) {
                    withAnimation(.easeOut(duration: 0.25)) { proxy.scrollTo("bottom", anchor: .bottom) }
                }
            }
            .onChange(of: awayFromBottom) { _, away in
                // Reader scrolled back to the end by hand — clear the pill; when the window
                // had slid into history, its end is not the tail: jump to the real end.
                if !away {
                    newBelowCount = 0
                    if windowSkipEnd > 0 { settleToBottom(proxy) }
                }
            }
        }
    }

    #if DEBUG
    /// `ARBOS_PERF_DRIVE` (perf rig): scripted scroll / typing / typing-row load so the
    /// frame log can be read per phase (`phase … begin/end` lines in the `perf` log).
    private func runPerfDrive(_ drive: Set<String>) async {
        try? await Task.sleep(for: .seconds(4))
        if drive.contains("scroll") {
            PerfLog.mark("phase scroll begin", ms: 0)
            // Page up ~12 rows at a time to the oldest shown row (growing the window on
            // the way), then page back down. Each hop is an animated scrollTo, so the
            // rows between are laid out and drawn like a flick.
            var hops = 0
            while hops < 60 {
                let shown = windowedRows
                guard let proxy = scroller, let topId = shown.first?.id else { break }
                let anchorIx = max(0, shown.count - 1 - (hops + 1) * 12)
                let target = shown[anchorIx].id
                withAnimation(.easeInOut(duration: 0.35)) { proxy.scrollTo(target, anchor: .top) }
                try? await Task.sleep(for: .milliseconds(500))
                hops += 1
                if target == topId, windowCount + windowSkipEnd >= visibleRows.count { break }
            }
            PerfLog.mark("phase scroll top", ms: 0)
            try? await Task.sleep(for: .seconds(1))
            withAnimation(.easeInOut(duration: 0.6)) { scroller?.scrollTo("bottom", anchor: .bottom) }
            try? await Task.sleep(for: .seconds(2))
            PerfLog.mark("phase scroll end", ms: 0)
        }
        if drive.contains("jump") {
            // Deep into history (the window has slid past its cap, or the top is
            // reached), then the ↓ button's own action; "stable" = at the bottom
            // and the scroll geometry quiet for 250 ms. Bubble bodies built on the
            // way come out of the counters (`rowBody`).
            for _ in 0..<100 where rows.isEmpty || scroller == nil {
                try? await Task.sleep(for: .milliseconds(200))
            }
            try? await Task.sleep(for: .seconds(2))
            var hops = 0
            while hops < 80 {
                let shown = windowedRows
                guard let proxy = scroller, let topId = shown.first?.id else { break }
                let target = shown[max(0, shown.count - 1 - (hops + 1) * 12)].id
                withAnimation(.easeInOut(duration: 0.35)) { proxy.scrollTo(target, anchor: .top) }
                try? await Task.sleep(for: .milliseconds(500))
                hops += 1
                if windowSkipEnd >= 300 || (target == topId && windowCount + windowSkipEnd >= visibleRows.count) { break }
            }
            if DebugHooks.perfDrive.contains("deep"), windowSkipEnd < 300 {
                // A reader 600 rows into history after the window slid: the state
                // `growWindow` leaves behind, set directly so a run stays short.
                var t = Transaction()
                t.disablesAnimations = true
                withTransaction(t) {
                    windowSkipEnd = min(600, max(0, visibleRows.count - Self.windowMax - 1))
                    windowCount = Self.windowMax
                }
                try? await Task.sleep(for: .milliseconds(600))
                if let first = windowedRows.first?.id { scroller?.scrollTo(first, anchor: .top) }
            }
            try? await Task.sleep(for: .milliseconds(1500))
            _ = PerfCounters.drain()
            PerfLog.mark("jump tap skipEnd=\(windowSkipEnd) window=\(windowCount) rows=\(rows.count) away=\(awayFromBottom)", ms: 0)
            let t0 = Date()
            PerfDriveProbe.lastScrollGeometry = t0
            jumpToLatest()
            var stableAt: Date?
            for _ in 0..<400 {
                try? await Task.sleep(for: .milliseconds(16))
                let last = PerfDriveProbe.lastScrollGeometry
                if !awayFromBottom, Date().timeIntervalSince(last) >= 0.25 {
                    stableAt = last
                    break
                }
            }
            let ms = (stableAt ?? Date()).timeIntervalSince(t0) * 1000
            PerfLog.mark("jump stable\(stableAt == nil ? " (timeout)" : "") skipEnd=\(windowSkipEnd) window=\(windowCount) \(PerfCounters.drain())", ms: ms)
        }
        if drive.contains("type") {
            PerfLog.mark("phase type begin", ms: 0)
            composerFocused = true
            try? await Task.sleep(for: .milliseconds(800))
            for ch in "hey everyone, is the launch call still at ten?" {
                let t = Date()
                composerDraft.text.append(ch)
                await Task.yield()
                PerfCounters.time("keystroke", ms: Date().timeIntervalSince(t) * 1000)
                try? await Task.sleep(for: .milliseconds(90))
            }
            try? await Task.sleep(for: .seconds(1))
            PerfLog.mark("phase type end", ms: 0)
            composerDraft.text = ""
            composerFocused = false
        }
        if drive.contains("typing") {
            PerfLog.mark("phase typing begin", ms: 0)
            let handles = (people?.humans ?? []).map(\.handle).filter { $0 != model.user?.login }.prefix(3)
            for i in 0..<8 {
                for h in handles {
                    model.noteTyping(["handle": h, "typing": i % 2 == 0], chatId: chatId)
                }
                try? await Task.sleep(for: .seconds(1))
            }
            PerfLog.mark("phase typing end", ms: 0)
        }
        PerfLog.mark("phase done", ms: 0)
    }
    #endif

    /// Pin the transcript to its end once the rows have a height: the first
    /// `scrollTo` runs before layout knows the content size, so repeat after
    /// the next runloop turn and once more after the rows settle.
    private func settleToBottom(_ proxy: ScrollViewProxy) {
        pinnedToBottom = true
        awayFromBottom = false
        newBelowCount = 0
        resetWindowToTail()
        proxy.scrollTo("bottom", anchor: .bottom)
        DispatchQueue.main.async { proxy.scrollTo("bottom", anchor: .bottom) }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { proxy.scrollTo("bottom", anchor: .bottom) }
    }

    private func m_text(_ row: Row) -> String {
        messageById[row.messageId]?.plainText ?? ""
    }

    /// "@sam" / "Bob" (the agent of this chat) / "you" for the composer's reply preview.
    /// My own rows carry `sender: user:<my id>` in shared projects — still "you".
    private func replyLabel(for m: Message) -> String {
        if let h = m.senderHandle, m.sender != "user", m.sender != model.myUserId { return "@\(h)" }
        if m.role == .assistant || m.role == .project { return project?.name ?? "the agent" }
        return "you"
    }

    /// Tap on a reply bubble's quote: scroll to the original and tint it for a moment. A
    /// cross-chat quote (Reply in DM) opens the source chat on the original instead; the
    /// header is not tappable when the viewer is not in that chat.
    private func revealQuoted(_ quote: Quote) {
        if let src = quote.source {
            if src.chatId == chatId { revealQuoted(src.messageId) } else { model.reveal(src) }
            return
        }
        revealQuoted(quote.messageId)
    }

    private func revealQuoted(_ messageId: Id) {
        guard let target = rows.first(where: { $0.messageId == messageId && $0.role != .system }) else { return }
        if includeInWindow(messageId: messageId) {
            // The original was outside the window: let the grown VStack lay out first.
            DispatchQueue.main.async {
                withAnimation(.easeInOut(duration: 0.35)) { scroller?.scrollTo(target.id, anchor: .center) }
            }
        } else {
            withAnimation(.easeInOut(duration: 0.35)) { scroller?.scrollTo(target.id, anchor: .center) }
        }
        highlightedMessage = messageId
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(1600))
            if highlightedMessage == messageId { highlightedMessage = nil }
        }
    }

    /// A real message in the shared chat doc (the full menu: Copy, Reply,
    /// Forward, Reply in DM, Delete) — not a date / join / system line.
    /// Channels carry no replies or deletes yet (C1 scope): Copy only.
    private func isMessageRow(_ row: Row) -> Bool {
        if isChannel { return false }
        switch row.kind {
        // A secret ghost row has nothing to copy, forward or quote; Delete stays off too —
        // the row is the record that a value went to the vault.
        case .system, .question, .error, .forwardHint, .secretGhost, .hidden:
            return false
        case .user, .other, .member, .assistant, .file, .voiceMemo, .image:
            return messageById[row.messageId] != nil
        }
    }

    /// Long-press on a bubble: the actions overlay (capsule + menu). The keyboard goes
    /// down first so nothing of the menu sits under it (iMessage / Grok Bot; feedback
    /// iphone-bubble-menu-keyboard-2026-10-25) — the typed draft stays in `composerDraft`,
    /// only focus moves, and `closeActions` gives it back.
    private func openActions(_ row: Row) {
        Haptics.pick()
        actionsRestoresKeyboard = composerFocused
        composerFocused = false
        searchFocused = false
        withAnimation(.easeOut(duration: 0.18)) { actionsRow = row }
    }

    /// Close the overlay; `item` is the menu action chosen (nil for a tap outside or a
    /// reaction). The composer is focused again only if it was before the press and the
    /// action leaves the chat on screen (`MessageActionsKeyboard.restoresFocus`).
    private func closeActions(after item: MessageMenuItem? = nil) {
        let restore = MessageActionsKeyboard.restoresFocus(wasFocused: actionsRestoresKeyboard, after: item)
        actionsRestoresKeyboard = false
        withAnimation(.easeOut(duration: 0.15)) { actionsRow = nil }
        if restore { composerFocused = true }
    }

    /// Emoji I already put on this message (tinted in the capsule).
    private func heldReactions(_ row: Row) -> Set<String> {
        guard let me = model.myUserId, let m = messageById[row.messageId] else { return [] }
        return Set(m.reactions.filter { $0.senders.contains(me) }.map(\.emoji))
    }

    /// The relay's delete rule for this row (`MessageDeleteRule`): owners moderate; members
    /// delete their own and their own agents'.
    private func canDelete(_ row: Row) -> Bool {
        guard let m = messageById[row.messageId] else { return false }
        let meId = model.user?.id
        let moderates: Bool
        var agentOwners: [String: String] = [:]
        if let people {
            moderates = people.myRole == "owner" || people.myRole == "admin"
            for a in people.agents { if let owner = a.ownerId { agentOwners[a.id] = owner } }
        } else if isChannel {
            moderates = false
        } else {
            moderates = !(project?.shared ?? false)
        }
        return MessageDeleteRule.mayDelete(sender: m.sender, meId: meId, moderates: moderates, agentOwnerIds: agentOwners)
    }

    /// The long-press menu's actions (`MessageMenu`): Copy the text, Reply
    /// (in a people chat the agent that wrote it becomes the recipient),
    /// View thread (when the message has replies), Forward to another chat,
    /// Reply in DM (open a private chat with the human who wrote it), Edit
    /// (own text), Delete for everyone.
    private func runMessageMenu(_ item: MessageMenuItem, row: Row) {
        switch item {
        case .copy:
            UIPasteboard.general.string = m_text(row)
        case .reply:
            beginReply(to: row)
        case .viewThread:
            openThread(for: row)
        case .forward:
            beginForward(row)
        case .replyInDM:
            // Open (or create) the DM with that person (#1455) with a cross-chat reply
            // to this message armed there (#1836): the DM bubble quotes it and names
            // this chat. Agents and your own messages have no peer to DM.
            guard let m = messageById[row.messageId] else { return }
            model.replyInDM(to: m, in: chatId, projectId: isPeople ? nil : projectId)
        case .edit:
            beginEdit(row)
        case .removePreview:
            model.removeLinkPreview(chatId: chatId, messageId: row.messageId, projectId: isPeople ? nil : projectId)
        case .delete:
            // Deletes at once, for everyone (Jacob, feedback remove-delete-confirms-2026-10-24:
            // no "Delete message?" step); the menu only offers Delete where `canDelete` holds.
            model.deleteMessage(chatId: chatId, messageId: row.messageId, projectId: isPeople ? nil : projectId)
        }
    }

    /// Long-press → View thread: open the held message and every reply in its chain.
    private func openThread(for row: Row) {
        guard let root = messageById[row.messageId] else { return }
        let replies = Transcript.threadReplies(rootId: root.id, in: messages)
        guard !replies.isEmpty else { return }
        pendingThread = PendingThread(root: root, replies: replies)
    }

    /// Own user text the long-press menu may offer Edit for.
    private func canEditMessage(_ row: Row) -> Bool {
        guard isMessageRow(row), let m = messageById[row.messageId] else { return false }
        guard isOwnMessage(m) else { return false }
        return !m.plainText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// A message this device's user wrote (people `user:<id>`, or bare `user` in
    /// a solo project chat).
    private func isOwnMessage(_ m: Message) -> Bool {
        guard m.role == .user else { return false }
        if let me = model.myUserId, m.sender == me { return true }
        return m.sender == "user" || m.sender.isEmpty
    }


    /// Long-press → Forward: open the chat picker with this row's text.
    private func beginForward(_ row: Row) {
        guard let m = messageById[row.messageId] else { return }
        let text = m.plainText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        // "Forwarded from <name>" names me by my display name, not "You".
        let from = m.role == .user && m.senderName == nil && (m.sender == "user" || m.sender == model.myUserId)
            ? (model.user.map { me in me.name.flatMap { $0.isEmpty ? nil : $0 } ?? me.login } ?? "You")
            : MediaAlbum.senderName(m) { model.project($0)?.name }
        pendingForward = PendingForward(text: text, from: from)
    }

    /// Reply to this row: composer preview + focus (people chats also address the agent).
    /// Long-press → Reply on the bubble, and the bubble's swipe right (`BubbleSwipe`).
    private func beginReply(to row: Row) {
        guard let m = messageById[row.messageId] else { return }
        editing = nil
        replyTo = m
        if isPeople, let member = agentMember(for: m)?.member {
            talkTo = [member]
        }
        composerFocused = true
    }

    /// Edit this own message: put its text in the composer with an Editing bar.
    private func beginEdit(_ row: Row) {
        guard let m = messageById[row.messageId] else { return }
        let text = m.plainText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, isOwnMessage(m) else { return }
        replyTo = nil
        editing = m
        composerDraft.text = m.plainText
        composerDraft.caret = (m.plainText as NSString).length
        composerDraft.mentions = MentionDraft.resolveAll(text: m.plainText, mentions: [], members: people?.members ?? [], me: model.user?.login, collectives: isPeople)
        composerFocused = true
    }

    /// Spacing: 8 pt between blocks of one message, 24 pt around sender changes,
    /// system lines, and each new agent reply (quick ack vs main answer must not
    /// look merged — #1447). Measured on the references.

    /// The machine's waking line (the scroll's quiet banner). The error state is the pinned
    /// `chatNote`; the link has no line here: the header's connection pill is its one place.
    private var machineBannerText: String? {
        // A channel lives on the servers' link, not the machine.
        if isChannel { return nil }
        switch model.machine.state {
        case .waking: return "Waking your machine…"
        case .error, .awake, .hibernatedLight, .hibernatedDeep, .stoppedByUser:
            return nil
        }
    }

    /// The chat-level failure, when there is one: the room's pushes refused
    /// (`not_available`, cleared by the room's next ACK) first, else the machine in error.
    private var chatLevelError: UserError? {
        if model.pushRefused.contains(chatId) {
            let why = model.pushRefusalText[chatId] ?? "Messages in this chat aren't being delivered right now. We'll keep trying."
            return UserError(verb: "", class: .refused(message: why), raw: "the relay refused the push: not_available")
        }
        if !isChannel, model.machine.state == .error {
            let detail = model.machineDetail ?? "machine state: error"
            return UserError(verb: "", class: .refused(message: "Can't reach your machine. Your agents are safe — it just can't be reached right now."),
                             raw: detail)
        }
        return nil
    }

    @ViewBuilder private var chatNote: some View {
        if let error = chatLevelError {
            ErrorNote(id: model.pushRefused.contains(chatId) ? "delivery-error" : "machine-error",
                      error: error,
                      detailsOpen: $chatNoteDetailsOpen,
                      onTryAgain: model.pushRefused.contains(chatId) ? nil : { model.startMachine() })
        }
    }

    /// The connection pill under the name pill ([feedback connection-pill], Jacob
    /// Oct 22): one small centred capsule — a status dot and two words — for
    /// whatever is wrong with this chat's link (offline > relay down >
    /// reconnecting > syncing), nothing when it is fine. It replaced the
    /// "Reconnecting…" line at the top of the transcript.
    @ViewBuilder private var linkPill: some View {
        let _ = model.linkPillRepaint
        if let state = model.linkPill(for: chatId, channel: isChannel), let label = state.label {
            let dot: Color = {
                switch state {
                case .offline: return theme.muted.opacity(0.6)
                case .relayDown: return theme.danger
                case .reconnecting, .restartSoon: return theme.warn
                case .syncing: return theme.accent
                case .live: return .clear
                }
            }()
            let pulsing = state == .reconnecting || state == .syncing || state == .restartSoon
            HStack(spacing: 6) {
                Circle()
                    .fill(dot)
                    .frame(width: 6, height: 6)
                    .opacity(pulsing ? (pulsePhase ? 1 : 0.5) : 1)
                    .animation(pulsing ? .easeInOut(duration: 0.7).repeatForever(autoreverses: true) : .default, value: pulsePhase)
                Text(label)
                    .font(theme.font(Theme.tiny, weight: .medium))
                    .foregroundStyle(theme.muted)
            }
            .padding(.leading, 9)
            .padding(.trailing, 11)
            .frame(height: 22)
            .background(Capsule().fill(theme.field)) // control-size: exception inlineChips
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("link-pill")
            .onAppear { pulsePhase = true }
            .onDisappear { pulsePhase = false }
            .transition(.opacity.combined(with: .move(edge: .top)))
        }
    }

    private var emptyHint: some View {
        VStack(spacing: 10) {
            if let project {
                AgentAvatar(project: project, theme: theme, size: 64)
            }
            Text(name)
                .font(theme.font(20, weight: .semibold))
                .foregroundStyle(theme.text)
            Text("Ask \(name) anything. It plans, delegates, or answers.")
                .font(theme.font(Theme.small))
                .foregroundStyle(theme.muted)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 140)
        .padding(.bottom, 40)
    }

    /// Avatars + handles of everyone in a people chat; agents show their owner.
    /// Tap a chip to insert `@handle` into the composer and address the next send.
    /// Agent chips toggle so several can be selected together (that combination all replies).
    /// Soft-deleted agents (no nested project) are hidden — they cannot reply.
    private func memberChips(_ people: PeopleChat) -> some View {
        // Skip me — you don't address yourself; keeps a 1:1 DM to one chip + Agent.
        let others = people.members.filter { $0.id != model.user?.id && (!$0.isAgent || $0.project != nil) }
        return ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(others) { m in
                    let selected = talkTo.contains(m.member)
                        || (talkTo.isEmpty && effectiveTalkTo == m.member)
                    Button {
                        // Agent chips toggle so combinations can all respond. Person chips stay
                        // single-select. Tapping the only selected chip clears it — do not promote
                        // an inferred / reply-only selection into a sticky talkTo.
                        if talkTo.contains(m.member) {
                            talkTo.remove(m.member)
                        } else if talkTo.isEmpty, effectiveTalkTo == m.member {
                            talkTo = []
                        } else if m.isAgent {
                            talkTo = Set(talkTo.filter { $0.hasPrefix("agent:") })
                            talkTo.insert(m.member)
                            ensureMention(m)
                            composerFocused = true
                        } else {
                            // Same path as the @-picker: put `@handle` in the draft and set talkTo.
                            completeMention(m)
                            composerFocused = true
                        }
                        replyTo = nil
                        editing = nil
                    } label: {
                        HStack(spacing: 6) {
                            if m.isAgent {
                                AgentAvatar(project: model.agent(member: m, chatId: chatId), theme: theme, size: 18, still: true)
                            } else {
                                PersonAvatar(url: m.avatarUrl, handle: m.handle, size: 18, name: m.name, id: m.id, colorIndex: m.avatarColorIndex)
                            }
                            Text(m.isAgent ? "@\(mentionSlug(m.project?.name ?? m.handle))" : "@\(m.handle)")
                                .font(theme.font(Theme.tiny, weight: .medium))
                                .foregroundStyle(theme.text)
                            if m.isAgent, let o = m.ownerHandle {
                                Text("· @\(o)").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                            }
                        }
                        .padding(.leading, 6).padding(.trailing, 10).padding(.vertical, 5)
                        .background(Capsule().fill(selected ? theme.bubbleUser.opacity(theme.isDark ? 0.55 : 0.18) : theme.bubbleAssistant))
                        .overlay(Capsule().strokeBorder(selected ? theme.text.opacity(0.35) : Color.clear, lineWidth: 1))
                        .contentShape(Capsule().inset(by: -8))
                    }
                    .buttonStyle(.plain)
                    .contextMenu {
                        Button {
                            if m.isAgent { model.showProfile(.agent(projectId: m.id)) } else { model.showProfile(.user(ProfilePerson(m))) }
                        } label: { Label("Profile", systemImage: "person.crop.circle") }
                    }
                    .accessibilityLabel(selected ? "Talking to \(m.displayName)" : "Talk to \(m.displayName)")
                }
                // "+ Agent" chip removed (2026-10-20): agents are added from
                // the profile page's Add members.
            }
            // Room for avatar + padding so a horizontal ScrollView does not clip chips.
            .padding(.vertical, 4)
        }
        // Cap height so the chip strip cannot expand vertically inside the header stack
        // (horizontal ScrollView is flexible on both axes by default).
        .frame(height: 36)
    }

    /// The presence row over the composer: ticks once a second so time-capped
    /// states (stale stream, hand-off grace, awaiting-reply cheat, a typing
    /// mark's 6 s) expire without a stuck line. Left edge = the bubbles' 16 pt.
    @ViewBuilder
    private var presenceRow: some View {
        TimelineView(.periodic(from: .now, by: 1)) { _ in
            let participants = presenceParticipants
            if !participants.isEmpty {
                // The tasks chip rides on the row while the agent has background jobs
                // open in this chat (duplex §2.4); a tap opens the sheet.
                let jobs = model.openJobs(chatId)
                let openTasks: (() -> Void)? = jobs.isEmpty ? nil : { showTasks = true }
                // The words link to the newest job's tab (not inside a job tab); the chip opens the list.
                let openNewest: (() -> Void)? = (jobs.isEmpty || isJobTab) ? nil : { model.openNewestJobTab(from: chatId) }
                PresenceRow(participants: participants,
                            detail: model.currentActivity(chatId),
                            queued: false,
                            taskCount: jobs.count,
                            onTasks: openTasks,
                            onWords: openNewest,
                            onPicture: { model.toggleProfile($0) },
                            pictureShowing: { model.profileShows($0) })
                    .padding(.horizontal, 16)
                    // Grok: 68 px from the picture's bottom to the composer's top.
                    .padding(.bottom, Theme.presenceBottomGap)
                    // Grok: 42 px from the last bubble to the picture's top — the row
                    // rises into the transcript's tail instead of stacking under it
                    // ([feedback status-row-gap-ios-2026-10-26]).
                    .padding(.top, -Theme.presenceLift)
            }
        }
    }

    /// Steer a job (contract: no command of its own): the composer becomes a reply to
    /// the job's ask and the user types the correction; the agent binds a correction
    /// that quotes the ask to that job.
    private func steerJob(_ job: Job) {
        guard let askId = job.askMessageId, let ask = messageById[askId] ?? messages.first(where: { $0.id == askId }) else {
            model.failRefused("steer the task", "That task's request is not in this chat any more.")
            return
        }
        showTasks = false
        editing = nil
        replyTo = ask
        composerFocused = true
    }

    // MARK: Composer

    /// The reply preview above the field: one fixed-height row — glyph, "Replying to
    /// @handle", one truncated line of the quoted message (newlines and markdown
    /// flattened), × — whatever the original's length ([feedback
    /// reply-preview-fixed-height]; same bar on the Mac). A quoted picture adds a small
    /// square thumbnail inside the row.
    private func replyPreview(_ r: Message) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "arrowshape.turn.up.left").font(.system(size: 12)).foregroundStyle(theme.muted)
            Text("Replying to \(replyLabel(for: r))").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                .lineLimit(1).fixedSize()
            if let ref = quotedImageRef(r) {
                ReplyThumb(projectId: projectId, blobRef: ref)
            }
            Text(Quote.line(of: r, maxChars: Theme.replyPreviewChars))
                .font(theme.font(Theme.tiny)).foregroundStyle(theme.text)
                .lineLimit(1).truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button { replyTo = nil } label: { Image(systemName: "xmark").font(.system(size: 11, weight: .bold)).foregroundStyle(theme.muted) }
                .buttonStyle(.plain)
        }
        .padding(.horizontal, 14)
        // Full composer width: the same edges as the message field (Jacob, Oct 21).
        .frame(maxWidth: .infinity)
        .frame(height: Theme.replyPreviewHeight)
        .clipped()
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(theme.bubbleAssistant))
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("composer-reply-preview")
    }

    /// Reply in DM: the same bar, "Replying to Jacob in Arbonites" + the snapshot the row
    /// will carry (#1836).
    private func replyRefPreview(_ r: ReplyRef) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "arrowshape.turn.up.left").font(.system(size: 12)).foregroundStyle(theme.muted)
            Text(r.label).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                .lineLimit(1).fixedSize()
            Text(Quote.firstWords(Quote.oneLine(r.text), maxChars: Theme.replyPreviewChars))
                .font(theme.font(Theme.tiny)).foregroundStyle(theme.text)
                .lineLimit(1).truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button { replyRef = nil } label: { Image(systemName: "xmark").font(.system(size: 11, weight: .bold)).foregroundStyle(theme.muted) }
                .buttonStyle(.plain)
        }
        .padding(.horizontal, 14)
        .frame(maxWidth: .infinity)
        .frame(height: Theme.replyPreviewHeight)
        .clipped()
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(theme.bubbleAssistant))
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("composer-reply-ref")
    }

    /// Same height/shape as the reply preview: glyph, "Editing", one truncated line, ×.
    private func editPreview(_ m: Message) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "pencil").font(.system(size: 12)).foregroundStyle(theme.muted)
            Text("Editing").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                .lineLimit(1).fixedSize()
            Text(Quote.line(of: m, maxChars: Theme.replyPreviewChars))
                .font(theme.font(Theme.tiny)).foregroundStyle(theme.text)
                .lineLimit(1).truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button { editing = nil } label: { Image(systemName: "xmark").font(.system(size: 11, weight: .bold)).foregroundStyle(theme.muted) }
                .buttonStyle(.plain)
        }
        .padding(.horizontal, 14)
        .frame(maxWidth: .infinity)
        .frame(height: Theme.replyPreviewHeight)
        .clipped()
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(theme.bubbleAssistant))
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Editing message")
    }

    /// The blob of the first picture on the quoted message, for the preview's thumbnail.
    private func quotedImageRef(_ m: Message) -> String? {
        for part in m.parts {
            if case .attachment(_, let a) = part, a.isImage, let ref = a.blobRef { return ref }
        }
        return nil
    }

    private var composer: some View {
        VStack(spacing: 8) {
            if let q = mentionQuery, isPeople, let people {
                mentionList(people, query: q)
            }
            if let e = editing {
                editPreview(e)
            } else if let r = replyRef {
                replyRefPreview(r)
            } else if let r = replyTo {
                replyPreview(r)
            }
            if !pending.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(pending) { a in
                            // Images show a real thumbnail (same 28 pt chip as Mac);
                            // other files keep the SF Symbol + name pill.
                            let thumb = a.mime.hasPrefix("image/") ? UIImage(data: a.data) : nil
                            HStack(spacing: 6) {
                                if let thumb {
                                    Image(uiImage: thumb)
                                        .resizable()
                                        .scaledToFill()
                                        .frame(width: 28, height: 28)
                                        .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
                                } else {
                                    Image(systemName: a.mime.hasPrefix("image/") ? "photo" : (a.mime.hasPrefix("audio/") ? "waveform" : "doc"))
                                        .font(theme.font(12))
                                }
                                Text(a.name).font(theme.font(Theme.tiny)).lineLimit(1)
                                // The remove ✕: always drawn, a full-size hit target
                                // (not the 10 pt glyph alone), named for VoiceOver
                                // ([feedback composer-attachment-removal-ios-2026-10-27]).
                                Button { removePending(a.id) } label: {
                                    Image(systemName: "xmark").font(.system(size: Theme.pendingRemoveGlyph, weight: .bold))
                                        .frame(width: Theme.pendingRemoveHit, height: Theme.pendingRemoveHit)
                                        .contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)
                                .accessibilityLabel("Remove attachment")
                                .accessibilityIdentifier("composer-remove-attachment")
                            }
                            .foregroundStyle(theme.text)
                            .padding(.leading, thumb != nil ? 4 : 10)
                            .padding(.trailing, Theme.pendingChipTrailing)
                            .padding(.vertical, thumb != nil ? 3 : 6)
                            .background(Capsule().fill(theme.bubbleAssistant))
                        }
                    }
                    .padding(.leading, Theme.plusButton + Theme.plusFieldGap)
                }
            }
            // Bottom-align the "+" with a multi-line field so growth expands upward.
            HStack(alignment: .bottom, spacing: Theme.plusFieldGap) {
                if !isChannel {
                    attachButton
                }
                textField
            }
        }
    }

    /// The chip's ✕: that one goes, the others and the draft stay.
    private func removePending(_ id: UUID) {
        pending = Self.removingPending(id, from: pending)
    }

    /// ⌫ on an empty draft (`MentionTextView.onDeleteBackwardEmpty`): the last
    /// pending attachment goes; with text in the draft the key edits text only.
    private func removeLastPending() {
        pending = Self.removingLastPending(from: pending, draftEmpty: composerDraft.text.isEmpty)
    }

    static func removingPending(_ id: UUID, from list: [AppModel.PendingAttachment]) -> [AppModel.PendingAttachment] {
        list.filter { $0.id != id }
    }

    static func removingLastPending(from list: [AppModel.PendingAttachment], draftEmpty: Bool) -> [AppModel.PendingAttachment] {
        guard draftEmpty, !list.isEmpty else { return list }
        return Array(list.dropLast())
    }

    /// The "+" before the field: photos and files. Channels send text only (C1), so none there.
    private var attachButton: some View {
        Button { showAttach = true } label: {
            ZStack {
                ControlSurface(shape: Circle())
                Image(systemName: "plus")
                    .font(theme.font(Theme.plusGlyph, weight: .regular)) // control-size: exception plusGlyph
                    .foregroundStyle(theme.text)
            }
            .frame(width: Theme.plusButton, height: Theme.plusButton)
            .contentShape(Circle())
            .accessibilityIdentifier("composer-plus")
        }
        .buttonStyle(.plain)
        .sheet(isPresented: $showAttach, onDismiss: demoDismissed) {
            AttachSheet {
                showAttach = false
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { showPhotos = true }
            } onFile: {
                showAttach = false
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { showFiles = true }
            }
            .environment(\.theme, theme)
        }
        .photosPicker(isPresented: $showPhotos, selection: $photoItem, matching: .any(of: [.images, .videos]))
        .fileImporter(isPresented: $showFiles, allowedContentTypes: [.item], allowsMultipleSelection: false) { result in
            if case .success(let urls) = result, let url = urls.first {
                let ok = url.startAccessingSecurityScopedResource()
                defer { if ok { url.stopAccessingSecurityScopedResource() } }
                if let data = try? Data(contentsOf: url) {
                    let mime = UTType(filenameExtension: url.pathExtension)?.preferredMIMEType ?? "application/octet-stream"
                    pending.append(.init(name: url.lastPathComponent, mime: mime, data: data))
                    if mime.hasPrefix("image/") || mime.hasPrefix("video/") {
                        composerFocused = true
                    }
                }
            }
        }
        .onChange(of: photoItem) { _, item in
            guard let item else { return }
            Task {
                if let data = try? await item.loadTransferable(type: Data.self) {
                    let type = item.supportedContentTypes.first
                    let picked = ChatView.imageForUpload(data, type: type)
                    // POSIX stamp (no locale letters, no `:` — voice memos use the same idea).
                    let stamp = ChatView.photoStamp.string(from: Date())
                    pending.append(.init(name: "Photo \(stamp).\(picked.ext)", mime: picked.mime, data: picked.data))
                    composerFocused = true
                }
                photoItem = nil
            }
        }
    }

    /// Picture or video pending in the composer — the field is their description bar.
    private static func isCaptionablePending(_ a: AppModel.PendingAttachment) -> Bool {
        let mime = a.mime.lowercased()
        return mime.hasPrefix("image/") || mime.hasPrefix("video/")
    }


    /// Composer field: type normally, or hold the mic to dictate. The mic stays
    /// mounted while recording so release can finish and send without a second tap.
    private var textField: some View {
        ComposerField(
            draft: composerDraft,
            recorder: recorder,
            focus: $composerFocused,
            chatId: chatId,
            model: model,
            isChannel: isChannel,
            needsAnswer: needsAnswer,
            hasPending: !pending.isEmpty,
            memoHoldActive: memoHoldActive,
            memoCancelArmed: memoCancelArmed,
            memoDragX: memoDragX,
            cancelDistance: Self.memoCancelDistance,
            prompt: { composerPrompt(for: $0) },
            onDeleteBackwardEmpty: removeLastPending,
            onCancelRecording: cancelMemoRecording,
            onMicDrag: { value in
                if !memoHoldActive {
                    memoHoldActive = true
                    memoPressStartedAt = Date()
                    memoFinishWhenPermission = false
                    memoCancelArmed = false
                    memoDragX = 0
                    if recorder.recording {
                        memoIgnoreRelease = true
                        finishMemoHold()
                    } else {
                        memoIgnoreRelease = false
                        Haptics.recordStart()
                        recorder.start()
                    }
                }
                // Slide left while holding to arm cancel (hold-to-talk).
                guard !memoIgnoreRelease else { return }
                let dx = value.translation.width
                memoDragX = min(0, dx)
                let armed = dx < -Self.memoCancelDistance
                if armed != memoCancelArmed {
                    memoCancelArmed = armed
                    if armed { Haptics.pick() }
                }
            },
            onMicEnd: { value in
                let held = Date().timeIntervalSince(memoPressStartedAt ?? Date())
                let cancel = memoCancelArmed || value.translation.width < -Self.memoCancelDistance
                memoHoldActive = false
                memoPressStartedAt = nil
                memoDragX = 0
                memoCancelArmed = false
                if memoIgnoreRelease {
                    memoIgnoreRelease = false
                    return
                }
                // Slide-left discard: drop the memo instead of sending.
                if cancel {
                    memoFinishWhenPermission = false
                    if recorder.recording { recorder.cancel() }
                    return
                }
                // Hold-to-talk: release after the threshold sends.
                if held >= 0.25 {
                    if recorder.recording {
                        finishMemoHold()
                    } else {
                        // Permission still pending; finish when it starts.
                        memoFinishWhenPermission = true
                    }
                }
                // Short tap: leave recording on (click-to-toggle).
            },
            onPrimary: {
                if recorder.recording {
                    memoHoldActive = false
                    memoIgnoreRelease = false
                    memoFinishWhenPermission = false
                    memoPressStartedAt = nil
                    memoCancelArmed = false
                    memoDragX = 0
                    finishMemoHold()
                } else if canSend {
                    send()
                } else if !isChannel {
                    requestCall(.voice)
                }
            }
        )
        // Hold released before permission returned: finish once recording starts.
        // A short tap leaves recording on (click-to-toggle), so do not auto-finish then.
        .onChange(of: recorder.recording) { _, isOn in
            guard isOn, memoFinishWhenPermission else { return }
            memoFinishWhenPermission = false
            finishMemoHold()
        }
    }

    /// Stop the hold-to-talk memo and send its transcript (no-op if too short).
    private func finishMemoHold() {
        guard recorder.recording else { return }
        Haptics.send()
        if let memo = recorder.stop() {
            sendMemo(.init(name: memo.name, mime: "audio/m4a", data: memo.data))
        }
    }

    /// Discard the in-progress voice memo (✕ or slide-to-cancel).
    private func cancelMemoRecording() {
        memoHoldActive = false
        memoIgnoreRelease = false
        memoFinishWhenPermission = false
        memoPressStartedAt = nil
        memoCancelArmed = false
        memoDragX = 0
        recorder.cancel()
    }

    /// Empty the field after a send (text, chips, caret, the saved draft).
    private func clearDraft() {
        composerDraft.text = ""
        composerDraft.mentions = []
        composerDraft.caret = 0
        model.setComposerDraft("", for: chatId)
    }

    private var canSend: Bool {
        !composerDraft.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !pending.isEmpty
    }


    /// `@` completion: the `@token` the caret sits at the end of (nil closes the picker).
    private func updateMentionQuery() {
        // Set only on change: this runs per keystroke from the draft hook, and the
        // screen must not re-render for a value that did not move.
        let next: String? = isPeople ? MentionDraft.openToken(text: composerDraft.text, caret: composerDraft.caret, mentions: composerDraft.mentions)?.query : nil
        if next != mentionQuery { mentionQuery = next }
    }

    /// The `@` picker (#1596, the approved mock): a card above the field inset
    /// like it, one 44 pt row per entity — 28 pt picture · name · `@handle`,
    /// hairlines between rows — people and agents in one list (behaviour 2).
    /// Tap inserts the chip.
    private func mentionList(_ people: PeopleChat, query: String) -> some View {
        let rows = MentionDraft.candidates(query: query, members: people.members, me: model.user?.login)
        // `@here` / `@agents` first (Jacob, 2026-10-24): every member of a group or people chat may use them.
        let collectives = MentionDraft.collectiveCandidates(query: query)
        let hairline = Rectangle().fill(theme.isDark ? Color.white.opacity(0.08) : Color.black.opacity(0.08))
            .frame(height: 0.5)
            .padding(.leading, 12 + 28 + 12)
            .padding(.trailing, 12)
        return Group {
            if !rows.isEmpty || !collectives.isEmpty {
                VStack(spacing: 0) {
                    ForEach(collectives, id: \.rawValue) { c in
                        Button { completeMention(.collective(c)) } label: {
                            HStack(spacing: 12) {
                                InitialsAvatar(name: c.handle, handle: c.handle, id: c.handle, size: 28)
                                HStack(alignment: .firstTextBaseline, spacing: 8) {
                                    Text("@" + c.handle)
                                        .font(theme.font(Theme.body)).foregroundStyle(theme.text).lineLimit(1)
                                    Text("· " + c.description)
                                        .font(theme.font(Theme.small)).foregroundStyle(theme.muted).lineLimit(1)
                                }
                                Spacer(minLength: 0)
                            }
                            .padding(.horizontal, 12)
                            .frame(height: 44)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        if c != collectives.last || !rows.isEmpty {
                            hairline
                        }
                    }
                    ForEach(Array(rows.enumerated()), id: \.element.id) { i, m in
                        Button { completeMention(m) } label: {
                            HStack(spacing: 12) {
                                if m.isAgent {
                                    AgentAvatar(project: model.agent(member: m, chatId: chatId), theme: theme, size: 28, still: true)
                                } else {
                                    PersonAvatar(url: m.avatarUrl, handle: m.handle, size: 28, name: m.name, id: m.id, colorIndex: m.avatarColorIndex)
                                }
                                HStack(alignment: .firstTextBaseline, spacing: 8) {
                                    Text(m.isAgent ? (m.project?.name ?? m.handle) : (m.name?.isEmpty == false ? m.name! : m.handle))
                                        .font(theme.font(Theme.body)).foregroundStyle(theme.text).lineLimit(1)
                                    Text("@" + ComposerMention.handle(of: m))
                                        .font(theme.font(Theme.small)).foregroundStyle(theme.muted).lineLimit(1)
                                    if let tag = MentionDraft.tag(for: m, among: rows, me: model.user?.login) {
                                        Text(tag).font(theme.font(Theme.tiny)).foregroundStyle(theme.placeholder)
                                    }
                                }
                                Spacer(minLength: 0)
                            }
                            .padding(.horizontal, 12)
                            .frame(height: 44)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        if i < rows.count - 1 {
                            hairline
                        }
                    }
                }
                .padding(.vertical, 6)
                .background(
                    RoundedRectangle(cornerRadius: 16, style: .continuous)
                        .fill(theme.card)
                        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous)
                            .stroke(theme.isDark ? Color.white.opacity(0.10) : Color.black.opacity(0.08), lineWidth: 0.5))
                        .shadow(color: .black.opacity(theme.isDark ? 0.4 : 0.12), radius: 14, y: 8)
                )
                .padding(.horizontal, composerSideInset)
            }
        }
    }

    /// The picked row becomes one chip over the open `@token` (behaviour 4).
    private func completeMention(_ m: ChatMember) {
        completeMention(.member(m))
    }

    /// A collective chip addresses everyone: no sticky conversation chip.
    private func completeMention(_ target: ComposerMentionTarget) {
        ensureMention(target)
        switch target {
        case .member(let m): talkTo = [m.member]
        case .collective: talkTo = []
        }
        mentionQuery = nil
    }

    private func ensureMention(_ m: ChatMember) {
        ensureMention(.member(m))
    }

    /// Put the chip in the draft: over the open `@token` when there is one,
    /// else at the end — unless the chip is already there (conversation chips).
    private func ensureMention(_ target: ComposerMentionTarget) {
        let probe = ComposerMention(range: 0..<0, target: target)
        let handle = probe.handle.lowercased()
        if composerDraft.mentions.contains(where: { $0.memberId == probe.memberId }) { return }
        var text = composerDraft.text
        var mentions = composerDraft.mentions
        let caret: Int
        if let open = MentionDraft.openToken(text: text, caret: composerDraft.caret, mentions: mentions) {
            caret = MentionDraft.insert(target, at: open.range, text: &text, mentions: &mentions)
        } else {
            let lower = text.lowercased()
            let present = lower.split(whereSeparator: { $0.isWhitespace }).contains(where: {
                $0.trimmingCharacters(in: CharacterSet(charactersIn: ".,!?:;")) == "@" + handle
            })
            if present {
                // Typed by hand: just make it the chip.
                let members: [ChatMember] = probe.member.map { [$0] } ?? []
                composerDraft.mentions = MentionDraft.resolveAll(text: text, mentions: mentions, members: members, me: nil)
                return
            }
            if !text.isEmpty, !text.hasSuffix(" ") { text += " " }
            let end = (text as NSString).length
            caret = MentionDraft.insert(target, at: end..<end, text: &text, mentions: &mentions)
        }
        composerDraft.mentions = mentions
        composerDraft.text = text
        composerDraft.caret = caret
    }

    /// Chip / inferred agent, or `"everyone"` when the draft says so — matches `composerPrompt`.
    /// A bare person `@handle` in the text drops a sticky agent so bob does not answer that ping.
    /// A person's display name alone does not drop an agent handle/slug match ("Arbos …" still
    /// addresses @arbos when a human is also named Arbos). A bare agent name ("Bob?") addresses
    /// that agent like `@bob`. Several selected agent chips (or "hey agents") address everyone /
    /// that combination via mentions on the send — see `chipMentions(for:)`.
    private func sendAddress(for text: String) -> String? {
        if talkingToEveryone || AppModel.addressesEveryone(text) { return "everyone" }
        if talkToAgents.count > 1 { return nil }
        let members = people?.members ?? []
        if AppModel.mentions(in: text, members: members, chipTokens: MentionDraft.tokens(composerDraft.mentions)).contains(where: { $0.hasPrefix("agent:") }) {
            return effectiveTalkTo
        }
        // Person @handle wins; display-name-only people matches must not block agent slugs.
        if !AppModel.namedPeopleByHandle(in: text, members: members).isEmpty {
            return nil
        }
        if let agent = AppModel.namedAgents(in: text, members: members).first {
            return agent
        }
        if !AppModel.namedPeople(in: text, members: members).isEmpty {
            return nil
        }
        return effectiveTalkTo
    }

    /// Extra agent member ids from the conversation chips when more than one is selected.
    private func chipMentions(for text: String) -> [String] {
        if talkingToEveryone || AppModel.addressesEveryone(text) { return [] }
        guard talkToAgents.count > 1 else { return [] }
        return talkToAgents
    }

    /// Finish dictation and send: transcript (plus any typed draft) goes out as text.
    /// The audio is only used for STT — it is not attached to the chat.
    private func sendMemo(_ memo: AppModel.PendingAttachment) {
        let typed = composerDraft.text.trimmingCharacters(in: .whitespacesAndNewlines)
        let extras = pending
        let reply = replyTo?.id
        let crossReply = replyRef
        let address = sendAddress(for: typed)
        let chips = chipMentions(for: typed)
        let tokens = MentionDraft.tokens(composerDraft.mentions)
        pending.removeAll()
        clearDraft()
        replyTo = nil
        replyRef = nil
        editing = nil
        mentionQuery = nil
        pinTalkTo(afterSending: address, chipMentions: chips)
        sentOnce = true
        transcribing = true
        Task {
            let sttProject: Id? = project?.id ?? (isPeople ? "\(peopleSlot):\(chatId)" : nil)
            let transcript = await model.transcribe(projectId: sttProject, memo: memo)
            transcribing = false
            let text = [typed, transcript ?? ""].filter { !$0.isEmpty }.joined(separator: "\n\n")
            guard !text.isEmpty || !extras.isEmpty else { return }
            if isChannel {
                if !text.isEmpty { model.sendChannel(chatId: chatId, text: text) }
            } else if isPeople {
                model.sendPeople(chatId: chatId, text: text, replyTo: reply, address: address,
                                 chipMentions: chips, attachments: extras, chipTokens: tokens,
                                 replyRef: crossReply)
            } else if needsAnswer && extras.isEmpty {
                model.answer(chatId: chatId, text: text)
            } else {
                model.send(chatId: chatId, text: text, attachments: extras, replyTo: reply)
            }
        }
    }

    /// After a send in a DM, keep an explicit agent chip selected so the next line stays with
    /// them. Do not promote a reply-only / @-only address into a sticky chip when a human peer
    /// is in the chat — otherwise Allan+bob DMs leave Message @bob on unrelated follow-ups.
    /// In a group, clear the chip so the next line is unaddressed and the decision model picks
    /// who replies (an explicit chip tap still addresses that send). Always clear everyone /
    /// multi-agent combinations.
    private func pinTalkTo(afterSending address: String?, chipMentions: [String]) {
        if address == "everyone" || people?.kind == "group" || chipMentions.count > 1 {
            talkTo = []
        } else if let address, address.hasPrefix("agent:") {
            let hasHumanPeer = people?.humans.contains { $0.id != model.user?.id } == true
            if hasHumanPeer, talkTo != [address] {
                // Reply-to or typed @agent addressed this send; leave the next line free.
                return
            }
            talkTo = [address]
        }
    }

    private func send() {
        let text = composerDraft.text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty || !pending.isEmpty else { return }
        #if DEBUG
        SendTrace.begin(chatId: chatId, rows: rows.count, caughtUp: model.caughtUp.contains(chatId))
        #endif
        Haptics.send()
        // A send means follow the reply: re-pin even if the reader had scrolled up.
        pinnedToBottom = true
        awayFromBottom = false
        newBelowCount = 0
        resetWindowToTail()
        if let edit = editing {
            // Save the edit in place — no new bubble, no send command.
            guard !text.isEmpty else { return }
            model.editMessage(chatId: chatId, messageId: edit.id, text: text)
            pending.removeAll()
            clearDraft()
            editing = nil
            replyTo = nil
            sentOnce = true
            return
        }
        if isChannel {
            // Rule 3/9: the composer sends through the shared client's `sendText`.
            guard !text.isEmpty else { return }
            model.sendChannel(chatId: chatId, text: text)
            pending.removeAll()
            clearDraft()
            replyTo = nil
            sentOnce = true
            // After the optimistic row is in `messages` (channel path paints inline).
            scrollAfterSend()
            return
        }
        if isPeople {
            let address = sendAddress(for: text)
            let chips = chipMentions(for: text)
            model.sendPeople(chatId: chatId, text: text, replyTo: replyTo?.id, address: address,
                             chipMentions: chips, attachments: pending,
                             chipTokens: MentionDraft.tokens(composerDraft.mentions), replyRef: replyRef)
            pending.removeAll()
            clearDraft()
            replyTo = nil
            replyRef = nil
            mentionQuery = nil
            pinTalkTo(afterSending: address, chipMentions: chips)
            sentOnce = true
            // After paintLocalUserMessage so scrollTo sees the new bubble.
            scrollAfterSend()
            return
        }
        if needsAnswer && pending.isEmpty {
            model.answer(chatId: chatId, text: text)
        } else {
            model.send(chatId: chatId, text: text, attachments: pending, replyTo: replyTo?.id)
        }
        pending.removeAll()
        clearDraft()
        replyTo = nil
        sentOnce = true
        // After paintLocalUserMessage so scrollTo sees the new bubble.
        scrollAfterSend()
    }

    private func stop() {
        model.stop(chatId: chatId)
    }
}

/// Soft fade + streaming shimmer on the presence row's line (Mac `presence::row`).
struct WorkingStatusText: View {
    @Environment(\.theme) private var theme
    let text: String
    let color: Color
    /// Point size; the presence row uses Grok's 17 pt body, the transcript's rows 15.
    var size: CGFloat = Theme.small
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        if reduceMotion {
            label(color)
        } else {
            TimelineView(.animation(minimumInterval: 1.0 / 30.0)) { context in
                let t = context.date.timeIntervalSinceReferenceDate
                // Triangle wave 0…1 over 1.1 s — same period as Mac PulseClock.
                let cycle = t.truncatingRemainder(dividingBy: 1.1) / 1.1
                let pulse = cycle < 0.5 ? cycle * 2.0 : 2.0 - cycle * 2.0
                let fade = 0.55 + 0.45 * pulse
                // Bright band streams left → right across the label.
                let stream = t.truncatingRemainder(dividingBy: 1.8) / 1.8
                label(color.opacity(fade))
                    .overlay {
                        GeometryReader { geo in
                            let width = geo.size.width
                            let band = max(width * 0.5, 1)
                            LinearGradient(
                                colors: [.clear, color, .clear],
                                startPoint: .leading,
                                endPoint: .trailing
                            )
                            .frame(width: band)
                            .offset(x: stream * (width + band) - band)
                        }
                        .mask(label(color))
                        .allowsHitTesting(false)
                    }
            }
        }
    }

    private func label<S: ShapeStyle>(_ style: S) -> some View {
        Text(text)
            .font(theme.font(size))
            .foregroundStyle(style)
    }
}

/// Clear `UIControl` that runs `action` on touch-down. SwiftUI `Button` /
/// `TapGesture` often fail while a `ScrollView` is still decelerating; UIKit
/// controls in an overlay above the scroll view keep receiving the touch.
private struct ScrollInterruptTap: UIViewRepresentable {
    var action: () -> Void

    func makeCoordinator() -> Coordinator { Coordinator(action: action) }

    func makeUIView(context: Context) -> UIControl {
        let control = UIControl()
        control.backgroundColor = .clear
        control.isExclusiveTouch = true
        // Touch-down: stop deceleration and jump immediately so the action is not
        // lost if SwiftUI tears the control down when the scroll view settles.
        control.addTarget(context.coordinator, action: #selector(Coordinator.touchDown(_:)), for: .touchDown)
        return control
    }

    func updateUIView(_ uiView: UIControl, context: Context) {
        context.coordinator.action = action
    }

    final class Coordinator: NSObject {
        var action: () -> Void
        init(action: @escaping () -> Void) { self.action = action }

        @objc func touchDown(_ sender: UIControl) {
            if let root = sender.window {
                Self.stopDeceleratingScrollViews(in: root)
            }
            action()
        }

        private static func stopDeceleratingScrollViews(in view: UIView) {
            if let scroll = view as? UIScrollView, scroll.isDecelerating || scroll.isDragging {
                scroll.setContentOffset(scroll.contentOffset, animated: false)
            }
            for child in view.subviews {
                stopDeceleratingScrollViews(in: child)
            }
        }
    }
}

/// "+" sheet: Photo or video, File — Grok's attach options as a short bottom sheet.
struct AttachSheet: View {
    @Environment(\.theme) private var theme
    let onPhoto: () -> Void
    let onFile: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            Capsule().fill(theme.muted.opacity(0.4)).frame(width: 36, height: 5).padding(.top, 8).padding(.bottom, 14)
            row(icon: "photo", title: "Photo or video", subtitle: "From your library", action: onPhoto)
            row(icon: "doc", title: "File", subtitle: "From Files or iCloud Drive", action: onFile)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 16)
        .background(theme.bg)
        .presentationDetents([.height(190)])
        .presentationDragIndicator(.hidden)
        .presentationBackground(theme.bg)
    }

    private func row(icon: String, title: String, subtitle: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 14) {
                GlassCircle(size: 44) { // control-size: exception rowIconTiles
                    Image(systemName: icon).font(.system(size: 18)).foregroundStyle(theme.text)
                }
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(theme.font(Theme.body)).foregroundStyle(theme.text)
                    Text(subtitle).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                }
                Spacer()
            }
            .padding(.vertical, 8)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// Talking-model picker: ids the relay proxy serves, plus the effort level.
struct ModelPickerView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let project: Project
    @State private var query = ""
    @State private var effort: Effort = .low

    /// Live project so the checkmark and effort writes follow `setModel`, not the sheet's snapshot.
    private var current: Project { model.project(project.id) ?? project }

    private var ids: [String] {
        var all = model.models
        if !all.contains(current.model) { all.insert(current.model, at: 0) }
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        return q.isEmpty ? all : all.filter { $0.lowercased().contains(q) }
    }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    HStack(spacing: 8) {
                        Image(systemName: "magnifyingglass")
                            .font(theme.font(15, weight: .medium))
                            .foregroundStyle(theme.muted)
                        TextField("", text: $query, prompt: Text("Filter models").foregroundStyle(theme.placeholder))
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .font(theme.font(Theme.body))
                            .foregroundStyle(theme.text)
                        if !query.isEmpty {
                            Button { query = "" } label: {
                                Image(systemName: "xmark.circle.fill").foregroundStyle(theme.muted)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .listRowBackground(theme.card)
                }
                Section("Reasoning") {
                    Picker("Effort", selection: $effort) {
                        ForEach(Effort.allCases) { e in Text(e.label).tag(e) }
                    }
                    .pickerStyle(.segmented)
                    .onChange(of: effort) { _, new in
                        model.setModel(projectId: project.id, model: current.model, effort: new)
                    }
                }
                Section("Model") {
                    if model.models.isEmpty {
                        Text("Loading models from the relay…").foregroundStyle(theme.muted)
                    }
                    ForEach(ids, id: \.self) { id in
                        Button {
                            model.setModel(projectId: project.id, model: id, effort: effort)
                            dismiss()
                        } label: {
                            HStack {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(Project.shortModel(id)).foregroundStyle(theme.text)
                                    Text(id).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                                }
                                Spacer()
                                if id == current.model {
                                    Image(systemName: "checkmark").foregroundStyle(theme.text)
                                }
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .listRowBackground(theme.card)
                        .accessibilityLabel("\(Project.shortModel(id))\(id == current.model ? ", selected" : "")")
                        .accessibilityAddTraits(id == current.model ? [.isSelected] : [])
                    }
                }
                if !current.modelNote.isEmpty {
                    Section {
                        Text(current.modelNote).font(theme.font(Theme.tiny)).foregroundStyle(theme.warn)
                    }
                }
            }
            .scrollContentBackground(.hidden)
            .background(theme.bg)
            .navigationTitle("Model")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() } }
            }
            .onAppear { effort = current.effort }
        }
        .presentationDetents([.medium, .large])
    }
}

/// The visible transcript rows. Equatable so a composer keystroke (ChatView's
/// composer draft) or an AppModel presence tick that re-evaluates ChatView
/// does not rebuild every bubble when the row list is unchanged.
private struct TranscriptRows: View, Equatable {
    let model: AppModel
    let theme: Theme
    let composerInset: CGFloat
    /// People chat: the wider bubble caps (`Theme.peopleBubbleInset`).
    let people: Bool
    let mentions: MentionTable
    let shown: [Row]
    /// Identity of `shown` (rows version + window bounds + search): compared instead of the array.
    let shownStamp: Int
    let chatId: Id
    let width: CGFloat
    let timeReveal: CGFloat
    /// The reply swipe in progress (one row follows the finger).
    let replySwipe: ReplySwipe
    /// Row frames the rows write on layout (not compared: a class, same instance for the chat).
    let rowFrames: RowFrameIndex
    let highlightedMessage: Id?
    let rowsCount: Int
    let messagesCount: Int
    let canReply: (Row) -> Bool
    let onQuoteTap: (Quote) -> Void
    let onMenu: (MessageMenuItem, Row) -> Void
    /// The bubble whose long-press actions are open (it alone reports its frame).
    let actionsRowId: String?
    let onLongPress: (Row) -> Void

    static func == (lhs: TranscriptRows, rhs: TranscriptRows) -> Bool {
        let same = lhs.shownStamp == rhs.shownStamp
            && lhs.actionsRowId == rhs.actionsRowId
            && lhs.composerInset == rhs.composerInset
            && lhs.people == rhs.people
            && lhs.theme == rhs.theme
            && lhs.chatId == rhs.chatId
            && lhs.width == rhs.width
            && lhs.timeReveal == rhs.timeReveal
            && lhs.replySwipe == rhs.replySwipe
            && lhs.highlightedMessage == rhs.highlightedMessage
            && lhs.mentions.stamp == rhs.mentions.stamp
        #if DEBUG
        PerfCounters.bump(same ? "rowsEqTrue" : "rowsEqFalse")
        if !same {
            PerfCounters.bump("rowsEq.\(lhs.shownStamp == rhs.shownStamp ? "" : "shown,")\(lhs.width == rhs.width ? "" : "width,")\(lhs.timeReveal == rhs.timeReveal ? "" : "reveal,")\(lhs.mentions.stamp == rhs.mentions.stamp ? "" : "stamp,")\(lhs.highlightedMessage == rhs.highlightedMessage ? "" : "hl,")")
        }
        #endif
        return same
    }

    var body: some View {
        #if DEBUG
        let _ = PerfCounters.bump("transcriptRowsBody")
        let _ = DebugHooks.perfChanges ? Self._printChanges() : ()
        #endif
        // Rows keyed by id, never by position: a grow step prepends older rows and
        // must not touch the ones already laid out (arbonites-chat-lag).
        let firstId = shown.first?.id
        ForEach(shown) { row in
            let reply = canReply(row)
            RowView(model: model, theme: theme, composerInset: composerInset, row: row, chatId: chatId, width: width,
                    people: people, timeReveal: timeReveal,
                    onQuoteTap: onQuoteTap,
                    highlighted: highlightedMessage == row.messageId,
                    canReply: reply,
                    replyDrag: replySwipe.rowId == row.id ? replySwipe.reveal : 0,
                    replyArmed: replySwipe.rowId == row.id && replySwipe.armed,
                    mentions: mentions)
                .equatable()
                // Where this row is, for the swipe recognizer's row-by-location lookup. The
                // transcript space scrolls with the content, so this fires on layout, not per
                // scroll frame; the index is a plain class, so no view is invalidated.
                .onGeometryChange(for: CGRect.self) { $0.frame(in: .named(TranscriptSpace.name)) } action: { frame in
                    rowFrames.set(row.id, frame: frame, canReply: reply)
                }
                .onDisappear { rowFrames.remove(row.id) }
                .padding(.top, row.id == firstId ? 0 : row.gapBefore)
                .id(row.id)
                .onAppear {
                    #if DEBUG
                    if SendTrace.traces(chatId) {
                        SendTrace.markIfPresent("row-in-tree", ids: [row.messageId], detail: "row=\(row.id)")
                    }
                    #endif
                    // Once per open — not on every row (group chats fire this dozens of times).
                    if row.id == firstId {
                        LaunchClock.mark("chat-first-row", detail: "shown=\(shown.count) rows=\(rowsCount) messages=\(messagesCount)")
                        #if DEBUG
                        PerfLog.shared.armScrollable()
                        #endif
                    }
                }
                // A message bubble: long-press → the reaction capsule + menu overlay
                // (`MessageActionsOverlay`). Other rows keep the plain Copy menu.
                .anchorPreference(key: RowFrameKey.self, value: .bounds) { anchor in
                    actionsRowId == row.id ? [row.id: anchor] : [:]
                }
                .messageRowGestures(isMessage: reply, onLongPress: { onLongPress(row) },
                                    onCopyMenu: { onMenu($0, row) })
        }
    }
}

/// Fires when the scroll view comes near its top (channel older page / grow window).
/// iOS 18+ uses scroll geometry; iOS 17 watches a top-of-content frame in a named
/// coordinate space so the row window can grow (feedback app-01M5NNJ001NHHYBHXTASM5NXBZ).
private struct NearTopReporter: ViewModifier {
    let enabled: Bool
    let onNear: () -> Void

    func body(content: Content) -> some View {
        if #available(iOS 18, *), enabled {
            content.onScrollGeometryChange(for: Bool.self) { g in
                // About a screen ahead of the top edge, so the next slice is in place
                // before the reader reaches it.
                g.contentSize.height > g.containerSize.height && g.contentOffset.y < 900
            } action: { _, near in
                if near { onNear() }
            }
        } else if enabled {
            content
                .coordinateSpace(name: "arbosChatScroll")
                .onPreferenceChange(ChatScrollTopKey.self) { minY in
                    // Content top near the scroll view's top → grow the row window.
                    if minY > -40 && minY < 900 { onNear() }
                }
        } else {
            content
        }
    }
}

private struct ChatScrollTopKey: PreferenceKey {
    /// Unset until the top sentinel lays out (must not look "near top").
    static var defaultValue: CGFloat = -.greatestFiniteMagnitude
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = nextValue()
    }
}

/// Reports when the scroll view sits more than 120 pt above its end (iOS 18+;
/// earlier systems never show the chevron / pill). While `pinned` is true, content
/// growth (streaming row height, a new row) must not look like a user scroll-up:
/// re-stick instead of flipping `away` and leaving the transcript stranded.
/// While unpinned, growth must not re-pin either — a bottom scroll anchor can
/// briefly shrink distance and would yank the reader out of history.
private struct AwayFromBottomReporter: ViewModifier {
    @Binding var away: Bool
    @Binding var pinned: Bool
    var onStick: () -> Void

    private struct Sample: Equatable {
        var height: CGFloat
        var distance: CGFloat
    }

    func body(content: Content) -> some View {
        if #available(iOS 18, *) {
            content.onScrollGeometryChange(for: Sample.self) { g in
                Sample(
                    height: g.contentSize.height,
                    distance: g.contentSize.height - g.contentOffset.y - g.containerSize.height
                )
            } action: { old, new in
                let grew = new.height > old.height + 0.5
                #if DEBUG
                PerfDriveProbe.lastScrollGeometry = Date()
                if SendTrace.recent {
                    SendTrace.mark("scroll-geometry", detail: "grew=\(grew) distance=\(Int(new.distance)) pinned=\(pinned)", once: false)
                    // The "bottom" spacer and padding sit ~60 pt under the last bubble.
                    if new.distance < 100 { SendTrace.mark("at-bottom", detail: "distance=\(Int(new.distance))") }
                }
                #endif
                if grew {
                    if pinned {
                        away = false
                        onStick()
                    } else {
                        // Keep history put; distance may dip from layout / bottom anchor.
                        away = true
                    }
                    return
                }
                let isAway = new.distance > 120
                away = isAway
                if isAway {
                    pinned = false
                } else {
                    pinned = true
                }
            }
        } else {
            content
        }
    }
}

/// The composer's text, observed by `ComposerField` alone. Typing re-renders the
/// field; the chat screen reads `text` at action time and is told about the two
/// things it shows elsewhere (the `@` picker, "everyone" chips) through `onChange`.
@MainActor
final class ComposerDraft: ObservableObject {
    /// The plain draft: `@handle` where the mention chips are (#1596).
    @Published var text = "" {
        didSet { if text != oldValue { onChange?(text) } }
    }
    /// The committed mention chips in `text` (spans + members); the field draws
    /// them as attachments and the send carries their member ids.
    @Published var mentions: [ComposerMention] = [] {
        didSet { if mentions != oldValue { onCaretOrMentions?() } }
    }
    /// Caret in `text` (UTF-16 offset), kept by the field.
    @Published var caret = 0 {
        didSet { if caret != oldValue { onCaretOrMentions?() } }
    }
    var onChange: ((String) -> Void)?
    var onCaretOrMentions: (() -> Void)?
}

/// The field, mic and send / call button. Owns nothing: every action calls back
/// into the chat screen, whose memo and send logic is unchanged.
private struct ComposerField: View {
    @Environment(\.theme) private var theme
    @ObservedObject var draft: ComposerDraft
    @ObservedObject var recorder: MemoRecorder
    @Binding var focus: Bool
    let chatId: Id
    let model: AppModel
    let isChannel: Bool
    let needsAnswer: Bool
    let hasPending: Bool
    /// Hold-to-talk state owned by the chat screen (slide-to-cancel).
    let memoHoldActive: Bool
    let memoCancelArmed: Bool
    let memoDragX: CGFloat
    let cancelDistance: CGFloat
    let prompt: (String) -> String
    /// ⌫ on an empty field: the chat screen drops its last pending attachment.
    let onDeleteBackwardEmpty: () -> Void
    let onCancelRecording: () -> Void
    let onMicDrag: (DragGesture.Value) -> Void
    let onMicEnd: (DragGesture.Value) -> Void
    let onPrimary: () -> Void

    private var canSend: Bool {
        !draft.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || hasPending
    }

    private var fieldShape: RoundedRectangle {
        RoundedRectangle(cornerRadius: Theme.composerHeight / 2, style: .circular)
    }

    /// Grok's filled mic (`MicGlyph`); the armed slide-to-cancel shows the bin instead.
    @ViewBuilder
    private var micGlyph: some View {
        if memoCancelArmed {
            Image(systemName: "trash").font(theme.font(17))
        } else {
            MicGlyph().frame(width: Theme.micGlyphWidth, height: Theme.micGlyphHeight)
        }
    }

    /// Inside the white pill: the send arrow with text or a recording; idle, Grok's
    /// voice-wave bars — a tap starts a voice call (agent or people chat). Channels
    /// have no calls (C1): the arrow stays, dimmed until there is text.
    @ViewBuilder
    private var actionGlyph: some View {
        if canSend || recorder.recording || isChannel {
            Image(systemName: "arrow.up")
                .font(theme.font(16, weight: .bold))
                .foregroundStyle(theme.onPrimary)
        } else {
            VoiceWaveGlyph()
                .fill(theme.onPrimary)
                .accessibilityHidden(true)
        }
    }

    /// Max height for the growing composer field.
    ///
    /// A vertical `TextField` fills the height its parent proposes (up to
    /// `lineLimit`), so an empty field inside `safeAreaInset` was stretching to
    /// eight lines and covering the transcript. Hug content with `fixedSize`,
    /// and still cap at 8 lines (Grok's 22 pt body pitch) for wrap glitches.
    private var composerFieldMaxHeight: CGFloat {
        Theme.composerHeight + 7 * 22
    }

    /// The field's rim: Grok's edge highlight, or the recording / needs-answer tint.
    private var rimTint: Color? {
        if recorder.recording { return theme.danger.opacity(0.5) }
        if needsAnswer { return theme.warn.opacity(0.7) }
        return nil
    }

    var body: some View {
        HStack(spacing: 8) {
            if recorder.recording {
                // Visible discard control (tap) — also used after a short tap that
                // left recording on; hold-to-talk can slide left instead.
                Button(action: onCancelRecording) {
                    Image(systemName: "xmark")
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundStyle(theme.danger)
                        .frame(width: 44, height: Theme.composerHeight)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Cancel")
                if memoCancelArmed {
                    Text("Release to cancel")
                        .font(theme.font(Theme.small))
                        // error-ui: exception status-mark
                        .foregroundStyle(theme.danger)
                        .frame(maxWidth: .infinity, alignment: .center)
                } else if memoHoldActive && memoDragX < -12 {
                    Text("Slide to cancel")
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                        .frame(maxWidth: .infinity, alignment: .center)
                        .opacity(min(1, -memoDragX / cancelDistance))
                } else {
                    HStack(spacing: 8) {
                        Circle().fill(theme.danger).frame(width: 8, height: 8)
                        // Level bars drawn into whatever width is left (newest on the right),
                        // so the recorder row never pushes the composer past the screen edge.
                        Canvas { ctx, size in
                            let pitch: CGFloat = 5, w: CGFloat = 3
                            let levels = recorder.levels
                            let n = min(levels.count, max(0, Int(size.width / pitch)))
                            for i in 0..<n {
                                let l = CGFloat(levels[levels.count - n + i])
                                let h = min(size.height, 4 + l * 20)
                                let x = size.width - CGFloat(n - i) * pitch
                                let rect = CGRect(x: x, y: (size.height - h) / 2, width: w, height: h)
                                ctx.fill(Path(roundedRect: rect, cornerRadius: w / 2), with: .color(theme.text))
                            }
                        }
                        .frame(maxWidth: .infinity, maxHeight: 24)
                        Text(recorder.timeLabel)
                            .font(theme.font(Theme.small, design: .monospaced))
                            .foregroundStyle(theme.text)
                    }
                }
            } else {
                // The field with mention chips (#1596): one to eight lines, the
                // chips as attachments; `draft.text` stays the plain `@handle` text.
                MentionTextView(text: $draft.text, mentions: $draft.mentions, caret: $draft.caret, isFocused: $focus,
                                placeholder: prompt(draft.text), theme: theme,
                                agentProject: { model.agent(member: $0, chatId: chatId) },
                                onDeleteBackwardEmpty: onDeleteBackwardEmpty)
                    .fixedSize(horizontal: false, vertical: true)
            }
            // Grok's trailing cluster: mic glyph centre 24 pt before the pill, the pill
            // 8⅔ pt from the field's edge, both centred on the field's bottom line.
            HStack(spacing: Theme.micPillGap) {
                if draft.text.isEmpty || recorder.recording {
                    // Press/release like Mac: tap toggles recording on; hold ≥0.25s then
                    // release sends. Slide left while holding to discard (WhatsApp-style).
                    // highPriorityGesture so a focused TextField cannot steal the touch.
                    // The mic stays mounted while recording so release can finish without
                    // a second tap.
                    micGlyph
                        .foregroundStyle(recorder.recording ? theme.danger : theme.icon)
                        .frame(width: Theme.micHitWidth, height: Theme.composerHeight)
                        .contentShape(Rectangle())
                        .id("memo-mic")
                        .accessibilityIdentifier("composer-mic")
                        .highPriorityGesture(
                            DragGesture(minimumDistance: 0)
                                .onChanged { onMicDrag($0) }
                                .onEnded { onMicEnd($0) }
                        )
                }
                // While recording: tap send also finishes (same as release). Otherwise
                // Grok keeps the composer usable while the bot works: text sends, an
                // empty field starts a voice call with the agent (Grok's voice-wave pill).
                Button(action: onPrimary) {
                    Capsule()
                        .fill(theme.primary)
                        .frame(width: Theme.actionPillWidth, height: Theme.actionPillHeight)
                        .overlay(actionGlyph)
                        .frame(height: Theme.composerHeight)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .opacity(isChannel && !canSend && !recorder.recording ? 0.45 : 1)
                .accessibilityLabel((canSend || recorder.recording) ? "Send" : (isChannel ? "Send" : "Call"))
                .accessibilityIdentifier("composer-action")
            }
            .padding(.trailing, Theme.actionPillTrailing)
        }
        // Hard max so a TextField layout glitch cannot paint over the transcript.
        .frame(minHeight: Theme.composerHeight, maxHeight: composerFieldMaxHeight, alignment: .bottom)
        .fixedSize(horizontal: false, vertical: true)
        // A true capsule at one line (Grok's edge follows a circle); the radius stays
        // 24 pt as the field grows.
        .background(fieldShape.fill(theme.field)) // control-size: source (the family's field, rim below)
        .overlay(ComposerRim(shape: fieldShape, tint: rimTint))
        .alert("Microphone access is off", isPresented: $recorder.permissionDenied) {
            Button("OK", role: .cancel) { }
        } message: {
            Text("Allow the microphone in Settings to record a voice memo.")
        }
    }
}
