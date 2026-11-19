import XCTest
@testable import Arbos

/// [feedback iphone-first-login-recovery-2026-11-18] A returning account whose only agent is
/// Home must go straight to its refreshed registry, not through first-time provisioning.
@MainActor
final class FirstLoginTests: XCTestCase {
    func testExistingHomeAgentSkipsFirstLogin() {
        let model = AppModel()
        model.isFixture = true
        defer { model.unpair(forget: false) }
        model.projects = [Project(
            id: "home", name: FirstLogin.agentName, icon: "H", color: "#000000", state: .awake,
            stateSince: nil, stateDetail: nil, model: "", effort: .low, spendTodayUsd: 0,
            spendCapUsd: 0, avatar: nil, avatarColor: nil
        )]
        let now = Date()
        model.firstLogin = FirstLogin(startedAt: now, stageSince: now, registrySeen: true)

        model.firstLoginTick()

        XCTAssertNil(model.firstLogin, "an existing Home agent proves this is not a fresh account")
    }

    func testFirstLoginFailureOffersSignOut() throws {
        let file = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Arbos/UI/FirstLoginView.swift")
        let source = try String(contentsOf: file, encoding: .utf8)
        XCTAssertTrue(source.contains("model.signOut()"), "the blocking failure must have a way back to sign-in")
        XCTAssertTrue(source.contains("first-login-sign-out"), "the escape action must be reachable in UI tests")
    }
}
