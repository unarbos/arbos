// Relay `/secrets` routes (D83 §10.1). Values travel one way: in a create or replace body,
// never back. Errors are the relay's `{"error":{"type","message"}}` through `RelayError`.

import ArbosFFI
import Foundation

extension RelayAPI {
    func secrets() async throws -> SecretsOverview {
        guard let v = SecretsOverview.parse(try await json("GET", "/secrets")) else {
            throw RelayError(status: 0, message: "Bad secrets reply")
        }
        return v
    }

    /// `POST /secrets` → the new secret. `value` (key/text) or `fields` (login) is sent once.
    func createSecret(_ body: [String: Any]) async throws -> SecretItem {
        let v = try await json("POST", "/secrets", body: body) as? [String: Any]
        guard let s = SecretItem.parse(v?["secret"]) else { throw RelayError(status: 0, message: "Bad secret reply") }
        return s
    }

    func secret(_ id: String) async throws -> (SecretItem, [SecretActivity]) {
        let v = try await json("GET", "/secrets/\(id)") as? [String: Any]
        guard let s = SecretItem.parse(v?["secret"]) else { throw RelayError(status: 404, message: "No such secret", type: "not_found") }
        return (s, (v?["activity"] as? [Any])?.compactMap(SecretActivity.parse) ?? [])
    }

    func patchSecret(_ id: String, _ body: [String: Any]) async throws -> SecretItem {
        let v = try await json("PATCH", "/secrets/\(id)", body: body) as? [String: Any]
        guard let s = SecretItem.parse(v?["secret"]) else { throw RelayError(status: 0, message: "Bad secret reply") }
        return s
    }

    func replaceSecretValue(_ id: String, _ body: [String: Any]) async throws -> SecretItem {
        let v = try await json("PUT", "/secrets/\(id)/value", body: body) as? [String: Any]
        guard let s = SecretItem.parse(v?["secret"]) else { throw RelayError(status: 0, message: "Bad secret reply") }
        return s
    }

    func deleteSecret(_ id: String) async throws {
        _ = try await request("DELETE", "/secrets/\(id)")
    }

    /// `PUT /secrets/{id}/access/{project_id}` — create or update one switch.
    func setSecretAccess(_ id: String, projectId: String, enabled: Bool, note: String? = nil, hosts: [String]? = nil, scope: [String: Any]? = nil) async throws -> SecretAgentAccess? {
        var body: [String: Any] = ["enabled": enabled]
        if let note { body["note"] = note }
        if let hosts { body["hosts"] = hosts }
        if let scope { body["scope"] = scope }
        let v = try await json("PUT", "/secrets/\(id)/access/\(projectId)", body: body) as? [String: Any]
        return SecretAgentAccess.parse(v?["access"])
    }

    func deleteSecretAccess(_ id: String, projectId: String) async throws {
        _ = try await request("DELETE", "/secrets/\(id)/access/\(projectId)")
    }

    func revokeSecretEverywhere(_ id: String) async throws -> SecretItem? {
        let v = try await json("POST", "/secrets/\(id)/revoke-all") as? [String: Any]
        return SecretItem.parse(v?["secret"])
    }

    /// `POST /secrets/{id}/people { handle, note? }` — put this person on the secret's People
    /// list (they may ask any agent with access to use it; spec §7.7).
    func addSecretDirector(_ id: String, handle: String, note: String?) async throws -> SecretItem {
        var body: [String: Any] = ["handle": handle]
        if let note { body["note"] = note }
        let v = try await json("POST", "/secrets/\(id)/people", body: body) as? [String: Any]
        guard let s = SecretItem.parse(v?["secret"]) else { throw RelayError(status: 0, message: "Bad secret reply") }
        return s
    }

    /// `PUT /secrets/{id}/people/{userId} { note?, enabled? }` — the row's note and switch.
    func setSecretDirector(_ id: String, userId: String, note: String?, enabled: Bool? = nil) async throws -> SecretItem {
        var body: [String: Any] = [:]
        if let note { body["note"] = note }
        if let enabled { body["enabled"] = enabled }
        let v = try await json("PUT", "/secrets/\(id)/people/\(userId)", body: body) as? [String: Any]
        guard let s = SecretItem.parse(v?["secret"]) else { throw RelayError(status: 0, message: "Bad secret reply") }
        return s
    }

    func deleteSecretDirector(_ id: String, userId: String) async throws -> SecretItem {
        let v = try await json("DELETE", "/secrets/\(id)/people/\(userId)") as? [String: Any]
        guard let s = SecretItem.parse(v?["secret"]) else { throw RelayError(status: 0, message: "Bad secret reply") }
        return s
    }

    /// `POST /secrets/{id}/scrub` — find the value in chats, sessions and logs; redact every copy.
    func scrubSecret(_ id: String) async throws -> SecretItem? {
        let v = try await json("POST", "/secrets/\(id)/scrub") as? [String: Any]
        return SecretItem.parse(v?["secret"])
    }

    func projectSecrets(_ projectId: String) async throws -> ProjectSecrets {
        guard let v = ProjectSecrets.parse(try await json("GET", "/projects/\(projectId)/secrets")) else {
            throw RelayError(status: 0, message: "Bad reply")
        }
        return v
    }

    /// `PUT /me/secrets/settings {paused}` — the kill switch.
    func setSecretsPaused(_ paused: Bool) async throws -> Bool {
        let v = try await json("PUT", "/me/secrets/settings", body: ["paused": paused]) as? [String: Any]
        return DocJSON.bool(v?["paused"])
    }

