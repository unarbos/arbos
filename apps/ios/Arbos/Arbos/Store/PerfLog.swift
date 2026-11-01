// Debug-only frame pacing and memory log for the long-chat performance rig
// (long-chat-perf, 2026-10-23). With `ARBOS_PERF_LOG=1` a display link counts
// frames, frames that took over 1.5× the nominal period ("dropped"), the worst
// frame and the summed time over budget — a main-thread hang shows up as one
// long frame — and logs them with the footprint every 2 s:
//
// The link asks for the screen's top rate, so on a ProMotion phone it budgets against
// 8.3 ms. These are Debug numbers: frame-rate and hitch figures for a fix come from
// Instruments (Animation Hitches) on a Release build on a device, not from this log.
//
//   log show --predicate 'subsystem == "com.unarbos.arbos.ios" AND category == "perf"'

#if DEBUG
import Combine
import Foundation
import QuartzCore
import SwiftUI
import UIKit
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
        // Without a range the link is held to 60 Hz on ProMotion phones and a dropped
        // 120 Hz frame never shows. Asking for 120 keeps the display at that rate while
        // the rig runs, so frame numbers from this log are not battery-representative.
        let maxFps = Float(UIScreen.main.maximumFramesPerSecond)
        link.preferredFrameRateRange = CAFrameRateRange(minimum: min(80, maxFps), maximum: maxFps, preferred: maxFps)
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
        // `duration` is the screen's base period; the interval the link is actually
        // driven at is `targetTimestamp - timestamp` (8.3 ms when ProMotion grants 120 Hz).
        let nominalMs = (link.targetTimestamp - link.timestamp) * 1000
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
        Self.log.notice("frames=\(self.frames) hz=\(Int((1000 / nominalMs).rounded())) dropped=\(self.dropped) worst=\(Int(self.worstMs))ms over_budget=\(Int(self.overBudgetMs))ms rss=\(Self.footprintMB())MB t=\(LaunchClock.msSinceLaunch())ms \(counts, privacy: .public)")
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

/// What the `ARBOS_PERF_DRIVE=jump` driver watches: when the transcript's scroll
/// geometry last changed (stamped by the chat's bottom reporter).
enum PerfDriveProbe {
    /// Main thread only (the reporter writes, the driver reads).
    nonisolated(unsafe) static var lastScrollGeometry = Date.distantPast
    /// The `switch` driver runs once per process (every chat it opens would start it again).
    nonisolated(unsafe) static var switchStarted = false
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

/// `ARBOS_PERF_TOUCH=1` (iphone-scroll-perf, 2026-10-27): what happens to a finger on the
/// chat screen. Every touch sequence logs, in the `perf` category:
///
///   touch #n down=(x,y) travel=Npt dur=Nms pan=+Nms move=+Nms [DROPPED] [won: …]
///
/// `pan` = touch-down → the transcript's `UIScrollView` pan recognizer reached `.began`;
/// `move` = touch-down → the first content-offset change (what the eye sees); `DROPPED` = the
/// finger travelled ≥ 20 pt and the transcript never moved; `won:` lists the recognizers that
/// were active at that moment. Touch-down time is the HID timestamp of the event (the same
/// clock as `CACurrentMediaTime`), so queueing before delivery counts. The first touch also
/// dumps the gesture-recognizer graph under the window once (`gesture-graph` lines).
@MainActor
final class TouchPerf {
    static let shared = TouchPerf()
    private static let log = Logger(subsystem: "com.unarbos.arbos.ios", category: "perf")

    private weak var scroll: UIScrollView?
    private var offsetObservation: NSKeyValueObservation?
    private var sequence = 0
    private var downAt: TimeInterval?
    private var downPoint = CGPoint.zero
    private var travel: CGFloat = 0
    private var panBeganAt: TimeInterval?
    private var firstMoveAt: TimeInterval?
    private var graphDumped = false
    private static var swizzled = false

