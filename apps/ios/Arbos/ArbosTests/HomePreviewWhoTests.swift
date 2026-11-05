import XCTest
@testable import Arbos

/// A people row names the sender in its preview only where the row's own header does
/// not ([feedback doubled-sender-prefix-2026-10-30], reported again by Scout
/// 2026-11-05): the shot read "Affinity @Z" in a 1:1 row's header and "@Z: hahahaha"
/// in its preview line, the same person twice. A group row is headed by the group and
/// carries no handle, so it still names whoever wrote.
final class HomePreviewWhoTests: XCTestCase {
    func testAOneToOneRowDoesNotRepeatThePersonItsHeaderNames() {
        XCTAssertEqual(HomeView.previewWho(group: false, mine: false, sender: "user:gh_461", handle: "Z"), "",
                       "the 1:1 row header is already \"Affinity @Z\"")
        XCTAssertEqual(HomeView.previewWho(group: true, mine: false, sender: "user:gh_461", handle: "Z"), "@Z",
                       "a group row is headed by the group, so it names the sender")
    }

    func testMyOwnLineStaysMineAndAnAgentKeepsItsName() {
        XCTAssertEqual(HomeView.previewWho(group: false, mine: true, sender: "user:gh_me", handle: "jacob"), "You")
        XCTAssertEqual(HomeView.previewWho(group: true, mine: true, sender: "user:gh_me", handle: "jacob"), "You")
        XCTAssertEqual(HomeView.previewWho(group: false, mine: false, sender: "agent:01COMMS", handle: "comms"), "@comms",
                       "an agent in a 1:1 chat is not the person the header names")
        XCTAssertEqual(HomeView.previewWho(group: false, mine: false, sender: "system", handle: nil), "",
                       "no handle, no label")
    }
}
