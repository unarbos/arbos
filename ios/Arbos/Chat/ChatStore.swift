import Foundation
import Network
import UIKit

/// The project's main agent chat, shared by the call screen (voice) and
/// the chat sheet (text). One agent, two ways in.
@MainActor
final class ChatStore: ObservableObject {
    enum Mode: Equatable {
        case offline
        case connecting
        /// Through the speech server: text channel + kernel mirror.
        case server
        /// Straight to the kernel's attach port.
        case live
        /// Nothing reachable; a scripted stand-in is answering.
        case mock

        var tag: String {
            switch self {
            case .offline: return "offline"
            case .connecting: return "connecting"
            case .server, .live: return "live"
            case .mock: return "demo"
            }
        }
    }

    @Published private(set) var mode: Mode = .offline
    @Published private(set) var items: [ChatItem] = []
    @Published private(set) var busy = false
    @Published private(set) var agents: [KernelAgent] = []
    /// The root's workers, for the worker lines and the Working pill.
    @Published private(set) var workers: [WorkerStatus] = []
    /// The root's live step while a turn runs ("Reading notes.md").
    @Published private(set) var step = ""
    /// The project's face from `.arbos/project.toml`, once read.
    @Published private(set) var identity: ProjectIdentity?
    /// The kernel's own hub address (`arbos://<machine>/<project>/`), so the
    /// list can tell that the direct kernel and a roster row are one project.
    @Published private(set) var store: String?
    /// Send → first token of the last typed turn.
    @Published private(set) var lastFirstToken: TimeInterval?
    /// Transcript lines before the first one shown (the kernel replays its
    /// last 200 on attach); a long project's earlier history.
    @Published private(set) var earlierLines = 0
    @Published private(set) var loadingEarlier = false
    /// After older lines are prepended, the row that was at the top; the
    /// view pins it there instead of jumping to the tail.
    @Published var anchorAfterPrepend: UUID?
    private var firstSeq = 0
    /// Seconds until the next reconnect try, while the link is down.
    @Published private(set) var reconnectIn: Int?
    /// Why this target cannot be reached at all (the hub does not know
    /// it); shown in place of the countdown, no retry.
    @Published private(set) var refusal: String?
    /// A named reason the link is down that retrying could still cure — a
    /// project's kernel not running, say. Unlike `refusal` it does not stop
    /// the countdown; it only replaces "Link lost" with what is actually
    /// missing.
    @Published private(set) var standing: String?

    /// Fires with each finished agent message. The call speaks it when the
    /// server does not.
    var onAgentMessage: ((String) -> Void)?
    /// A live `notify` while the user may not be looking.
    var onNotify: ((KernelNotification) -> Void)?
    /// Every client saw up to this id: banners and the badge go.
    var onSeen: ((Int) -> Void)?
    /// The hub answered `push`: can it push to this phone?
    var onPushed: ((Bool, String?) -> Void)?
    /// This phone's APNs token, once iOS gave one; sent on every attach.
    var pushToken: String? {
        didSet { if pushToken != oldValue { registerPushIfLive() } }
    }
    /// Debug builds get sandbox tokens; the hub posts to the sandbox host for them.
    #if DEBUG
    private let pushSandbox = true
    #else
    private let pushSandbox = false
    #endif
    /// What the user missed while away (replayed on attach) plus what
    /// arrived live and is not yet seen; the "while you were away" card.
    @Published private(set) var unseen: [KernelNotification] = []

    let settings: AppSettings
    private let link: VoiceLink
    private var source: ChatSource?
    private var pump: Task<Void, Never>?
    private var sentAt: Date?
    private var reconnectTask: Task<Void, Never>?
    /// The last refusal said out loud. Retrying every ten seconds against an
    /// offline machine must not fill the chat with the same sentence.
    private var lastRefusal: String?
    /// The last transport failure said out loud. The retry loop runs every
    /// few seconds and must not write the same line each time round.
    private var lastTransport: String?
    private var reconnectAttempt = 0
    /// Typed lines the kernel has not echoed yet, oldest first.
    private var pendingSends: [(id: UUID, text: String, steer: Bool, target: KernelTarget)] = []
    /// Spoken lines this app put in the chat itself, so the kernel's replay
    /// of the same question replaces them instead of doubling them.
    private var spokenLocally: [(id: UUID, text: String)] = []
    /// Set once the caller has heard this turn's answer out loud. A delegated
    /// turn is answered twice — the kernel writes it and the voice says it in
    /// its own words — and the chat shows the conversation, so the spoken
    /// wording is the one that stays. Clears at the next question.
    private var answerWasSpoken = false
    private let pathMonitor = NWPathMonitor()
    private var pathWasSatisfied = true

