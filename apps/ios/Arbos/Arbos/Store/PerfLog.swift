// Debug-only frame pacing and memory log for the long-chat performance rig
// (long-chat-perf, 2026-10-23). With `ARBOS_PERF_LOG=1` a display link counts
// frames, frames that took over 1.5× the nominal period ("dropped"), the worst
// frame and the summed time over budget — a main-thread hang shows up as one
// long frame — and logs them with the footprint every 2 s:
//
//   log show --predicate 'subsystem == "com.unarbos.arbos.ios" AND category == "perf"'

#if DEBUG
import Combine
import Foundation
import QuartzCore
import os

@MainActor
final class PerfLog {
    static let shared = PerfLog()
    private static let log = Logger(subsystem: "com.unarbos.arbos.ios", category: "perf")

    private var link: CADisplayLink?
    private var lastTimestamp: CFTimeInterval = 0
    private var windowStart: CFTimeInterval = 0
    private var frames = 0
    private var dropped = 0
    private var worstMs = 0.0
    private var overBudgetMs = 0.0
    private var started = false
    /// "Scrollable": after `armScrollable()`, the first moment 30 frames in a row stay
    /// under 33 ms — the main thread has been free for half a second.
    private var scrollableArmedAt: CFTimeInterval = 0
    private var smoothRun = 0
    private var scrollableLogged = false

    func armScrollable() {
        scrollableArmedAt = CACurrentMediaTime()
        smoothRun = 0
        scrollableLogged = false
    }

    private var publishSink: AnyCancellable?

    /// Count every AppModel publish (any `@Published` write) into the perf line.
    func observe(_ model: AppModel) {
        guard DebugHooks.perfLog, publishSink == nil else { return }
        publishSink = model.objectWillChange.sink { _ in PerfCounters.bump("pub.any") }
    }

    func startIfRequested() {
        guard DebugHooks.perfLog, !started else { return }
        started = true
        let link = CADisplayLink(target: self, selector: #selector(tick(_:)))
        link.add(to: .main, forMode: .common)
        self.link = link
        Self.log.notice("perf log on t=\(LaunchClock.msSinceLaunch())ms rss=\(Self.footprintMB())MB")
    }

    @objc private func tick(_ link: CADisplayLink) {
        let now = link.timestamp
        defer { lastTimestamp = now }
        guard lastTimestamp > 0 else {
            windowStart = now
            return
        }
        let frameMs = (now - lastTimestamp) * 1000
        let nominalMs = link.duration * 1000
        frames += 1
        if scrollableArmedAt > 0, !scrollableLogged {
            smoothRun = frameMs < 33 ? smoothRun + 1 : 0
            if smoothRun >= 30 {
                scrollableLogged = true
                let sinceArm = (now - scrollableArmedAt) * 1000 - 30 * nominalMs
                Self.log.notice("first-scrollable +\(Int(max(0, sinceArm)))ms after chat-first-row t=\(LaunchClock.msSinceLaunch())ms")
            }
        }
        if frameMs > nominalMs * 1.5 {
            dropped += 1
            overBudgetMs += frameMs - nominalMs
        }
        worstMs = max(worstMs, frameMs)
        guard now - windowStart >= 2 else { return }
        let counts = PerfCounters.drain()
        Self.log.notice("frames=\(self.frames) dropped=\(self.dropped) worst=\(Int(self.worstMs))ms over_budget=\(Int(self.overBudgetMs))ms rss=\(Self.footprintMB())MB t=\(LaunchClock.msSinceLaunch())ms \(counts, privacy: .public)")
        frames = 0
        dropped = 0
        worstMs = 0
        overBudgetMs = 0
        windowStart = now
    }

    /// Milestone with a duration, same log stream as the frame windows.
    static func mark(_ what: String, ms: Double) {
        log.notice("\(what, privacy: .public) \(Int(ms))ms t=\(LaunchClock.msSinceLaunch())ms")
    }

    /// Physical footprint (what Xcode's memory gauge shows), in MB.
    static func footprintMB() -> Int {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
        let kr = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        guard kr == KERN_SUCCESS else { return 0 }
        return Int(info.phys_footprint / (1024 * 1024))
    }
}

/// Body-evaluation and work counters, drained into each 2 s perf line
/// (`rowBody=… blockBody=… chatBody=… rows()=…/…ms`). Any thread.
final class PerfCounters {
    private static let lock = NSLock()
    private static var counts: [String: Int] = [:]
    private static var millis: [String: Double] = [:]

    static func bump(_ key: String) {
        guard DebugHooks.perfLog else { return }
        lock.lock(); counts[key, default: 0] += 1; lock.unlock()
    }

    static func time(_ key: String, ms: Double) {
        guard DebugHooks.perfLog else { return }
        lock.lock(); counts[key, default: 0] += 1; millis[key, default: 0] += ms; lock.unlock()
    }

    static func drain() -> String {
        lock.lock(); defer { lock.unlock() }
        let out = counts.keys.sorted().map { k -> String in
            if let ms = millis[k] { return "\(k)=\(counts[k]!)/\(Int(ms))ms" }
            return "\(k)=\(counts[k]!)"
        }.joined(separator: " ")
        counts = [:]; millis = [:]
        return out
    }
}
#endif
