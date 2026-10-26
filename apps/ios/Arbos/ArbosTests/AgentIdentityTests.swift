import XCTest
import ArbosFFI
@testable import Arbos

/// One agent identity for every surface (#1592): the bubble disc, the caption and the
/// presence row ask `AppModel.agent` with the same row and get the same `Project` back —
/// the roster's leaf and display name for someone else's agent, never a handle-seeded
/// initials disc or the raw `@slug`.
@MainActor
final class AgentIdentityTests: XCTestCase {
    private func rosterAgent(id: String, name: String, owner: String = "sami", leaf: Int? = 5, color: Int? = 2, imageKey: String? = nil) -> ChatMember {
        var project = Project(
            id: id, name: name, icon: "T", color: "#8b7cf6", state: .awake, stateSince: nil, stateDetail: nil,
            model: "", effort: .low, spendTodayUsd: 0, spendCapUsd: 0, avatar: nil, avatarColor: nil
        )
        project.avatarLeaf = leaf
        project.avatarLeafColor = color
        project.imageKey = imageKey
        return ChatMember(
            id: id, kind: .agent, member: "agent:\(id)", role: "member", handle: mentionSlug(name), avatarUrl: nil,
            name: nil, status: "member", joinedAt: 0, project: project, ownerId: "gh_\(owner)", ownerHandle: owner,
            replyPolicy: nil
        )
    }

    private func arbonites(_ agents: [ChatMember]) -> PeopleChat {
        PeopleChat(
            id: "chat_arbonites", kind: "group", createdBy: "gh_jacob", createdAt: 0, title: "Arbonites",
            pending: false, members: agents, headSeq: 0, readSeq: 0, unread: false, lockState: "open",
            lastActivityAt: 0, myRole: "owner", muted: false, silenced: false
        )
    }

    private func mine(id: String, name: String) -> Project {
        Project(
            id: id, name: name, icon: "M", color: "#8b7cf6", state: .awake, stateSince: nil, stateDetail: nil,
            model: "", effort: .low, spendTodayUsd: 0, spendCapUsd: 0, avatar: nil, avatarColor: nil
        )
    }

    /// The core rule through the FFI, as a table: id wins, then handle, then the row's own name.
    func testResolverTable() {
        let cards = [
            AgentCard(id: "01TEST", name: "TestAgent", handle: "testagent", leaf: 3, leafColor: 7, colorIndex: nil, imageKey: "avatar-1.jpg", avatarVersion: 0),
            AgentCard(id: "01HOME", name: "Home", handle: "home", leaf: nil, leafColor: nil, colorIndex: nil, imageKey: nil, avatarVersion: 0),
        ]
        let byId = resolveAgent(sender: "agent:01TEST", handle: "old", name: "Old Name", cards: cards)
        XCTAssertEqual(byId.name, "TestAgent")
        XCTAssertEqual(byId.handle, "testagent")
        XCTAssertEqual(byId.leaf, LeafPick(leaf: 3, color: 7))
        XCTAssertEqual(byId.imageKey, "avatar-1.jpg")
        XCTAssertTrue(byId.known)

        let byHandle = resolveAgent(sender: "", handle: "TestAgent", name: nil, cards: cards)
        XCTAssertEqual(byHandle.id, "01TEST")

        let unknown = resolveAgent(sender: "agent:01GHOST", handle: "ghost", name: "Ghost Writer", cards: cards)
        XCTAssertFalse(unknown.known)
        XCTAssertEqual(unknown.id, "01GHOST", "seeded by the bare id, not `agent:…`")
        XCTAssertEqual(unknown.name, "Ghost Writer", "the row's name, never the handle")
        XCTAssertEqual(unknown.leaf, leafPick(seed: "01GHOST", leaf: nil, color: nil))
        XCTAssertNotEqual(unknown.leaf, leafPick(seed: "agent:01GHOST", leaf: nil, color: nil))

        XCTAssertEqual(resolveAgent(sender: "agent:01GHOST", handle: "ghost", name: nil, cards: cards).name, "@ghost")
        XCTAssertEqual(agentSlug(name: "  Rail  Layout Bot "), "rail-layout-bot")
        XCTAssertEqual(agentSenderId(sender: "agent:01X"), "01X")
        XCTAssertEqual(agentDisplayName(name: nil, handle: "x"), "@x")
    }

    /// Jacob's screenshot: TestAgent (someone else's agent) in Arbonites. The bubble asks with
    /// the lane (`agent:<id>` + handle), the presence row with the row's sender / handle /
    /// name — one `Project`, so one picture and one name.
    func testPresenceRowAndBubbleShareOneIdentityForAForeignAgent() {
        let model = AppModel()
        model.projects = [mine(id: "01MINE", name: "Mine")]
        model.peopleChats = [arbonites([rosterAgent(id: "01TEST", name: "TestAgent")])]

        let bubble = model.agent(sender: "agent:01TEST", handle: "testagent", name: "TestAgent", chatId: "chat_arbonites")
        let presence = model.agent(sender: "agent:01TEST", handle: "testagent", name: "TestAgent", chatId: "chat_arbonites")
        XCTAssertEqual(bubble, presence)
        XCTAssertEqual(bubble.name, "TestAgent", "the display name, never `testagent`")
        XCTAssertEqual(LeafArt.pick(bubble), LeafPick(leaf: 5, color: 2), "the roster's leaf, not initials")
        XCTAssertEqual(LeafArt.pick(bubble), LeafArt.pick(presence))
        XCTAssertTrue(bubble.shared, "someone else's agent keeps its roster record")
        XCTAssertEqual(bubble.ownerHandle, "sami")

        // A row that only carries the handle (an older box) lands on the same card.
        let handleOnly = model.agent(sender: "", handle: "testagent", name: nil, chatId: "chat_arbonites")
        XCTAssertEqual(handleOnly.id, "01TEST")
        XCTAssertEqual(handleOnly.name, "TestAgent")
    }

    func testTheRosterCardWinsOverAStaleCopyInMyProjectsAndCarriesThePicture() {
        let model = AppModel()
        model.projects = [mine(id: "01HOME", name: "Home")]
        model.peopleChats = [arbonites([rosterAgent(id: "01HOME", name: "GigChadAgent", imageKey: "avatar-gen-1.jpg")])]
        let p = model.agent(sender: "agent:01HOME", handle: "gigchadagent", name: "GigChadAgent", chatId: "chat_arbonites")
        XCTAssertEqual(p.name, "GigChadAgent", "the chat roster is the relay's current truth")
        XCTAssertEqual(p.imageKey, "avatar-gen-1.jpg", "the uploaded picture travels with the roster card")
        XCTAssertEqual(model.agent(member: model.peopleChats[0].agents[0], chatId: "chat_arbonites").name, "GigChadAgent")
        XCTAssertEqual(model.foreignAgent("01HOME")?.name, "GigChadAgent")
    }

    func testAnAgentNoCardKnowsStillAgreesWithItself() {
        let model = AppModel()
        model.peopleChats = [arbonites([])]
        let bubble = model.agent(sender: "agent:01GHOST", handle: "ghost", name: nil, chatId: "chat_arbonites")
        let presence = model.agent(sender: "agent:01GHOST", handle: "ghost", name: "Ghost Writer", chatId: "chat_arbonites")
        XCTAssertEqual(LeafArt.pick(bubble), LeafArt.pick(presence), "both seed the leaf with the bare id")
        XCTAssertEqual(presence.name, "Ghost Writer")
        XCTAssertEqual(bubble.name, "@ghost", "no name anywhere → the handle, marked as one")
        XCTAssertEqual(bubble.id, "01GHOST")
    }
}
