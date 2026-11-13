// D82: the first sign-in on a fresh account. Instead of "Loading your agents…"
// the user watches a terminal type while the relay creates their computer
// (Daytona box, `GET /machine` state waking → awake), the first agent
// ("Home": project + workspace) and its first chat (room sync + the agent's
// greeting). The terminal then morphs into that greeting bubble and the real
// chat takes over. Every line here is driven by relay facts, never by a timer.
//
// Stages (internal/onboarding-lottie-implementation.md):
//   computer    — no project yet, or machine.rawState is waking / asleep
//   agent       — machine awake, project exists, its workspace not running yet
//   almostReady — workspace running; chat room syncing, greeting being written
//   finishing   — the 1.5 s morph into the greeting bubble (loop-boundary aligned)
//   failed      — machine.rawState == error, or more than 3 minutes elapsed

import Foundation
import SwiftUI

struct FirstLogin: Equatable {
    enum Stage: Equatable {
        case computer, agent, almostReady, finishing, failed
    }

    /// Period of the terminal loop; the finish starts on a boundary of it.
    static let loopSeconds: Double = 4.0
    static let finishSeconds: Double = 1.5
    static let minStageHold: Double = 1.2
    static let timeoutSeconds: Double = 180
    static let slowAfterSeconds: Double = 45
    static let agentName = "Home"

    var stage: Stage = .computer
    var startedAt: Date
    var stageSince: Date
    /// Second line under the hero: a fact from the relay, or nil.
    var detail: String?
    /// S14 quiet block text while `.failed`.
    var error: String?
    var projectId: Id?
    var chatId: Id?
    var agentName: String = FirstLogin.agentName
    var finishStartedAt: Date?
    /// The first registry answer arrived (so an empty list really means "new account").
    var registrySeen = false
    /// `POST /projects` in flight.
    var creating = false
    var greetingWritten = false
    /// First moment the machine reported awake (fallback when the box never
    /// reports its workspace state).
    var awakeSince: Date?
    /// Debug / fixture: frozen on one stage, no transitions.
    var demo = false

    var title: String {
        switch stage {
        case .computer: return "Creating your computer"
        case .agent: return "Setting up your first agent"
        case .almostReady, .finishing: return "Almost ready"
        case .failed: return "Can’t reach your computer"
        }
    }

    /// Seconds into the current loop, 0 ..< loopSeconds.
    func loopPhase(at now: Date) -> Double {
        now.timeIntervalSince(startedAt).truncatingRemainder(dividingBy: FirstLogin.loopSeconds)
    }

    /// 0…1 through the morph, nil before it starts.
    func finishProgress(at now: Date) -> Double? {
        guard let t0 = finishStartedAt else { return nil }
        return min(1, max(0, now.timeIntervalSince(t0) / FirstLogin.finishSeconds))
    }
}

extension AppModel {
    private static let pendingKey = "firstLogin.pending"

    /// A fresh sign-in on this device: show the first-login screen until the
    /// registry proves the account already has agents.
    func beginFirstLogin() {
        guard !isFixture, firstLogin == nil else { return }
        let now = Date()
        firstLogin = FirstLogin(startedAt: now, stageSince: now)
        UserDefaults.standard.set(true, forKey: AppModel.pendingKey)
        // Always-on (D73): the relay starts the machine at sign-in; asking again is
        // a no-op there and covers relays (and test accounts) that did not.
        if let owner = accountLifetime.ticket {
            Task { await startMachineQuietly(expectedOwner: owner) }
        }
        startFirstLoginPolling()
    }

    /// App relaunched while provisioning was still running: pick it back up.
    func resumeFirstLoginIfPending() {
        guard UserDefaults.standard.bool(forKey: AppModel.pendingKey) else { return }
        beginFirstLogin()
    }

