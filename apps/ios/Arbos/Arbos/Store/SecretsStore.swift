// Secrets state (D83): the Settings overview, per-agent pages and the actions behind every
// switch. Errors land in `notice` with the relay's message; the Settings sheet shows them above
// itself. Nothing here ever holds a value after the call that sent it returns.

import Foundation

extension String {
    /// `"approval:abc".stripping(prefix: "approval:")` → `"abc"`; nil when the prefix is absent.
    func stripping(prefix: String) -> String? {
        hasPrefix(prefix) ? String(dropFirst(prefix.count)) : nil
    }
}

@MainActor
extension AppModel {
    var secretsRowValue: String? {
        guard credentials != nil else { return nil }
        return secrets?.rowValue ?? "No secrets yet"
    }

    /// The relay can connect a 1Password vault (Phase 2 deployed) **and** the remote flag
    /// `features.vault_connect` is on (default off since 2026-10-21). Gates every entry point
    /// to the Connect 1Password flow; connected vaults still show and work.
    var vaultProvidersAvailable: Bool {
        guard let s = secrets else { return false }
        return s.vaultConnect && !s.providers.isEmpty
    }

    func refreshSecrets() async {
        guard credentials != nil, !isFixture else { return }
        do {
            secrets = try await relay.secrets()
        } catch let e as RelayError where e.status == 404 {
            // Older relays have no secrets route.
            secrets = SecretsOverview(paused: false, pausedAt: nil, vaults: [], secrets: [], requestsPending: 0, activity: [])
        } catch {
            // Leave the previous state; opening Settings must not alert on a blip.
        }
    }

    func refreshProjectSecrets(_ projectId: Id) async {
        guard credentials != nil, !isFixture else { return }
        if let v = try? await relay.projectSecrets(projectId) {
            projectSecrets[projectId] = v
        }
    }

    /// Run one secrets call; refresh the overview after; surface the relay's message on failure.
    /// Returns the error text (nil on success) so sheets can show it inline too.
    /// The infinitive the note reads for a Secrets / goals action's label.
    static func secretsVerb(_ what: String) -> String {
        switch what {
        case "Save": return "save the secret"
        case "Delete secret": return "delete the secret"
        case "Disconnect": return "disconnect the vault"
        case "Give access": return "give access"
        case "Remove director": return "remove the director"
        case "Remove from all agents": return "remove it from the agents"
        case "Replace token": return "replace the token"
        case "Scrub from context": return "scrub it from the context"
        case "Turn off": return "turn it off"
        case "Create goal": return "create the goal"
        case "Delete goal": return "delete the goal"
        case "Save goal": return "save the goal"
        default: return what.prefix(1).lowercased() + what.dropFirst()
        }
    }

    @discardableResult
    func secretsAction(_ what: String, refreshProject: Id? = nil, _ body: @escaping () async throws -> Void) async -> String? {
        guard !secretsBusy else { return "Busy" }
        secretsBusy = true
        defer { secretsBusy = false }
        do {
            try await body()
            await refreshSecrets()
            if let p = refreshProject { await refreshProjectSecrets(p) }
            return nil
        } catch {
            let failure = UserError.classify(Self.secretsVerb(what), error)
            fail(failure)
            return failure.sentence
        }
    }

    func setSecretsPaused(_ paused: Bool) {
        // Optimistic: the switch flips now, the relay confirms (or the refresh puts it back).
        secrets?.paused = paused
        Task { await secretsAction(paused ? "Pause all secrets" : "Secrets") { try await self.relay.setSecretsPaused(paused) } }
    }

    func setSecretAccess(secretId: String, projectId: String, enabled: Bool, note: String? = nil, hosts: [String]? = nil, scope: [String: Any]? = nil) {
        if let i = secrets?.secrets.firstIndex(where: { $0.id == secretId }),
           let j = secrets?.secrets[i].agents.firstIndex(where: { $0.projectId == projectId }) {
            secrets?.secrets[i].agents[j].enabled = enabled
        }
        if let i = projectSecrets[projectId]?.items.firstIndex(where: { $0.secret?.id == secretId }) {
            projectSecrets[projectId]?.items[i].enabled = enabled
        }
        Task {
            await secretsAction(enabled ? "Turn on" : "Turn off", refreshProject: projectId) {
                _ = try await self.relay.setSecretAccess(secretId, projectId: projectId, enabled: enabled, note: note, hosts: hosts, scope: scope)
            }
        }
    }

