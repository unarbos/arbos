// Settings → Machine: live machine details (Jacob 2026-10-26, [feedback
// machine-page-ios-2026-10-26]; mock approved 14:18 CR as drawn). `MachineMetrics` is the pure twin of the Mac's
// `ui/profiles/machine.rs`: the rows in order, the gauges and their levels, the warning card's
// words, the per-agent breakdown and the poll step. `MachinePage` (SettingsView.swift) draws them.
//
// Numbers come from `GET /machine` (`Machine`) and the box's own `GET /box/state` through
// `GET /machine/debug` (`BoxReport`), polled every `pollSeconds` while the page is on screen.
// A metric the image does not report yet (agentd < 0.6.277) is simply not drawn.

import Foundation

enum MachineMetrics {
    /// Used share at which a gauge turns amber and the warning card appears — agentd's disk
    /// gate and the OOM "near cap" rule (#1777) both sit at 85 %.
    static let warnPct: Double = 85
    /// Used share at which a gauge turns red.
    static let critPct: Double = 90
    /// `GET /machine/debug` cadence while the page is open.
    static let pollSeconds: Double = 5
    /// File slots are shown only on a filesystem that counts them in a quota-sized way (a
    /// 2 GiB work image has 131 072); the Daytona overlay reports the host's 750 M.
    static let inodesShownBelow: Int64 = 10_000_000

    enum Level: Equatable { case ok, warn, crit }

    static func level(_ pct: Double) -> Level {
        if pct >= critPct { return .crit }
        if pct >= warnPct { return .warn }
        return .ok
    }

    /// Used share, 0–100, when a total is known and non-zero.
    static func pctUsed(used: Int64, total: Int64) -> Double? {
        guard total > 0 else { return nil }
        return min(100, max(0, Double(used) / Double(total) * 100))
    }

    /// "12.4 GB" / "840 MB" from megabytes (the Computer tab's helper).
    static func sizeMb(_ mb: Int64) -> String { AgentPane.sizeMb(mb) }

    /// "93 k" / "1.2 k" / "640".
    static func countK(_ n: Int64) -> String {
        if n >= 10_000 { return "\(n / 1000) k" }
        if n >= 1000 { return String(format: "%.1f k", Double(n) / 1000) }
        return "\(n)"
    }

    /// "1.2 MB/s" / "80 kB/s" / "0 B/s".
    static func rate(_ bps: Double) -> String {
        let b = max(0, bps)
        if b >= 1e9 { return String(format: "%.1f GB/s", b / 1e9) }
        if b >= 1e6 { return String(format: "%.1f MB/s", b / 1e6) }
        if b >= 1e3 { return String(format: "%.0f kB/s", b / 1e3) }
        return "\(Int(b.rounded())) B/s"
    }

    /// "2.1 GB" / "640 MB" / "12 kB" from bytes.
    static func bytes(_ n: Int64) -> String {
        let b = Double(n)
        if b >= 1e9 { return String(format: "%.1f GB", b / 1e9) }
        if b >= 1e6 { return String(format: "%.0f MB", b / 1e6) }
        if b >= 1e3 { return String(format: "%.0f kB", b / 1e3) }
        return "\(n) B"
    }

    /// One gauge row's numbers.
    struct Gauge: Equatable {
        var pct: Double
        var level: Level
        /// "3.1 GB used of 10 GB" / "23 %".
        var value: String
        /// The muted line under the bar, when there is one.
        var sub: String?
    }

    private static func usedOf(_ used: Int64, _ total: Int64) -> String {
        "\(sizeMb(used)) used of \(sizeMb(total))"
    }

    static func diskGauge(_ r: BoxReport) -> Gauge? {
        guard let total = r.diskTotalMb, let avail = r.diskAvailableMb else { return nil }
        let used = max(0, total - avail)
        guard let pct = pctUsed(used: used, total: total) else { return nil }
        var sub = "\(sizeMb(avail)) free"
        if let t = r.inodesTotal, let a = r.inodesAvail, t > 0, t < inodesShownBelow {
            sub += " · \(countK(a)) file slots free"
        }
        return Gauge(pct: pct, level: level(pct), value: usedOf(used, total), sub: sub)
    }