    init(settings: AppSettings, link: VoiceLink) {
        self.settings = settings
        self.link = link
        // The network came back (Wi-Fi to cellular, a tunnel, a dead spot):
        // reconnect now instead of waiting out the backoff.
        pathMonitor.pathUpdateHandler = { [weak self] path in
            let satisfied = path.status == .satisfied
            Task { @MainActor in
                guard let self else { return }
                if satisfied, !self.pathWasSatisfied || self.reconnectIn != nil { self.resumeIfNeeded() }
                self.pathWasSatisfied = satisfied
            }
        }
        pathMonitor.start(queue: DispatchQueue(label: "arbos.path"))
    }

    var agentName: String {
        let root = agents.first { $0.parent == nil }
        guard let name = root?.name, name != "root" else { return "main" }
        return name
    }

    /// The kernel itself first (it replays history and streams tokens),
    /// then the speech server's mirror, then the scripted chat so the
    /// screen is still real. If the configured kernel host is down, the
    /// published endpoint file is consulted once. Safe to call again.
    func connect() async {
        guard mode == .offline else { return }
        mode = .connecting
        if let endpoint = settings.chatEndpoint {
            if await attachKernel(endpoint) { return }
            // The configured address is the user's; it moves only when the
            // host itself is gone (a quick tunnel that rotated) and the
            // published directory names another — and then the chat says
            // so. A hub that answers but refuses (a token, a machine that is
            // not there) or a kernel that is slow is an outage: keep the
            // address, keep retrying (M-63).
            if await Self.hostGone(endpoint.url), let directory = await EndpointDirectory.fetch() {
                var moved = false
                if case .pod = settings.kernelTarget, let fresh = directory.kernelURL, fresh != settings.kernelURL {
                    settings.kernelURL = fresh
                    moved = true
                }
                if case .hub = settings.kernelTarget, let hub = directory.hubURL, hub != settings.hubURL {
                    settings.hubURL = hub
                    moved = true
                }
                if moved, let next = settings.chatEndpoint, next != endpoint {
                    items.append(ChatItem(.notice("The kernel moved to \(next.url.host ?? "a new address"); following it.", failed: false)))
                    if await attachKernel(next) { return }
                }
            }
            // A configured kernel that is not answering is an outage, not a
            // reason to show a stand-in: stay offline and retry.
            mode = .offline
            return
        }
        if case .hub(let machine, let project) = settings.kernelTarget {
            // The hub target has no hub any more (Settings cleared it):
            // say so instead of quietly opening some other project.
            items.append(ChatItem(.notice("\(project) on \(machine) needs the hub — set it in Settings.", failed: true)))
            mode = .offline
            return
        }
        // Nothing configured at all: the speech server can carry a chat
        // (it says so), else the scripted one.
        if settings.provider == .selfHosted, settings.isConfigured {
            let server = VoiceServerChat(link: link)
            if (try? await server.start()) != nil {
                adopt(server, mode: .server)
                items.append(ChatItem(.notice("No kernel is set; this chat goes through the speech server.", failed: false)))
                return
            }
            server.stop()
        }
        let mock = MockKernelChat()
        try? await mock.start()
        adopt(mock, mode: .mock)
    }

