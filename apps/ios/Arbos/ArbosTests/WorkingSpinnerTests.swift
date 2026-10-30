import Combine
import SwiftUI
import XCTest
@testable import Arbos

/// The working spinner ([feedback working-braille-spinner-ios-2026-10-27], Jacob
/// 2026-10-27): the braille cycle in a fixed cell where the row's dot sat, one
/// shared ticker for every spinner on screen, Reduce Motion → the still glyph.
/// Twin of the Mac's `working::tests` + `sidebar::status_cell_tests`.
@MainActor
final class WorkingSpinnerTests: XCTestCase {
    // MARK: frame sequence / timing

    func testTheFramesAreTheClassicCycleAtEightyToAHundredMs() {
        XCTAssertEqual(WorkingSpinnerFrames.frames.joined(), "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
        XCTAssertEqual(WorkingSpinnerFrames.frames.count, 10)
        XCTAssertGreaterThanOrEqual(WorkingSpinnerFrames.interval, 0.080)
        XCTAssertLessThanOrEqual(WorkingSpinnerFrames.interval, 0.100)
        let dt = WorkingSpinnerFrames.interval
        XCTAssertEqual(WorkingSpinnerFrames.index(elapsed: 0), 0)
        XCTAssertEqual(WorkingSpinnerFrames.index(elapsed: dt - 0.001), 0)
        XCTAssertEqual(WorkingSpinnerFrames.index(elapsed: dt), 1)
        XCTAssertEqual(WorkingSpinnerFrames.index(elapsed: dt * 9), 9)
        XCTAssertEqual(WorkingSpinnerFrames.index(elapsed: dt * 10), 0, "wraps after ten")
        XCTAssertEqual(WorkingSpinnerFrames.index(elapsed: -1), 0, "a clock reading before the epoch is frame 0")
        let walk = (0..<10).map { WorkingSpinnerFrames.index(elapsed: Double($0) * dt + 0.007) }
        XCTAssertEqual(walk, Array(0..<10), "one step per frame")
        for f in WorkingSpinnerFrames.frames + [WorkingSpinnerFrames.still] {
            XCTAssertEqual(f.unicodeScalars.count, 1, "\(f) is one scalar")
            let v = f.unicodeScalars.first!.value
            XCTAssertTrue((0x2800...0x28FF).contains(v), "\(f) is a braille pattern")
        }
    }

    // MARK: Reduce Motion fallback

    func testReduceMotionGlyphIsTheFullSixDotCell() {
        // ⠿ U+283F: all six dots of the cell the frames use (⠋…⠏ are six-dot
        // patterns), so the still glyph is the same height as the moving ones.
        // Not ⣿ U+28FF (the eight-dot cell), which would stand taller.
        XCTAssertEqual(WorkingSpinnerFrames.still, "⠿")
        XCTAssertEqual(WorkingSpinnerFrames.still.unicodeScalars.first!.value, 0x283F, "all six dots of the frames' cell")
        for f in WorkingSpinnerFrames.frames {
            XCTAssertLessThanOrEqual(f.unicodeScalars.first!.value, 0x283F, "\(f) stays inside the six-dot cell")
        }
    }

    // MARK: single-ticker invariant

    func testOneTickerForEverySpinnerAndItParksWhenNoneIsLeft() {
        let clock = WorkingSpinnerClock(observeLifecycle: false)
        XCTAssertFalse(clock.isTicking, "idle until a spinner attaches")
        XCTAssertEqual(clock.starts, 0)

        for _ in 0..<12 { clock.attach() }
        XCTAssertTrue(clock.isTicking)
        XCTAssertEqual(clock.attached, 12)
        XCTAssertEqual(clock.starts, 1, "twelve spinners, one timer")

        // Twelve more rows scrolling in and out: still the one timer.
        for _ in 0..<12 { clock.attach(); clock.detach() }
        XCTAssertEqual(clock.starts, 1)
        XCTAssertTrue(clock.isTicking)

        for _ in 0..<12 { clock.detach() }
        XCTAssertEqual(clock.attached, 0)
        XCTAssertFalse(clock.isTicking, "nothing attached → parked")
        clock.detach()
        XCTAssertEqual(clock.attached, 0, "never negative")

        clock.attach()
        XCTAssertEqual(clock.starts, 2, "a new timer only when the first spinner is back")
        clock.detach()
    }

    func testTheTickerParksInTheBackgroundAndResumesWithTheApp() {
        let clock = WorkingSpinnerClock(observeLifecycle: false)
        clock.attach()
        XCTAssertTrue(clock.isTicking)
        clock.setForeground(false)
        XCTAssertFalse(clock.isTicking, "background → parked, the spinner still attached")
        XCTAssertEqual(clock.attached, 1)
        clock.setForeground(true)
        XCTAssertTrue(clock.isTicking, "foreground → resumes")
        XCTAssertEqual(clock.starts, 2)
        clock.detach()
        XCTAssertFalse(clock.isTicking)
        // Coming to the foreground with nothing attached starts nothing.
        clock.setForeground(false)
        clock.setForeground(true)
        XCTAssertFalse(clock.isTicking)
        XCTAssertEqual(clock.starts, 2)
    }

    func testATickPublishesOnlyWhenTheFrameChanges() {
        let clock = WorkingSpinnerClock(observeLifecycle: false)
        clock.attach()
        var published = 0
        let sink = clock.objectWillChange.sink { _ in published += 1 }
        let before = clock.frame
        // Two ticks inside the same 90 ms window change nothing; the glyph
        // Text is not re-evaluated for them.
        clock.tick()
        clock.tick()
        XCTAssertEqual(clock.ticks, 2)
        if clock.frame == before {
            XCTAssertEqual(published, 0, "same frame → no publish")
        } else {
            XCTAssertEqual(published, 1, "the window rolled over once at most")
        }
        _ = sink
        clock.detach()
    }

    func testTheTimerRunsInTheDefaultModeSoScrollTrackingPausesIt() {
        // The timer must not be in `.common`: a `.default`-mode timer does not fire
        // while UIKit tracks a scroll (UITrackingRunLoopMode), which is the "paused
        // during scroll" rule. Pinned by reading the source: no other way to ask a
        // Timer its modes.
        let source = try? String(contentsOfFile: WorkingSpinnerTests.sourcePath, encoding: .utf8)
        guard let source else { return XCTFail("WorkingSpinner.swift not found at \(WorkingSpinnerTests.sourcePath)") }
        XCTAssertTrue(source.contains("RunLoop.main.add(t, forMode: .default)"))
        XCTAssertFalse(source.contains("forMode: .common"))
    }

    // MARK: alignment pins

    func testTheCellIsTheUnreadDotsFootprintAndTheGlyphTheTimeLabelsSize() {
        // The spinner replaces the dot at the right end of the preview line: the
        // same square, so its right edge is the time label's right edge above it.
        XCTAssertEqual(WorkingSpinnerFrames.cell, Theme.unreadDot)
        XCTAssertEqual(WorkingSpinnerFrames.cell, 10)
        // The glyph is the row's tiny text (the time label is `Theme.small` 15 over
        // a 13 glyph: the dots read lighter than the time, like the old dot did).
        XCTAssertEqual(WorkingSpinnerFrames.glyphSize, Theme.tiny)
        XCTAssertLessThan(WorkingSpinnerFrames.glyphSize, Theme.small)
    }

    func testTheRowStatusShowsOneThingInThisOrder() {
        // Agent rows: the spinner replaces the plain unread dot while the agent works.
        XCTAssertEqual(RowStatus.of(streaming: true, unread: true), .working)
        XCTAssertEqual(RowStatus.of(streaming: true, unread: false), .working)
        XCTAssertEqual(RowStatus.of(streaming: false, unread: true), .unread)
        XCTAssertEqual(RowStatus.of(streaming: false, unread: false), .quiet)
    }

    func testAPeopleRowsCountPillWinsTheSlotOverTheSpinner() throws {
        // Information over animation: `PeopleRow` shows the unread count pill ahead
        // of the spinner; the spinner replaces only the plain dot. Pinned on the
        // source's branch order (the row is a view, not a pure function).
        let path = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Arbos/UI/HomeView.swift").path
        let source = try String(contentsOfFile: path, encoding: .utf8)
        guard let peopleRow = source.range(of: "struct PeopleRow: View") else { return XCTFail("PeopleRow not found") }
        let body = source[peopleRow.lowerBound...]
        let pill = body.range(of: "if count > 0 {")
        let spinner = body.range(of: "} else if streaming {")
        let dot = body.range(of: "} else if unread {")
        XCTAssertNotNil(pill); XCTAssertNotNil(spinner); XCTAssertNotNil(dot)
        if let pill, let spinner, let dot {
            XCTAssertLessThan(pill.lowerBound, spinner.lowerBound, "the count pill is tested first")
            XCTAssertLessThan(spinner.lowerBound, dot.lowerBound, "the spinner beats the plain dot")
        }
    }

    func testTheSpinnerCellLaysOutAtTheDotsSize() {
        let host = UIHostingController(rootView: WorkingSpinner().environment(\.theme, Theme.dark))
        host.view.frame = CGRect(x: 0, y: 0, width: 100, height: 100)
        let size = host.sizeThatFits(in: CGSize(width: 100, height: 100))
        XCTAssertEqual(size.width, WorkingSpinnerFrames.cell, accuracy: 0.5)
        XCTAssertEqual(size.height, WorkingSpinnerFrames.cell, accuracy: 0.5)
    }

    private static var sourcePath: String {
        // …/apps/ios/Arbos/ArbosTests/WorkingSpinnerTests.swift → …/Arbos/Arbos/UI/WorkingSpinner.swift
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Arbos/UI/WorkingSpinner.swift")
            .path
    }
}
