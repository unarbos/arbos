import XCTest
@testable import Arbos

/// Jacob, 2026-10-26 ("nuke vampire"): a generation that times out ends in a clean state — one
/// short sentence, the raw words only behind Details, Try again re-sends — and the sheet shows
/// progress with the seconds while it runs. The Mac twin is `avatar_picker::me_picker_tests`.
@MainActor
final class AvatarGenerateErrorStateTests: XCTestCase {
    func testFailureIsTheSharedUserErrorUnderTheGenerateVerb() {
        let relay = RelayError(status: 504, message: "Couldn't generate the image. The server took too long.", type: "avatar_timeout",
                               detail: "gpt-image-1-mini: operation timed out")
        let e1 = AvatarSourceState.failure(relay)
        XCTAssertEqual(e1.sentence, "Couldn't generate the image. The server took too long.")
        XCTAssertEqual(e1.raw, "HTTP 504 avatar_timeout — Couldn't generate the image. The server took too long. — gpt-image-1-mini: operation timed out")
        XCTAssertEqual(e1.action, .tryAgain)

        let refused = RelayError(status: 422, message: "That prompt was refused — try a different one.", type: "avatar_refused", detail: "safety system")
        XCTAssertEqual(AvatarSourceState.failure(refused).sentence, "Couldn't generate the image. That prompt was refused — try different words.")
        XCTAssertEqual(AvatarSourceState.failure(refused).action, UserError.Action.none)

        let timedOut = NSError(domain: NSURLErrorDomain, code: NSURLErrorTimedOut, userInfo: [NSLocalizedDescriptionKey: "The request timed out."])
        let (s2, d2) = AvatarSourceState.failureWords(timedOut)
        XCTAssertEqual(s2, "Couldn't generate the image. The server took too long.")
        XCTAssertTrue(d2?.contains("The request timed out.") == true, "the raw words live in the detail, not the sentence")

        let offline = NSError(domain: NSURLErrorDomain, code: NSURLErrorNotConnectedToInternet, userInfo: nil)
        XCTAssertEqual(AvatarSourceState.failureWords(offline).0, "Couldn't generate the image. You're offline.")
    }

    func testRunShowsProgressThenACleanErrorAndTryAgainClearsIt() async throws {
        let state = AvatarSourceState()
        XCTAssertNil(state.generatingFor(Date()))
        state.run {
            try await Task.sleep(nanoseconds: 150_000_000)
            throw RelayError(status: 504, message: "Couldn't generate the image. The server took too long.", type: "avatar_timeout", detail: "stub stalled")
        }
        // Progress: running, the clock started, the seconds count from 0.
        XCTAssertTrue(state.generating)
        XCTAssertNotNil(state.startedAt)
        XCTAssertEqual(state.generatingFor(state.startedAt!.addingTimeInterval(7.4)), 7)
        XCTAssertNil(state.error)
        try await Task.sleep(nanoseconds: 400_000_000)
        // Clean error state: sentence, the raw words hidden until asked, nothing to set.
        XCTAssertFalse(state.generating)
        XCTAssertNil(state.startedAt)
        XCTAssertEqual(state.errorText, "Couldn't generate the image. The server took too long.")
        XCTAssertTrue(state.error?.raw.contains("stub stalled") == true)
        XCTAssertFalse(state.detailsOpen)
        XCTAssertNil(state.generatedKey)
        XCTAssertNil(state.draftPicture(for: .generate))
        state.detailsOpen = true
        // Try again: the error clears, it runs again, and this time the picture lands.
        state.run { "avatar-gen-2.jpg" }
        XCTAssertTrue(state.generating)
        XCTAssertNil(state.error)
        XCTAssertFalse(state.detailsOpen)
        try await Task.sleep(nanoseconds: 200_000_000)
        XCTAssertFalse(state.generating)
        XCTAssertEqual(state.generatedKey, "avatar-gen-2.jpg")
        XCTAssertNil(state.errorText)
    }

    func testTimeoutsLineUpSoTheRelayAnswersFirst() {
        // Relay: 40 s per attempt, 75 s overall (#1936); the app waits longer than that.
        XCTAssertEqual(RelayAPI.avatarGenerateTimeout, 90)
        XCTAssertGreaterThan(RelayAPI.avatarGenerateTimeout, 75)
    }
}
