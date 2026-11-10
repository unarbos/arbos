import SwiftUI
import XCTest
@testable import Arbos

/// Guards for the Jacob asks that had no test of their own.
///
/// Every `[feedback …]` tag in the app is one of Jacob's asks. Most already have a dedicated
/// suite — `ListTopFadeTests`, `GroupedMessagesTests`, `PresenceRowTests` and the rest — and the
/// audit in `docs/product/jacob-asks.md` found these had nothing holding them at all. An ask with
/// no test is an ask that regresses silently and is noticed in a screenshot weeks later, which is
/// the failure this file exists to stop.
///
/// Each case names its tag, so `scripts/ci/jacob-asks-check.py` can tie registry entry to guard.
@MainActor
final class JacobAskGuardTests: XCTestCase {
    /// [feedback agent-bubble-inset-2026-10-22] — the agent bubble leaves 76 pt, not the 71 pt it
    /// was. Measured off Grok dark at 3 px/pt: 16 pt own margin + 60 pt on the far side.
    func testTheAgentBubbleKeepsItsSeventySixPointInset() {
        XCTAssertEqual(Theme.bubbleInset, 76, accuracy: 0.001,
                       "the agent bubble inset is the measured Grok number; 71 was the old one")
    }

    /// [feedback in-call-bar] — the in-call strip's chip is a recorded exception to the one
    /// app-wide control height, so a sweep that unifies control sizes must not quietly take it.
    func testTheInCallChipKeepsItsRecordedException() {
        XCTAssertEqual(Theme.voiceButton, 36, accuracy: 0.001,
                       "the in-call chip is 36 pt by exception, not Controls.height")
        XCTAssertNotEqual(Theme.voiceButton, Theme.fieldHeight,
                          "if these ever match, the exception has been absorbed by a sweep")
    }

    /// [feedback mention-chip-hover-2026-10-24] — a mention chip is accent on accent at 14 %, and
    /// 30 % while the finger is down. The two must stay distinct or the chip stops answering touch.
    func testTheMentionChipStillAnswersTheFinger() {
        XCTAssertEqual(ChipLinkStyle.restAlpha, 0.14, accuracy: 0.001)
        XCTAssertEqual(ChipLinkStyle.pressedAlpha, 0.30, accuracy: 0.001)
        XCTAssertGreaterThan(ChipLinkStyle.pressedAlpha, ChipLinkStyle.restAlpha,
                             "pressed must read heavier than rest, or the chip looks dead")
    }

    /// [feedback connection-pill] — when several signals disagree the worst one is shown, and a
    /// healthy link says nothing at all. Ordering is the whole contract.
    func testTheConnectionPillShowsTheWorstSignalAndStaysQuietWhenLive() {
        let worstFirst: [LinkState] = [.offline, .relayDown, .reconnecting, .syncing, .restartSoon, .live]
        for (worse, better) in zip(worstFirst, worstFirst.dropFirst()) {
            XCTAssertGreaterThan(worse, better, "\(worse) must outrank \(better) in the pill")
        }
        XCTAssertNil(LinkState.live.label, "a live link has nothing to say")
        for state in worstFirst where state != .live {
            XCTAssertNotNil(state.label, "\(state) must give the pill something to show")
        }
    }

    /// [feedback math-render-ios-2026-10-26] — display math on its own line, in either delimiter,
    /// reaches the transcript as a math block rather than as literal dollars and brackets.
    func testDisplayMathSurvivesBothDelimiters() {
        for source in ["$$x^2 + y^2$$", "\\[x^2 + y^2\\]"] {
            let blocks = Markdown.blocks(source)
            let isMath = blocks.contains { if case .math = $0 { return true }; return false }
            XCTAssertTrue(isMath, "\(source) must parse to a math block, got \(blocks)")
        }
    }

    /// [feedback blocked-state-relay-2026-10-26] — the relay's refusal reason reaches the chat as
    /// a sentence a person can read, and the machine code is carried through untouched.
    func testARefusedPushExplainsItselfInWords() {
        let why = PushRefusalWhy(code: "not_available", reason: "blocked",
                                 byHandle: "sam", byName: "Sam Okafor")
        XCTAssertEqual(why.code, "not_available", "the code the relay sent is never rewritten")
        XCTAssertFalse(why.sentence().isEmpty, "a refusal the chat cannot explain is a blank bubble")
    }

