import XCTest
import ArbosFFI
@testable import Arbos

/// Jacob, 2026-10-25: "when talking to the agent sometimes it shows its old pfp, sometimes
/// the new one switches in" — and a foreign agent drew the automatic leaf instead of the
/// picture its owner set. One copy of the avatar everywhere: after a change the header (my
/// project record), the bubble (the open chat's roster through the resolver) and the presence
/// row (the resolver by id) all resolve to the new key and the same stamp; a stale roster that
/// arrives later cannot flip them back; a relay push lands in place; every picture URL is the
/// public versioned route, never the owner-only blob slot.
@MainActor
final class AgentAvatarFlipTests: XCTestCase {
    private func project(id: String, name: String, imageKey: String?, version: Int) -> Project {
        var p = Project(
            id: id, name: name, icon: "H", color: "#8b7cf6", state: .awake, stateSince: nil, stateDetail: nil,
            model: "", effort: .low, spendTodayUsd: 0, spendCapUsd: 0, avatar: nil, avatarColor: nil
        )
        p.avatarLeaf = 4
        p.avatarLeafColor = 1
        p.imageKey = imageKey
        p.avatarVersion = version
        return p
    }

    private func rosterCopy(_ p: Project, owner: String = "unarbos") -> ChatMember {
        ChatMember(
            id: p.id, kind: .agent, member: "agent:\(p.id)", role: "member", handle: mentionSlug(p.name), avatarUrl: nil,
            name: nil, status: "member", joinedAt: 0, project: p, ownerId: "gh_\(owner)", ownerHandle: owner,
            replyPolicy: nil
        )
    }

    private func group(_ agents: [ChatMember]) -> PeopleChat {
        PeopleChat(
            id: "chat_group", kind: "group", createdBy: "gh_jacob", createdAt: 0, title: "Launch week",
            pending: false, members: agents, headSeq: 0, readSeq: 0, unread: false, lockState: "open",
            lastActivityAt: 0, myRole: "owner", muted: false, silenced: false
        )
    }

    /// The three chat surfaces, as they ask the model.
    private func surfaces(_ model: AppModel, id: String) -> (header: Project, bubble: Project, presence: Project) {
        let header = model.project(id)!
        let bubble = model.agent(sender: "agent:\(id)", handle: mentionSlug(header.name), name: header.name, chatId: "chat_group")
        let presence = model.agent(id: id, name: header.name)
        return (header, bubble, presence)
    }

    func testChangingThePictureMovesEverySurfaceAndNeverFlipsBack() async {
        let model = AppModel()
        model.isFixture = true
        let old = project(id: "01HEZ", name: "Hezbola", imageKey: "avatar-1700000000000.jpg", version: 1_700_000_000_000)
        model.projects = [old]
        model.peopleChats = [group([rosterCopy(old)])]

        var s = surfaces(model, id: "01HEZ")
        XCTAssertEqual(s.header.imageKey, "avatar-1700000000000.jpg")
        XCTAssertEqual(s.bubble.imageKey, "avatar-1700000000000.jpg")
        XCTAssertEqual(s.presence.imageKey, "avatar-1700000000000.jpg")

        // The owner sets a new picture on this device: every surface shows it at once.
        let set = await model.setProjectImageKey(projectId: "01HEZ", key: "avatar-1700000099000.jpg")
        XCTAssertTrue(set)
        s = surfaces(model, id: "01HEZ")
        for (name, p) in [("header", s.header), ("bubble", s.bubble), ("presence", s.presence)] {
            XCTAssertEqual(p.imageKey, "avatar-1700000099000.jpg", name)
            XCTAssertEqual(p.avatarVersion, s.header.avatarVersion, "\(name): one stamp")
        }
        XCTAssertGreaterThan(s.header.avatarVersion, 1_700_000_000_000, "the local write is newer than every copy")
        XCTAssertEqual(model.peopleChats[0].agents[0].project?.imageKey, "avatar-1700000099000.jpg", "the roster copy moved too")

        // A stale roster arrives (a people-list fetch that started before the change): the
        // newest copy wins everywhere — no surface flips back.
        model.peopleChats = [group([rosterCopy(old)])]
        model.reconcileAgentAvatars()
        s = surfaces(model, id: "01HEZ")
        XCTAssertEqual(s.bubble.imageKey, "avatar-1700000099000.jpg", "the bubble reads the roster first and still draws the new picture")
        XCTAssertEqual(s.presence.imageKey, "avatar-1700000099000.jpg")
        XCTAssertEqual(s.header.imageKey, "avatar-1700000099000.jpg")

        // Reset to the leaf: the newest copy has no picture; the stale roster copy's picture
        // does not come back.
        model.pickAvatarLeaf("01HEZ", leaf: 2, color: 3)
        s = surfaces(model, id: "01HEZ")
        XCTAssertNil(s.header.imageKey)
        XCTAssertNil(s.bubble.imageKey)
        XCTAssertNil(s.presence.imageKey)
        XCTAssertEqual(LeafArt.pick(s.bubble), LeafPick(leaf: 2, color: 3))
        XCTAssertEqual(LeafArt.pick(s.presence), LeafPick(leaf: 2, color: 3))
    }

