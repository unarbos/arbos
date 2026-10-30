import XCTest
@testable import Arbos

/// Who may pin under each group policy (#1547). The permission rule is enforced in the
/// client, so these are the only thing standing between the `admins` policy and anyone
/// pinning — there is no relay check behind them.
final class PinPolicyTests: XCTestCase {
    private func member(_ id: String, role: String) -> ChatMember {
        ChatMember(id: id, kind: .user, member: "user:\(id)", role: role, handle: id, avatarUrl: nil,
                   name: id, status: "member", joinedAt: 0, project: nil, ownerId: nil,
                   ownerHandle: nil, replyPolicy: nil)
    }

    /// `myRole` is what the relay says I am in this chat; `createdBy` is who made it.
    /// They come apart when a creator is later demoted, which is the case that matters.
    private func model(myRole: String, createdBy: String, rowRole: String? = nil) -> AppModel {
        let m = AppModel()
        m.isFixture = true
        m.setFixtureUser(AuthUser(id: "me", login: "me", name: "Me"))
        m.peopleChats = [PeopleChat(id: "g1", kind: "group", createdBy: createdBy, createdAt: 0, title: "Launch week",
                                    pending: false, members: [member("me", role: rowRole ?? myRole), member("other", role: "owner")],
                                    headSeq: 0, readSeq: 0, unread: false, lockState: "visible_to_agents",
                                    lastActivityAt: 0, myRole: myRole, muted: false, silenced: false)]
        return m
    }

    private func chat(_ m: AppModel) throws -> PeopleChat { try XCTUnwrap(m.peopleChat("g1")) }

    func testAdminsPolicyIsTheDefaultAndLetsOwnersAndAdminsPin() throws {
        for role in ["owner", "admin"] {
            let m = model(myRole: role, createdBy: "someone-else")
            XCTAssertEqual(m.pinPolicy(for: "g1"), .admins, "a group with no stored policy reads as admins")
            XCTAssertTrue(m.canPinMessages(in: try chat(m)), "\(role) may pin under the admins policy")
        }
    }

    func testAdminsPolicyRefusesAPlainMember() throws {
        let m = model(myRole: "member", createdBy: "someone-else")
        XCTAssertFalse(m.canPinMessages(in: try chat(m)))
    }

    /// The bug the screenshot rig caught (30 Oct): `canPinMessages` delegated to
    /// `ownsGroup`, which also grants whoever created the group. A creator demoted to
    /// plain member kept the Pin item under the `admins` policy, and the menu looked
    /// identical to an owner's. `ownsGroup` is the delete-or-leave rule and must not
    /// decide this one.
    func testAdminsPolicyRefusesTheCreatorOnceDemotedToMember() throws {
        let m = model(myRole: "member", createdBy: "me")
        XCTAssertTrue(m.ownsGroup(try chat(m)), "unchanged: the creator may still delete the group")
        XCTAssertFalse(m.administersGroup(try chat(m)), "but is not an admin any more")
        XCTAssertFalse(m.canPinMessages(in: try chat(m)), "so may not pin under the admins policy")
    }

    func testEveryonePolicyLetsAPlainMemberPin() throws {
        let m = model(myRole: "member", createdBy: "someone-else")
        m.setPinPolicyForTests(["g1": "everyone"])
        XCTAssertEqual(m.pinPolicy(for: "g1"), .everyone)
        XCTAssertTrue(m.canPinMessages(in: try chat(m)))
    }

    /// Only owners and admins change the policy, whoever may pin under it.
    func testOnlyAdminsChangeThePolicy() throws {
        let demotedCreator = model(myRole: "member", createdBy: "me")
        demotedCreator.setPinPolicy(.everyone, chatId: "g1")
        XCTAssertEqual(demotedCreator.pinPolicy(for: "g1"), .admins, "a demoted creator cannot open pinning up")

        let owner = model(myRole: "owner", createdBy: "someone-else")
        owner.setPinPolicy(.everyone, chatId: "g1")
        XCTAssertEqual(owner.pinPolicy(for: "g1"), .everyone)
    }

    /// A DM has no pin policy and no pinning: the bar and the menu item are group-only.
    func testDirectMessagesNeverPin() throws {
        let m = AppModel()
        m.isFixture = true
        m.setFixtureUser(AuthUser(id: "me", login: "me", name: "Me"))
        m.peopleChats = [PeopleChat(id: "dm1", kind: "dm", createdBy: "me", createdAt: 0, title: "@other",
                                    pending: false, members: [member("me", role: "owner")], headSeq: 0, readSeq: 0,
                                    unread: false, lockState: "people_only", lastActivityAt: 0, myRole: "owner",
                                    muted: false, silenced: false)]
        XCTAssertFalse(m.canPinMessages(in: try XCTUnwrap(m.peopleChat("dm1"))))
    }
}
