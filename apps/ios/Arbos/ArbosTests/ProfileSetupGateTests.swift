import XCTest
@testable import Arbos

// When the relay says the account was created, and the gap to the next sign-in. File-scope
// rather than static members: `Self` cannot be named from a stored property's initializer.
private let creationMs: Int64 = 1_700_000_000_000
private let thirtyDaysMs: Int64 = 30 * 24 * 60 * 60 * 1000

/// Who sees the name/handle setup form, and whose profile it may write
/// ([feedback ios-signin-onboarding-repeat-2026-10-31]).
///
/// The form used to appear after every GitHub sign-in, so an existing account signing in on
/// a second phone was asked to choose a name and handle it already had — and Continue sent
/// whatever was typed to `PATCH /me`, which replaces the name and handle on the account for
/// every device. The decision now comes from the relay's own stamps on the account
/// (`created_at` / `last_login_at`), which a new device cannot get wrong because it never
/// consults the device.
final class ProfileSetupGateTests: XCTestCase {
    /// A first-ever sign-in: `upsert_github_user` inserts both stamps from one clock.
    private let brandNew = AuthUser(id: "gh_909", login: "affinity", name: "Affinity",
                                    createdAt: creationMs, lastLoginAt: creationMs)
    /// The same account thirty days later, signing in on a new phone: only `last_login_at` moved.
    private let returning = AuthUser(id: "gh_909", login: "affinity", name: "Affinity",
                                     createdAt: creationMs,
                                     lastLoginAt: creationMs + thirtyDaysMs)

    func testOnlyTheSignInThatCreatedTheAccountIsAskedToSetUpAProfile() {
        XCTAssertTrue(AppModel.profileSetupApplies(to: brandNew), "a genuinely new account still gets onboarding")
        XCTAssertFalse(AppModel.profileSetupApplies(to: returning), "an existing account on a new device does not")
    }

    func testASecondSignInOneMillisecondLaterAlreadyCounts() {
        var again = brandNew
        again.lastLoginAt = creationMs + 1
        XCTAssertFalse(AppModel.profileSetupApplies(to: again))
    }

    /// Both ways of learning nothing. Neither may raise the form, because the cost of
    /// guessing "new" is an overwritten profile and the cost of guessing "existing" is a
    /// trip to Settings.
    func testAnUnansweredAccountIsNeverTreatedAsNew() {
        XCTAssertFalse(AppModel.profileSetupApplies(to: nil), "no user at all")
        XCTAssertFalse(AppModel.profileSetupApplies(to: AuthUser(id: "gh_909", login: "affinity")),
                       "a payload with neither stamp")
        XCTAssertTrue(AuthUser(id: "gh_909", login: "affinity").accountPredatesThisSignIn)
    }

    /// The stamps have to survive the decode, not just exist on the relay. This reads the
    /// relay's own captured `POST /auth/token` body rather than a hand-written one.
    func testTheRelaysCapturedTokenBodyCarriesBothStamps() throws {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()  // ArbosTests
            .deletingLastPathComponent()  // Arbos
            .deletingLastPathComponent()  // ios
            .deletingLastPathComponent()  // apps
            .deletingLastPathComponent()  // repo root
            .appendingPathComponent("crates/arbos-relay/tests/fixtures/api/token_response.json")
        let body = try JSONSerialization.jsonObject(with: try Data(contentsOf: url))
        let (_, who) = try RelayAPI.parseTokens(body, relayUrl: "https://relay.arbos.life")
        let parsed = try XCTUnwrap(who)
        XCTAssertEqual(parsed.createdAt, 1_700_000_000_000)
        XCTAssertEqual(parsed.lastLoginAt, 1_700_000_000_000)
        XCTAssertTrue(AppModel.profileSetupApplies(to: parsed),
                      "that fixture is a first sign-in, so it is the case that still asks")
    }

    /// A cache written before this fix has no stamps. It must decode, and it must land on
    /// "existing" rather than crash or re-ask.
    func testAProfileCachedByAnOlderBuildStillDecodes() throws {
        let legacy = Data(#"{"id":"gh_909","login":"affinity","name":"Affinity"}"#.utf8)
        let who = try JSONDecoder().decode(AuthUser.self, from: legacy)
        XCTAssertEqual(who.login, "affinity")
        XCTAssertNil(who.createdAt)
        XCTAssertFalse(AppModel.profileSetupApplies(to: who))
    }
}
