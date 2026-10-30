import XCTest
@testable import Arbos

/// Home's people rows name the sender only where the row's header does not
/// ([feedback doubled-sender-prefix-ios-2026-10-30], Jacob 2026-10-30): his shot read
/// "@DaWizzaa" in the row's header and "@DaWizzaa: …" in its preview line, the same name
/// twice. A group row is headed by the group, so it still names whoever wrote.
final class HomePreviewWhoTests: XCTestCase {
    func testAOneToOneRowDoesNotRepeatThePersonItsHeaderNames() {
        XCTAssertEqual(HomeView.previewWho(group: false, mine: false, sender: "user:gh_461", handle: "dawizzaa"), "",
                       "the 1:1 row is already titled with this person")
        XCTAssertEqual(HomeView.previewWho(group: true, mine: false, sender: "user:gh_461", handle: "dawizzaa"), "@dawizzaa",
                       "a group row is titled with the group, so it names the sender")
    }

    func testMyOwnLineStaysMineAndAnAgentKeepsItsName() {
        XCTAssertEqual(HomeView.previewWho(group: false, mine: true, sender: "user:gh_me", handle: "jacob"), "You")
        XCTAssertEqual(HomeView.previewWho(group: true, mine: true, sender: "user:gh_me", handle: "jacob"), "You")
        XCTAssertEqual(HomeView.previewWho(group: false, mine: false, sender: "agent:01COMMS", handle: "comms"), "@comms",
                       "an agent in a 1:1 chat is not the person the row names")
        XCTAssertEqual(HomeView.previewWho(group: false, mine: false, sender: "system", handle: nil), "",
                       "no handle, no label")
    }
}