    // MARK: - Asks whose rule lives in a view

    /// Source of a file under `Arbos/`, for the wiring checks. There is no UITests target for
    /// these surfaces, so the wiring is asserted on the source, as `ChatSearchCloseTests` does.
    private func source(_ path: String) -> String {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Arbos/\(path)")
        return (try? String(contentsOf: url, encoding: .utf8)) ?? ""
    }

    /// [feedback group-library-links-2026-10-23] — a group's Library lists links the way the agent
    /// pane does. One list builder serves both, so the guard is that links survive it and that the
    /// group pane has not grown a private copy.
    func testAGroupsLibraryListsLinksThroughTheSharedBuilder() {
        let link = ChatItem(id: "i1", kind: .link, title: "arbos.life", subtitle: "",
                            url: URL(string: "https://www.arbos.life/launch/"), at: 1_700_000_000_000,
                            fromHandle: "samir", mime: "")
        let sections = LibraryList.sections(items: [link], empty: "Nothing shared here yet.")
        let isEmptyState = sections.contains { if case .empty = $0 { return true }; return false }
        XCTAssertFalse(isEmptyState, "a link is a library item; the group pane must not drop it")
        // The rows are drawn inside the day card, which `sections` hands back as an opaque
        // `.custom` view, so the reachable claim is the grouping that card draws from.
        XCTAssertEqual(LibraryList.days([link]).flatMap(\.items).map(\.id), ["i1"],
                       "the link must reach the day it belongs to")

        XCTAssertTrue(source("UI/ProfilePages.swift").contains("LibraryList.sections(items: items, empty:"),
                      "the group pane builds its Library through the shared builder, not its own")
    }

    /// [feedback group-row-avatar-ios-2026-10-27] — a group shows its own picture through the
    /// resolver, and the placeholder only when it has none. The order is the contract.
    func testAGroupFallsToThePlaceholderOnlyWhenItHasNothing() {
        let model = AppModel()
        model.isFixture = true
        var group = PeopleChat(id: "chat_launch", kind: "group", createdBy: "gh_jacob", createdAt: 0,
                               title: "Launch week", pending: false, members: [], headSeq: 0, readSeq: 0,
                               unread: false, lockState: "open", lastActivityAt: 0, myRole: "owner",
                               muted: false, silenced: false)

        if case .placeholder = GroupAvatar.resolution(group, model: model) {} else {
            XCTFail("a group with neither picture nor colour is the placeholder")
        }

        group.avatarDiscColor = 3
        if case .disc = GroupAvatar.resolution(group, model: model) {} else {
            XCTFail("a group created with a colour draws that colour, never the grey placeholder")
        }

        // The picture is resolved from the chat's own key, and only for a group. (Whether a URL
        // comes back also depends on a signed-in endpoint, which a unit test has no business
        // standing up — so the two gates are asserted, not the string.)
        group.imageKey = ""
        XCTAssertNil(GroupAvatar.pictureURL(group, model: model), "an empty key is no picture")
        var dm = group
        dm.kind = "dm"
        dm.imageKey = "group-1700000000000.jpg"
        XCTAssertNil(GroupAvatar.pictureURL(dm, model: model), "only a group resolves a group picture")
    }

    /// [feedback link-icons-ios-adopt-2026-10-27] — search and the Library draw the one link tile
    /// at the row's size, rather than each inventing a glyph.
    func testSearchAndLibraryBothDrawTheOneLinkTile() {
        XCTAssertEqual(PaneKit.avatar, 36, accuracy: 0.001, "the row's tile size")
        XCTAssertEqual(LinkRowTile.iconHost(URL(string: "https://WWW.Arbos.life:8443/launch/")), "www.arbos.life",
                       "the icon is keyed by a lower-cased host with www. kept and the port dropped")
        XCTAssertEqual(LinkRowTile.iconHost(nil), "", "no URL, no host, and no crash")

        XCTAssertTrue(source("UI/GlobalSearchView.swift").contains("LinkRowTile(url: url, size: PaneKit.avatar)"),
                      "search draws the shared tile at the row's size")
        XCTAssertTrue(source("UI/LibraryList.swift").contains("LinkRowTile(url: item.url, size: LibraryList.thumb)"),
                      "the Library draws the same tile")
    }