    /// Fixture / rig hook: `ARBOS_SHOW=onboarding` with `ARBOS_ONBOARDING_STAGE=1|2|3|fail`.
    func demoFirstLogin(stage: String?) {
        let now = Date()
        var fl = FirstLogin(startedAt: now, stageSince: now)
        fl.demo = true
        switch stage ?? "1" {
        case "2":
            fl.stage = .agent
            fl.detail = "Starting Home’s workspace · arbos-agent"
        case "3":
            fl.stage = .almostReady
            fl.detail = "Syncing your first chat"
        case "fail":
            fl.stage = .failed
            fl.error = "The relay could not start your computer."
        default:
            fl.detail = "Starting a 4-core machine with 8 GB of memory"
        }
        firstLogin = fl
        // `ARBOS_ONBOARDING_AUTOPLAY=1`: walk every stage on a timer (rig recordings).
        if DebugHooks.onboardingAutoplay, let owner = accountLifetime.ticket {
            firstLoginTask = Task { [weak self] in
                let steps: [(FirstLogin.Stage, String?)] = [
                    (.agent, "Starting Home’s workspace · arbos-agent"),
                    (.almostReady, "Syncing your first chat"),
                ]
                for (stage, detail) in steps {
                    try? await Task.sleep(nanoseconds: 2_500_000_000)
                    guard let self, self.accountLifetime.accepts(owner), var fl = self.firstLogin else { return }
                    fl.stage = stage; fl.detail = detail; fl.stageSince = Date()
                    self.firstLogin = fl
                }
                try? await Task.sleep(nanoseconds: 2_500_000_000)
                guard let self, self.accountLifetime.accepts(owner), var fl = self.firstLogin else { return }
                fl.stage = .finishing; fl.finishStartedAt = Date(); fl.stageSince = Date()
                self.firstLogin = fl
                try? await Task.sleep(nanoseconds: UInt64((FirstLogin.finishSeconds + 0.1) * 1_000_000_000))
                guard self.accountLifetime.accepts(owner) else { return }
                self.endFirstLogin()
            }
        }
    }

    private func startFirstLoginPolling() {
        guard let owner = accountLifetime.ticket else { return }
        firstLoginTask?.cancel()
        firstLoginTask = Task { [weak self] in
            // Fast lane while provisioning: the 15 s registry poll is too slow
            // for stage changes that take seconds.
            var tick = 0
            while !Task.isCancelled {
                guard let self, self.accountLifetime.accepts(owner), self.firstLogin != nil else { return }
                if tick % 3 == 0, !self.firstLoginIsDemo {
                    await self.refreshRegistry(expectedOwner: owner)
                }
                guard self.accountLifetime.accepts(owner), !Task.isCancelled else { return }
                self.firstLoginTick()
                tick += 1
                try? await Task.sleep(nanoseconds: 500_000_000)
            }
        }
    }

    private var firstLoginIsDemo: Bool { firstLogin?.demo == true }

    func endFirstLogin() {
        firstLoginTask?.cancel()
        firstLoginTask = nil
        firstLogin = nil
        greetingBubbleFrame = nil
        UserDefaults.standard.removeObject(forKey: AppModel.pendingKey)
        applyOpenServerHook()
    }

    /// Retry from the S14 block: ask the relay to (re)start the machine and
    /// run the stages again from the top.
    func retryFirstLogin() {
        guard var fl = firstLogin, let owner = accountLifetime.ticket else { return }
        let now = Date()
        fl.stage = .computer
        fl.stageSince = now
        fl.startedAt = now
        fl.error = nil
        fl.detail = nil
        fl.awakeSince = nil
        firstLogin = fl
        Task {
            guard accountLifetime.accepts(owner) else { return }
            await startMachineQuietly(expectedOwner: owner)
            guard accountLifetime.accepts(owner) else { return }
            await refreshRegistry(expectedOwner: owner)
            guard accountLifetime.accepts(owner) else { return }
            firstLoginTick()
        }
        if firstLoginTask == nil { startFirstLoginPolling() }
    }

