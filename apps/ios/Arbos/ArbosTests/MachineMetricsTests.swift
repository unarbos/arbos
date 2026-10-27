import XCTest
@testable import Arbos

/// Settings › Machine ([feedback machine-page-ios-2026-10-26]): the rows in order, the gauges
/// and thresholds, the warning card, the per-agent breakdown and the poll step. The Mac twin is
/// `crates/arbos-mac/src/ui/profiles/machine.rs` `tests`.
final class MachineMetricsTests: XCTestCase {
    /// The `/machine/debug` body an agentd ≥ 0.6.277 answers (the sampler's `resources`,
    /// `load_scope`, per-workspace `usage` + `jobs_running`).
    private func debug(diskAvail: Int, memUsed: Int, cpu: Double? = 23.4, usage: Bool = true) -> [String: Any] {
        var arbos: [String: Any] = ["project_id": "p1", "name": "Arbos", "healthy": true,
                                    "turn": ["running": true, "current": ["elapsed_ms": 42_000, "tool_calls": 7]]]
        var comms: [String: Any] = ["project_id": "p2", "name": "Comms", "healthy": true, "turn": ["running": false]]
        if usage {
            arbos["jobs_running"] = 1
            arbos["usage"] = ["cpu_pct": 61.0, "mem_mb": 1900, "pids": 40, "disk_mb": 2300]
            comms["jobs_running"] = 0
            comms["usage"] = ["cpu_pct": 0.0, "mem_mb": 600, "pids": 12, "disk_mb": 400]
        }
        var resources: [String: Any] = [
            "memory": ["total_mb": 8192, "used_mb": memUsed, "source": "cgroup"],
            "disk": ["total_mb": 10240, "used_mb": 10240 - diskAvail, "available_mb": diskAvail,
                     "inodes_total": 131_072, "inodes_avail": 93_000],
            "net": ["rx_bps": 1_200_000.0, "tx_bps": 80_000.0, "rx_bytes_today": 2_000_000_000, "tx_bytes_today": 100_000_000],
        ]
        if let cpu { resources["cpu"] = ["cores": 4.0, "pct": cpu, "source": "cgroup"] }
        return [
            "relay": ["state": "awake", "awake_for_s": 100, "agentd": ["image": "arbos-agent-0.1.152"]],
            "box": ["reachable": true, "state": [
                "uptime_s": 100, "load": [38.5, 36.9, 32.1], "load_scope": "host",
                "memory_mb": ["total": 8192, "available": 8192 - memUsed, "source": "cgroup"],
                "disk": ["total_mb": 10240, "available_mb": diskAvail],
                "resources": resources,
                "workspaces": [arbos, comms],
            ]],
        ]
    }

    private func hosted() -> Machine {
        Machine(state: .awake, since: 1, spec: "4 vCPU · 8 GiB · 10 GiB", provider: "daytona",
                sandboxId: "sb-1", alwaysOn: true, machineTodayUsd: 12.56, reported: true)
    }

    func testParserReadsTheSamplersResourcesAndPerWorkspaceUsage() {
        let r = BoxReport(debug: debug(diskAvail: 6900, memUsed: 2892), now: 5)
        XCTAssertTrue(r.reachable)
        XCTAssertTrue(r.loadIsHost, "inside Daytona the load average is the host's")
        XCTAssertEqual(r.cpuCores, 4.0)
        XCTAssertEqual(r.cpuPct, 23.4)
        XCTAssertEqual(r.memTotalMb, 8192)
        XCTAssertEqual(r.memAvailableMb, 8192 - 2892, "memory comes from the cgroup cap, not /proc")
        XCTAssertEqual(r.inodesAvail, 93_000)
        XCTAssertEqual(r.netRxBps, 1_200_000)
        XCTAssertEqual(r.jobsRunning, 1)
        let rows = r.usageRows
        XCTAssertEqual(rows.map(\.name), ["Arbos", "Comms"], "busy first, then biggest memory")
        XCTAssertEqual(rows[0].diskMb, 2300)
        // An older image: nothing extra, nothing invented.
        let old = BoxReport(debug: ["relay": ["state": "awake"], "box": ["reachable": true, "state": [
            "memory_mb": ["total": 8192, "available": 5300], "disk": ["total_mb": 10240, "available_mb": 6900],
            "load": [0.42], "workspaces": [["project_id": "p1", "name": "Arbos", "healthy": true, "turn": ["running": true]]],
        ]]], now: 5)
        XCTAssertFalse(old.loadIsHost)
        XCTAssertNil(old.cpuPct)
        XCTAssertTrue(old.usageRows.isEmpty)
        XCTAssertNil(old.jobsRunning)
    }

