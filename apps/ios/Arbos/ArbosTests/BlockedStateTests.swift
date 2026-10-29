import XCTest
@testable import Arbos

/// The Blocked state ([feedback blocked-state-ios-2026-10-26]; the Mac's `blocked::tests` /
/// `push_refusal_tests` twin): the words, the block-list match, the refusal mapping, and the
/// structure — every surface draws the shared tag / chip / bar and every Unblock asks first.
final class BlockedStateTests: XCTestCase {
    private func person(_ id: String, _ handle: String, _ name: String? = nil) -> Person {
        Person(id: id, handle: handle, avatarUrl: nil, name: name, status: "member")
    }

    func testTheWords() {
        XCTAssertEqual(BlockedState.label, "Blocked")
        XCTAssertEqual(BlockedState.unblock, "Unblock")
        XCTAssertEqual(BlockedState.confirmTitle(handle: "sam"), "Unblock @sam?")
        let confirm = BlockedState.unblockConfirm(handle: "sam")
        XCTAssertEqual(confirm.title, "Unblock @sam?")
        XCTAssertEqual(confirm.message, BlockedState.confirmNote)
        XCTAssertEqual(confirm.buttons, ["Unblock", "Cancel"])
        XCTAssertFalse(confirm.destructive, "unblocking undoes a block; not red")
        XCTAssertEqual(BlockedState.composerLine(name: "Sam Rivera"), "You blocked Sam Rivera")
        XCTAssertFalse(BlockedState.confirmNote.isEmpty)

        // Mute-until label ([feedback app-01M61GZ5B66VCSKF40458KH3DP]): fixed UTC clock.
        let tz = TimeZone(secondsFromGMT: 0)!
        let nowMs: Int64 = 1_793_188_800_000 // 2026-10-28 12:00:00 UTC
        let laterToday: Int64 = 1_793_197_800_000 // 2026-10-28 14:30:00 UTC
        let otherDay: Int64 = 1_793_370_600_000 // 2026-10-30 14:30:00 UTC
        XCTAssertEqual(
            BlockedState.mutedUntilLabel(untilMs: laterToday, nowMs: nowMs, timeZone: tz),
            "Muted until 14:30"
        )
        XCTAssertEqual(
            BlockedState.mutedUntilLabel(untilMs: otherDay, nowMs: nowMs, timeZone: tz),
            "Muted until Oct 30, 14:30"
        )
        XCTAssertEqual(BlockedState.mutedUntilLabel(untilMs: nil, nowMs: nowMs, timeZone: tz), "Muted")
        XCTAssertEqual(
            BlockedState.refusalSentence(
                reason: "silenced", name: nil, handle: nil,
                silencedUntilMs: laterToday, nowMs: nowMs
            ),
            "Muted until 14:30. Your messages aren't being delivered."
        )
        XCTAssertEqual(
            BlockedState.refusalSentence(
                reason: "silenced", name: nil, handle: nil,
                silencedUntilMs: nil, nowMs: nowMs
            ),
            "Muted. Your messages aren't being delivered."
        )
    }

    func testBlockedMatchesByIdThenHandle() {
        let list = [person("gh_2", "sam", "Sam Rivera")]
        XCTAssertTrue(BlockedState.isBlocked(id: "gh_2", handle: "someone-else", in: list))
        XCTAssertTrue(BlockedState.isBlocked(id: "", handle: "Sam", in: list))
        XCTAssertFalse(BlockedState.isBlocked(id: "gh_3", handle: "priya", in: list))
        XCTAssertFalse(BlockedState.isBlocked(id: "gh_2", handle: "sam", in: []))
    }

