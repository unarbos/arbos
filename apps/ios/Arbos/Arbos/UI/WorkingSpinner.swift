// The working spinner: the braille cycle ⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ in a fixed glyph cell,
// drawn wherever a list row says "this agent is working" — the home rows'
// status slot under the time, a Machine page agent row (Jacob, 2026-10-27,
// [feedback working-braille-spinner-ios-2026-10-27]; replaces the small dot).
// Mirrors the Mac's `ui/working.rs`.
//
// One clock, one ticker. Every spinner on screen reads the same frame index
// from `WorkingSpinnerClock.shared`; a spinner attaches on appear and
// detaches on disappear, and the one timer runs only while something is
// attached and the app is in the foreground. The timer sits in the run loop's
// `.default` mode, so it does not fire while a scroll view tracks a finger —
// the spinner never competes with scroll frames. Only the glyph `Text`
// observes the clock, so a tick re-evaluates the cell, not the row.
// Reduce Motion draws the still glyph ⠿ and attaches nothing.

import Combine
import SwiftUI
import UIKit

/// Frames, timing and geometry — shared with the tests and the Mac.
enum WorkingSpinnerFrames {
    /// The classic cycle, in order.
    static let frames: [String] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]
    /// One frame every 90 ms (the 80–100 ms band, ≈ 11 fps).
    static let interval: TimeInterval = 0.09
    /// Reduce Motion: all six dots of the frames' cell (U+283F), nothing moves.
    static let still = "⠿"
    /// The cell: the unread dot's footprint (`Theme.unreadDot`, 10 pt) square,
    /// so the spinner's right edge is the dot's right edge under the time.
    static let cell: CGFloat = Theme.unreadDot
    /// The glyph's design size: the row's tiny text (13), the time label's.
    static let glyphSize: CGFloat = Theme.tiny

    /// The frame index for a clock reading: `elapsed / interval`, round the ten.
    static func index(elapsed: TimeInterval) -> Int {
        let steps = Int((max(0, elapsed) / interval).rounded(.down))
        return steps % frames.count
    }

    static func glyph(elapsed: TimeInterval) -> String {
        frames[index(elapsed: elapsed)]
    }
}

/// The one ticker behind every spinner on screen.
@MainActor
final class WorkingSpinnerClock: ObservableObject {
    static let shared = WorkingSpinnerClock()

    /// The frame every attached spinner shows. Written only when it changes.
    @Published private(set) var frame: Int = 0

    private(set) var attached = 0
    private(set) var foreground = true
    private var timer: Timer?
    private let epoch = Date()
    private var lifecycle: [NSObjectProtocol] = []

    /// Trace counters (Debug builds with `ARBOS_PERF_LOG=1` also bump `spinner.tick` / `spinner.start`;
    /// `PerfCounters` only exists under `#if DEBUG`).
    private(set) var ticks = 0
    private(set) var starts = 0

    var isTicking: Bool { timer != nil }

    /// `observeLifecycle` false in tests: they drive `setForeground` by hand.
    init(observeLifecycle: Bool = true) {
        guard observeLifecycle else { return }
        let nc = NotificationCenter.default
        lifecycle = [
            nc.addObserver(forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.setForeground(false) }
            },
            nc.addObserver(forName: UIApplication.willResignActiveNotification, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.setForeground(false) }
            },
            nc.addObserver(forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.setForeground(true) }
            },
        ]
    }

    /// A spinner came on screen.
    func attach() {
        attached += 1
        reconcile()
    }

    /// A spinner left the screen.
    func detach() {
        attached = max(0, attached - 1)
        reconcile()
    }

    /// The app's foreground state; the ticker parks in the background.
    func setForeground(_ on: Bool) {
        guard foreground != on else { return }
        foreground = on
        reconcile()
    }

    /// What the clock reads right now (no tick needed).
    var currentIndex: Int { WorkingSpinnerFrames.index(elapsed: Date().timeIntervalSince(epoch)) }

    private func reconcile() {
        let wanted = attached > 0 && foreground
        if wanted, timer == nil {
            start()
        } else if !wanted, let t = timer {
            t.invalidate()
            timer = nil
        }
    }

    private func start() {
        starts += 1
        #if DEBUG
        PerfCounters.bump("spinner.start")
        #endif
        frame = currentIndex
        let t = Timer(timeInterval: WorkingSpinnerFrames.interval, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.tick() }
        }
        t.tolerance = WorkingSpinnerFrames.interval / 10
        // `.default`, not `.common`: paused while a scroll view tracks a finger.
        RunLoop.main.add(t, forMode: .default)
        timer = t
    }

    /// For tests: one tick as the timer would deliver it.
    func tick() {
        ticks += 1
        #if DEBUG
        PerfCounters.bump("spinner.tick")
        #endif
        let i = currentIndex
        if i != frame { frame = i }
    }
}

/// The spinner's cell: a fixed square, the glyph centred in it, in the
/// theme's muted text colour (the time label's — never the green).
struct WorkingSpinner: View {
    @Environment(\.theme) private var theme
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    var size: CGFloat = WorkingSpinnerFrames.cell
    var glyphSize: CGFloat = WorkingSpinnerFrames.glyphSize

    var body: some View {
        Group {
            if reduceMotion {
                glyphText(WorkingSpinnerFrames.still)
            } else {
                WorkingSpinnerGlyph(clock: .shared, glyphSize: glyphSize)
            }
        }
        .frame(width: size, height: size)
        .accessibilityLabel("Working")
    }

    private func glyphText(_ s: String) -> some View {
        Text(s)
            .font(theme.font(glyphSize, design: .monospaced))
            .foregroundStyle(theme.muted)
            .lineLimit(1)
            .fixedSize()
    }
}

/// The live glyph: the only view that observes the clock, so a tick
/// re-evaluates this `Text` and nothing around it.
private struct WorkingSpinnerGlyph: View {
    @Environment(\.theme) private var theme
    @ObservedObject var clock: WorkingSpinnerClock
    let glyphSize: CGFloat

    var body: some View {
        Text(WorkingSpinnerFrames.frames[clock.frame])
            .font(theme.font(glyphSize, design: .monospaced))
            .foregroundStyle(theme.muted)
            .lineLimit(1)
            .fixedSize()
            .onAppear { clock.attach() }
            .onDisappear { clock.detach() }
    }
}