    func removeSecretAccess(secretId: String, projectId: String) {
        Task { await secretsAction("Turn off", refreshProject: projectId) { try await self.relay.deleteSecretAccess(secretId, projectId: projectId) } }
    }

    func revokeSecretEverywhere(_ secretId: String) {
        Task { await secretsAction("Remove from all agents") { _ = try await self.relay.revokeSecretEverywhere(secretId) } }
    }

    func scrubSecret(_ secretId: String) {
        Task { await secretsAction("Scrub from context") { _ = try await self.relay.scrubSecret(secretId) } }
    }

    func deleteSecret(_ secretId: String) {
        secrets?.secrets.removeAll { $0.id == secretId }
        Task { await secretsAction("Delete secret") { try await self.relay.deleteSecret(secretId) } }
    }

    func patchSecret(_ secretId: String, _ body: [String: Any]) {
        // The two switches are exclusive on the relay; show that at once.
        if let i = secrets?.secrets.firstIndex(where: { $0.id == secretId }) {
            if let on = body["inject_env"] as? Bool {
                secrets?.secrets[i].injectEnv = on
                if on { secrets?.secrets[i].askEveryTime = false }
            }
            if let ask = body["ask_every_time"] as? Bool {
                secrets?.secrets[i].askEveryTime = ask
                if ask { secrets?.secrets[i].injectEnv = false }
            }
            if let others = body["others_ask"] as? String {
                secrets?.secrets[i].othersAsk = others
            }
        }
        Task { await secretsAction("Save") { _ = try await self.relay.patchSecret(secretId, body) } }
    }

    /// One People row's switch (spec §7.7). Optimistic; the relay's reply follows.
    func setSecretPerson(secretId: String, userId: String, enabled: Bool) {
        if let i = secrets?.secrets.firstIndex(where: { $0.id == secretId }),
           let j = secrets?.secrets[i].directors.firstIndex(where: { $0.userId == userId }) {
            secrets?.secrets[i].directors[j].enabled = enabled
            secrets?.secrets[i].directors[j].disabledAt = enabled ? nil : Int64(Date().timeIntervalSince1970 * 1000)
        }
        Task {
            await secretsAction("Save") {
                let s = try await self.relay.setSecretDirector(secretId, userId: userId, note: nil, enabled: enabled)
                if let i = self.secrets?.secrets.firstIndex(where: { $0.id == s.id }) {
                    self.secrets?.secrets[i] = s
                }
            }
        }
    }

    /// People picked in the member picker join the list (rows on, no note yet). Returns false
    /// when any add failed (the notice says which).
    func addSecretPeople(secretId: String, handles: [String]) async -> Bool {
        var ok = true
        for handle in handles {
            if await secretsAction("Give access", {
                let s = try await self.relay.addSecretDirector(secretId, handle: handle, note: nil)
                if let i = self.secrets?.secrets.firstIndex(where: { $0.id == s.id }) {
                    self.secrets?.secrets[i] = s
                }
            }) != nil {
                ok = false
            }
        }
        return ok
    }

    func updateSecretDirector(secretId: String, userId: String, note: String) {
        Task {
            await secretsAction("Save") {
                let s = try await self.relay.setSecretDirector(secretId, userId: userId, note: note)
                if let i = self.secrets?.secrets.firstIndex(where: { $0.id == s.id }) {
                    self.secrets?.secrets[i] = s
                }
            }
        }
    }

    func removeSecretDirector(secretId: String, userId: String) {
        if let i = secrets?.secrets.firstIndex(where: { $0.id == secretId }) {
            secrets?.secrets[i].directors.removeAll { $0.userId == userId }
        }
        Task {
            await secretsAction("Remove director") {
                let s = try await self.relay.deleteSecretDirector(secretId, userId: userId)
                if let i = self.secrets?.secrets.firstIndex(where: { $0.id == s.id }) {
                    self.secrets?.secrets[i] = s
                }
            }
        }
    }

