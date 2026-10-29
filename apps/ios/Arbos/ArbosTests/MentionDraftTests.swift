import SwiftUI
import XCTest
@testable import Arbos

/// Composer @-mention autocomplete (#1596): the open `@token`, the chip spans
/// in the plain draft, exact-handle auto-resolve, the picker's rows and what
/// the send carries.
///
/// Main actor: `AppModel.mentions(in:)` / `addressesEveryone` are statics on
/// the `@MainActor` model and `MentionTextView.makeCoordinator()` is
/// main-actor isolated — a nonisolated test method cannot call them
/// synchronously (main's unit-test target failed to compile, 2026-10-24).
@MainActor
final class MentionDraftTests: XCTestCase {
    private func project(_ id: String, _ name: String) -> Project {
        Project(id: id, name: name, icon: "A", color: "#000000", state: .awake, stateSince: nil, stateDetail: nil,
                model: "", effort: .low, spendTodayUsd: 0, spendCapUsd: 0)
    }

    private func human(_ id: String, _ handle: String, _ name: String? = nil) -> ChatMember {
        ChatMember(id: id, kind: .user, member: "user:\(id)", role: "member", handle: handle, avatarUrl: nil,
                   name: name, status: "member", joinedAt: 0, project: nil, ownerId: nil, ownerHandle: nil, replyPolicy: nil)
    }

    private func agent(_ id: String, _ name: String) -> ChatMember {
        ChatMember(id: id, kind: .agent, member: "agent:\(id)", role: "member", handle: mentionSlug(name), avatarUrl: nil,
                   name: name, status: "member", joinedAt: 0, project: project(id, name), ownerId: nil, ownerHandle: nil, replyPolicy: nil)
    }

    private var sam: ChatMember { human("gh_sam", "sam", "Sam Okafor") }
    private var mira: ChatMember { human("gh_mira", "mira-k", "Mira Kaplan") }
    private var me: ChatMember { human("gh_me", "unconst", "Jacob") }
    private var studio: ChatMember { agent("p_studio", "Studio") }
    private var scout: ChatMember { agent("p_scout", "Scout") }
    private var members: [ChatMember] { [me, sam, mira, studio, scout] }

    func testOpenTokenIsTheAtWordTheCaretEnds() {
        let t = "hi @sa"
        let open = MentionDraft.openToken(text: t, caret: 6, mentions: [])
        XCTAssertEqual(open?.query, "sa")
        XCTAssertEqual(open?.range, 3..<6)
        XCTAssertNil(MentionDraft.openToken(text: "hi @sam ", caret: 8, mentions: []), "a space ends the token")
        XCTAssertNil(MentionDraft.openToken(text: "mail a@b", caret: 8, mentions: []), "an @ glued to a word is not a mention")
        XCTAssertEqual(MentionDraft.openToken(text: "@", caret: 1, mentions: [])?.query, "", "a bare @ opens the picker")
        // The caret in the middle of the text: the token before it, not the tail.
        XCTAssertEqual(MentionDraft.openToken(text: "@mi and more", caret: 3, mentions: [])?.query, "mi")
        // A chip's own `@` never opens the picker.
        let chip = ComposerMention(range: 0..<4, member: sam)
        XCTAssertNil(MentionDraft.openToken(text: "@sam", caret: 4, mentions: [chip]))
    }

    func testPickedRowBecomesOneChipAndTheSendCarriesItsId() {
        var text = "hi @sa"
        var mentions: [ComposerMention] = []
        let open = MentionDraft.openToken(text: text, caret: 6, mentions: mentions)!
        let caret = MentionDraft.insert(sam, at: open.range, text: &text, mentions: &mentions)
        XCTAssertEqual(text, "hi @sam ")
        XCTAssertEqual(caret, 8)
        XCTAssertEqual(mentions.count, 1)
        XCTAssertEqual(mentions[0].range, 3..<7)
        XCTAssertEqual(mentions[0].plain, "@sam")
        XCTAssertEqual(mentions[0].name, "Sam Okafor")
        XCTAssertEqual(MentionDraft.tokens(mentions), ["sam": "user:gh_sam"])
        XCTAssertEqual(AppModel.mentions(in: text, members: members, chipTokens: MentionDraft.tokens(mentions)), ["user:gh_sam"])
    }

    func testAnAgentChipReadsItsSlug() {
        var text = ""
        var mentions: [ComposerMention] = []
        MentionDraft.insert(studio, at: 0..<0, text: &text, mentions: &mentions)
        XCTAssertEqual(text, "@studio ")
        XCTAssertEqual(mentions[0].memberId, "agent:p_studio")
        XCTAssertEqual(AppModel.mentions(in: text, members: members, chipTokens: MentionDraft.tokens(mentions)), ["agent:p_studio"])
    }