    func testARelayPushLandsInPlaceAndAnOlderPushIsIgnored() {
        let model = AppModel()
        model.isFixture = true
        let p = project(id: "01HEZ", name: "Hezbola", imageKey: "avatar-a.jpg", version: 100)
        model.projects = [p]
        model.peopleChats = [group([rosterCopy(p)])]

        // Another device (or the owner, for a foreign agent) changed it: the push's key and
        // stamp reach every copy without a fetch.
        model.applyAgentAvatarPush(projectId: "01HEZ", imageKey: "avatar-b.jpg", version: 200)
        var s = surfaces(model, id: "01HEZ")
        XCTAssertEqual(s.header.imageKey, "avatar-b.jpg")
        XCTAssertEqual(s.bubble.imageKey, "avatar-b.jpg")
        XCTAssertEqual(s.presence.imageKey, "avatar-b.jpg")
        XCTAssertEqual(s.header.avatarVersion, 200)
        XCTAssertEqual(LeafArt.pick(s.bubble), LeafPick(leaf: 4, color: 1), "the leaf picks are kept until the re-read")

        // A push that was queued before the change (older stamp) changes nothing.
        model.applyAgentAvatarPush(projectId: "01HEZ", imageKey: "avatar-a.jpg", version: 150)
        s = surfaces(model, id: "01HEZ")
        XCTAssertEqual(s.header.imageKey, "avatar-b.jpg")
        XCTAssertEqual(s.bubble.imageKey, "avatar-b.jpg")
        XCTAssertEqual(s.presence.imageKey, "avatar-b.jpg")

        // The push for a foreign agent I only know from a roster lands on that copy.
        let foreign = project(id: "01FOR", name: "Foreign", imageKey: nil, version: 0)
        model.peopleChats = [group([rosterCopy(p), rosterCopy(foreign, owner: "sami")])]
        model.applyAgentAvatarPush(projectId: "01FOR", imageKey: "avatar-f.jpg", version: 300)
        let bubble = model.agent(sender: "agent:01FOR", handle: "foreign", name: "Foreign", chatId: "chat_group")
        XCTAssertEqual(bubble.imageKey, "avatar-f.jpg")
        XCTAssertEqual(bubble.avatarVersion, 300)
    }

    /// Jacob's "Hezbola" shot: a viewer who only shares a group with the agent must fetch the
    /// picture from a route they may read. The owner-only blob slot is never used.
    func testThePictureUrlIsThePublicVersionedRoute() {
        let url = Project.pictureURL(httpUrl: "https://relay.example", id: "01HEZ", imageKey: "avatar-1.jpg", version: 1_700_000_000_000)
        XCTAssertEqual(url, "https://relay.example/projects/01HEZ/avatar.jpg?v=1700000000000")
        XCTAssertNil(Project.pictureURL(httpUrl: "https://relay.example", id: "01HEZ", imageKey: nil, version: 1), "no picture, no fetch")
        XCTAssertNil(Project.pictureURL(httpUrl: "https://relay.example", id: "01HEZ", imageKey: "", version: 1))
        XCTAssertNil(Project.pictureURL(httpUrl: "", id: "01HEZ", imageKey: "avatar-1.jpg", version: 1), "no relay (fixture)")
        // A change is a new URL, so no cache can hand back the old bytes.
        XCTAssertNotEqual(
            Project.pictureURL(httpUrl: "https://relay.example", id: "01HEZ", imageKey: "avatar-1.jpg", version: 1),
            Project.pictureURL(httpUrl: "https://relay.example", id: "01HEZ", imageKey: "avatar-2.jpg", version: 2)
        )
        XCTAssertFalse(url!.contains("/blob/"))
        XCTAssertFalse(url!.contains("token="))
    }

