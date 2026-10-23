import XCTest
@testable import Arbos

/// Home search (Jacob, Mac + iPhone, 2026-10-23): "carro" showed only the group
/// Testing, not the person Carro; "robertalan" showed Carro (handle) and Testing
/// again. Mirrors crates/arbos-mac `search::tests`.
final class HomeSearchTests: XCTestCase {
    private func none() -> [HomeSearch.MessageText] { [] }

    private func msg(_ chat: String, _ id: String, _ at: Int64, _ text: String) -> HomeSearch.MessageText {
        HomeSearch.MessageText(chatId: chat, messageId: id, createdAt: at, text: text)
    }

    func testDisplayNameFindsThePerson() {
        let hit = HomeSearch.dmHit(name: "Carro", handle: "robertalanm", messages: none, q: HomeSearch.normalizeQuery("carro"))
        XCTAssertEqual(hit, HomeSearch.Hit(kind: .person), "the row's own name explains the hit")
    }

    func testHandlePrefixFindsThePersonWithOrWithoutAt() {
        for typed in ["robertalan", "@robertalan", "ROBERTALAN", " robertalanm "] {
            let hit = HomeSearch.dmHit(name: "Carro", handle: "robertalanm", messages: none, q: HomeSearch.normalizeQuery(typed))
            XCTAssertEqual(hit?.kind, .person, typed)
        }
    }

    func testUnrelatedGroupDoesNotAppear() {
        let members = [HomeSearch.Member(handle: "kenobijon", name: "Kenobi")]
        let messages = { [self.msg("c1", "m1", 10, "Missed call from Kenobi")] }
        XCTAssertNil(HomeSearch.titledHit(title: "Testing", members: members, messages: messages, q: HomeSearch.normalizeQuery("carro")))
        XCTAssertNil(HomeSearch.titledHit(title: "Testing", members: members, messages: messages, q: HomeSearch.normalizeQuery("robertalan")))
    }

    func testGroupWithThePersonAsMemberSaysSo() {
        let members = [HomeSearch.Member(handle: "kenobijon", name: "Kenobi"), HomeSearch.Member(handle: "robertalanm", name: "Carro")]
        let hit = HomeSearch.titledHit(title: "Testing", members: members, messages: none, q: HomeSearch.normalizeQuery("carro"))
        XCTAssertEqual(hit?.kind, .member)
        XCTAssertEqual(hit?.reason, "Member · Carro @robertalanm")
        let byHandle = HomeSearch.titledHit(title: "Testing", members: members, messages: none, q: HomeSearch.normalizeQuery("robertalan"))
        XCTAssertEqual(byHandle?.reason, "Member · Carro @robertalanm")
        let noName = HomeSearch.titledHit(title: "Testing", members: [HomeSearch.Member(handle: "sam", name: nil)], messages: none, q: "sam")
        XCTAssertEqual(noName?.reason, "Member · @sam", "no name → handle only")
    }

    func testMessageBodyHitNamesTheMessage() {
        let messages = {
            [self.msg("c1", "old", 10, "ask carro about the deck"),
             self.msg("c1", "new", 20, "Carro said the launch is Tuesday, so plan for it")]
        }
        let hit = HomeSearch.titledHit(title: "Testing", members: [], messages: messages, q: HomeSearch.normalizeQuery("carro"))
        XCTAssertEqual(hit?.kind, .message)
        XCTAssertEqual(hit?.chatId, "c1")
        XCTAssertEqual(hit?.messageId, "new", "newest match wins")
        XCTAssertEqual(hit?.reason, "Message · Carro said the launch is Tuesday, so plan for it")
    }

    func testRankingPeopleThenTitlesThenMembersThenMessages() {
        let q = HomeSearch.normalizeQuery("carro")
        let kinds: [HomeSearch.Kind] = [
            HomeSearch.titledHit(title: "Research", members: [], messages: { [self.msg("c", "m", 1, "ping carro")] }, q: q)!.kind,
            HomeSearch.titledHit(title: "Testing", members: [HomeSearch.Member(handle: "robertalanm", name: "Carro")], messages: none, q: q)!.kind,
            HomeSearch.titledHit(title: "Carro's launch", members: [], messages: none, q: q)!.kind,
            HomeSearch.dmHit(name: "Carro", handle: "robertalanm", messages: none, q: q)!.kind,
        ]
        XCTAssertEqual(kinds.sorted(), [.person, .title, .member, .message])
        XCTAssertEqual(kinds, Array(kinds.sorted().reversed()))
    }

    func testCaseAndDiacriticsFoldBothWays() {
        for typed in ["zoe", "ZOË", "martin", "Martín", "zoë martin"] {
            let hit = HomeSearch.dmHit(name: "Zoë Martín", handle: "zoe-m", messages: none, q: HomeSearch.normalizeQuery(typed))
            XCTAssertEqual(hit?.kind, .person, typed)
        }
        XCTAssertNotNil(HomeSearch.dmHit(name: "Jose", handle: "jose", messages: none, q: HomeSearch.normalizeQuery("José")))
    }

    func testAgentRowsMatchNameChatTitlesAndAgents() {
        let q = HomeSearch.normalizeQuery("research")
        XCTAssertEqual(HomeSearch.titledHit(title: "Research", members: [], messages: none, q: q)?.kind, .title)
        let chats = [HomeSearch.ChatTitle(id: "c1", title: "Interface research"), HomeSearch.ChatTitle(id: "c2", title: "Budget")]
        let hit = HomeSearch.titledHit(title: "Arbos", members: [], chats: chats, messages: none, q: q)
        XCTAssertEqual(hit, HomeSearch.Hit(kind: .title, reason: "Chat · Interface research", chatId: "c1"))
        let agent = HomeSearch.titledHit(title: "Launch week", members: [HomeSearch.Member(handle: "arbos", name: "Arbos", isAgent: true)],
                                         messages: none, q: HomeSearch.normalizeQuery("arbos"))
        XCTAssertEqual(agent?.reason, "Agent · Arbos")
    }

    func testEmptyQueryShowsEverythingAndNothingMatchesGibberish() {
        XCTAssertNotNil(HomeSearch.dmHit(name: "Carro", handle: "robertalanm", messages: none, q: HomeSearch.normalizeQuery("")))
        XCTAssertNotNil(HomeSearch.dmHit(name: "Carro", handle: "robertalanm", messages: none, q: HomeSearch.normalizeQuery("@")))
        XCTAssertNil(HomeSearch.dmHit(name: "Carro", handle: "robertalanm", messages: none, q: HomeSearch.normalizeQuery("zzqx")))
    }

    func testServersMatchNameOrChannel() {
        XCTAssertEqual(HomeSearch.serverHit(name: "Bittensor", channels: ["general"], q: "bitt")?.reason, nil)
        XCTAssertEqual(HomeSearch.serverHit(name: "Bittensor", channels: ["general"], q: "gener")?.reason, "#general")
        XCTAssertNil(HomeSearch.serverHit(name: "Bittensor", channels: ["general"], q: "carro"))
    }

    func testSnippetsCutAroundTheMatch() {
        let long = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty twenty-one twenty-two"
        let s = HomeSearch.snippet(long, q: "ten")!
        XCTAssertTrue(s.hasPrefix("…"), s)
        XCTAssertTrue(s.contains("ten"), s)
        XCTAssertTrue(s.hasSuffix("…"), s)
        XCTAssertEqual(HomeSearch.snippet("Zoë\nwas here", q: "zoe"), "Zoë was here")
        XCTAssertNil(HomeSearch.snippet("plain", q: "x"))
    }
}