    func testSpliceShiftsLaterChipsAndRemovesATouchedOneWhole() {
        var text = "a @sam b @mira-k c"
        var mentions = [ComposerMention(range: 2..<6, member: sam), ComposerMention(range: 9..<16, member: mira)]
        // Insert before both: both shift.
        MentionDraft.splice(&text, 0..<0, with: "hey ", mentions: &mentions)
        XCTAssertEqual(text, "hey a @sam b @mira-k c")
        XCTAssertEqual(mentions.map(\.range), [6..<10, 13..<20])
        // Delete one character inside the first chip: the whole chip goes.
        MentionDraft.splice(&text, 9..<10, with: "", mentions: &mentions)
        XCTAssertEqual(text, "hey a  b @mira-k c")
        XCTAssertEqual(mentions.map(\.range), [9..<16])
        XCTAssertEqual(mentions[0].member, mira)
    }

    func testExactHandleAndSeparatorResolveWithoutThePicker() {
        let text = "ping @sam "
        let m = MentionDraft.autoResolve(text: text, caret: 10, mentions: [], members: members, me: "unconst")
        XCTAssertEqual(m?.range, 5..<9)
        XCTAssertEqual(m?.member, sam)
        // Punctuation completes it too; case does not matter; the slug of an agent's name works.
        XCTAssertEqual(MentionDraft.autoResolve(text: "@STUDIO?", caret: 8, mentions: [], members: members, me: nil)?.member, studio)
        // Unknown handles stay plain; no separator yet → nothing.
        XCTAssertNil(MentionDraft.autoResolve(text: "@nobody ", caret: 8, mentions: [], members: members, me: nil))
        XCTAssertNil(MentionDraft.autoResolve(text: "@sam", caret: 4, mentions: [], members: members, me: nil))
        // Already a chip → nothing twice.
        XCTAssertNil(MentionDraft.autoResolve(text: text, caret: 10, mentions: [ComposerMention(range: 5..<9, member: sam)], members: members, me: nil))
    }

    func testASharedHandleStaysPlainUnlessItIsMine() {
        // `@arbos` is an agent and a person (#1538).
        let agentArbos = agent("p_arbos", "Arbos")
        let personArbos = human("gh_arbos", "arbos", "Arbos the human")
        let shared = [agentArbos, personArbos, sam]
        XCTAssertNil(MentionDraft.resolve(handle: "arbos", members: shared, me: "sam"), "two entities: the picker decides")
        XCTAssertEqual(MentionDraft.resolve(handle: "arbos", members: shared, me: "arbos"), agentArbos, "nobody @-mentions themselves")
        XCTAssertNil(MentionDraft.autoResolve(text: "@arbos ", caret: 7, mentions: [], members: shared, me: "sam"))
        // Picked from the picker, the chip names the person — and the send carries that id, not the agent's.
        var text = "@ar"
        var mentions: [ComposerMention] = []
        MentionDraft.insert(personArbos, at: 0..<3, text: &text, mentions: &mentions)
        XCTAssertEqual(text, "@arbos ")
        XCTAssertEqual(AppModel.mentions(in: text, members: shared, senderHandle: "sam", chipTokens: MentionDraft.tokens(mentions)), ["user:gh_arbos"])
        // Without a chip the send falls back to the text scan (#1538 rule: not mine → the person... first hit).
        XCTAssertFalse(AppModel.mentions(in: text, members: shared, senderHandle: "sam").isEmpty)
    }

    func testCandidatesMatchHandleOrNameWordMixedAndOrdered() {
        let rows = MentionDraft.candidates(query: "s", members: members, me: "unconst")
        XCTAssertEqual(rows.map(\.id), ["gh_sam", "p_scout", "p_studio"], "handle matches, alphabetical, people and agents mixed")
        XCTAssertEqual(MentionDraft.candidates(query: "k", members: members, me: "unconst").map(\.id), ["gh_mira"], "a word of the name")
        XCTAssertEqual(MentionDraft.candidates(query: "mi", members: members, me: "unconst").map(\.id), ["gh_mira"])
        XCTAssertFalse(MentionDraft.candidates(query: "", members: members, me: "unconst").contains(where: { $0.id == "gh_me" }), "never me")
        XCTAssertEqual(MentionDraft.candidates(query: "", members: members, me: "unconst").count, 4)
        XCTAssertTrue(MentionDraft.candidates(query: "zz", members: members, me: nil).isEmpty)
        // Six at most.
        let many = (0..<9).map { human("gh_\($0)", "sam\($0)") }
        XCTAssertEqual(MentionDraft.candidates(query: "sam", members: many, me: nil).count, 6)
    }

