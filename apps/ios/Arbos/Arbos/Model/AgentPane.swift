// The agent pane's live data and pure copy ([feedback iphone-agent-pane-tabs-2026-10-25],
// the Mac's `profiles/agent.rs` + `model::BoxReport`): routines as the relay lists them,
// the box as `GET /machine/debug` reports it, and the order / wording of the Details and
// Computer tabs. Pure, so the twin of the Mac's `agent::tests` can pin it without a view.

import Foundation

// MARK: - Routines (relay `routines` rows, F3)

/// How a routine fires (`routines.trigger`).
enum RoutineTrigger: Equatable {
    case cron(expr: String, tz: String)
    case once(at: Int64)
    case webhook
    case event(String)
    case other(String)

    /// Short label for the row ("Weekdays 09:00", "Webhook", "On box awake") — the Mac's
    /// `RoutineTrigger::label`.
    var label: String {
        switch self {
        case .cron(let expr, let tz):
            let base: String
            switch expr.trimmingCharacters(in: .whitespaces) {
            case "* * * * *": base = "Every minute"
            case "0 * * * *": base = "Hourly"
            case "0 9 * * *": base = "Daily 09:00"
            case "0 9 * * 1-5": base = "Weekdays 09:00"
            case let other: base = "cron \(other)"
            }
            return tz == "UTC" ? base : "\(base) \(tz)"
        case .once: return "Once"
        case .webhook: return "Webhook"
        case .event(let event):
            switch event {
            case "box_awake": return "On box awake"
            case "cross_project_message": return "On project message"
            case "connector_event": return "On connector event"
            default: return "On \(event)"
            }
        case .other(let kind): return kind.isEmpty ? "Routine" : kind.prefix(1).uppercased() + kind.dropFirst()
        }
    }

    static func parse(_ v: Any?) -> RoutineTrigger {
        guard let d = v as? [String: Any] else { return .other("") }
        let kind = d["kind"] as? String ?? ""
        switch kind {
        case "cron": return .cron(expr: d["expr"] as? String ?? "", tz: d["tz"] as? String ?? "UTC")
        case "once": return .once(at: (d["at"] as? NSNumber)?.int64Value ?? (d["at_ms"] as? NSNumber)?.int64Value ?? 0)
        case "webhook": return .webhook
        case "event": return .event(d["event"] as? String ?? "")
        default: return .other(kind)
        }
    }
}

/// A scheduled or event-triggered prompt (`GET /projects/{id}/routines`).
struct Routine: Identifiable, Equatable {
    let id: Id
    let projectId: Id
    var name: String
    var trigger: RoutineTrigger
    var enabled: Bool
    var lastRunAt: Int64?
    var nextRunAt: Int64?

    /// Lenient: every field but the id is optional on the wire.
    static func parse(_ v: Any) -> Routine? {
        guard let d = v as? [String: Any], let id = d["id"] as? String, !id.isEmpty else { return nil }
        return Routine(
            id: id,
            projectId: d["project_id"] as? String ?? "",
            name: d["name"] as? String ?? "Routine",
            trigger: RoutineTrigger.parse(d["trigger"]),
            enabled: d["enabled"] as? Bool ?? true,
            lastRunAt: (d["last_run_at"] as? NSNumber)?.int64Value,
            nextRunAt: (d["next_run_at"] as? NSNumber)?.int64Value
        )
    }
}

// MARK: - The box report (`GET /machine/debug`)

/// One agent's workspace on the box.
struct BoxWorkspace: Equatable {
    var projectId: Id
    var name: String
    var healthy: Bool
    /// A turn is running right now (`turn.running`).
    var running: Bool
    /// How long the current turn has run (`turn.current.elapsed_ms`).
    var elapsedMs: Int64?
    var toolCalls: Int64?
}

