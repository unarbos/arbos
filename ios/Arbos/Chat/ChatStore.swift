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
    private var reconnectAttempt = 0
    /// Typed lines the kernel has not echoed yet, oldest first.
    private var pendingSends: [(id: UUID, text: String, steer: Bool, target: KernelTarget)] = []
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
            registerPushIfLive()
            return true
        } catch {
            #if DEBUG
            print("attach \(endpoint.url): \(error)")
            #endif
        }
        live.stop()
        pump?.cancel()
        source = nil
        return false
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

    /// A line the app itself has to say (a picker or dictation problem).
    func notice(_ text: String) {
        items.append(ChatItem(.notice(text, failed: true)))
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

    /// What the header calls the chat: the machine/project, then the
    /// root agent's name when the kernel gave it one.
    var title: String {
        let target = settings.kernelTarget.label
        let agent = agentName
        return agent == "main" ? target : "\(target) · \(agent)"
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
        case .dropped:
            // One calm line under the transcript (the mode notice), not an
            // error plus a reassurance. A second drop for the same close
            // (the hub's word, then the socket) leaves the countdown alone.
            guard mode != .offline || reconnectTask == nil else { return }
            mode = .offline
            busy = false
            if settings.chatEndpoint != nil, refusal == nil { scheduleReconnect() }
        case .refused(let why):
            refusal = why.replacingOccurrences(of: "hub: ", with: "")
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