    /// [feedback sidecar-sessions-ios-2026-10-28] — the Session log card shows the newest four
    /// lines worth reading; wakes, reads and uploads are plumbing and stay behind View all.
    func testTheSessionLogCardShowsTheNewestFourRealLines() {
        func entry(_ id: Int64, _ kind: String) -> OutsideLogEntry {
            OutsideLogEntry(json: ["id": NSNumber(value: id), "at": NSNumber(value: id), "kind": kind])!
        }
        let oldestFirst = [entry(1, "reply"), entry(2, "wake"), entry(3, "session.new"), entry(4, "read"),
                           entry(5, "error"), entry(6, "reply"), entry(7, "upload"), entry(8, "post")]
        let shown = OutsideLogEntry.newestShown(oldestFirst)

        XCTAssertEqual(shown.map(\.id), [8, 6, 5, 3], "newest first, plumbing dropped, four at most")
        XCTAssertTrue(shown.allSatisfy(\.shown))
        XCTAssertEqual(OutsideLogEntry.newestShown([]).count, 0, "an agent that has never run shows no lines")

        XCTAssertTrue(source("UI/AgentProfileView.swift").contains("OutsideLogEntry.newestShown(entries)"),
                      "the card uses the rule rather than its own prefix")
        XCTAssertTrue(source("UI/AgentProfileView.swift").contains("PaneRow(title: \"View all\", chevron: true)"),
                      "the rest stay reachable behind View all")
    }

    /// [feedback outside-agents-wake-ios-2026-10-28] — the wake form patches one field at a time:
    /// an untouched field is left alone, a cleared one is actually cleared.
    func testTheWakeFormTellsClearingApartFromLeavingAlone() {
        XCTAssertTrue(RelayAPI.wakeBody(url: nil, auth: nil).isEmpty,
                      "touching nothing must not patch anything")

        let set = RelayAPI.wakeBody(url: "https://hook.arbos.life/wake", auth: nil)
        XCTAssertEqual(set["wake_url"] as? String, "https://hook.arbos.life/wake")
        XCTAssertNil(set["wake_auth"], "the untouched field is absent, which leaves it alone")

        for blank in ["", "   "] {
            let cleared = RelayAPI.wakeBody(url: blank, auth: blank)
            XCTAssertTrue(cleared["wake_url"] is NSNull, "a blank field clears, and never sends \"\"")
            XCTAssertTrue(cleared["wake_auth"] is NSNull)
        }
    }

    /// [feedback reply-preview-fixed-height] — the reply bar above the field is one row high
    /// whatever it quotes: the quoted text is flattened to a line and capped.
    func testTheReplyPreviewIsOneRowWhateverItQuotes() {
        let sprawling = "# Heading\n\nFirst para\n\n- one\n- two\n\n```\ncode here\n```"
        let line = Quote.oneLine(sprawling)
        XCTAssertFalse(line.contains("\n"), "a multi-line quote is flattened, or the bar grows")
        XCTAssertFalse(line.contains("#"), "markdown is flattened too, not shown raw")

        let long = String(repeating: "word ", count: 400)
        XCTAssertLessThanOrEqual(Quote.firstWords(long, maxChars: Theme.replyPreviewChars).count,
                                 Theme.replyPreviewChars,
                                 "the cap is what keeps the row a fixed height")
        XCTAssertEqual(Quote.firstWords("short", maxChars: Theme.replyPreviewChars), "short",
                       "a short quote is left alone")

        XCTAssertTrue(source("UI/ChatView.swift").contains("Text(Quote.line(of: r, maxChars: Theme.replyPreviewChars))"),
                      "the bar draws the capped line")
    }

