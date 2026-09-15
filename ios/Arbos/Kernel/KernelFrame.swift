import Foundation

/// One agent in the kernel's tree (`TreeNode` in `arbos-core/src/wire.rs`).
struct KernelAgent: Identifiable, Equatable {
    let id: String
    let name: String
    let parent: String?
    let paused: Bool
    let model: String
}

/// One transcript line (`EventKind` in `arbos-core/src/event.rs`). The
/// JSON is tagged by `kind`, snake_case, with the variant's fields
/// flattened beside it.
enum KernelEvent: Equatable {
    case user(text: String)
    case assistant(text: String)
    case thinking
    case tool(KernelToolRecord)
    case ask(question: String)
    case answer(text: String)
    /// A child agent spoke to this one. `from` is the child's id.
    case say(from: String, text: String)
    case notice(text: String, failed: Bool)
    case turnComplete
    case interrupted(detail: String)
    case other(kind: String)

    init(json object: [String: Any]) {
        let kind = object["kind"] as? String ?? ""
        let text = object["text"] as? String ?? ""
        switch kind {
        case "user": self = .user(text: text)
        case "assistant": self = .assistant(text: text)
        case "thinking": self = .thinking
        case "tool": self = .tool(KernelToolRecord(json: object))
        case "ask": self = .ask(question: object["question"] as? String ?? "")
        case "answer": self = .answer(text: text)
        case "say": self = .say(from: object["from"] as? String ?? "", text: text)
        case "notice": self = .notice(text: text, failed: object["failed"] as? Bool ?? false)
        case "turn_complete": self = .turnComplete
        case "interrupted": self = .interrupted(detail: object["detail"] as? String ?? "")
        default: self = .other(kind: kind)
        }
    }
}

/// `ToolRec`: one tool call, already finished by the time it is on disk.
struct KernelToolRecord: Equatable {
    let name: String
    let paths: [String]
    let args: [String: Any]?
    let error: String?
    let child: String?
    let seconds: Int?

    init(json object: [String: Any]) {
        name = object["name"] as? String ?? "tool"
        paths = object["paths"] as? [String] ?? []
        args = object["args"] as? [String: Any]
        error = object["error"] as? String
        child = object["child"] as? String
        if let started = object["started"] as? Int64, let ended = object["ended"] as? Int64 {
            seconds = Int(max(0, ended - started) / 1000)
        } else {
            seconds = nil
        }
    }

    init(name: String, paths: [String] = [], args: [String: Any]? = nil, error: String? = nil,
         child: String? = nil, seconds: Int? = nil) {
        self.name = name
        self.paths = paths
        self.args = args
        self.error = error
        self.child = child
        self.seconds = seconds
    }

    static func == (lhs: KernelToolRecord, rhs: KernelToolRecord) -> Bool {
        lhs.name == rhs.name && lhs.paths == rhs.paths && lhs.error == rhs.error
            && lhs.child == rhs.child && lhs.seconds == rhs.seconds
    }

    /// One line for the chat: what ran, on what.
    var label: String {
        let subject: String
        if ["bash", "shell", "run"].contains(name), let command = args?["command"] as? String ?? args?["cmd"] as? String {
            // A shell call is what it ran, not the log file it wrote.
            subject = command
        } else if let first = paths.first, !first.isEmpty {
            subject = (first as NSString).lastPathComponent
        } else if let path = args?["path"] as? String ?? args?["file"] as? String ?? args?["dir"] as? String {
            subject = (path as NSString).lastPathComponent
        } else if let command = args?["command"] as? String ?? args?["cmd"] as? String {
            subject = command
        } else if let brief = args?["brief"] as? String {
            subject = brief
        } else if let query = args?["pattern"] as? String ?? args?["query"] as? String {
            subject = query
        } else {
            subject = ""
        }
        let trimmed = subject.replacingOccurrences(of: "\n", with: " ")
        let short = trimmed.count > 48 ? String(trimmed.prefix(47)) + "…" : trimmed
        return short.isEmpty ? name : "\(name) · \(short)"
    }
}

/// The kernel → client frames the phone cares about. The wire type is
/// `Frame` in `crates/arbos-core/src/wire.rs`: one JSON object per line,
/// tagged by `type` in snake_case.
enum KernelFrame {
    /// First frame from a 0.2 kernel over WebSocket.
    case hello(focus: String, kernel: String)
    /// The agent tree and which agent the desktop last focused.
    case snapshot(focus: String, agents: [KernelAgent])
    case tree([KernelAgent])
    /// One line of the transcript as it was before we attached.
    case replayed(agent: String, event: KernelEvent)
    /// Replay is over; what follows is live.
    case historyEnd(agent: String)
    case event(agent: String, event: KernelEvent)
    /// One streamed token of the reply being written.
    case assistantDelta(agent: String, text: String)
    /// `running` or `idle`.
    case turn(agent: String, state: String)
    case ask(agent: String, question: String, options: [String])
    /// What `agent` is doing now, in a few words; empty means idle.
    case status(agent: String, step: String, source: String)
    /// The model call for `agent` is alive but silent for `secs` seconds.
    case working(agent: String, secs: Int)
    /// A file under `.arbos/`, answering a `read`.
    case file(path: String, text: String, error: String?)
    case thinkingDelta(agent: String, text: String)
    case other(type: String)

    init?(json object: [String: Any]) {
        guard let type = object["type"] as? String else { return nil }
        switch type {
        case "status":
            self = .status(
                agent: object["agent"] as? String ?? "",
                step: object["step"] as? String ?? "",
                source: object["source"] as? String ?? ""
            )
        case "working":
            self = .working(agent: object["agent"] as? String ?? "", secs: object["secs"] as? Int ?? 0)
        case "file":
            self = .file(
                path: object["path"] as? String ?? "",
                text: object["text"] as? String ?? "",
                error: object["error"] as? String
            )
        case "thinking_delta":
            self = .thinkingDelta(
                agent: object["agent"] as? String ?? "",
                text: object["text"] as? String ?? ""
            )
        case "hello":
            self = .hello(
                focus: object["focus"] as? String ?? "root",
                kernel: object["kernel"] as? String ?? ""
            )
        case "replayed":
            self = .replayed(
                agent: object["agent"] as? String ?? "",
                event: KernelEvent(json: object["event"] as? [String: Any] ?? [:])
            )
        case "history_end":
            self = .historyEnd(agent: object["agent"] as? String ?? "")
        case "assistant_delta":
            self = .assistantDelta(
                agent: object["agent"] as? String ?? "",
                text: object["text"] as? String ?? ""
            )
        case "snapshot":
            self = .snapshot(
                focus: object["focus"] as? String ?? "",
                agents: Self.agents(object["tree"])
            )
        case "tree":
            self = .tree(Self.agents(object["tree"]))
        case "event":
            self = .event(
                agent: object["agent"] as? String ?? "",
                event: KernelEvent(json: object["event"] as? [String: Any] ?? [:])
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
