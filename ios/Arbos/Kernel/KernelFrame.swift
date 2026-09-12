import Foundation

/// One agent in the kernel's tree (`TreeNode` in `arbos-core/src/wire.rs`).
struct KernelAgent: Identifiable {
    let id: String
    let name: String
    let parent: String?
    let paused: Bool
    let model: String
}

/// The subset of kernel → client frames the phone cares about. The wire
/// type is `Frame` in `crates/arbos-core/src/wire.rs`: one JSON object per
/// line, tagged by `type` in snake_case.
enum KernelFrame {
    /// First thing the kernel sends on attach: the agent tree and which
    /// agent the desktop last focused.
    case snapshot(focus: String, agents: [KernelAgent])
    case tree([KernelAgent])
    /// A transcript line for `agent`. `kind` is the `EventKind` tag
    /// (`user`, `assistant`, `say`, `notice`, `thinking`, `turn_complete`, …).
    case event(agent: String, kind: String, text: String?)
    /// `running` or `idle`.
    case turn(agent: String, state: String)
    case ask(agent: String, question: String, options: [String])
    case other(type: String)

    init?(json object: [String: Any]) {
        guard let type = object["type"] as? String else { return nil }
        switch type {
        case "snapshot":
            self = .snapshot(
                focus: object["focus"] as? String ?? "",
                agents: Self.agents(object["tree"])
            )
        case "tree":
            self = .tree(Self.agents(object["tree"]))
        case "event":
            let event = object["event"] as? [String: Any] ?? [:]
            self = .event(
                agent: object["agent"] as? String ?? "",
                kind: event["type"] as? String ?? "",
                text: event["text"] as? String
            )
        case "turn":
            self = .turn(
                agent: object["agent"] as? String ?? "",
                state: object["state"] as? String ?? ""
            )
        case "ask":
            self = .ask(
                agent: object["agent"] as? String ?? "",
                question: object["question"] as? String ?? "",
                options: object["options"] as? [String] ?? []
            )
        default:
            self = .other(type: type)
        }
    }

    private static func agents(_ raw: Any?) -> [KernelAgent] {
        guard let rows = raw as? [[String: Any]] else { return [] }
        return rows.compactMap { row in
            guard let id = row["id"] as? String else { return nil }
            return KernelAgent(
                id: id,
                name: row["name"] as? String ?? id,
                parent: row["parent"] as? String,
                paused: row["paused"] as? Bool ?? false,
                model: row["model"] as? String ?? ""
            )
        }
    }
}
