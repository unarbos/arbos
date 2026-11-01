// Release-build performance marks (ios-perf-baseline, 2026-11-01). `PerfLog` is Debug-only,
// and Debug SwiftUI numbers say nothing about what a tester feels, so these compile into
// every build:
//
// - `PerfSignpost` intervals under Points of Interest, always on (an OSSignposter interval
//   costs nanoseconds when nothing records). Instruments draws them; the perf UI tests read
//   them with `XCTOSSignpostMetric(subsystem:category:name:)`.
// - `FrameProbe`, only with `ARBOS_PERF_FRAMES=1` in the launch environment: a display link
//   that logs every late frame and main-thread stall, a once-a-second line with the memory
//   footprint and the body runs counted that second (`PerfCount`), each interval's duration
//   and each transcript rebuild (`rows-build`), and closes the `*-settled` intervals:
//
//   log show --predicate 'subsystem == "com.unarbos.arbos.ios" AND category == "perf-frames"'

import Foundation
import QuartzCore
import UIKit
import os

@MainActor
enum PerfSignpost {
    static let subsystem = "com.unarbos.arbos.ios"
    static let category = "PointsOfInterest"
    private static let signposter = OSSignposter(subsystem: subsystem, category: .pointsOfInterest)

    enum Interval: CaseIterable {
        /// Home row tapped (the route lands on `path`) → the chat's newest row is in a committed frame.
        case openChat
        /// Same start → the push has finished and the probe saw four on-time frames in a row.
        case openChatSettled
        /// A chat popped off `path` → the pop has finished and four on-time frames followed.
        case backSettled
        /// The composer asked for focus → `keyboardDidShow`.
        case keyboardUp
        /// One key into the composer → the main thread's next turn (the commit carrying it).
        case keystroke
        /// Send pressed → my new bubble is in a committed frame.
        case send
        /// A long press opened a message's actions → the overlay is in a committed frame.
        case messageMenu
        /// The home account button → the Settings sheet's content is in a committed frame.
        case settingsSheet

        var name: StaticString {
            switch self {
            case .openChat: return "open-chat"
            case .openChatSettled: return "open-chat-settled"
            case .backSettled: return "back-settled"
            case .keyboardUp: return "keyboard-up"
            case .keystroke: return "keystroke"
            case .send: return "send"
            case .messageMenu: return "message-menu"
            case .settingsSheet: return "settings-sheet"
            }
        }

        /// Closed by the frame probe, not by a call site.
        var waitsForSettle: Bool {
            switch self {
            case .openChatSettled, .backSettled: return true
            case .openChat, .keyboardUp, .keystroke, .send, .messageMenu, .settingsSheet: return false
            }
        }
    }

    private struct Running {
        let state: OSSignpostIntervalState
        let started: CFTimeInterval
    }

    private static var running: [Interval: Running] = [:]
    /// The chat an `openChat` is waiting on, and the newest row a `send` started from.
    private static var openingChat: String?
    private static var sendFrom: (chatId: String, rowId: String)?

    /// A second begin replaces the first, which is left unmatched: XCTest and Instruments pair
    /// by signpost id, so an abandoned interval is dropped rather than measured.
    static func begin(_ interval: Interval) {
        if interval.waitsForSettle, !FrameProbe.enabled { return }
        running[interval] = Running(state: signposter.beginInterval(interval.name, id: signposter.makeSignpostID()),
                                    started: CACurrentMediaTime())
        if interval.waitsForSettle { FrameProbe.shared.awaitSettle() }
    }

    static func end(_ interval: Interval) {
        guard let run = running.removeValue(forKey: interval) else { return }
        signposter.endInterval(interval.name, run.state)
        FrameProbe.shared.logInterval(interval, ms: (CACurrentMediaTime() - run.started) * 1000)
    }

    /// End on the main thread's next turn, after the commit that carries the change.
    static func endAfterCommit(_ interval: Interval) {
        DispatchQueue.main.async { end(interval) }
    }

    static func isRunning(_ interval: Interval) -> Bool { running[interval] != nil }