    /// Called by `refreshRegistry` once the project list is in.
    func firstLoginRegistryUpdated() {
        guard var fl = firstLogin, !fl.demo else { return }
        fl.registrySeen = true
        firstLogin = fl
        firstLoginTick()
    }

    /// Called when a chat room reaches ROWS_DONE.
    func firstLoginChatSynced(_ chatId: Id) {
        guard let fl = firstLogin, !fl.demo, fl.chatId == nil || fl.chatId == chatId else { return }
        firstLoginTick()
    }

    /// One evaluation of the stage machine against the current model.
    func firstLoginTick() {
        guard var fl = firstLogin, !fl.demo else { return }
        let now = Date()
        let elapsed = now.timeIntervalSince(fl.startedAt)

        // The account already has agents: this was never a first login.
        if fl.registrySeen, fl.projectId == nil, !fl.creating,
           projects.contains(where: { !$0.shared && $0.name != FirstLogin.agentName }) {
            NSLog("first login: account already has agents (\(projects.map(\.name))) — not a first login")
            endFirstLogin()
            return
        }

        if fl.stage == .finishing {
            if let p = fl.finishProgress(at: now), p >= 1 {
                endFirstLogin()
            }
            return
        }

        // Failures: the relay says error, or nothing happened for 3 minutes.
        if fl.stage != .failed {
            if machine.reported, machine.rawState == .error {
                fl.stage = .failed
                fl.error = machineDetail ?? machineError ?? "The relay could not start your computer."
                fl.stageSince = now
                firstLogin = fl
                return
            }
            if elapsed > FirstLogin.timeoutSeconds {
                fl.stage = .failed
                fl.error = "Your computer is taking longer than expected."
                fl.stageSince = now
                firstLogin = fl
                return
            }
        } else {
            return
        }

        // First project: adopt the account's existing one, else create "Home".
        if fl.projectId == nil, fl.registrySeen, !fl.creating {
            if let own = projects.first(where: { !$0.shared }) {
                fl.projectId = own.id
                fl.agentName = own.name
            } else {
                guard let owner = accountLifetime.ticket else { return }
                fl.creating = true
                firstLogin = fl
                Task { [weak self] in
                    guard let self, self.accountLifetime.accepts(owner) else { return }
                    do {
                        try await self.createFirstProject(name: FirstLogin.agentName, expectedOwner: owner)
                    } catch {
                        guard self.accountLifetime.accepts(owner) else { return }
                        // 409: another device of this account made it first.
                        if (error as? RelayError)?.status != 409 {
                            NSLog("first login: create project failed: \(error.localizedDescription)")
                        }
                    }
                    guard self.accountLifetime.accepts(owner) else { return }
                    await self.refreshRegistry(expectedOwner: owner)
                    guard self.accountLifetime.accepts(owner) else { return }
                    if var fl = self.firstLogin {
                        fl.creating = false
                        self.firstLogin = fl
                    }
                    self.firstLoginTick()
                }
                return
            }
        }

        let home: Project? = fl.projectId.flatMap { id in self.project(id) }
        if let home, fl.chatId == nil, let chat = singleChat(of: home.id) {
            fl.chatId = chat.id
        }
        let machineAwake = machine.reported && machine.rawState == .awake
        if machineAwake, fl.awakeSince == nil { fl.awakeSince = now }
        if !machineAwake { fl.awakeSince = nil }
        let workspaceRunning = home?.workspace == .active
        // Boxes on an image older than the workspace report never say "running":
        // after 25 s awake with a synced chat, take the chat as the signal.
        let workspaceFallback = fl.awakeSince.map { now.timeIntervalSince($0) > 25 } ?? false
        let chatSynced = fl.chatId.map { caughtUp.contains($0) } ?? false

        if chatSynced, let chatId = fl.chatId, let home, !fl.greetingWritten {
            fl.greetingWritten = writeGreetingIfMissing(chatId: chatId, project: home)
        }

        let target: FirstLogin.Stage
        if !machineAwake || home == nil {
            target = .computer
        } else if !(workspaceRunning || (workspaceFallback && chatSynced)) {
            target = .agent
        } else {
            target = .almostReady
        }
        let ready = target == .almostReady && chatSynced && fl.greetingWritten

        // Detail: one real fact per stage.
        switch target {
        case .computer:
            if !machine.reported {
                fl.detail = "Connecting to the relay"
            } else if let cpu = machine.cpu, let mem = machine.memGib {
                fl.detail = "Starting a \(cpu)-core machine with \(mem) GB of memory"
            } else {
                fl.detail = "Starting your machine"
            }
            if elapsed > FirstLogin.slowAfterSeconds, let d = fl.detail {
                fl.detail = d + " · this usually takes under a minute"
            }
        case .agent:
            let name = fl.agentName
            if home?.workspace == .active {
                fl.detail = "\(name) is online"
            } else if let image = machine.image, !image.isEmpty {
                fl.detail = "Starting \(name)’s workspace · \(image)"
            } else {
                fl.detail = "Starting \(name)’s workspace on your computer"
            }
        case .almostReady:
            if !chatSynced {
                fl.detail = "Syncing your first chat"
            } else if !fl.greetingWritten {
                fl.detail = "Writing \(fl.agentName)’s first message"
            } else {
                fl.detail = "Opening your chat with \(fl.agentName)"
            }
        case .finishing, .failed:
            break
        }

        // Stages only move forward, and each one holds for at least 1.2 s.
        let order: [FirstLogin.Stage] = [.computer, .agent, .almostReady]
        let heldLongEnough = now.timeIntervalSince(fl.stageSince) >= FirstLogin.minStageHold
        if let ti = order.firstIndex(of: target), let ci = order.firstIndex(of: fl.stage), ti > ci, heldLongEnough {
            // Advance one stage per tick so a fast machine still shows every line.
            fl.stage = order[ci + 1]
            fl.stageSince = now
            NSLog("first login: stage → \(fl.stage) (\(fl.detail ?? ""))")
        }

        if ready, fl.stage == .almostReady, heldLongEnough, !reduceMotionForFirstLogin {
            // Wait for the loop seam so the morph starts from the state it was authored for.
            let phase = fl.loopPhase(at: now)
            if phase < 0.6 {
                beginFirstLoginFinish(&fl, now: now)
            }
        } else if ready, fl.stage == .almostReady, heldLongEnough {
            beginFirstLoginFinish(&fl, now: now)
        }
        firstLogin = fl
    }

