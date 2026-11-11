import XCTest
@testable import Arbos

/// Pure helpers for Settings → Agent access link
/// ([feedback agent-access-link-ios-2026-11-10] is guarded by `AgentAccessLinkUITests`;
/// these cases lock the Mac-matched naming and "never opened" rules the UI relies on).
///
/// No fixture here carries a real bearer or open secret — only fake ids and labels.
final class AgentLinksStoreTests: XCTestCase {
    private let now: Int64 = 1_762_200_000_000

    private func link(_ id: String, _ label: String, scope: AgentLinkScope = .read,
                      live: Bool = true, opened: Int64? = nil, seen: Int64? = nil) -> AgentAccessLink {
        AgentAccessLink(
            id: id, origin: "link", label: label, scope: scope,
            createdAt: now - 2 * 24 * 3_600_000, expiresAt: now + 5 * 24 * 3_600_000,
            openedAt: opened, revokedAt: live ? nil : now - 3_600_000,
            lastSeenAt: seen, live: live, note: nil
        )
    }

    func testABlankNameIsNumberedPastTheLiveOnes() {
        XCTAssertEqual(AgentAccessLink.nextLabel([]), "Agent link")
        let first = link("a", "Agent link")
        XCTAssertEqual(AgentAccessLink.nextLabel([first]), "Agent link 2")
        XCTAssertEqual(AgentAccessLink.labelFor(typed: "  ", items: [first]), "Agent link 2")
        XCTAssertEqual(AgentAccessLink.labelFor(typed: "Jacob's Claude", items: [first]), "Jacob's Claude")
        let long = String(repeating: "x", count: AgentAccessLink.labelMax + 40)
        XCTAssertEqual(AgentAccessLink.labelFor(typed: long, items: []).count, AgentAccessLink.labelMax)
    }

    func testUsedLabelStaysNeverOpenedUntilTheUrlIsFetched() {
        XCTAssertEqual(link("a", "A").usedLabel(now: now), "never opened")
        XCTAssertEqual(link("a", "A", opened: nil, seen: now).usedLabel(now: now), "never opened")
        XCTAssertEqual(
            link("a", "A", opened: now - 2 * 3_600_000, seen: now - 2 * 3_600_000).usedLabel(now: now),
            "used 2 h ago"
        )
    }

    func testActiveListsOnlyLiveLinksNewestFirst() {
        var dead = link("b", "Old")
        dead.live = false
        let list = AgentAccessLink.active([
            link("older", "Older", opened: now - 3_600_000),
            dead,
            AgentAccessLink(id: "newer", origin: "link", label: "Newer", scope: .reply,
                            createdAt: now - 3_600_000, expiresAt: now + 3_600_000,
                            openedAt: nil, revokedAt: nil, lastSeenAt: nil, live: true, note: nil),
        ])
        XCTAssertEqual(list.map(\.id), ["newer", "older"])
    }

    func testShortUrlShowsHostAndTailNeverABearer() {
        // Long open-secret: host + last 6 only (Mac `short_url`).
        XCTAssertEqual(
            AgentLinkReveal.shortUrl("https://relay.arbos.life/agent-link/open/Kq2vB7xZ9kT2vQ"),
            "relay.arbos.life/…/9kT2vQ"
        )
        // Fixture tail `fxOpen1` is 7 characters — still capped at 6, never the full open secret.
        let shown = AgentLinkReveal.shortUrl("https://relay.arbos.life/agent-link/open/fxOpen1")
        XCTAssertEqual(shown, "relay.arbos.life/…/xOpen1")
        // Short tails stay whole.
        XCTAssertEqual(
            AgentLinkReveal.shortUrl("http://127.0.0.1:7700/agent-link/open/abc"),
            "127.0.0.1:7700/…/abc"
        )
        XCTAssertFalse(shown.contains("arbl_"))
        XCTAssertFalse(AgentAccessLink.fixture(now: now).contains { $0.label.contains("arbl_") })
    }

    func testSignedInDevicesDropReaderTokens() {
        let devices = AuthDevice.fixture(now: now)
        XCTAssertEqual(AuthDevice.agentTokens(devices).map(\.id), ["dev-fx-grokbot"])
        XCTAssertFalse(AuthDevice.signedIn(devices).contains { $0.isAgentToken })
        XCTAssertEqual(AuthDevice.signedIn(devices).count, 2)
    }
}