    // MARK: Transcript rows

    /// Which branch of `ChatSession.refreshRows()` ran: `full` walks every message in the chat.
    enum RowsPath: String {
        case unchanged, tail, append, full
    }

    struct RowsMark {
        fileprivate let state: OSSignpostIntervalState
        fileprivate let started: CFTimeInterval
    }

    static func rowsBegin() -> RowsMark {
        RowsMark(state: signposter.beginInterval("rows-build", id: signposter.makeSignpostID()), started: CACurrentMediaTime())
    }

    static func rowsEnd(_ mark: RowsMark, path: RowsPath, messages: Int, rows: Int, window: Int) {
        signposter.endInterval("rows-build", mark.state,
                               "\(path.rawValue, privacy: .public) messages=\(messages) rows=\(rows) window=\(window)")
        FrameProbe.shared.logRows(path: path, messages: messages, rows: rows, window: window,
                                  ms: (CACurrentMediaTime() - mark.started) * 1000)
    }

    // MARK: Call sites

    static func pathChanged(from old: [AppModel.Route], to new: [AppModel.Route]) {
        let oldChat = old.last.flatMap(chatId(of:))
        let newChat = new.last.flatMap(chatId(of:))
        guard oldChat != newChat else { return }
        if let newChat {
            openingChat = newChat
            running[.backSettled] = nil
            begin(.openChat)
            begin(.openChatSettled)
        } else if oldChat != nil {
            openingChat = nil
            running[.openChat] = nil
            running[.openChatSettled] = nil
            begin(.backSettled)
        }
    }

    /// The transcript's newest row entered the view tree.
    static func newestRowAppeared(chatId: String, rowId: String) {
        if openingChat == chatId, isRunning(.openChat) {
            openingChat = nil
            endAfterCommit(.openChat)
        }
        if let from = sendFrom, from.chatId == chatId, from.rowId != rowId {
            sendFrom = nil
            endAfterCommit(.send)
        }
    }

    static func sendBegan(chatId: String, newestRowId: String?) {
        sendFrom = (chatId, newestRowId ?? "")
        begin(.send)
    }

    private static func chatId(of route: AppModel.Route) -> String? {
        switch route {
        case .chat(let chatId, _): return chatId
        case .server: return nil
        }
    }

    // MARK: Keyboard

    private static var keyboardObserver: NSObjectProtocol?

    static func composerAskedForFocus() {
        if keyboardObserver == nil {
            keyboardObserver = NotificationCenter.default.addObserver(forName: UIResponder.keyboardDidShowNotification,
                                                                      object: nil, queue: .main) { _ in
                MainActor.assumeIsolated { end(.keyboardUp) }
            }
        }
        begin(.keyboardUp)
    }
}

/// What the probe's once-a-second line counts besides frames.
enum PerfCount: Int, CaseIterable {
    case homeBody, chatBody, rowBody, blockBody, streamToken

    var label: String {
        switch self {
        case .homeBody: return "home_body"
        case .chatBody: return "chat_body"
        case .rowBody: return "row_body"
        case .blockBody: return "block_body"
        case .streamToken: return "tokens"
        }
    }
}

/// `ARBOS_PERF_FRAMES=1`: frame pacing, stalls and memory for the perf baseline. Late is
/// measured against the screen's maximum rate, so on a 120 Hz screen a 60 Hz frame is late.
@MainActor
final class FrameProbe {
    static let enabled = ProcessInfo.processInfo.environment["ARBOS_PERF_FRAMES"] == "1"
    static let shared = FrameProbe()
    private static let log = Logger(subsystem: PerfSignpost.subsystem, category: "perf-frames")
    /// On-time frames in a row (with no navigation transition running) that count as settled.
    private static let settleFrames = 4

    private var link: CADisplayLink?
    private var last: CFTimeInterval = 0
    private var nominalMs = 1000.0 / 60
    private var windowStart: CFTimeInterval = 0
    private var frames = 0
    private var late = 0
    private var dropped = 0
    private var overMs = 0.0
    private var worstMs = 0.0
    private var steady = 0
    private var settling = false
    private var counts = [Int](repeating: 0, count: PerfCount.allCases.count)

