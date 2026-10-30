import XCTest
@testable import Arbos

/// Settings → Devices lists only the devices I am still signed in on
/// ([feedback devices-hide-revoked-ios-2026-10-28]; the Mac twin is
/// `devices::tests::signed_out_devices_are_not_listed_or_counted_and_this_mac_stays_first`).
/// A row the relay marks `revoked_at` ended its session (signed out here or elsewhere) and
/// must neither render nor count; this iPhone comes first.
final class DevicesVisibleTests: XCTestCase {
    private let now: Int64 = 1_793_200_000_000

    private func device(_ id: String, _ name: String, current: Bool = false, revoked: Bool = false) -> AuthDevice {
        AuthDevice(
            id: id, name: name, kind: "github",
            createdAt: now - 5 * 24 * 3_600_000, lastSeenAt: now - 60_000,
            revokedAt: revoked ? now - 3_600_000 : nil, current: current
        )
    }

    func testRevokedRowsAreHiddenAndThisIphoneIsFirst() {
        // What the relay used to hand back for a tester who signed four Macs out.
        let list = [
            device("p1", "iPhone"),
            device("m1", "Mac (Mac)", revoked: true),
            device("m2", "Mac (Mac)", revoked: true),
            device("me", "iPhone", current: true),
            device("m3", "Mac (Mac)", revoked: true),
            device("mac", "Mac (Mac)"),
        ]
        let visible = AuthDevice.visible(list)
        XCTAssertEqual(visible.map(\.id), ["me", "p1", "mac"], "live rows only, this iPhone first, then the relay's order")
        XCTAssertTrue(visible.allSatisfy(\.isLive))
        XCTAssertEqual(visible.count, 3, "the Settings row counts the same rows the page draws")
        XCTAssertTrue(AuthDevice.visible([device("m1", "Mac (Mac)", revoked: true)]).isEmpty)
    }

    func testParseReadsRevokedAtAndTreatsMissingAsLive() {
        let revoked = AuthDevice.parse(["id": "a", "name": "Mac", "kind": "github", "created_at": 1, "revoked_at": 2])
        XCTAssertEqual(revoked?.revokedAt, 2)
        XCTAssertEqual(revoked?.isLive, false)
        let live = AuthDevice.parse(["id": "b", "name": "iPhone", "kind": "github", "created_at": 1, "revoked_at": NSNull()])
        XCTAssertNil(live?.revokedAt)
        XCTAssertEqual(live?.isLive, true)
        XCTAssertEqual(AuthDevice.parse(["id": "c", "created_at": 1])?.isLive, true, "an older relay without the field means live")
    }

    func testTheFixtureHasNoRevokedRows() {
        let fixture = AuthDevice.fixture(now: now)
        XCTAssertEqual(AuthDevice.visible(fixture).count, fixture.count)
        XCTAssertTrue(AuthDevice.visible(fixture).first?.current ?? false)
    }
}