    private var reduceMotionForFirstLogin: Bool { UIAccessibility.isReduceMotionEnabled }

    private func beginFirstLoginFinish(_ fl: inout FirstLogin, now: Date) {
        NSLog("first login: finishing → chat \(fl.chatId ?? "?")")
        fl.stage = .finishing
        fl.stageSince = now
        fl.finishStartedAt = now
        // Land in the chat underneath the overlay so the morph has a real bubble to meet.
        if let chatId = fl.chatId, let projectId = fl.projectId {
            var txn = Transaction()
            txn.disablesAnimations = true
            withTransaction(txn) {
                path = [.chat(chatId: chatId, projectId: projectId)]
            }
        }
    }

    /// True once a greeting row exists in the chat (ours or another device's).
    private func writeGreetingIfMissing(chatId: Id, project: Project) -> Bool {
        if messages(of: chatId).contains(where: { $0.greeting }) { return true }
        guard let doc = liveDoc(chatId) else { return false }
        if doc.readAll().contains(where: { $0.greeting }) { return true }
        let text = "Hi, I’m \(project.name). Your computer is ready — what should we build first?"
        do {
            try doc.insertAssistantGreeting(id: newId(), text: text, deviceId: deviceId,
                                            projectId: project.id, agentName: project.name)
            doc.commit()
            return true
        } catch {
            NSLog("first login: greeting failed: \(error.localizedDescription)")
            return false
        }
    }
}