    static func memGauge(_ r: BoxReport) -> Gauge? {
        guard let total = r.memTotalMb, let avail = r.memAvailableMb else { return nil }
        let used = max(0, total - avail)
        guard let pct = pctUsed(used: used, total: total) else { return nil }
        let lvl = level(pct)
        return Gauge(pct: pct, level: lvl, value: usedOf(used, total),
                     sub: lvl == .ok ? nil : "Over \(sizeMb(total)) the machine restarts")
    }

    static func cpuGauge(_ r: BoxReport) -> Gauge? {
        guard let p = r.cpuPct else { return nil }
        let pct = min(100, max(0, p))
        var sub: String?
        if let c = r.cpuCores {
            sub = abs(c - c.rounded()) < 0.01 ? "\(Int(c.rounded())) cores" : String(format: "%.1f cores", c)
        }
        return Gauge(pct: pct, level: level(pct), value: "\(Int(pct.rounded())) %", sub: sub)
    }

    /// The Network row: ("↓ 1.2 MB/s  ↑ 80 kB/s", "2.1 GB today").
    static func netValue(_ r: BoxReport) -> (String, String?)? {
        guard let rx = r.netRxBps, let tx = r.netTxBps else { return nil }
        var today: String?
        if let a = r.netRxToday, let b = r.netTxToday { today = "\(bytes(a + b)) today" }
        return ("↓ \(rate(rx))  ↑ \(rate(tx))", today)
    }

    /// The warning card's words when disk or memory is at `warnPct` or more.
    struct Warning: Equatable {
        var title: String
        var body: String
        var diskPct: Double?
        var memPct: Double?
    }

    static func warning(_ r: BoxReport) -> Warning? {
        let disk = diskGauge(r).flatMap { $0.level == .ok ? nil : $0 }
        let mem = memGauge(r).flatMap { $0.level == .ok ? nil : $0 }
        if disk == nil, mem == nil { return nil }
        let title: String
        let consequence: String
        switch (disk != nil, mem != nil) {
        case (true, true):
            title = "Running low on disk and memory"
            consequence = "saves fail and the machine restarts — agents lose their turn"
        case (true, false):
            title = "Running low on disk"
            consequence = "saves fail and agents cannot finish their work"
        default:
            title = "Running low on memory"
            consequence = "the machine restarts and agents lose their turn"
        }
        var parts: [String] = []
        if let d = disk { parts.append("Disk \(Int(d.pct.rounded())) %") }
        if let m = mem { parts.append("memory \(Int(m.pct.rounded())) %") }
        return Warning(title: title, body: "\(parts.joined(separator: " · ")). Past the limit, \(consequence).",
                       diskPct: disk?.pct, memPct: mem?.pct)
    }

    /// The workspace to ask for space: the one using the most disk, when any reports it.
    static func biggestDiskUser(_ r: BoxReport) -> BoxWorkspace? {
        r.workspaces.filter { $0.diskMb != nil }.max { ($0.diskMb ?? 0) < ($1.diskMb ?? 0) }
    }

    /// The composer draft when the user asks that agent to free space (agentd's disk gate
    /// answers "free space" itself, #1641).
    static let freeSpaceAsk =
        "Please free up disk space in your workspace: remove caches, build outputs and downloads you no longer need, then tell me how much is free."

    /// "3 of 6 agents".
    static func runningNow(_ r: BoxReport) -> String {
        let total = r.workspaces.count
        return "\(r.running.count) of \(total) agent\(total == 1 ? "" : "s")"
    }

    /// "2 running" / "None".
    static func jobsValue(_ jobs: Int64) -> String {
        jobs == 0 ? "None" : "\(jobs) running"
    }

