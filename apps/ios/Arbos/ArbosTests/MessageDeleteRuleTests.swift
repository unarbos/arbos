import XCTest
@testable import Arbos

/// Who may delete a message for everyone — mirrors `arbos-relay/src/moderation.rs` and the
/// Mac's `model::may_delete_message`, so the long-press menu never offers a Delete the relay
/// refuses.
final class MessageDeleteRuleTests: XCTestCase {
    private let agents = ["p_bob": "u_bob"]

    func testMemberDeletesOwnAndOwnAgentsOnly() {
        XCTAssertTrue(MessageDeleteRule.mayDelete(sender: "user:u_bob", meId: "u_bob", moderates: false, agentOwnerIds: agents))
        XCTAssertFalse(MessageDeleteRule.mayDelete(sender: "user:u_alice", meId: "u_bob", moderates: false, agentOwnerIds: agents))
        XCTAssertTrue(MessageDeleteRule.mayDelete(sender: "agent:p_bob", meId: "u_bob", moderates: false, agentOwnerIds: agents))
        XCTAssertFalse(MessageDeleteRule.mayDelete(sender: "agent:p_bob", meId: "u_carol", moderates: false, agentOwnerIds: agents))
        XCTAssertFalse(MessageDeleteRule.mayDelete(sender: "system", meId: "u_bob", moderates: false, agentOwnerIds: agents))
    }

    func testOwnerModeratesEverything() {
        for sender in ["user:u_alice", "agent:p_bob", "system"] {
            XCTAssertTrue(MessageDeleteRule.mayDelete(sender: sender, meId: "u_owner", moderates: true, agentOwnerIds: agents), sender)
        }
    }

    func testMenuHidesDeleteWhenNotAllowed() {
        XCTAssertEqual(MessageMenu.items(forMessage: true, canDelete: true), [.copy, .reply, .forward, .replyInDM, .delete])
        XCTAssertEqual(MessageMenu.items(forMessage: true, canDelete: false), [.copy, .reply, .forward, .replyInDM])
        XCTAssertEqual(MessageMenu.items(forMessage: true, canDelete: true, hasReplies: true),
                       [.copy, .reply, .viewThread, .forward, .replyInDM, .delete])
        XCTAssertEqual(MessageMenu.items(forMessage: true, canDelete: false, hasReplies: true),
                       [.copy, .reply, .viewThread, .forward, .replyInDM])
        XCTAssertEqual(MessageMenu.items(forMessage: false, canDelete: true), [.copy])
    }
}