    /// True when nothing answers at the address at all (no DNS, no route,
    /// refused) — as opposed to a host that answers and says no.
    private static func hostGone(_ url: URL) async -> Bool {
        guard var components = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return false }
        components.scheme = components.scheme?.lowercased() == "ws" ? "http" : "https"
        components.path = "/"
        components.queryItems = nil
        guard let probe = components.url else { return false }
        var request = URLRequest(url: probe)
        request.timeoutInterval = 6
        do {
            _ = try await URLSession.shared.data(for: request)
            return false
        } catch let error as URLError {
            switch error.code {
            case .cannotFindHost, .cannotConnectToHost, .dnsLookupFailed, .networkConnectionLost, .notConnectedToInternet, .timedOut:
                return true
            default:
                return false
            }
        } catch {
            return false
        }
    }

    private func attachKernel(_ endpoint: ArbosKernelClient.Endpoint) async -> Bool {
        let live = LiveKernelChat(endpoint: endpoint)
        // Frames start arriving during attach; the pump must be running
        // before the history replay lands.
        adopt(live, mode: .connecting)
        do {
            try await live.start()
            mode = .live
            lastRefusal = nil
            lastTransport = nil
            standing = nil
            registerPushIfLive()
            return true
        } catch {
            #if DEBUG
            print("attach \(endpoint.url): \(error)")
            #endif
            // Two different things wearing the same coat. A refusal is a
            // verdict — somebody said no and meant it, so say their reason
            // and stop. A transport failure is the path, which usually
            // clears by itself, so name it and keep trying. The app had
            // these the wrong way round.
            switch failure(from: error) {
            case .refusal(let why)?:
                let said = Self.inPlainWords(why)
                if said != lastRefusal {
                    lastRefusal = said
                    items.append(ChatItem(.notice(said, failed: true)))
                }
                refusal = said
            case .transport(let what)?:
                if what != lastTransport {
                    lastTransport = what
                    items.append(ChatItem(.notice(what, failed: false)))
                }
            case nil:
                break
            }
        }
        live.stop()
        pump?.cancel()
        source = nil
        return false
    }

    /// The hub's refusals, said the way a person would say them.
    ///
    /// The hub is right to log `no machine named "arboslife" is registered
    /// (known: none)`, and the app was showing that sentence to Jacob. It is
    /// accurate and it is a record of our internals; what he needs to know is
    /// that ArbosLife is off and this chat will not open until it is back.
    ///
    /// Anything whose shape is not recognised keeps the hub's own words.
    /// Guessing at a refusal is worse than quoting one.
    static func inPlainWords(_ why: String) -> String {
        let text = why.replacingOccurrences(of: "hub: ", with: "")
        if let machine = quoted(in: text), text.contains("no machine named") {
            return "\(machine) is not connected — its kernel isn't running, or the machine is off."
        }
        if let project = quoted(in: text), text.contains("no project named") {
            return "\(project) isn't on that machine any more."
        }
        // Arrived with #417: the machine is registered and the project is
        // known, and nothing is serving it. Different from the two above,
        // and different from what to do about it.
        if let project = quoted(in: text), text.contains("has no kernel serving") {
            let machine = text.split(separator: " ").first.map(String.init) ?? "its machine"
            return "\(project)'s kernel on \(machine) isn't running."
        }
        // The hub calls this its commonest refusal, and it is the one it says
        // most about: a timestamp, the project the machine used to serve, and
        // the remedy. That is the hub's record of itself. A person needs the
        // machine's name and how long it has been gone.
        if text.contains("is offline: nothing of it has been connected since") {
            let machine = text.split(separator: " ").first.map(String.init) ?? "That machine"
            let stamp = text.components(separatedBy: "connected since ").last?
                .components(separatedBy: ";").first?
                .trimmingCharacters(in: .whitespaces)
            if let stamp, let howLong = sinceStamp(stamp) {
                return "\(machine) has been off for \(howLong). It comes back when a kernel starts on it."
            }
            return "\(machine) is off. It comes back when a kernel starts on it."
        }
        return text
    }

    /// How long ago an RFC3339 stamp was, spelled out. The list says `2h`
    /// because it has one column; a sentence has room to say it properly.
    /// Nil when the stamp will not parse, so the caller can drop the clause
    /// rather than print a stamp nobody can read at a glance.
    private static func sinceStamp(_ stamp: String) -> String? {
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime]
        guard let at = parser.date(from: stamp) else { return nil }
        let seconds = Int(Date().timeIntervalSince(at))
        guard seconds >= 0 else { return nil }
        func plural(_ n: Int, _ unit: String) -> String { "\(n) \(unit)\(n == 1 ? "" : "s")" }
        switch seconds {
        case ..<60: return "under a minute"
        case ..<3600: return plural(seconds / 60, "minute")
        case ..<86_400: return plural(seconds / 3600, "hour")
        default: return plural(seconds / 86_400, "day")
        }
    }

    /// The first `"…"` in a hub message: the name it is talking about.
    private static func quoted(in text: String) -> String? {
        let parts = text.split(separator: "\"", omittingEmptySubsequences: false)
        guard parts.count >= 2 else { return nil }
        let name = String(parts[1])
        return name.isEmpty ? nil : name
    }

    /// What kind of failure this was, when the error carries the answer.
    private func failure(from error: Error) -> KernelFailure? {
        guard case KernelClientError.failed(let failure) = error else { return nil }
        let text = failure.reason.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return nil }
        let said = text.prefix(1).uppercased() + text.dropFirst()
        switch failure {
        case .refusal: return .refusal(said)
        case .transport: return .transport(said)
        }
    }

    /// Stop, as the kernel's stall notice offers it: the turn ends, what
    /// ran so far stands, and the kernel's `interrupted` line says so.
    func stopTurn() {
        guard busy, let source else { return }
        Task { try? await source.interrupt() }
    }

    func disconnect() {
        reconnectTask?.cancel()
        reconnectTask = nil
        reconnectIn = nil
        pump?.cancel()
        pump = nil
        source?.stop()
        source = nil
        mode = .offline
        busy = false
    }

    /// The app came back to the front (the phone woke, the user returned):
    /// a link iOS cut while the app slept is reopened at once.
    func resumeIfNeeded() {
        guard mode == .offline || mode == .mock, settings.chatEndpoint != nil, refusal == nil else { return }
        reconnectTask?.cancel()
        reconnectIn = nil
        reconnectAttempt = 0
        Task { await reconnect() }
    }

    /// The link went: try again after 2 s, then 4, 8, capped at 15, until
    /// it holds (the path monitor cuts the wait short when the network
    /// returns). `reconnectIn` counts down for the chat's notice.
    private func scheduleReconnect() {
        // Somebody said no and gave a reason. Trying the same thing every
        // few seconds will get the same answer, and the countdown suggests
        // to the user that waiting will help. `reconnect()` clears the
        // refusal, so a deliberate retry still works.
        guard refusal == nil else {
            reconnectTask?.cancel()
            reconnectTask = nil
            reconnectIn = nil
            return
        }
        reconnectTask?.cancel()
        let delay = min(15, 2 << min(reconnectAttempt, 3))
        reconnectAttempt += 1
        reconnectIn = delay
        reconnectTask = Task { [weak self] in
            for remaining in stride(from: delay, to: 0, by: -1) {
                guard let self, !Task.isCancelled else { return }
                self.reconnectIn = remaining
                try? await Task.sleep(for: .seconds(1))
            }
            guard let self, !Task.isCancelled else { return }
            self.reconnectIn = nil
            // This task is the one about to run reconnect(); disconnect()
            // inside it cancels `reconnectTask` — which was this task, so
            // the attach died with CancellationError every time and the
            // countdown started over for ever (M-109). Hand the slot back first.
            self.reconnectTask = nil
            await self.reconnect()
        }
    }

    /// Drop the current source and try the kernel again. The transcript
    /// stays on screen until the replay replaces it, so a reconnect never
    /// shows an empty chat.
    func reconnect() async {
        let attempt = reconnectAttempt
        refusal = nil
        disconnect()
        reconnectAttempt = attempt
        agents.removeAll()
        workers.removeAll()
        step = ""
        lastFirstToken = nil
        await connect()
        if mode == .live || mode == .server {
            reconnectAttempt = 0
            flushPending()
        } else if settings.chatEndpoint != nil, mode != .connecting {
            scheduleReconnect()
        }
    }

    /// A worker's transcript, replayed once from the kernel.
    func history(agent: String) async -> [ChatItem] {
        await source?.history(agent: agent) ?? []
    }

    var running: Int { workers.filter(\.running).count }

    /// Older lines of a long project, 200 at a time, from the top of what
    /// is shown; the view keeps its place.
    func loadEarlier() async {
        guard earlierLines > 0, !loadingEarlier, firstSeq > 1, let source else { return }
        loadingEarlier = true
        defer { loadingEarlier = false }
        guard let page = await source.earlier(before: firstSeq, limit: 200) else { return }
        if page.from > 0, page.to >= page.from, !page.items.isEmpty {
            anchorAfterPrepend = items.first?.id
            items.insert(contentsOf: page.items, at: 0)
            firstSeq = page.from
            earlierLines = max(0, page.from - 1)
        } else {
            earlierLines = 0
        }
    }

    private func registerPushIfLive() {
        guard mode == .live, let token = pushToken, let source else { return }
        source.registerPush(token: token, sandbox: pushSandbox)
    }

    /// The user has seen the card: tell the kernel, which tells every
    /// other client, so the desktop's badge drops too.
    func markSeen() {
        guard let last = unseen.last?.id else { return }
        source?.markSeen(through: last)
        unseen.removeAll()
        onSeen?(last)
    }

    /// A line said out loud on the call, put in the chat so the
    /// conversation reads there afterwards. **Display only**: it is never
    /// sent anywhere and never wakes the kernel. Typed lines still go
    /// through `send`, which does.
    ///
    /// A delegated turn is worded twice: the kernel writes an answer and the
    /// voice says it in its own words. The chat shows the conversation, so
    /// the spoken wording is the one that stays and the kernel's goes. The
    /// question is the other way round — one line either way, never two.
    func spoke(_ text: String, byUser: Bool) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        var item = ChatItem(byUser ? .user(trimmed) : .agent(trimmed, streaming: false))
        item.spoken = true
        if byUser {
            answerWasSpoken = false
        } else {
            // The kernel's wording of the same answer, if it got here first.
            dropKernelWordingOfThisTurn()
            answerWasSpoken = true
        }
        items.append(item)
        if byUser {
            spokenLocally.append((item.id, trimmed))
            if spokenLocally.count > 20 { spokenLocally.removeFirst() }
        }
    }

    /// Removes the kernel's own reply text for the turn in progress, leaving
    /// its record of the work — the tool lines and the time it took — alone.
    /// Those say what happened; only the prose is said twice.
    private func dropKernelWordingOfThisTurn() {
        closeOpenAgentMessage()
        let turnStart = items.lastIndex(where: { if case .user = $0.kind { return true } else { return false } }).map { $0 + 1 } ?? 0
        guard turnStart < items.count else { return }
        let written = Set(items[turnStart...].filter { $0.isAgent && !$0.spoken }.map(\.id))
        guard !written.isEmpty else { return }
        items.removeAll { written.contains($0.id) }
    }

    /// A line the app itself has to say (a picker or dictation problem).
    /// `failed` is red, and is for something that went wrong and stayed
    /// wrong. A setback the app has already handled is said in the calm
    /// muted voice instead — red on a line whose news is "nothing of yours
    /// was lost" reads as the opposite of what it says.
    func notice(_ text: String, failed: Bool = true) {
        items.append(ChatItem(.notice(text, failed: failed)))
    }

    /// Open another kernel: the pod, or a machine/project on the hub.
    func switchTarget(_ target: KernelTarget) async {
        guard target != settings.kernelTarget || mode != .live else { return }
        if target != settings.kernelTarget {
            items.removeAll()
            earlierLines = 0
            identity = nil
            store = nil
            unseen.removeAll()
            // Lines typed for the old project stay with it: never carried
            // to the next chat and sent there (Jacob, build 956).
            pendingSends.removeAll()
        }
        settings.kernelTarget = target
        reconnectAttempt = 0
        refusal = nil
        await reconnect()
    }

    /// What to call this chat: the project's own name from its
    /// `project.toml`, then the folder, and only then the raw target — the
    /// same order the chat's own header uses. The banner is drawn from this
    /// and used to say `arboslife/phone` where every other surface said
    /// `phone`, because it went straight to the target's label.
    ///
    /// The root agent's name is appended when the kernel gave it one.
    var title: String {
        let name = identity?.name ?? settings.kernelTarget.folder ?? settings.kernelTarget.label
        let agent = agentName
        return agent == "main" ? name : "\(name) · \(agent)"
    }

    /// The open question, if the kernel is waiting on one.
    var pendingAsk: (id: String?, options: [String])? {
        for item in items.reversed() {
            if case .ask(_, let options, let id, answered: false) = item.kind { return (id, options) }
        }
        return nil
    }

    /// Answer the open question — a tapped option or the typed line. The
    /// answer goes as an `answer` frame with the kernel's id, and the card
    /// closes; the kernel echoes the answer as a user line.
    func answer(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let ask = pendingAsk else { return }
        if let index = items.lastIndex(where: { if case .ask(_, _, _, answered: false) = $0.kind { return true } else { return false } }),
           case .ask(let q, let o, let id, _) = items[index].kind {
            items[index].kind = .ask(question: q, options: o, id: id, answered: true)
        }
        var card = ChatItem(.user(trimmed, pending: true))
        card.step = 0
        items.append(card)
        pendingSends.append((card.id, trimmed, false, settings.kernelTarget))
        busy = true
        sentAt = Date()
        source?.answer(text: trimmed, id: ask.id)
    }

    func send(_ text: String, attachments: [PendingAttachment] = []) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty || !attachments.isEmpty else { return }
        // A typed line while a question is open is the answer to it.
        if attachments.isEmpty, pendingAsk != nil { answer(trimmed); return }
        // Shown at once, as pending; the kernel's echo of the same words
        // makes it real. A turn already running gets the new words as a
        // steer at its next tool boundary; otherwise this starts one.
        let steer = busy
        answerWasSpoken = false
        if !unseen.isEmpty { markSeen() }
        // Files are named on the card; photos are drawn on it.
        let files = attachments.filter { !$0.isImage }
        let shown = files.isEmpty ? trimmed : (trimmed.isEmpty ? "" : trimmed + "\n") + files.map { "📎 \($0.name)" }.joined(separator: "\n")
        var card = ChatItem(.user(shown, pending: true))
        card.images = attachments.filter(\.isImage).map(\.storedName)
        for file in attachments where file.isImage { AttachmentCache.store(file.data, as: file.storedName) }
        // A steer does not close the reply it interrupts: the words keep
        // streaming above the card, which waits at the tail for its echo.
        if !steer { closeOpenAgentMessage() }
        items.append(card)
        pendingSends.append((card.id, trimmed, steer, settings.kernelTarget))
        guard let source, mode == .live || mode == .server || mode == .mock else { return }
        busy = true
        sentAt = Date()
        Task {
            do {
                try await source.send(text: trimmed, steer: steer, attachments: attachments)
            } catch {
                busy = false
            }
        }
    }

    /// After a reconnect: whatever was typed while the link was down goes
    /// out now, oldest first, in one turn each.
    private func flushPending() {
        guard let source, !pendingSends.isEmpty else { return }
        guard pendingSends.allSatisfy({ $0.target == settings.kernelTarget }) else {
            pendingSends.removeAll { $0.target != settings.kernelTarget }
            if pendingSends.isEmpty { return }
            return flushPending()
        }
        let queue = pendingSends
        busy = true
        Task {
            for entry in queue {
                try? await source.send(text: entry.text, steer: false, attachments: [])
                try? await Task.sleep(for: .milliseconds(200))
            }
        }
    }

    // MARK: - Private

    private func adopt(_ source: ChatSource, mode: Mode) {
        pump?.cancel()
        self.source = source
        self.mode = mode
        pump = Task { [weak self] in
            for await update in source.updates {
                guard let self, !Task.isCancelled else { return }
                self.apply(update)
            }
        }
    }

    private func apply(_ update: ChatUpdate) {
        switch update {
        case .history(let seed, let earlier, let first):
            let stillPending = items.filter { if case .user(_, pending: true) = $0.kind { return true } else { return false } }
            items = seed + stillPending
            earlierLines = earlier
            firstSeq = first
        case .item(let item):
            if case .user = item.kind { answerWasSpoken = false }
            // This turn's answer was already said out loud, in the voice's
            // own words. The kernel's wording of it is the second telling.
            if item.isAgent, answerWasSpoken { return }
            // The kernel's own record of a question that was spoken here:
            // one line, not two. The spoken copy goes and the kernel's stays,
            // because the kernel's carries its seq and its answer.
            if case .user(let text, _) = item.kind,
               let spokenIndex = spokenLocally.firstIndex(where: { $0.text == text }) {
                let local = spokenLocally.remove(at: spokenIndex)
                items.removeAll { $0.id == local.id }
            }
            if case .user(let text, _) = item.kind, let index = pendingSends.firstIndex(where: { $0.text == text || ($0.text.isEmpty && text.isEmpty) }) {
                // The kernel echoed a line typed here: the pending card is real now.
                let pending = pendingSends.remove(at: index)
                if let row = items.firstIndex(where: { $0.id == pending.id }), case .user(let shown, _) = items[row].kind {
                    items[row].kind = .user(shown)
                    return
                }
            }
            // The transcript's own line for a question the card already
            // shows: the card is the line.
            if case .agent(let text, _) = item.kind,
               items.contains(where: { if case .ask(let q, _, _, _) = $0.kind { return q.trimmingCharacters(in: .whitespacesAndNewlines) == text.trimmingCharacters(in: .whitespacesAndNewlines) } else { return false } }) {
                return
            }
            // …and the kernel's "Waiting for your answer" notice: the card says it.
            if case .notice(let text, false) = item.kind, text.hasPrefix("Waiting for your answer"), pendingAsk != nil { return }
            closeOpenAgentMessage()
            items.append(item)
        case .agentDelta(let delta, let step):
            if answerWasSpoken { return }
            if let sentAt {
                lastFirstToken = Date().timeIntervalSince(sentAt)
                self.sentAt = nil
            }
            // The open bubble is the last item, or the last one above the
            // steer cards typed while it streamed: those ride at the tail
            // so the reply keeps its place (journey run 5).
            if let index = openAgentIndex, case .agent(let text, streaming: true) = items[index].kind,
               step == 0 || items[index].step == 0 || items[index].step == step {
                items[index].kind = .agent(text + delta, streaming: true)
                if items[index].step == 0 { items[index].step = step }
            } else {
                closeOpenAgentMessage()
                let at = items.lastIndex(where: { !$0.isPendingUser }).map { $0 + 1 } ?? items.count
                items.insert(ChatItem(.agent(delta, streaming: true), step: step), at: at)
            }
        case .agentDone:
            closeOpenAgentMessage()
        case .agentReplace(let raw, let step):
            if answerWasSpoken { return }
            let text = ToolMarkup.strip(raw)
            // The step's own item when the kernel numbers steps; the last
            // agent item when it does not (older kernels). Steps restart
            // every turn, so only this turn's items — after the last prompt
            // card — are candidates; a settled line must never reach back
            // into an earlier reply.
            let userCards = items.indices.filter { if case .user(_, pending: false) = items[$0].kind { return true } else { return false } }
            let turnStart = userCards.last.map { $0 + 1 } ?? 0
            var index = step > 0
                ? items[turnStart...].lastIndex(where: { $0.isAgent && $0.step == step })
                : items[turnStart...].lastIndex(where: \.isAgent)
            // A steer typed while the reply streamed closed its bubble and
            // now sits between the streamed words and the settled text
            // ("That" … steer … "That line is already…", journey run 5).
            // Reach back one card, but only for the bubble whose words the
            // settled text begins with — never into an earlier reply.
            if index == nil, userCards.count >= 2, !text.isEmpty {
                let before = (userCards[userCards.count - 2] + 1)..<userCards[userCards.count - 1]
                index = items[before].lastIndex(where: { item in
                    guard item.isAgent, step == 0 || item.step == step, case .agent(let streamed, _) = item.kind else { return false }
                    let head = streamed.trimmingCharacters(in: .whitespacesAndNewlines)
                    return !head.isEmpty && text.hasPrefix(head)
                })
            }
            if let index {
                let wasOpen = items[index].isStreamingAgent
                // The kernel's whole text for one step can land after the
                // next step's tokens already started (step 0 only): when the
                // streamed text runs past it, keep streaming instead of cutting.
                if wasOpen, step == 0, case .agent(let streamedText, _) = items[index].kind,
                   streamedText.trimmingCharacters(in: .whitespacesAndNewlines).hasPrefix(text),
                   streamedText.trimmingCharacters(in: .whitespacesAndNewlines).count > text.count {
                    break
                }
                if text.isEmpty {
                    items.remove(at: index)
                    break
                }
                items[index].kind = .agent(text, streaming: false)
                if wasOpen { onAgentMessage?(text) }
            } else if !text.isEmpty {
                // Nothing streamed for this step (attached mid-turn): a new message.
                closeOpenAgentMessage()
                items.append(ChatItem(.agent(text, streaming: false), step: step))
                onAgentMessage?(text)
            }
        case .turn(let running):
            busy = running
            // A fresh turn: whatever was last said out loud belongs to the
            // one before it, and must not swallow this one's text.
            if running { answerWasSpoken = false }
            if !running { closeOpenAgentMessage() }
        case .notify(let notification):
            // Live, with the chat in front: the user is reading it — seen,
            // and every other client hears so. Otherwise it waits on the
            // card (and, away, becomes a banner).
            if !notification.replayed, UIApplication.shared.applicationState == .active {
                source?.markSeen(through: notification.id)
                return
            }
            if !unseen.contains(where: { $0.id == notification.id }) {
                unseen.append(notification)
                unseen.sort { $0.id < $1.id }
            }
            if !notification.replayed { onNotify?(notification) }
        case .seen(let through):
            unseen.removeAll { $0.id <= through }
            onSeen?(through)
        case .pushed(let enabled, let reason):
            onPushed?(enabled, reason)
        case .ask(let question, let options, let id):
            // The transcript line that asked may already be on screen (a
            // replay, or the settled text): it becomes the card, so the
            // question is not drawn twice.
            let wanted = question.trimmingCharacters(in: .whitespacesAndNewlines)
            let same = items.lastIndex(where: { item in
                if case .agent(let text, _) = item.kind { return text.trimmingCharacters(in: .whitespacesAndNewlines) == wanted }
                return false
            })
            if let same {
                items[same].kind = .ask(question: question, options: options, id: id, answered: false)
                // The replay's own "Waiting for your answer" line says what the card says.
                items.removeAll { item in
                    if case .notice(let text, false) = item.kind { return text.hasPrefix("Waiting for your answer") }
                    return false
                }
            } else if !items.contains(where: { if case .ask(_, _, let known, false) = $0.kind, known != nil, known == id { return true } else { return false } }) {
                closeOpenAgentMessage()
                items.append(ChatItem(.ask(question: question, options: options, id: id, answered: false)))
            }
            busy = false
        case .agents(let list):
            agents = list
        case .workers(let list):
            workers = list
        case .step(let text):
            step = text
        case .identity(let face):
            identity = face
        case .store(let address):
            store = address
        case .dropped(let why):
            // One calm line under the transcript (the mode notice), not an
            // error plus a reassurance. A second drop for the same close
            // (the hub's word, then the socket) leaves the countdown alone.
            //
            // The line keeps retrying either way, because a kernel that is
            // not running can start; it just says which thing is not there
            // when the hub told us. "Link lost" is wrong for a live link and
            // a stopped kernel, and the two want different things of Jacob.
            //
            // The reason and the close are two drops, in that order: the hub
            // says why, then the socket goes. Only the first carries a reason,
            // so a nameless second one must not erase it — it used to, which
            // is why a stopped kernel read "Link lost" a second later despite
            // the hub having said exactly which thing was not there. Cleared
            // on connect instead, where it is known to be stale.
            let named = Self.inPlainWords(why)
            if named != why { standing = named }
            guard mode != .offline || reconnectTask == nil else { return }
            mode = .offline
            busy = false
            if settings.chatEndpoint != nil, refusal == nil { scheduleReconnect() }
        case .refused(let why):
            refusal = Self.inPlainWords(why)
            reconnectTask?.cancel()
            reconnectTask = nil
            reconnectIn = nil
            mode = .offline
            busy = false
        }
        // A long project pages back 200 at a time; the cap is for a day-long
        // stream, not for the history the user asked to see.
        if items.count > 2000 { items.removeFirst(items.count - 2000) }
    }

    /// The streaming bubble: the last item, or the last one above the
    /// pending steer cards at the tail.
    private var openAgentIndex: Int? {
        guard let index = items.lastIndex(where: { !$0.isPendingUser }), items[index].isStreamingAgent else { return nil }
        return index
    }

    private func closeOpenAgentMessage() {
        guard let index = openAgentIndex,
              case .agent(let text, streaming: true) = items[index].kind else { return }
        // Markup a model wrote as a call goes; a reply that was only markup
        // settles to no line on the kernel (#278), so no bubble stays here.
        let final = ToolMarkup.strip(text)
        if final.isEmpty {
            items.remove(at: index)
            return
        }
        items[index].kind = .agent(final, streaming: false)
        onAgentMessage?(final)
    }
}