    func testTheRefusalSentenceFollowsTheRelaysReason() {
        XCTAssertEqual(
            BlockedState.refusalSentence(reason: "blocked_by", name: "Sam Rivera", handle: "sam"),
            "Sam Rivera has blocked you in this chat."
        )
        XCTAssertEqual(
            BlockedState.refusalSentence(reason: "blocked_by", name: nil, handle: "sam"),
            "@sam has blocked you in this chat."
        )
        XCTAssertEqual(
            BlockedState.refusalSentence(reason: "blocked_by", name: " ", handle: ""),
            "Someone in this chat has blocked you."
        )
        XCTAssertEqual(
            BlockedState.refusalSentence(reason: "blocks_member", name: "Sam Rivera", handle: "sam"),
            "Couldn't post: you've blocked Sam Rivera, who is in this chat. Unblock to post."
        )
        XCTAssertEqual(
            BlockedState.refusalSentence(reason: "silenced", name: nil, handle: nil),
            "Muted. Your messages aren't being delivered."
        )
        for reason in [nil, "chat_unavailable", "something_new"] {
            XCTAssertEqual(
                BlockedState.refusalSentence(reason: reason, name: nil, handle: nil),
                "Messages in this chat aren't being delivered right now. We'll keep trying.",
                "an old relay / an unknown reason keeps the generic line"
            )
        }
        XCTAssertTrue(BlockedState.offersUnblock(reason: "blocks_member"))
        XCTAssertFalse(BlockedState.offersUnblock(reason: "blocked_by"))
        XCTAssertFalse(BlockedState.offersUnblock(reason: nil))
        let why = PushRefusalWhy(code: "not_available", reason: "blocked_by", byHandle: "sam", byName: "Sam Rivera")
        XCTAssertEqual(why.sentence(), "Sam Rivera has blocked you in this chat.")
    }

    // MARK: - Structure

    private var sourceRoot: URL {
        // …/apps/ios/Arbos/ArbosTests/BlockedStateTests.swift → …/apps/ios/Arbos/Arbos
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Arbos")
    }

    private func source(_ rel: String) throws -> String? {
        let url = sourceRoot.appendingPathComponent(rel)
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        return try String(contentsOf: url, encoding: .utf8)
    }

    /// The person sheet: the chip under the @handle, Unblock as the hero action and the
    /// footer row, both through the one confirm; the member row carries the tag.
    func testThePersonSheetAndMemberRowsSayBlocked() throws {
        guard let pages = try source("UI/ProfilePages.swift") else {
            throw XCTSkip("sources not beside the test bundle (CI without the tree)")
        }
        XCTAssertTrue(pages.contains("subtitleBelow: blockedEntry != nil && !isMe ? AnyView(BlockedChip()) : nil"), "the chip under the @handle")
        XCTAssertTrue(pages.contains("? AnyView(unblockButton)"), "Unblock is the hero action once blocked")
        XCTAssertTrue(pages.contains("PanePillButton(glyph: \"nosign\", title: BlockedState.unblock) { confirmUnblock = true }"), "the pill asks first")
        XCTAssertTrue(pages.contains("PaneActionRow(title: BlockRow.title(blocked: true), color: theme.accent) { confirmUnblock = true }"), "the row asks first")
        XCTAssertFalse(pages.contains("color: theme.accent) { model.unblock(entry) }"), "no unblock without the confirm")
        XCTAssertTrue(pages.contains(".confirm(BlockedState.unblockConfirm(handle: person.handle), isPresented: $confirmUnblock)"), "one confirm for both, the centred alert")
        XCTAssertTrue(pages.contains("case (true, let manage?): return AnyView(HStack(spacing: 6) { BlockedTag(); manage })"), "the member row's tag before the menu")
        XCTAssertTrue(pages.contains("case (true, nil): return AnyView(BlockedTag())"))
    }

    /// The DM: the header pill's tag, the composer bar in the composer's place, the confirm.
    func testTheDmHeaderTagsAndTheComposerIsTheBlockedBar() throws {
        guard let chat = try source("UI/ChatView.swift") else {
            throw XCTSkip("sources not beside the test bundle (CI without the tree)")
        }
        XCTAssertTrue(chat.contains("if blockedPeer != nil { BlockedTag() }"), "the tag after the DM peer's name")
        XCTAssertTrue(chat.contains("} else if let peer = blockedPeer {"), "the bar takes the composer's branch")
        XCTAssertTrue(chat.contains("BlockedComposerBar(name: peer.displayName) { confirmUnblock = true }"), "the bar's Unblock asks first")
        XCTAssertTrue(chat.contains(".confirm(BlockedState.unblockConfirm(handle: blockedPeer?.handle ?? counterpart?.handle ?? \"\"), isPresented: $confirmUnblock)"), "the centred alert, not a dialog floating over the transcript")
        XCTAssertTrue(chat.contains("BlockedState.isBlocked(id: other.id, handle: other.handle, in: model.blocked)"), "from the relay's block list")
    }

