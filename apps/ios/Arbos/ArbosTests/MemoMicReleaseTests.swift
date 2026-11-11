import XCTest
@testable import Arbos

/// Composer mic release decisions ([feedback app-01M69J94RZRMZXKGMC6XDGWBFH]).
/// A tap that never sees `onChanged` must still start recording; the rim must not
/// steal the hit; permission goes through `CallManager.micGranted`.
final class MemoMicReleaseTests: XCTestCase {
    private let cancelDistance: CGFloat = 56

    func testEndedOnlyTapTogglesRecordingOn() {
        // Pure tap: onEnded with no onChanged → holdActive stays false.
        let action = MemoMicRelease.decide(
            holdActive: false,
            recording: false,
            ignoreRelease: false,
            cancelArmed: false,
            translationWidth: 0,
            heldSeconds: 0,
            cancelDistance: cancelDistance)
        XCTAssertEqual(action, .toggleTap)
    }

    func testEndedOnlyTapWhileRecordingFinishes() {
        let action = MemoMicRelease.decide(
            holdActive: false,
            recording: true,
            ignoreRelease: false,
            cancelArmed: false,
            translationWidth: 0,
            heldSeconds: 0,
            cancelDistance: cancelDistance)
        XCTAssertEqual(action, .toggleTap)
    }

    func testHoldPastThresholdFinishes() {
        let action = MemoMicRelease.decide(
            holdActive: true,
            recording: true,
            ignoreRelease: false,
            cancelArmed: false,
            translationWidth: 0,
            heldSeconds: 0.3,
            cancelDistance: cancelDistance)
        XCTAssertEqual(action, .finishHold)
    }

    func testHoldPastThresholdWithoutPermissionWaits() {
        let action = MemoMicRelease.decide(
            holdActive: true,
            recording: false,
            ignoreRelease: false,
            cancelArmed: false,
            translationWidth: 0,
            heldSeconds: 0.3,
            cancelDistance: cancelDistance)
        XCTAssertEqual(action, .waitForPermission)
    }

    func testShortTrackedTapKeepsRecordingOn() {
        let action = MemoMicRelease.decide(
            holdActive: true,
            recording: true,
            ignoreRelease: false,
            cancelArmed: false,
            translationWidth: 0,
            heldSeconds: 0.1,
            cancelDistance: cancelDistance)
        XCTAssertEqual(action, .keepRecording)
    }

    func testSlideLeftCancels() {
        let action = MemoMicRelease.decide(
            holdActive: true,
            recording: true,
            ignoreRelease: false,
            cancelArmed: false,
            translationWidth: -cancelDistance - 1,
            heldSeconds: 0.4,
            cancelDistance: cancelDistance)
        XCTAssertEqual(action, .cancel)
    }

    func testCancelArmedCancelsEvenWithoutSlide() {
        let action = MemoMicRelease.decide(
            holdActive: true,
            recording: true,
            ignoreRelease: false,
            cancelArmed: true,
            translationWidth: 0,
            heldSeconds: 0.4,
            cancelDistance: cancelDistance)
        XCTAssertEqual(action, .cancel)
    }

    func testIgnoreReleaseSkipsFinish() {
        let action = MemoMicRelease.decide(
            holdActive: true,
            recording: false,
            ignoreRelease: true,
            cancelArmed: false,
            translationWidth: 0,
            heldSeconds: 0.4,
            cancelDistance: cancelDistance)
        XCTAssertEqual(action, .ignore)
    }

    // MARK: Drift guards — the two other halves of the same bug

    private var sourceRoot: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Arbos")
    }

    func testRecorderAsksPermissionThroughCallManager() throws {
        let url = sourceRoot.appendingPathComponent("UI/Recorder.swift")
        try XCTSkipIf(!FileManager.default.fileExists(atPath: url.path),
                      "sources not beside the test bundle (CI without the tree)")
        let text = try String(contentsOf: url, encoding: .utf8)
        XCTAssertTrue(text.contains("CallManager.micGranted()"),
                      "MemoRecorder.start must use CallManager.micGranted — the deprecated AVAudioSession.requestRecordPermission callback can fail to fire")
        // Code only: a comment may still name the deprecated API.
        let code = text
            .components(separatedBy: "\n")
            .map { line -> String in
                guard let range = line.range(of: "//") else { return line }
                return String(line[..<range.lowerBound])
            }
            .joined(separator: "\n")
        XCTAssertFalse(
            code.contains("requestRecordPermission"),
            "MemoRecorder must not call AVAudioSession.requestRecordPermission")
    }

    func testComposerRimDoesNotStealMicHits() throws {
        let url = sourceRoot.appendingPathComponent("UI/ChatView.swift")
        try XCTSkipIf(!FileManager.default.fileExists(atPath: url.path),
                      "sources not beside the test bundle (CI without the tree)")
        let text = try String(contentsOf: url, encoding: .utf8)
        XCTAssertTrue(
            text.contains("ComposerRim(shape: fieldShape, tint: rimTint).allowsHitTesting(false)"),
            "ComposerRim must be paint-only so it cannot swallow taps on the mic")
    }
}
