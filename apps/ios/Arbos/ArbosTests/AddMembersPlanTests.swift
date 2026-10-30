import XCTest
@testable import Arbos

/// The group pane's Add members picker: members of the group are already in
/// (their rows are disabled) and Add targets that group, never a new one.
/// Mirrors crates/arbos-mac `model::add_members_tests`.
final class AddMembersPlanTests: XCTestCase {
    private func person(_ id: String, _ handle: String) -> Person {
        Person(id: id, handle: handle, avatarUrl: nil, name: nil, status: "member")
    }

    private func member(_ id: String, _ handle: String, agent: Bool = false) -> ChatMember {
        ChatMember(id: id, kind: agent ? .agent : .user, member: "\(agent ? "agent" : "user"):\(id)", role: "member",
                   handle: handle, avatarUrl: nil, name: nil, status: "member", joinedAt: 0,
                   project: nil, ownerId: nil, ownerHandle: nil, replyPolicy: nil)
    }

    /// The group "Testing": me (gh_me), allan and const, plus one agent.
    private func testingGroup() -> PeopleChat {
        PeopleChat(id: "chat_testing", kind: "group", createdBy: "gh_me", createdAt: 0, title: "Testing", pending: false,
                   members: [member("gh_me", "unarbos"), member("gh_1", "allan"), member("gh_2", "const"), member("proj_lium", "lium", agent: true)],
                   headSeq: 0, readSeq: 0, unread: false, lockState: "people_only", lastActivityAt: 0, myRole: "owner", muted: false, silenced: false)
    }

    func testExistingMembersCountAsAlreadyInTheGroup() {
        let g = testingGroup()
        XCTAssertTrue(g.hasPerson(person("gh_1", "allan"), meId: "gh_me"))
        XCTAssertTrue(g.hasPerson(person("gh_2", "const"), meId: "gh_me"))
        XCTAssertTrue(g.hasPerson(person("gh_me", "unarbos"), meId: "gh_me"))
        XCTAssertTrue(g.hasAgent("proj_lium"))
        // A handle match is enough: search results need not share the roster's id.
        XCTAssertTrue(g.hasPerson(person("github:allan", "Allan"), meId: "gh_me"))
        XCTAssertFalse(g.hasPerson(person("gh_3", "alice"), meId: "gh_me"))
        XCTAssertFalse(g.hasAgent("proj_other"))
    }

    func testAddTargetsThisGroupAndSkipsWhoIsInIt() {
        let g = testingGroup()
        let other = Project(id: "proj_other", name: "Other", icon: "O", color: "#000000", state: .awake, stateSince: nil, stateDetail: nil,
                            model: "", effort: .low, spendTodayUsd: 0, spendCapUsd: 0)
        let lium = Project(id: "proj_lium", name: "Lium", icon: "L", color: "#000000", state: .awake, stateSince: nil, stateDetail: nil,
                           model: "", effort: .low, spendTodayUsd: 0, spendCapUsd: 0)
        let plan = AddMembersPlan.make(chat: g, meId: "gh_me",
                                       picked: [person("gh_3", "alice"), person("gh_1", "allan"), person("gh_me", "unarbos")],
                                       pickedAgents: [other, lium])
        XCTAssertEqual(plan.chatId, "chat_testing", "adds to THIS group, not a new one")
        XCTAssertEqual(plan.handles, ["alice"])
        XCTAssertEqual(plan.agentIds, ["proj_other"])
        XCTAssertFalse(plan.isEmpty)
    }

    func testNothingNewMakesAnEmptyPlan() {
        let plan = AddMembersPlan.make(chat: testingGroup(), meId: "gh_me", picked: [person("gh_1", "allan")], pickedAgents: [])
        XCTAssertEqual(plan.chatId, "chat_testing")
        XCTAssertTrue(plan.isEmpty)
    }
}