    /// "Working · 42 s · 1 job" / "Idle" (the tool count stays on the Computer tab).
    static func agentStatus(_ w: BoxWorkspace) -> String {
        if !w.running {
            if let j = w.jobsRunning, j > 0 { return "Idle · \(j) job\(j == 1 ? "" : "s")" }
            return "Idle"
        }
        var parts = ["Working"]
        if let ms = w.elapsedMs { parts.append(AgentPane.durationS(ms / 1000)) }
        if let j = w.jobsRunning, j > 0 { parts.append("\(j) job\(j == 1 ? "" : "s")") }
        return parts.joined(separator: " · ")
    }

    /// "CPU 61 %".
    static func agentCpu(_ w: BoxWorkspace) -> String? {
        w.cpuPct.map { "CPU \(Int($0.rounded())) %" }
    }

    /// "1.9 GB · 2.3 GB disk".
    static func agentMemDisk(_ w: BoxWorkspace) -> String? {
        var parts: [String] = []
        if let m = w.memMb { parts.append(sizeMb(m)) }
        if let d = w.diskMb { parts.append("\(sizeMb(d)) disk") }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    /// Everything the page may draw, in order, by card.
    enum Row: Equatable {
        // warning card
        case warning, askToFreeSpace, requestBiggerMachineInWarning
        // machine card
        case whereItRuns, inLine, state, spec, machineTime, workspaces, sandbox
        // resources card
        case disk, memory, cpu, network, notAnswering
        // agents card
        case runningNow, jobs, breakdown, agent(Id)
        // action card
        case requestBiggerMachine, restart, start, stop
    }

    struct Plan: Equatable {
        var warning: [Row] = []
        var machine: [Row] = []
        var resources: [Row] = []
        var agents: [Row] = []
        var actions: [Row] = []
    }

    /// The order the page draws, from what the relay and the box say right now.
    static func plan(machine m: Machine, waitlisted: Bool, report: BoxReport?, breakdownOpen: Bool) -> Plan {
        var p = Plan()
        let live = report.flatMap { $0.reachable ? $0 : nil }
        if let r = live, let w = warning(r) {
            p.warning.append(.warning)
            if w.diskPct != nil, biggestDiskUser(r) != nil { p.warning.append(.askToFreeSpace) }
            p.warning.append(.requestBiggerMachineInWarning)
        }
        p.machine.append(.whereItRuns)
        if waitlisted { p.machine.append(.inLine) }
        p.machine.append(contentsOf: [.state, .spec])
        if m.reported {
            p.machine.append(.machineTime)
            // The workspace count moves to the Agents card once the box reports; the sandbox
            // id stays a debugging row at the end.
            if live == nil { p.machine.append(.workspaces) }
            if let id = m.sandboxId, !id.isEmpty { p.machine.append(.sandbox) }
        }
        if let r = live {
            if diskGauge(r) != nil { p.resources.append(.disk) }
            if memGauge(r) != nil { p.resources.append(.memory) }
            if cpuGauge(r) != nil { p.resources.append(.cpu) }
            if netValue(r) != nil { p.resources.append(.network) }
            p.agents.append(.runningNow)
            if r.jobsRunning != nil { p.agents.append(.jobs) }
            let usage = r.usageRows
            if !usage.isEmpty {
                p.agents.append(.breakdown)
                if breakdownOpen { p.agents.append(contentsOf: usage.map { .agent($0.projectId) }) }
            }
        } else if report != nil, m.alwaysOn, !m.held {
            p.resources.append(.notAnswering)
        }
        if m.alwaysOn {
            p.actions.append(.requestBiggerMachine)
            p.actions.append(m.held ? .start : .restart)
        } else {
            switch m.state {
            case .awake, .waking: p.actions.append(.stop)
            case .hibernatedLight, .hibernatedDeep, .stoppedByUser, .error: p.actions.append(.start)
            }
        }
        return p
    }

    /// The 5-second poll, decided from what is on screen: the page starts it, leaving the
    /// page stops it, and a running poll ticks while the page stays.
    enum PollStep: Equatable { case start, tick, stop, idle }

    static func pollStep(polling: Bool, onPage: Bool) -> PollStep {
        switch (polling, onPage) {
        case (false, true): return .start
        case (true, true): return .tick
        case (true, false): return .stop
        case (false, false): return .idle
        }
    }
}