    /// Hook `UIWindow.sendEvent` once; a no-op without the env flag.
    static func installIfRequested() {
        guard DebugHooks.perfTouch, !swizzled,
              let original = class_getInstanceMethod(UIWindow.self, #selector(UIWindow.sendEvent(_:))),
              let hook = class_getInstanceMethod(UIWindow.self, #selector(UIWindow.arbosPerfSendEvent(_:))) else { return }
        method_exchangeImplementations(original, hook)
        swizzled = true
        log.notice("touch perf on")
    }

    /// The transcript's scroll view (found by `ScrollPerfProbe` inside the ScrollView).
    func attach(_ found: UIScrollView) {
        guard scroll !== found else { return }
        scroll?.panGestureRecognizer.removeTarget(self, action: #selector(pan(_:)))
        scroll = found
        found.panGestureRecognizer.addTarget(self, action: #selector(pan(_:)))
        offsetObservation = found.observe(\.contentOffset, options: [.old, .new]) { [weak self] _, change in
            guard let self, let old = change.oldValue, let new = change.newValue, old != new else { return }
            // contentOffset is written on the main thread; read the clock right here.
            MainActor.assumeIsolated { self.offsetMoved() }
        }
        Self.log.notice("touch perf: scroll view attached delaysContentTouches=\(found.delaysContentTouches) canCancelContentTouches=\(found.canCancelContentTouches) panDelaysTouchesBegan=\(found.panGestureRecognizer.delaysTouchesBegan)")
    }

    private func offsetMoved() {
        guard downAt != nil, firstMoveAt == nil else { return }
        firstMoveAt = CACurrentMediaTime()
    }

    @objc private func pan(_ gr: UIPanGestureRecognizer) {
        guard gr.state == .began, downAt != nil, panBeganAt == nil else { return }
        panBeganAt = CACurrentMediaTime()
    }

    fileprivate func handle(_ event: UIEvent, in window: UIWindow) {
        guard event.type == .touches, let touches = event.allTouches, let touch = touches.first else { return }
        switch touch.phase {
        case .began:
            guard downAt == nil else { return }
            sequence += 1
            downAt = event.timestamp
            downPoint = touch.location(in: window)
            travel = 0
            panBeganAt = nil
            firstMoveAt = nil
            if !graphDumped {
                graphDumped = true
                dumpGraph(window)
            }
        case .moved, .stationary:
            let p = touch.location(in: window)
            travel = max(travel, hypot(p.x - downPoint.x, p.y - downPoint.y))
        case .ended, .cancelled:
            guard let down = downAt, touches.allSatisfy({ $0.phase == .ended || $0.phase == .cancelled }) else { return }
            let p = touch.location(in: window)
            travel = max(travel, hypot(p.x - downPoint.x, p.y - downPoint.y))
            let durMs = Int((event.timestamp - down) * 1000)
            let pan = panBeganAt.map { "+\(Int(($0 - down) * 1000))ms" } ?? "never"
            let move = firstMoveAt.map { "+\(Int(($0 - down) * 1000))ms" } ?? "never"
            let dropped = travel >= 20 && firstMoveAt == nil
            var line = "touch #\(sequence) down=(\(Int(downPoint.x)),\(Int(downPoint.y))) travel=\(Int(travel))pt dur=\(durMs)ms pan=\(pan) move=\(move)"
            if dropped {
                line += " DROPPED won: \(activeRecognizers(window))"
                PerfCounters.bump("touch.dropped")
            }
            if travel >= 20 {
                PerfCounters.bump("touch.pans")
                if let m = firstMoveAt { PerfCounters.time("touch.lat", ms: (m - down) * 1000) }
            }
            Self.log.notice("\(line, privacy: .public)")
            downAt = nil
        default:
            break
        }
    }

    private func activeRecognizers(_ window: UIWindow) -> String {
        var out: [String] = []
        Self.walk(window) { view in
            for gr in view.gestureRecognizers ?? [] where gr.state == .began || gr.state == .changed || gr.state == .ended {
                out.append("\(Self.name(gr))@\(Self.name(view))")
            }
        }
        return out.isEmpty ? "none" : out.joined(separator: " ")
    }

    private func dumpGraph(_ window: UIWindow) {
        var lines: [String] = []
        var total = 0
        Self.walk(window) { view in
            guard let grs = view.gestureRecognizers, !grs.isEmpty else { return }
            total += grs.count
            let scrollNote: String
            if let s = view as? UIScrollView {
                scrollNote = " SCROLL delaysContentTouches=\(s.delaysContentTouches) canCancel=\(s.canCancelContentTouches)"
            } else {
                scrollNote = ""
            }
            let list = grs.map { gr -> String in
                var flags: [String] = []
                if gr.delaysTouchesBegan { flags.append("delaysBegan") }
                if gr.delaysTouchesEnded { flags.append("delaysEnded") }
                if !gr.cancelsTouchesInView { flags.append("noCancel") }
                if !gr.isEnabled { flags.append("disabled") }
                return Self.name(gr) + (flags.isEmpty ? "" : "[\(flags.joined(separator: ","))]")
            }
            lines.append("\(Self.name(view))\(scrollNote): \(list.joined(separator: ", "))")
        }
        Self.log.notice("gesture-graph recognizers=\(total) views=\(lines.count)")
        for l in lines { Self.log.notice("gesture-graph \(l, privacy: .public)") }
    }

    private static func walk(_ view: UIView, _ visit: (UIView) -> Void) {
        visit(view)
        for sub in view.subviews { walk(sub, visit) }
    }

    private static func name(_ o: AnyObject) -> String {
        let full = String(describing: type(of: o))
        return full.count > 48 ? String(full.prefix(48)) : full
    }
}

extension UIWindow {
    @objc fileprivate func arbosPerfSendEvent(_ event: UIEvent) {
        TouchPerf.shared.handle(event, in: self)
        // Swizzled: this selector now holds the original implementation.
        arbosPerfSendEvent(event)
    }
}

/// Finds the transcript's `UIScrollView` for `TouchPerf` (placed inside the ScrollView's content).
struct ScrollPerfProbe: UIViewRepresentable {
    func makeUIView(context: Context) -> Probe {
        let v = Probe()
        v.isUserInteractionEnabled = false
        v.backgroundColor = .clear
        return v
    }

    func updateUIView(_ uiView: Probe, context: Context) {}

    final class Probe: UIView {
        override func didMoveToWindow() {
            super.didMoveToWindow()
            guard window != nil else { return }
            var node: UIView? = superview
            while let current = node {
                if let scroll = current as? UIScrollView {
                    TouchPerf.shared.attach(scroll)
                    return
                }
                node = current.superview
            }
        }
    }
}
#endif
