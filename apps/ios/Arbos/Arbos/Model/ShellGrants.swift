// Standing shell approvals (#1544): the owner chose "Always allow <agent> in /tmp" on a
// destructive-shell card. The relay holds them (`/projects/{id}/shell-grants`) and the box
// applies them; the agent pane's Permissions card that listed them left on 2026-10-25
// ([feedback agent-details-remove-sections-ios-2026-10-25]) — the calls stay.

import Foundation

struct ShellGrant: Equatable, Identifiable {
    var id: String
    var ruleClass: String
    var pathClass: String
    var chatId: String?
    var grantedBy: String
    var createdAt: Int64
    /// "File deletes in /tmp" (relay-rendered).
    var label: String
    /// "scratch only: /tmp and /work/tmp" (relay-rendered).
    var detail: String

    static func parse(_ v: Any?) -> ShellGrant? {
        guard let m = v as? [String: Any], let id = m["id"] as? String else { return nil }
        return ShellGrant(
            id: id,
            ruleClass: m["rule_class"] as? String ?? "",
            pathClass: m["path_class"] as? String ?? "",
            chatId: m["chat_id"] as? String,
            grantedBy: m["granted_by"] as? String ?? "",
            createdAt: (m["created_at"] as? NSNumber)?.int64Value ?? 0,
            label: m["label"] as? String ?? "Shell approval",
            detail: m["detail"] as? String ?? ""
        )
    }

    /// The value of the Settings "Shell approvals" row: "Destructive · /tmp" or "Asks every time".
    static func summary(_ grants: [ShellGrant]) -> String {
        if grants.isEmpty { return "Asks every time" }
        let classes = grants.map { g -> String in
            switch g.pathClass {
            case "scratch": return "/tmp"
            case "workspace": return "workspace"
            default: return g.pathClass
            }
        }
        return "Destructive · " + classes.joined(separator: ", ")
    }
}

extension RelayAPI {
    /// `GET /projects/{id}/shell-grants` → the active standing grants.
    func shellGrants(_ projectId: String) async throws -> [ShellGrant] {
        let v = try await json("GET", "/projects/\(projectId)/shell-grants") as? [String: Any]
        return (v?["items"] as? [Any])?.compactMap(ShellGrant.parse) ?? []
    }

    /// `DELETE /projects/{id}/shell-grants/{grant}` — revoke; the agent asks again next time.
    func revokeShellGrant(_ projectId: String, grantId: String) async throws {
        _ = try await request("DELETE", "/projects/\(projectId)/shell-grants/\(grantId)")
    }
}