    /// Jacob, 2026-10-26 (relaunch): the cold launch restores the record from the registry
    /// cache and the roster copies from the people cache — two snapshots written at different
    /// times, and agents whose avatar predates #1813 carry stamp 0 on both. A roster copy that
    /// lost its picture must not win the tie; every surface (header record, status row, the
    /// unattributed activity row, bubble, members list, the Secrets picker's `model.projects`)
    /// resolves to the picture, and every URL is the public versioned route.
    func testColdLaunchOverThePersistedStoreResolvesThePictureOnEverySurface() throws {
        // What the two caches hold after a relaunch: the record (Codable, with its stamp and
        // URL) and a roster copy whose picture did not survive.
        var record = project(id: "01DOUG", name: "Doug agent", imageKey: "avatar-gen-1.jpg", version: 0)
        record.imageURL = "https://relay.example/blob/01DOUG/avatar-gen-1.jpg?token=OLD" // a pre-#1813 cache
        let data = try JSONEncoder().encode([record])
        let restored = try JSONDecoder().decode([Project].self, from: data)
        var lossy = project(id: "01DOUG", name: "Doug agent", imageKey: nil, version: 0)
        lossy.imageURL = nil

        let model = AppModel()
        model.isFixture = true
        model.projects = restored
        model.peopleChats = [group([rosterCopy(lossy)])]
        model.reconcileAgentAvatars()

        let header = model.project("01DOUG")!
        let status = model.agent(id: "01DOUG", name: "Doug agent")
        let activity = model.agent(sender: "agent", handle: nil, name: nil, chatId: "chat_group")
        let bubble = model.agent(sender: "agent:01DOUG", handle: "doug-agent", name: "Doug agent", chatId: "chat_group")
        let member = model.agent(member: model.peopleChats[0].agents[0], chatId: "chat_group")
        for (name, p) in [("header", header), ("status row", status), ("activity row", activity), ("bubble", bubble), ("members list", member)] {
            XCTAssertEqual(p.imageKey, "avatar-gen-1.jpg", name)
            XCTAssertEqual(p.id, "01DOUG", name)
        }
        // The Secrets picker draws `model.projects` records: the same key, and the URL is the
        // public route (the fixture has no relay, so the URL itself is nil there) — never the
        // blob URL the old cache held.
        XCTAssertEqual(model.projects[0].imageKey, "avatar-gen-1.jpg")
        XCTAssertFalse(model.projects[0].imageURL?.contains("/blob/") ?? false, "the old blob URL is gone")
        XCTAssertEqual(model.peopleChats[0].agents[0].project?.imageKey, "avatar-gen-1.jpg", "the lossy roster copy was repaired")
        // With several agents on the roster an unattributed row stays unknown (no guess).
        let other = project(id: "01OTHER", name: "Other", imageKey: nil, version: 0)
        model.peopleChats = [group([rosterCopy(record), rosterCopy(other)])]
        XCTAssertNotEqual(model.agent(sender: "agent", handle: nil, name: nil, chatId: "chat_group").id, "01DOUG")
    }

    /// The resolver itself (shared with the Mac through the FFI) draws the newest copy whatever
    /// order a surface hands the cards in.
    func testTheSharedResolverDrawsTheNewestCopy() {
        let stale = AgentCard(id: "01HEZ", name: "Hezbola", handle: "hezbola", leaf: 4, leafColor: 1, colorIndex: nil, imageKey: "avatar-a.jpg", avatarVersion: 100)
        let fresh = AgentCard(id: "01HEZ", name: "Hezbola", handle: "hezbola", leaf: 4, leafColor: 1, colorIndex: nil, imageKey: "avatar-b.jpg", avatarVersion: 200)
        for cards in [[stale, fresh], [fresh, stale]] {
            let r = resolveAgent(sender: "agent:01HEZ", handle: "hezbola", name: "Hezbola", cards: cards)
            XCTAssertEqual(r.imageKey, "avatar-b.jpg")
            XCTAssertEqual(r.avatarVersion, 200)
        }
    }
}