    func testTagOnlyOnASharedHandle() {
        let agentArbos = agent("p_arbos", "Arbos")
        let personArbos = human("gh_arbos", "arbos")
        let rows = [agentArbos, personArbos, sam]
        XCTAssertEqual(MentionDraft.tag(for: agentArbos, among: rows, me: nil), "agent")
        XCTAssertEqual(MentionDraft.tag(for: personArbos, among: rows, me: nil), "person")
        XCTAssertNil(MentionDraft.tag(for: sam, among: rows, me: nil))
    }

    func testARestoredDraftGetsItsChipsBack() {
        let chips = MentionDraft.resolveAll(text: "@sam and @studio, not @nobody or a@b", mentions: [], members: members, me: "unconst")
        XCTAssertEqual(chips.map(\.range), [0..<4, 9..<16])
        XCTAssertEqual(chips.map(\.memberId), ["user:gh_sam", "agent:p_studio"])
    }

    func testCollectivesAreOfferedAndChippedInPeopleChats() {
        // Jacob (2026-10-24): "@here" typed on the iPhone stayed plain text.
        XCTAssertEqual(MentionDraft.collectiveCandidates(query: ""), [.here, .agents])
        XCTAssertEqual(MentionDraft.collectiveCandidates(query: "h"), [.here])
        XCTAssertEqual(MentionDraft.collectiveCandidates(query: "ag"), [.agents])
        XCTAssertTrue(MentionDraft.collectiveCandidates(query: "x").isEmpty)
        XCTAssertEqual(CollectiveMention.here.description, "everyone in this chat")
        XCTAssertEqual(CollectiveMention.agents.description, "all agents here")
        // Auto-resolve on the separator, in a people chat; never outside one.
        let text = "How is the iPhone now @here "
        let m = MentionDraft.autoResolve(text: text, caret: (text as NSString).length, mentions: [], members: members, me: "unconst")
        XCTAssertEqual(m?.target, .collective(.here))
        XCTAssertEqual(m?.range, 22..<27)
        XCTAssertEqual(m?.name, "here")
        XCTAssertEqual(m?.plain, "@here")
        XCTAssertNil(MentionDraft.autoResolve(text: text, caret: (text as NSString).length, mentions: [], members: members, me: "unconst", collectives: false))
        // Picked from the row: one chip, the marker as its id.
        var draft = "ping "
        var chips: [ComposerMention] = []
        let caret = MentionDraft.insert(.collective(.agents), at: 5..<5, text: &draft, mentions: &chips)
        XCTAssertEqual(draft, "ping @agents ")
        XCTAssertEqual(caret, 13)
        XCTAssertEqual(chips.map(\.memberId), ["agents"])
        XCTAssertTrue(chips[0].isCollective)
        XCTAssertEqual(MentionDraft.tokens(chips), ["agents": "agents"])
        // A restored draft gets the collective chips back too.
        XCTAssertEqual(MentionDraft.resolveAll(text: "@here and @sam", mentions: [], members: members, me: "unconst").map(\.memberId), ["here", "user:gh_sam"])
    }

    func testTheSentRowCarriesTheCollectiveAsMemberIds() {
        // What the agents' gate reads: every agent's `agent:<id>` (rule 3), two or more → shared.
        XCTAssertEqual(AppModel.mentions(in: "How is the iPhone now @here", members: members, senderHandle: "unconst"),
                       ["user:gh_sam", "user:gh_mira", "agent:p_studio", "agent:p_scout"], "everyone in the chat but me")
        XCTAssertEqual(AppModel.mentions(in: "@agents status?", members: members, senderHandle: "unconst", chipTokens: ["agents": "agents"]),
                       ["agent:p_studio", "agent:p_scout"], "all agents here")
        XCTAssertEqual(AppModel.mentions(in: "@sam and @agents", members: members, senderHandle: "unconst", chipTokens: ["sam": "user:gh_sam"]),
                       ["user:gh_sam", "agent:p_studio", "agent:p_scout"])
        XCTAssertTrue(AppModel.addressesEveryone("hi @here"))
        XCTAssertTrue(AppModel.addressesEveryone("@agents?"))
        XCTAssertFalse(AppModel.addressesEveryone("@heresy"))
    }

