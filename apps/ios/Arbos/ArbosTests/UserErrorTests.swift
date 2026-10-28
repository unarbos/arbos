import XCTest
@testable import Arbos

/// Jacob, 2026-10-26 ([feedback error-messages-ios-2026-10-26]): every failure the user sees
/// is one `UserError` — a verb and a class — read as "Couldn't <verb>. <why>", with Try again
/// (or Sign in) only where it helps and the raw words behind Details / Copy, never in the
/// sentence. The Mac twin is `sync::user_error::tests`.
final class UserErrorTests: XCTestCase {
    private func relay(_ status: Int, _ type: String? = nil, _ message: String = "", detail: String? = nil) -> RelayError {
        RelayError(status: status, message: message, type: type, detail: detail)
    }

    private func url(_ code: Int, _ text: String) -> NSError {
        NSError(domain: NSURLErrorDomain, code: code, userInfo: [NSLocalizedDescriptionKey: text])
    }

    func testTheTenClassesMakeTheAgreedSentences() {
        let timeout = UserError.classify("generate the image", url(NSURLErrorTimedOut, "The request timed out."))
        XCTAssertEqual(timeout.class, .timeout)
        XCTAssertEqual(timeout.sentence, "Couldn't generate the image. The server took too long.")
        XCTAssertEqual(timeout.action, .tryAgain)
        XCTAssertEqual(timeout.details.first, "Timed out")
        XCTAssertTrue(timeout.copyText.contains("The request timed out."))

        let offline = UserError.classify("send your message", url(NSURLErrorNotConnectedToInternet, "The Internet connection appears to be offline."))
        XCTAssertEqual(offline.sentence, "Couldn't send your message. You're offline.")
        XCTAssertEqual(offline.details.first, "No network route")

        let refused = UserError.classify("save the secret", relay(409, "exists", "A secret with that name already exists"))
        XCTAssertEqual(refused.sentence, "Couldn't save the secret. A secret with that name already exists.")
        XCTAssertEqual(refused.action, UserError.Action.none)

        let notFound = UserError.classify("open the file", relay(404))
        XCTAssertEqual(notFound.sentence, "Couldn't open the file. It isn't there any more.")
        XCTAssertEqual(notFound.action, UserError.Action.none)

        let server = UserError.classify("save the secret", relay(502, nil, "Bad Gateway"))
        XCTAssertEqual(server.sentence, "Couldn't save the secret. The server had a problem.")
        XCTAssertEqual(server.details.first, "HTTP 502 from the relay")

        let limited = UserError.classify("send your message", relay(429))
        XCTAssertEqual(limited.class, .rateLimited)
        XCTAssertEqual(limited.action, .tryAgain)

        let content = UserError.classify("generate the image", relay(422, "prompt_refused", "That prompt was refused — try a different one."))
        XCTAssertEqual(content.sentence, "Couldn't generate the image. That prompt was refused — try different words.")
        XCTAssertEqual(content.action, UserError.Action.none)

        let large = UserError.classify("upload the picture", relay(413))
        XCTAssertEqual(large.sentence, "Couldn't upload the picture. That file is too large (25 MB max).")
        XCTAssertEqual(large.action, UserError.Action.none)

        let auth = UserError.classify("send your message", relay(401))
        XCTAssertEqual(auth.sentence, "Couldn't send your message. You were signed out.")
        XCTAssertEqual(auth.action, .signIn)

        let unknown = UserError.fromText("save the secret", "boom")
        XCTAssertEqual(unknown.sentence, "Couldn't save the secret. Something went wrong.")
        XCTAssertEqual(unknown.action, .tryAgain)
    }

    /// A permission 403 (`admin_only`, [feedback group-settings-permissions-2026-10-27]) reads as
    /// the relay's sentence, not "You were signed out"; a session 403 still signs out.
    func testAPermission403ReadsAsTheRelaysSentence() {
        let sentence = "Only the group's owner and admins can change its name, picture and description."
        let denied = UserError.classify("update the group", relay(403, "admin_only", sentence))
        XCTAssertEqual(denied.sentence, "Couldn't update the group. \(sentence)")
        XCTAssertEqual(denied.action, UserError.Action.none)
        let session = UserError.classify("send your message", relay(403, "forbidden", "forbidden"))
        XCTAssertEqual(session.class, .authExpired)
    }