    func decideSecretRequest(_ id: String, decision: String, projectId: String? = nil) {
        Task {
            await secretsAction("Secrets", refreshProject: projectId) { try await self.relay.decideSecretRequest(id, decision: decision) }
        }
    }

    func setVaultAccess(vaultId: String, projectId: String, enabled: Bool, note: String? = nil) {
        if let i = projectSecrets[projectId]?.items.firstIndex(where: { $0.vault?.id == vaultId }) {
            projectSecrets[projectId]?.items[i].enabled = enabled
        }
        Task { await secretsAction(enabled ? "Turn on" : "Turn off", refreshProject: projectId) { try await self.relay.setVaultAccess(vaultId, projectId: projectId, enabled: enabled, note: note) } }
    }

    func deleteVault(_ vaultId: String) {
        secrets?.vaults.removeAll { $0.id == vaultId }
        Task { await secretsAction("Disconnect") { try await self.relay.deleteVault(vaultId) } }
    }
}

// MARK: - Goals (long-running objectives)

@MainActor
extension AppModel {
    var goalsRowValue: String? {
        guard credentials != nil else { return nil }
        let open = goals.filter(\.isOpen)
        if open.isEmpty { return goals.isEmpty ? "No goals yet" : "\(goals.count) done" }
        return "\(open.count) active"
    }

    var openGoals: [GoalItem] { goals.filter(\.isOpen) }

    func refreshGoals() async {
        guard credentials != nil, !isFixture else { return }
        do {
            goals = try await relay.goals()
        } catch let e as RelayError where e.status == 404 {
            goals = []
        } catch {
            // Keep the previous list; opening Settings must not alert on a blip.
        }
    }

    @discardableResult
    func goalsAction(_ what: String, _ body: @escaping () async throws -> Void) async -> String? {
        guard !goalsBusy else { return "Busy" }
        goalsBusy = true
        defer { goalsBusy = false }
        do {
            try await body()
            await refreshGoals()
            return nil
        } catch {
            let failure = UserError.classify(Self.secretsVerb(what), error)
            fail(failure)
            return failure.sentence
        }
    }

    func createGoal(projectId: Id, title: String, outcome: String, nextStep: String, checkInCron: String?, autonomy: String) {
        var body: [String: Any] = [
            "title": title,
            "outcome": outcome,
            "next_step": nextStep,
            "status": "active",
            "autonomy": autonomy,
        ]
        if let checkInCron, !checkInCron.isEmpty {
            body["checkInCron"] = checkInCron
            body["checkInTz"] = TimeZone.current.identifier
        }
        Task {
            await goalsAction("Create goal") {
                _ = try await self.relay.createGoal(projectId: projectId, body)
            }
        }
    }

    func patchGoal(_ goal: GoalItem, _ body: [String: Any]) {
        if let i = goals.firstIndex(where: { $0.id == goal.id }) {
            if let s = body["status"] as? String { goals[i].status = s }
            if let t = body["title"] as? String { goals[i].title = t }
            if let n = body["next_step"] as? String { goals[i].nextStep = n }
            if let a = body["autonomy"] as? String { goals[i].autonomy = a }
            if let c = body["checkInCron"] as? String { goals[i].checkInCron = c.isEmpty ? nil : c }
        }
        Task {
            await goalsAction("Save goal") {
                _ = try await self.relay.patchGoal(projectId: goal.projectId, goalId: goal.id, body)
            }
        }
    }

    func setGoalStatus(_ goal: GoalItem, status: String) {
        patchGoal(goal, ["status": status])
    }

    func deleteGoal(_ goal: GoalItem) {
        goals.removeAll { $0.id == goal.id }
        Task {
            await goalsAction("Delete goal") {
                try await self.relay.deleteGoal(projectId: goal.projectId, goalId: goal.id)
            }
        }
    }
}