    /// The refusal: the room keeps the relay's reason, the model shows its sentence.
    func testTheRefusalNoticeUsesTheRelaysReason() throws {
        guard let room = try source("Sync/SessionRoom.swift"),
              let model = try source("Store/AppModel.swift") else {
            throw XCTSkip("sources not beside the test bundle (CI without the tree)")
        }
        XCTAssertTrue(room.contains("case .pushRefused(_, let code, let reason, let byHandle, let byName):"), "the frame's fields are read")
        XCTAssertTrue(room.contains("lastRefusal = PushRefusalWhy(code: code, reason: reason, byHandle: byHandle, byName: byName)"))
        XCTAssertTrue(model.contains("let why = room?.lastRefusal"), "the model reads the reason")
        XCTAssertTrue(
            model.contains("why?.sentence(silencedUntilMs: until)"),
            "the model passes silencedUntil into the sentence"
        )
        XCTAssertTrue(model.contains("BlockedState.refusalSentence(reason: nil, name: nil, handle: nil)"))
        XCTAssertFalse(model.contains("self.notice = Notice(text: \"Messages in this chat are not being delivered\")"), "no fixed sentence left")
    }
}

/// A group block hides the blocked person's rows from the blocker alone (Jacob 2026-10-26;
/// the Mac's `blocked::hidden_tests` twin): runs fold into `.hidden` rows, nothing folds for
/// everyone else, the preview skips them, and the structure wires the hint / dot / fold.
final class BlockedHiddenRowsTests: XCTestCase {
    private let chat = "fx-group-launch"
    private let t0: Int64 = 1_700_000_000_000

    private func person(_ i: Int, sender: String, handle: String, name: String, _ text: String) -> Message {
        Message(id: "m\(i)", chatId: chat, role: .user, sender: sender, status: .done, turn: nil, decision: nil, subagentId: nil,
                subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: t0 + Int64(i) * 1_000, completedAt: nil,
                pending: false, parts: [.text(id: "m\(i)-t", text: text)], hiddenCount: 0, senderHandle: handle, senderName: name)
    }

    private var messages: [Message] {
        [
            person(1, sender: "user:gh_3", handle: "priya", name: "Priya Natarajan", "morning all"),
            person(2, sender: "user:gh_2", handle: "sam", name: "Sam Rivera", "hey"),
            person(3, sender: "user:gh_2", handle: "sam", name: "Sam Rivera", "anyone around?"),
            person(4, sender: "user:gh_3", handle: "priya", name: "Priya Natarajan", "here"),
            person(5, sender: "user:gh_2", handle: "sam", name: "Sam Rivera", "cool"),
        ]
    }

    private func shape(_ rows: [Row]) -> [String] {
        rows.compactMap { row in
            switch row.kind {
            case .other: return "user:\(row.messageId)"
            case .hidden(let count, _, let handle, _): return "hidden:\(handle)x\(count)"
            default: return nil
            }
        }
    }

    func testABlockedSendersRunsFoldIntoHiddenRows() {
        let blocker = Transcript.rows(messages, projectId: "people:\(chat)", me: "user:fx-me", people: true, multiParty: true,
                                      hiddenSenders: ["user:gh_2"])
        XCTAssertEqual(shape(blocker), ["user:m1", "hidden:samx2", "user:m4", "hidden:samx1"])
        XCTAssertFalse(blocker.contains { $0.id == "m2#date" || $0.id == "m3#date" || $0.id == "m5#date" }, "no date line for a hidden row")
        // A third member (nothing hidden): every row is a bubble.
        let third = Transcript.rows(messages, projectId: "people:\(chat)", me: "user:fx-me", people: true, multiParty: true)
        XCTAssertEqual(shape(third).filter { $0.hasPrefix("user:") }.count, 5)
        XCTAssertFalse(third.contains { if case .hidden = $0.kind { return true } else { return false } })
    }