    func secretsActivity(secretId: String? = nil, projectId: String? = nil, vaultId: String? = nil, before: Int64? = nil, limit: Int = 100) async throws -> (items: [SecretActivity], nextBefore: Int64?) {
        var q: [String] = ["limit=\(limit)"]
        if let secretId { q.append("secret_id=\(secretId)") }
        if let projectId { q.append("project_id=\(projectId)") }
        if let vaultId { q.append("vault_id=\(vaultId)") }
        if let before { q.append("before=\(before)") }
        let v = try await json("GET", "/secrets/activity?" + q.joined(separator: "&")) as? [String: Any]
        return ((v?["items"] as? [Any])?.compactMap(SecretActivity.parse) ?? [], DocJSON.int(v?["next_before"]))
    }

    /// `POST /secrets/requests/{id}/decide {decision}` — `once | chat | always | deny`.
    func decideSecretRequest(_ id: String, decision: String) async throws {
        _ = try await request("POST", "/secrets/requests/\(id)/decide", body: ["decision": decision])
    }

    // MARK: Connected vaults (Phase 2)

    func connectVault(provider: String, token: String, name: String?, agents: [[String: Any]]) async throws -> ConnectedVault {
        var body: [String: Any] = ["provider": provider, "token": token, "agents": agents]
        if let name, !name.isEmpty { body["name"] = name }
        let v = try await json("POST", "/vaults", body: body) as? [String: Any]
        guard let vault = ConnectedVault.parse(v?["vault"]) else { throw RelayError(status: 0, message: "Bad vault reply") }
        return vault
    }

    func vault(_ id: String) async throws -> (ConnectedVault, [[String: Any]], [SecretActivity]) {
        let v = try await json("GET", "/vaults/\(id)") as? [String: Any]
        guard let vault = ConnectedVault.parse(v?["vault"]) else { throw RelayError(status: 404, message: "No such vault", type: "not_found") }
        return (vault, (v?["items"] as? [[String: Any]]) ?? [], (v?["activity"] as? [Any])?.compactMap(SecretActivity.parse) ?? [])
    }

    func replaceVaultToken(_ id: String, token: String) async throws {
        _ = try await request("PUT", "/vaults/\(id)/token", body: ["token": token])
    }

    func refreshVault(_ id: String) async throws {
        _ = try await request("POST", "/vaults/\(id)/refresh")
    }

    func setVaultAccess(_ id: String, projectId: String, enabled: Bool, note: String? = nil) async throws {
        var body: [String: Any] = ["enabled": enabled]
        if let note { body["note"] = note }
        _ = try await request("PUT", "/vaults/\(id)/access/\(projectId)", body: body)
    }

    func deleteVault(_ id: String) async throws {
        _ = try await request("DELETE", "/vaults/\(id)")
    }

    /// `PUT /vaults/{id}/items/env { ref, inject_env }` → the vault's items with the mapping.
    func setVaultItemEnv(_ id: String, ref: String, on: Bool) async throws -> [[String: Any]] {
        let v = try await json("PUT", "/vaults/\(id)/items/env", body: ["ref": ref, "inject_env": on]) as? [String: Any]
        return (v?["items"] as? [[String: Any]]) ?? []
    }
}

// Agent-to-agent messaging: "Who may message X" on the agent pane
// (`docs/agent-to-agent-messaging.md`). Only the agent's owner may change the list.
extension RelayAPI {
    /// `PUT /projects/{id}/message-access {grantee, enabled}` — `grantee` is `@handle`,
    /// `user:<id>` or `project:<id>`; returns the full list.
    func setMessageGrant(_ projectId: Id, grantee: String, enabled: Bool) async throws -> [MessageGrant] {
        let (data, _) = try await request("PUT", "/projects/\(projectId)/message-access",
                                          body: ["grantee": grantee, "enabled": enabled])
        return (try? JSONDecoder().decode(MessageGrantsDto.self, from: data))?.grants.map(MessageGrant.init(dto:)) ?? []
    }

    /// `DELETE /projects/{id}/message-access/{grantee}`.
    func removeMessageGrant(_ projectId: Id, grantee: String) async throws -> [MessageGrant] {
        let (data, _) = try await request("DELETE", "/projects/\(projectId)/message-access/\(grantee)")
        return (try? JSONDecoder().decode(MessageGrantsDto.self, from: data))?.grants.map(MessageGrant.init(dto:)) ?? []
    }
}

// The agent pane's Details and Computer tabs ([feedback iphone-agent-pane-tabs-2026-10-25]):
// the agent's routines with their switches, and the box as `GET /machine/debug` reports it.
extension RelayAPI {
    /// `GET /projects/{id}/routines` → every routine of the agent, in the relay's order.
    func routines(_ projectId: Id) async throws -> [Routine] {
        let v = try await json("GET", "/projects/\(projectId)/routines")
        return (v as? [Any] ?? []).compactMap(Routine.parse)
    }

    /// `PATCH /projects/{id}/routines/{rid} {enabled}` — the row's switch.
    func setRoutineEnabled(_ projectId: Id, routineId: Id, enabled: Bool) async throws {
        _ = try await request("PATCH", "/projects/\(projectId)/routines/\(routineId)", body: ["enabled": enabled])
    }

    /// `GET /machine/debug` → the Computer tab's report (the relay's view plus the box's own
    /// `/box/state`). Lenient on the wire; a relay without the route throws a 404.
    func machineDebug() async throws -> BoxReport {
        let v = try await json("GET", "/machine/debug")
        return BoxReport(debug: v, now: nowMs())
    }
}