    func testARelayTimeoutKeepsTheMeasuredSecondsAndTheDetailForDetails() {
        let e = UserError.classify("generate the image",
                                   relay(504, "avatar_timeout", "Couldn't generate the image. The server took too long.",
                                         detail: "gpt-image-1-mini: operation timed out after 40 s (attempt 2 of 2)"))
        XCTAssertEqual(e.class, .timeout)
        XCTAssertEqual(e.details.first, "Timed out after 40 s")
        XCTAssertTrue(e.raw.contains("gpt-image-1-mini"))
        XCTAssertEqual(e.details.count, 2, "what + where (no call in a relay body)")
    }

    func testSentencesNeverCarryURLsVerbsIdsOrChains() {
        let raw = NSError(domain: NSURLErrorDomain, code: NSURLErrorCannotConnectToHost,
                          userInfo: [NSLocalizedDescriptionKey: "Could not connect to the server. POST https://relay.arbos.life/projects/01M5N1WSR91G8CJKNYXKJESK7X/avatar/generate"])
        for verb in ["generate the image", "send your message", ""] {
            let s = UserError.classify(verb, raw).sentence
            XCTAssertFalse(s.contains("://"), s)
            XCTAssertFalse(s.contains("POST"), s)
            XCTAssertFalse(s.contains("01M5N1"), s)
        }
        XCTAssertEqual(UserError.classify("", raw).sentence, "Arbos can't reach the server right now.")
        XCTAssertEqual(UserError.shortenIds("/projects/01M5N1WSR91G8CJKNYXKJESK7X/x"), "/projects/01M5N1…ESK7X/x")
    }

    func testWordsAloneAreClassified() {
        XCTAssertEqual(UserError.fromText("x", "HTTP 503 Service Unavailable").class, .server)
        XCTAssertEqual(UserError.fromText("x", "The relay did not answer in time").class, .timeout)
        XCTAssertEqual(UserError.fromText("x", "token expired").class, .authExpired)
        XCTAssertEqual(UserError.fromText("x", "image too large").class, .upload(.tooLarge))
        XCTAssertEqual(UserError.refused("add members", "Pick someone first.").sentence, "Couldn't add members. Pick someone first.")
    }

    @MainActor
    func testAFailureIsAnErrorToastAndAClaimedVerbStaysWithItsSurface() {
        let model = AppModel()
        model.notify("Forwarded to Studio")
        model.fail("download report-q3.pdf", NSError(domain: NSURLErrorDomain, code: NSURLErrorTimedOut), retry: {})
        XCTAssertEqual(model.toasts.count, 2)
        XCTAssertNil(model.toasts[0].error)
        let toast = model.toasts[1]
        XCTAssertEqual(toast.error?.sentence, "Couldn't download report-q3.pdf. The server took too long.")
        XCTAssertTrue(toast.hasRetry)
        XCTAssertEqual(toast.ttl, Toast.errorTTL)
        model.setToastDetails(toast.id, open: true)
        XCTAssertTrue(model.toasts[1].detailsOpen && model.toasts[1].held, "Details holds the toast")
        model.retryToast(toast.id)
        XCTAssertEqual(model.toasts.count, 1, "Try again drops the toast")

        // A sheet with a form up claims its verbs: the failure draws there, not as a toast.
        model.failureClaim = FailureClaim(surface: "secrets", verbs: ["save the secret"])
        model.fail("save the secret", RelayError(status: 502, message: "Bad Gateway"))
        XCTAssertEqual(model.claimedFailure?.sentence, "Couldn't save the secret. The server had a problem.")
        XCTAssertEqual(model.toasts.count, 1)
        model.failureClaim = nil
        XCTAssertNil(model.claimedFailure, "releasing the claim drops its note")

        // The legacy `notice` setter is an info toast, never red.
        model.notice = Notice(text: "Token replaced.")
        XCTAssertEqual(model.toasts.last?.text, "Token replaced.")
        XCTAssertNil(model.toasts.last?.error)
    }
}