    func testTheHiddenLine() {
        XCTAssertEqual(BlockedState.hiddenLine(count: 1, who: "@sam"), "1 message hidden because you blocked @sam")
        XCTAssertEqual(BlockedState.hiddenLine(count: 3, who: "Sam Rivera"), "3 messages hidden because you blocked Sam Rivera")
    }

    @MainActor
    func testTheModelHidesForTheBlockerAndRevealsOnUnblock() {
        let model = AppModel()
        var chatDto = PeopleChat.parse([
            "id": chat, "kind": "group", "created_by": "gh_1", "created_at": t0, "title": "Launch week",
            "pending": false, "head_seq": 5, "read_seq": 0, "unread": false, "lock_state": "people_only",
            "room": "/room/people/\(chat)", "last_activity_at": t0,
            "members": [
                ["member": "user:gh_2", "role": "member", "added_by": "gh_1", "joined_at": t0, "handle": "sam", "name": "Sam Rivera"],
                ["member": "user:gh_3", "role": "member", "added_by": "gh_1", "joined_at": t0, "handle": "priya", "name": "Priya Natarajan"],
            ],
            "me": ["role": "owner"],
            "blocked_senders": ["user:gh_2"],
        ])
        XCTAssertNotNil(chatDto, "the hint parses")
        XCTAssertEqual(chatDto?.blockedSenders, ["user:gh_2"])
        XCTAssertEqual(chatDto.map { model.hiddenSenders(in: $0) }, ["user:gh_2"], "from the relay's hint")
        // Without the hint, my block list decides (a block taken a moment ago).
        chatDto?.blockedSenders = []
        XCTAssertEqual(chatDto.map { model.hiddenSenders(in: $0) }, [])
        model.setBlockedForTests([Person(id: "gh_2", handle: "sam", avatarUrl: nil, name: "Sam Rivera", status: "member")])
        XCTAssertEqual(chatDto.map { model.hiddenSenders(in: $0) }, ["user:gh_2"], "from my block list")
        model.setBlockedForTests([])
        XCTAssertEqual(chatDto.map { model.hiddenSenders(in: $0) }, [], "unblocked: nothing hidden")
        // A DM never hides (its composer bar says Blocked instead).
        var dm = chatDto!
        dm.kind = "dm"
        dm.members = [dm.members[0]]
        dm.blockedSenders = ["user:gh_2"]
        XCTAssertEqual(model.hiddenSenders(in: dm), [])
    }

    private var sourceRoot: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Arbos")
    }

    private func source(_ rel: String) throws -> String? {
        let url = sourceRoot.appendingPathComponent(rel)
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        return try String(contentsOf: url, encoding: .utf8)
    }

    /// The chat passes the hint to every rows() call, the fold draws the shared line with
    /// Unblock, the home preview and the unread dot skip hidden rows.
    func testTheSurfacesSkipHiddenRows() throws {
        guard let chat = try source("UI/ChatView.swift"),
              let transcript = try source("UI/Transcript.swift"),
              let home = try source("UI/HomeView.swift"),
              let model = try source("Store/AppModel.swift") else {
            throw XCTSkip("sources not beside the test bundle (CI without the tree)")
        }
        XCTAssertEqual(chat.components(separatedBy: "hiddenSenders: hiddenSenders)").count - 1, 3, "every rows() call carries the hint")
        XCTAssertTrue(chat.contains("let hiddenSenders = people.map { model.hiddenSenders(in: $0) } ?? []"))
        XCTAssertTrue(transcript.contains("case .hidden(let count, _, let handle, let name):"), "the fold is drawn")
        XCTAssertTrue(transcript.contains("Text(BlockedState.hiddenLine(count: count, who: who))"))
        XCTAssertTrue(transcript.contains(".accessibilityIdentifier(\"hidden-row-unblock\")"), "with Unblock")
        XCTAssertTrue(home.contains("!model.hidesMessage($0, chatId: c.id)"), "the preview skips hidden rows")
        XCTAssertTrue(model.contains("if hidesMessage(last, chatId: chatId) { return }"), "a hidden arrival lights no dot")
    }
}