/// The box as `GET /machine/debug` reports it — the relay's view plus the box's own
/// `GET /box/state` (image, uptime, load, memory, disk, every workspace with its turn
/// lock). The agent pane's Computer tab reads this; nothing else.
struct BoxReport: Equatable {
    var fetchedAt: Int64
    /// `relay.state`: awake / waking / asleep / stopped / error.
    var state: String
    var awakeForS: Int64?
    /// `relay.agentd.image` → the image the box booted (e.g. `arbos-agent-0.1.114`).
    var image: String?
    /// `box.reachable`: the box answered `/box/state`; the rest is nil when it did not.
    var reachable: Bool
    var unreachableReason: String?
    var uptimeS: Int64?
    /// 1-minute load average.
    var load1m: Double?
    var memTotalMb: Int64?
    var memAvailableMb: Int64?
    var diskTotalMb: Int64?
    var diskAvailableMb: Int64?
    var workspaces: [BoxWorkspace]

    /// `GET /machine/debug` → the report. Lenient: every field is optional on the wire
    /// (older relays, an unreachable box); nothing here fails.
    init(debug v: Any, now: Int64) {
        let d = v as? [String: Any] ?? [:]
        let relay = d["relay"] as? [String: Any] ?? [:]
        let onBox = d["box"] as? [String: Any] ?? [:]
        let st = onBox["state"] as? [String: Any] ?? [:]
        fetchedAt = now
        state = relay["state"] as? String ?? "unknown"
        awakeForS = (relay["awake_for_s"] as? NSNumber)?.int64Value
        image = ((relay["agentd"] as? [String: Any])?["image"] as? String) ?? (st["image_version"] as? String)
        reachable = onBox["reachable"] as? Bool ?? false
        unreachableReason = onBox["error"] as? String
        workspaces = []
        guard reachable else { return }
        uptimeS = (st["uptime_s"] as? NSNumber)?.int64Value
        load1m = ((st["load"] as? [Any])?.first as? NSNumber)?.doubleValue
        let mem = st["memory_mb"] as? [String: Any] ?? [:]
        memTotalMb = (mem["total"] as? NSNumber)?.int64Value
        memAvailableMb = (mem["available"] as? NSNumber)?.int64Value
        let disk = st["disk"] as? [String: Any] ?? [:]
        diskTotalMb = (disk["total_mb"] as? NSNumber)?.int64Value
        diskAvailableMb = (disk["available_mb"] as? NSNumber)?.int64Value
        workspaces = (st["workspaces"] as? [[String: Any]] ?? []).map { w in
            let turn = w["turn"] as? [String: Any] ?? [:]
            let current = turn["current"] as? [String: Any] ?? [:]
            return BoxWorkspace(
                projectId: w["project_id"] as? String ?? "",
                name: w["name"] as? String ?? "",
                healthy: w["healthy"] as? Bool ?? false,
                running: turn["running"] as? Bool ?? false,
                elapsedMs: (current["elapsed_ms"] as? NSNumber)?.int64Value,
                toolCalls: (current["tool_calls"] as? NSNumber)?.int64Value
            )
        }
    }

    /// The workspace of one agent, if the box lists it.
    func workspace(_ projectId: Id) -> BoxWorkspace? {
        workspaces.first { $0.projectId == projectId }
    }

    /// Workspaces with a turn running right now — the box's running jobs.
    var running: [BoxWorkspace] { workspaces.filter(\.running) }
}

// MARK: - The tabs' order and copy (the Mac's `agent.rs` pure functions)

enum AgentPane {
    /// Grok's "Add a label": the owner's subtitle while the agent has no description.
    static let labelPlaceholder = "Add a label"

    /// The Details tab, top to bottom — live content first, the options after it, the
    /// edit fields last and folded.
    enum DetailsBlock: Equatable {
        /// `permissions`: Shell approvals — the standing grants from destructive-shell cards (#1544).
        case coding, routines, settings, access, permissions, profile
    }

    static func detailsOrder(mine: Bool) -> [DetailsBlock] {
        mine ? [.coding, .routines, .settings, .access, .permissions, .profile] : [.coding, .settings, .profile]
    }

    /// The Computer tab's blocks, in order.
    enum ComputerBlock: Equatable {
        case machine, resources, running
    }

    static func computerOrder(mine: Bool, report: BoxReport?) -> [ComputerBlock] {
        var out: [ComputerBlock] = [.machine]
        if mine {
            if report?.reachable == true { out.append(.resources) }
            out.append(.running)
        }
        return out
    }