    /// `let _ = FrameProbe.count(.homeBody)` in a body: one branch on a constant when off.
    static func count(_ what: PerfCount) {
        guard enabled else { return }
        shared.counts[what.rawValue] &+= 1
    }

    static func startIfRequested() {
        guard enabled, shared.link == nil else { return }
        shared.start()
    }

    private func start() {
        let maxFps = UIScreen.main.maximumFramesPerSecond
        nominalMs = 1000.0 / Double(max(1, maxFps))
        let link = CADisplayLink(target: self, selector: #selector(tick(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: Float(maxFps), maximum: Float(maxFps), preferred: Float(maxFps))
        link.add(to: .main, forMode: .common)
        self.link = link
        Self.log.notice("probe on max_fps=\(maxFps) nominal_ms=\(self.nominalMs, format: .fixed(precision: 2)) t=\(LaunchClock.msSinceLaunch())ms rss=\(Self.footprintMB())MB")
    }

    func awaitSettle() {
        settling = true
        steady = 0
    }

    func logInterval(_ interval: PerfSignpost.Interval, ms: Double) {
        guard Self.enabled else { return }
        Self.log.notice("interval \(String(describing: interval.name), privacy: .public) ms=\(ms, format: .fixed(precision: 1))")
    }

    func logRows(path: PerfSignpost.RowsPath, messages: Int, rows: Int, window: Int, ms: Double) {
        guard Self.enabled else { return }
        Self.log.notice("rows-build path=\(path.rawValue, privacy: .public) messages=\(messages) rows=\(rows) window=\(window) ms=\(ms, format: .fixed(precision: 2))")
    }

    @objc private func tick(_ link: CADisplayLink) {
        let now = link.timestamp
        defer { last = now }
        guard last > 0 else {
            windowStart = now
            return
        }
        let ms = (now - last) * 1000
        frames += 1
        worstMs = max(worstMs, ms)
        let isLate = ms > nominalMs * 1.5
        if isLate {
            late += 1
            let missed = Int((ms / nominalMs).rounded()) - 1
            dropped += max(0, missed)
            overMs += ms - nominalMs
            Self.log.notice("late ms=\(ms, format: .fixed(precision: 1)) missed=\(missed)\(ms >= 250 ? " HANG" : ms >= 100 ? " STALL" : "", privacy: .public)")
        }
        if settling {
            steady = isLate || Self.transitionRunning() ? 0 : steady + 1
            if steady >= Self.settleFrames {
                PerfSignpost.end(.backSettled)
                if !PerfSignpost.isRunning(.openChat) { PerfSignpost.end(.openChatSettled) }
                settling = PerfSignpost.isRunning(.openChatSettled)
            }
        }
        guard now - windowStart >= 1 else { return }
        let counted = PerfCount.allCases.map { "\($0.label)=\(counts[$0.rawValue])" }.joined(separator: " ")
        Self.log.notice("second frames=\(self.frames) late=\(self.late) dropped=\(self.dropped) over_ms=\(Int(self.overMs)) worst_ms=\(Int(self.worstMs)) rss=\(Self.footprintMB())MB \(counted, privacy: .public)")
        counts = [Int](repeating: 0, count: PerfCount.allCases.count)
        frames = 0
        late = 0
        dropped = 0
        overMs = 0
        worstMs = 0
        windowStart = now
    }

    /// A push or pop still animating (its coordinator lives until the transition ends).
    private static func transitionRunning() -> Bool {
        let windows = UIApplication.shared.connectedScenes.compactMap { ($0 as? UIWindowScene)?.keyWindow }
        return windows.contains { window in
            var stack: [UIViewController] = window.rootViewController.map { [$0] } ?? []
            while let vc = stack.popLast() {
                if let nav = vc as? UINavigationController, nav.transitionCoordinator != nil { return true }
                stack.append(contentsOf: vc.children)
                if let presented = vc.presentedViewController { stack.append(presented) }
            }
            return false
        }
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