    func testPlainAndAttributedOffsetsAgree() {
        let field = MentionTextView(text: .constant(""), mentions: .constant([]), caret: .constant(0), isFocused: .constant(false),
                                    placeholder: "", theme: Theme.dark)
        let c = field.makeCoordinator()
        let mentions = [ComposerMention(range: 3..<7, member: sam)]  // "hi @sam ok" → "hi \u{FFFC} ok"
        XCTAssertEqual(c.attributedOffset(forPlain: 0, mentions: mentions), 0)
        XCTAssertEqual(c.attributedOffset(forPlain: 3, mentions: mentions), 3)
        XCTAssertEqual(c.attributedOffset(forPlain: 7, mentions: mentions), 4, "after the chip")
        XCTAssertEqual(c.attributedOffset(forPlain: 10, mentions: mentions), 7)
        XCTAssertEqual(c.attributedOffset(forPlain: 5, mentions: mentions), 4, "inside the chip snaps after it")
    }
}

/// The empty composer's "Message …" name ([feedback group-composer-placeholder-ios-2026-10-28]):
/// a group says the chat's name, a DM says the other person's.
final class ComposerPlaceholderTests: XCTestCase {
    private func human(_ id: String, _ handle: String, _ name: String? = nil) -> ChatMember {
        ChatMember(id: id, kind: .user, member: "user:\(id)", role: "member", handle: handle, avatarUrl: nil,
                   name: name, status: "member", joinedAt: 0, project: nil, ownerId: nil, ownerHandle: nil, replyPolicy: nil)
    }

    private func agent(_ id: String, _ name: String) -> ChatMember {
        let p = Project(id: id, name: name, icon: "A", color: "#000000", state: .awake, stateSince: nil, stateDetail: nil,
                        model: "", effort: .low, spendTodayUsd: 0, spendCapUsd: 0)
        return ChatMember(id: id, kind: .agent, member: "agent:\(id)", role: "member", handle: mentionSlug(name), avatarUrl: nil,
                          name: name, status: "member", joinedAt: 0, project: p, ownerId: nil, ownerHandle: nil, replyPolicy: nil)
    }

    private func chat(_ kind: String, _ title: String, _ members: [ChatMember]) -> PeopleChat {
        PeopleChat(id: "chat_\(title)", kind: kind, createdBy: "gh_jacob", createdAt: 0, title: title, pending: false,
                   members: members, headSeq: 0, readSeq: 0, unread: false, lockState: "people_only", lastActivityAt: 0,
                   myRole: "member", muted: false, silenced: false)
    }

    private var me: ChatMember { human("gh_terry", "terry", "Terry") }
    private var jacob: ChatMember { human("gh_jacob", "unarbos", "Jacob") }
    private var sam: ChatMember { human("gh_sam", "sam", "Sam Okafor") }

    func testAGroupIsNamedByItsTitleNeverByAMember() {
        let arbonites = chat("group", "Arbonites", [jacob, me, sam, agent("p_comms", "Comms")])
        XCTAssertEqual(arbonites.composerName(me: me.id), "Arbonites")
        XCTAssertEqual(arbonites.composerName(me: jacob.id), "Arbonites", "the owner sees the same name")
        XCTAssertEqual(chat("group", "Pair", [jacob, me]).composerName(me: me.id), "Pair", "two people, still a group")
        XCTAssertEqual(chat("group", "Solo", [me]).composerName(me: me.id), "Solo")
    }

    /// Affinity, 2026-10-29 (Mac twin in `mention.rs`): `@A` (Scout, a chat member with a photo)
    /// and a directory person also named "Scout" — the chip is the first one on every rebuild,
    /// never the dictionary's pick.
    func testASharedDisplayNameResolvesToTheFirstEntityOnEveryRebuild() {
        for _ in 0..<32 {
            let entry = { (id: String) in
                MentionEntry(name: "Scout", target: .agent(projectId: id), picture: AnyView(EmptyView()))
            }
            let table = MentionTable(entries: [("a", entry("p_member")), ("le-scout", entry("p_directory"))])
            let chips = table.find(in: table.display("@A did our server get nuked?"))
            XCTAssertEqual(chips.count, 1)
            XCTAssertEqual(chips.first?.1.target, .agent(projectId: "p_member"))
        }
    }

    func testADMIsNamedByTheOtherPerson() {
        XCTAssertEqual(chat("dm", "Chat", [me, sam]).composerName(me: me.id), "Sam Okafor")
        XCTAssertEqual(chat("dm", "Chat", [sam, me]).composerName(me: me.id), "Sam Okafor", "member order does not matter")
        XCTAssertEqual(chat("dm", "Chat", [me, human("gh_k", "kenobi")]).composerName(me: me.id), "kenobi", "no display name: the handle")
        XCTAssertEqual(chat("dm", "Chat", [me, sam, agent("p_comms", "Comms")]).composerName(me: me.id), "Sam Okafor",
                       "a DM that gained an agent keeps the person, like the header")
    }
}
