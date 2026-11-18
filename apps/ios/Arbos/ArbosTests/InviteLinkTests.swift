import XCTest
@testable import Arbos

/// Arbonites (tester report, 2026-11-16): "i can delete yours as a non admin". The relay now
/// lets only a link's maker, the owner or an admin revoke it; the app hides the control for
/// everyone else.
final class InviteLinkTests: XCTestCase {
    private func link(by maker: String?) -> ServerInviteLink {
        var raw: [String: Any] = ["id": "i1", "code": "abc", "url": "https://arbos.life/g/abc"]
        raw["created_by"] = maker
        return ServerInviteLink.parse(raw)!
    }

    func testOnlyTheMakerOrWhoeverManagesTheGroupMayRevoke() {
        let theirs = link(by: "u-mira")
        XCTAssertTrue(theirs.revocable(by: "u-mira", managesGroup: false))
        XCTAssertFalse(theirs.revocable(by: "u-sam", managesGroup: false))
        XCTAssertTrue(theirs.revocable(by: "u-sam", managesGroup: true))
        // No maker on the wire and no signed-in user never match each other.
        XCTAssertFalse(link(by: nil).revocable(by: nil, managesGroup: false))
    }
}