    func testThresholdsAre85AmberAnd90Red() {
        XCTAssertEqual(MachineMetrics.level(84.9), .ok)
        XCTAssertEqual(MachineMetrics.level(85.0), .warn)
        XCTAssertEqual(MachineMetrics.level(89.9), .warn)
        XCTAssertEqual(MachineMetrics.level(90.0), .crit)
        XCTAssertEqual(MachineMetrics.pctUsed(used: 9216, total: 10240), 90)
        XCTAssertNil(MachineMetrics.pctUsed(used: 1, total: 0))
    }

    func testGaugesReadUsedOfTotalAndTheDiskNamesFileSlots() {
        let r = BoxReport(debug: debug(diskAvail: 6900, memUsed: 2892), now: 5)
        let d = try! XCTUnwrap(MachineMetrics.diskGauge(r))
        XCTAssertEqual(d.value, "3.3 GB used of 10 GB")
        XCTAssertEqual(d.level, .ok)
        XCTAssertEqual(d.sub, "6.7 GB free · 93 k file slots free")
        let m = try! XCTUnwrap(MachineMetrics.memGauge(r))
        XCTAssertEqual(m.value, "2.8 GB used of 8.0 GB")
        XCTAssertNil(m.sub)
        let c = try! XCTUnwrap(MachineMetrics.cpuGauge(r))
        XCTAssertEqual(c.value, "23 %")
        XCTAssertEqual(c.sub, "4 cores")
        // Daytona's overlay counts the host's 750 M inodes: no file-slot line.
        var host = r
        host.inodesTotal = 750_132_736
        XCTAssertEqual(MachineMetrics.diskGauge(host)?.sub, "6.7 GB free")
        let (net, today) = try! XCTUnwrap(MachineMetrics.netValue(r))
        XCTAssertEqual(net, "↓ 1.2 MB/s  ↑ 80 kB/s")
        XCTAssertEqual(today, "2.1 GB today")
        XCTAssertEqual(MachineMetrics.rate(0), "0 B/s")
        XCTAssertEqual(MachineMetrics.bytes(640_000_000), "640 MB")
        XCTAssertEqual(MachineMetrics.countK(1234), "1.2 k")
    }

    func testWarningAppearsAtTheThresholdWithTheTwoWaysOut() {
        XCTAssertNil(MachineMetrics.warning(BoxReport(debug: debug(diskAvail: 6900, memUsed: 2892), now: 5)))
        // disk 92 % (0.8 GB free of 10), memory 88 % (1.0 GB free of 8)
        let r = BoxReport(debug: debug(diskAvail: 819, memUsed: 7209), now: 5)
        let w = try! XCTUnwrap(MachineMetrics.warning(r))
        XCTAssertEqual(w.title, "Running low on disk and memory")
        XCTAssertEqual(w.body, "Disk 92 % · memory 88 %. Past the limit, saves fail and the machine restarts — agents lose their turn.")
        XCTAssertEqual(MachineMetrics.diskGauge(r)?.level, .crit)
        XCTAssertEqual(MachineMetrics.memGauge(r)?.level, .warn)
        XCTAssertEqual(MachineMetrics.memGauge(r)?.sub, "Over 8.0 GB the machine restarts")
        let p = MachineMetrics.plan(machine: hosted(), waitlisted: false, report: r, breakdownOpen: false)
        XCTAssertEqual(p.warning, [.warning, .askToFreeSpace, .requestBiggerMachineInWarning])
        XCTAssertEqual(MachineMetrics.biggestDiskUser(r)?.name, "Arbos")
        // Memory alone: no "ask to free space" (it is a disk remedy).
        let memOnly = BoxReport(debug: debug(diskAvail: 6900, memUsed: 7209), now: 5)
        XCTAssertEqual(MachineMetrics.plan(machine: hosted(), waitlisted: false, report: memOnly, breakdownOpen: false).warning,
                       [.warning, .requestBiggerMachineInWarning])
        XCTAssertEqual(MachineMetrics.warning(memOnly)?.title, "Running low on memory")
    }