    /// [feedback group-settings-ios-2026-10-27] — the group pane hides Edit group from a plain
    /// member. The screenshot rig can stand in as one, and only as a real role.
    func testAPlainMemberHasNoEditGroupAndTheRigCanOnlyBeARealRole() {
        let pane = source("UI/ProfilePages.swift")
        XCTAssertTrue(pane.contains("private var canEditGroupSettings: Bool"),
                      "one gate decides, rather than each row guessing")
        XCTAssertTrue(pane.contains("} else if !canEditGroupSettings {"),
                      "Edit group is behind that gate")
        XCTAssertTrue(source("App/DebugHooks.swift").contains("[\"member\", \"admin\"].contains($0) ? $0 : nil"),
                      "the rig may only claim a role the app really has")
    }

    /// [feedback iphone-entity-profile-edit-2026-11-10] — Coding and Routines are overview
    /// rows on the agent profile (Grok Bot). Supersedes the Details-tab hide ask
    /// (`agent-details-remove-sections-ios-2026-10-25`).
    func testTheAgentOverviewKeepsCodingAndRoutinesRows() {
        XCTAssertEqual(PaneKinds.agent, ["Coding", "Routines", "Library", "Computer"],
                       "Grok's overview rows; dropping Coding/Routines is the regression")
        let profile = source("UI/AgentProfileView.swift")
        XCTAssertTrue(profile.contains("overviewRow(icon: \"chevron.left.forwardslash.chevron.right\", title: \"Coding\""),
                      "Coding is an overview row that pushes")
        XCTAssertTrue(profile.contains("overviewRow(icon: \"clock\", title: \"Routines\""),
                      "Routines is an overview row that pushes")
        XCTAssertTrue(profile.contains("OverviewEdit.action"),
                      "Edit pill opens the Grok-style edit page")
        XCTAssertTrue(source("UI/AgentEditView.swift").contains("struct AgentEditView"),
                      "Edit is its own pushed page")
    }

    /// [feedback iphone-create-entity-edit-style-2026-11-10] — Create uses the Edit-page
    /// hierarchy (sectioned cards, Done/Create pill), not the old bottom-sheet layout.
    func testCreateEntityUsesTheEditPageHierarchy() {
        let create = source("UI/PeopleSheets.swift")
        XCTAssertTrue(create.contains("EntityEditTopBar"),
                      "Create draws the Grok edit top bar")
        XCTAssertTrue(create.contains("EntitySectionHeader(title: \"Name\")"),
                      "Name is a sectioned card")
        XCTAssertTrue(create.contains("EntitySectionHeader(title: \"Appearance\")"),
                      "Appearance is a sectioned card")
        XCTAssertTrue(create.contains("accessibilityIdentifier(\"create-entity\")"),
                      "the create page is named for UI tests")
        XCTAssertFalse(create.contains(".frame(width: 84, height: 44).background(Capsule().fill(cancelBg))"),
                       "the old Cancel/Create footer pills are gone")
    }

    /// [feedback iphone-entity-nav-push-2026-11-10] — Edit and create sub-pages push; avatar /
    /// model / voice / reasoning from Edit are pushes, not sheets.
    func testEntityEditSubpagesPushNotSheet() {
        let edit = source("UI/AgentEditView.swift")
        XCTAssertTrue(edit.contains("navigationDestination(isPresented: $showAvatar)"),
                      "avatar from Edit is a push")
        XCTAssertTrue(edit.contains("AvatarPickerPage"),
                      "avatar uses the push wrapper")
        XCTAssertTrue(edit.contains("ModelPickerPage") && edit.contains("pushed: true"),
                      "model picker is pushed without sheet chrome")
        let group = source("UI/ProfilePages.swift")
        XCTAssertTrue(group.contains("struct GroupEditView"),
                      "group edit is a pushed page, not GroupEditSheet")
        XCTAssertFalse(group.contains("struct GroupEditSheet"),
                       "the sheet form of group edit is gone")
        XCTAssertTrue(group.contains("struct ServerEditView"),
                      "server edit is a pushed page")
    }
}