    /// The hero's subtitle: the description as the label, the placeholder for the owner,
    /// who runs it on someone else's agent.
    static func subtitle(description: String, mine: Bool, ownerHandle: String?) -> String {
        let label = description.trimmingCharacters(in: .whitespacesAndNewlines)
        if !label.isEmpty { return label }
        if mine { return labelPlaceholder }
        if let o = ownerHandle?.trimmingCharacters(in: CharacterSet(charactersIn: "@ ")), !o.isEmpty { return "Run by @\(o)" }
        return "Agent"
    }

    /// The Coding row's subtitle: what the agent is doing right now.
    static func codingStatus(streaming: Bool, workspace: BoxWorkspace?, machine: BoxState, alwaysOn: Bool) -> String {
        if streaming || workspace?.running == true {
            var s = "Working"
            if let w = workspace, w.running {
                if let ms = w.elapsedMs { s += " · \(durationS(ms / 1000))" }
                if let n = w.toolCalls, n > 0 { s += " · \(n) tool call\(n == 1 ? "" : "s")" }
            }
            return s
        }
        if alwaysOn || machine == .awake { return "Idle" }
        switch machine {
        case .waking: return "Machine waking"
        case .hibernatedLight, .hibernatedDeep: return "Machine asleep"
        case .stoppedByUser: return "Machine stopped"
        case .error: return "Machine error"
        case .awake: return "Idle"
        }
    }

    /// A routine row's subtitle: its trigger, then the next or last run.
    static func routineStatus(_ r: Routine, now: Int64) -> String {
        guard r.enabled else { return "Paused" }
        if let next = r.nextRunAt { return "\(r.trigger.label) · next \(relTime(next, now: now))" }
        if let last = r.lastRunAt { return "\(r.trigger.label) · last \(relTime(last, now: now))" }
        return r.trigger.label
    }

    /// "42 s · 7 tool calls" for a running workspace; empty when nothing is known.
    static func runningDetail(_ w: BoxWorkspace) -> String {
        var parts: [String] = []
        if let ms = w.elapsedMs { parts.append(durationS(ms / 1000)) }
        if let n = w.toolCalls, n > 0 { parts.append("\(n) tool call\(n == 1 ? "" : "s")") }
        return parts.joined(separator: " · ")
    }

    /// "6.9 GB free of 10 GB".
    static func freeOf(available: Int64?, total: Int64?) -> String? {
        switch (available, total) {
        case (let a?, let t?): return "\(sizeMb(a)) free of \(sizeMb(t))"
        case (let a?, nil): return "\(sizeMb(a)) free"
        default: return nil
        }
    }

    /// "12.4 GB" / "840 MB" from megabytes.
    static func sizeMb(_ mb: Int64) -> String {
        guard mb >= 1024 else { return "\(mb) MB" }
        let gb = Double(mb) / 1024
        return gb >= 10 ? String(format: "%.0f GB", gb) : String(format: "%.1f GB", gb)
    }

    /// "3 d 4 h" / "2 h 10 m" / "45 s" from seconds.
    static func durationS(_ s: Int64) -> String {
        let s = max(0, s)
        let (d, h, m) = (s / 86_400, (s % 86_400) / 3600, (s % 3600) / 60)
        if d > 0 { return "\(d) d \(h) h" }
        if h > 0 { return "\(h) h \(m) m" }
        if m > 0 { return "\(m) m" }
        return "\(s) s"
    }

    /// "in 2 h" / "3 min ago" / "in a moment" (the Mac's `routines::rel_time`).
    static func relTime(_ atMs: Int64, now: Int64) -> String {
        let delta = atMs - now
        let future = delta > 0
        let secs = abs(delta) / 1000
        let body: String
        if secs < 45 { return future ? "in a moment" : "just now" }
        else if secs < 3_600 { body = "\((secs + 30) / 60) min" }
        else if secs < 86_400 { body = "\((secs + 1_800) / 3_600) h" }
        else { body = "\((secs + 43_200) / 86_400) d" }
        return future ? "in \(body)" : "\(body) ago"
    }
}