    func testPlanOrderOnALiveBox() {
        let r = BoxReport(debug: debug(diskAvail: 6900, memUsed: 2892), now: 5)
        let p = MachineMetrics.plan(machine: hosted(), waitlisted: false, report: r, breakdownOpen: false)
        XCTAssertTrue(p.warning.isEmpty)
        XCTAssertEqual(p.machine, [.whereItRuns, .state, .spec, .machineTime, .sandbox])
        XCTAssertEqual(p.resources, [.disk, .memory, .cpu, .network])
        XCTAssertEqual(p.agents, [.runningNow, .jobs, .breakdown])
        XCTAssertEqual(p.actions, [.requestBiggerMachine, .restart])
        XCTAssertEqual(MachineMetrics.runningNow(r), "1 of 2 agents")
        XCTAssertEqual(MachineMetrics.jobsValue(r.jobsRunning ?? 0), "1 running")
        XCTAssertEqual(MachineMetrics.jobsValue(0), "None")
    }

    func testBreakdownExpandsToOneRowPerWorkspace() {
        let r = BoxReport(debug: debug(diskAvail: 6900, memUsed: 2892), now: 5)
        let open = MachineMetrics.plan(machine: hosted(), waitlisted: false, report: r, breakdownOpen: true)
        XCTAssertEqual(open.agents, [.runningNow, .jobs, .breakdown, .agent("p1"), .agent("p2")])
        let closed = MachineMetrics.plan(machine: hosted(), waitlisted: false, report: r, breakdownOpen: false)
        XCTAssertFalse(closed.agents.contains { if case .agent = $0 { return true } else { return false } })
        let rows = r.usageRows
        XCTAssertEqual(MachineMetrics.agentStatus(rows[0]), "Working · 42 s · 1 job")
        XCTAssertEqual(MachineMetrics.agentCpu(rows[0]), "CPU 61 %")
        XCTAssertEqual(MachineMetrics.agentMemDisk(rows[0]), "1.9 GB · 2.2 GB disk")
        XCTAssertEqual(MachineMetrics.agentStatus(rows[1]), "Idle")
    }

    func testAnOlderImageDrawsOnlyWhatItReports() {
        let r = BoxReport(debug: debug(diskAvail: 6900, memUsed: 2892, cpu: nil, usage: false), now: 5)
        let p = MachineMetrics.plan(machine: hosted(), waitlisted: false, report: r, breakdownOpen: true)
        XCTAssertEqual(p.resources, [.disk, .memory, .network])
        XCTAssertEqual(p.agents, [.runningNow])
    }

    func testAnUnansweringBoxSaysSoOnceAndNoReportKeepsTheOldRows() {
        var asleep = BoxReport(debug: [String: Any](), now: 5)
        asleep.unreachableReason = "box unreachable: timeout"
        let p = MachineMetrics.plan(machine: hosted(), waitlisted: false, report: asleep, breakdownOpen: false)
        XCTAssertEqual(p.resources, [.notAnswering])
        XCTAssertTrue(p.agents.isEmpty)
        XCTAssertEqual(p.machine, [.whereItRuns, .state, .spec, .machineTime, .workspaces, .sandbox])
        let none = MachineMetrics.plan(machine: hosted(), waitlisted: true, report: nil, breakdownOpen: false)
        XCTAssertTrue(none.resources.isEmpty)
        XCTAssertEqual(none.machine, [.whereItRuns, .inLine, .state, .spec, .machineTime, .workspaces, .sandbox])
        // A held always-on box offers Start; a self-hosted one Stop / Start by state.
        var held = hosted(); held.held = true
        XCTAssertEqual(MachineMetrics.plan(machine: held, waitlisted: false, report: nil, breakdownOpen: false).actions, [.requestBiggerMachine, .start])
        var own = Machine(state: .hibernatedLight, since: nil, spec: "")
        XCTAssertEqual(MachineMetrics.plan(machine: own, waitlisted: false, report: nil, breakdownOpen: false).actions, [.start])
        own.state = .awake
        XCTAssertEqual(MachineMetrics.plan(machine: own, waitlisted: false, report: nil, breakdownOpen: false).actions, [.stop])
    }

    func testThePollStartsWithThePageAndStopsWhenItLeaves() {
        XCTAssertEqual(MachineMetrics.pollStep(polling: false, onPage: true), .start)
        XCTAssertEqual(MachineMetrics.pollStep(polling: true, onPage: true), .tick)
        XCTAssertEqual(MachineMetrics.pollStep(polling: true, onPage: false), .stop)
        XCTAssertEqual(MachineMetrics.pollStep(polling: false, onPage: false), .idle)
        XCTAssertEqual(MachineMetrics.pollSeconds, 5)
    }
}
