import XCTest
@testable import Arbos

/// How the Search-in-chat bar closes ([feedback iphone-search-close-2026-10-28], Jacob
/// 2026-10-28: "The x button doesn't work on the search on iPhone. Also clicking away
/// from the search should close it."). The rules are pure (`ChatSearchClose`); the view
/// checks pin that `ChatView` wires them — there is no UITests target, so the wiring is
/// asserted on the source, as `ChatSearchShortcutTests` does.
final class ChatSearchCloseTests: XCTestCase {
    private var chatView: String {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Arbos/UI/ChatView.swift")
        return (try? String(contentsOf: url, encoding: .utf8)) ?? ""
    }

    private var transcript: String {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Arbos/UI/Transcript.swift")
        return (try? String(contentsOf: url, encoding: .utf8)) ?? ""
    }

    func testTheCloseButtonIsAFullHeightCellNotTheGlyph() {
        XCTAssertGreaterThanOrEqual(ChatSearchClose.closeHitWidth, 44, "Apple's minimum touch target")
        let src = chatView
        // The ✕ closes through closeSearch and owns a 44 × bar-height hit shape.
        XCTAssertTrue(src.contains(".frame(width: ChatSearchClose.closeHitWidth, height: Controls.height)\n                                .contentShape(Rectangle())"),
                      "the ✕ cell must carry a contentShape the size of the cell")
        XCTAssertTrue(src.contains(".accessibilityIdentifier(\"chat-search-close\")"))
        let closeButton = src.range(of: "Button { closeSearch() } label: {\n                            Image(systemName: \"xmark.circle.fill\")")
        XCTAssertNotNil(closeButton, "the ✕ button calls closeSearch")
    }

    func testTapOutsideClosesAndSwipeClosesOnlyWithoutAKeyword() {
        XCTAssertTrue(ChatSearchClose.tapOutsideCloses(open: true))
        XCTAssertFalse(ChatSearchClose.tapOutsideCloses(open: false))
        XCTAssertTrue(ChatSearchClose.scrollCloses(open: true, query: ""))
        XCTAssertTrue(ChatSearchClose.scrollCloses(open: true, query: "   "))
        XCTAssertFalse(ChatSearchClose.scrollCloses(open: true, query: "canvas"), "scrolling the results keeps the bar")
        XCTAssertFalse(ChatSearchClose.scrollCloses(open: false, query: ""))
    }

    func testATapOnAResultClosesThenReveals() {
        XCTAssertTrue(ChatSearchClose.resultTapReveals(open: true, query: "canvas"))
        XCTAssertFalse(ChatSearchClose.resultTapReveals(open: true, query: ""), "no keyword, no results to tap")
        XCTAssertFalse(ChatSearchClose.resultTapReveals(open: false, query: "canvas"))
    }

    /// The view: the transcript tap closes the bar; result rows reveal through
    /// `revealQuoted` after the close; the close path resets the window and never leaves
    /// a filter behind; a keyword with no hit shows a line instead of a blank list.
    func testChatViewWiresTheClosePaths() {
        let src = chatView
        XCTAssertTrue(src.contains("guard ChatSearchClose.tapOutsideCloses(open: showSearch) else { return }"), "transcript tap closes")
        XCTAssertTrue(src.contains("if searchResultTapped == nil { closeSearch() }"), "…unless a result row took the tap")
        XCTAssertTrue(src.contains("if ChatSearchClose.scrollCloses(open: showSearch, query: searchText) { closeSearch() }"), "swipe closes")
        XCTAssertTrue(src.contains("SearchResultTap(active: searchHitsActive && Self.isMessageBubble(row))"), "result rows take the tap")
        XCTAssertTrue(src.contains("closeSearch(scrollToTail: false)\n        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { revealQuoted(messageId) }"), "a result closes, then reveals")
        // closeSearch: bar, query, keyboard, window.
        guard let start = src.range(of: "private func closeSearch(scrollToTail: Bool = true) {"),
              let end = src.range(of: "private func openSearchResult", range: start.upperBound..<src.endIndex) else {
            return XCTFail("closeSearch / openSearchResult not found")
        }
        let body = String(src[start.lowerBound..<end.lowerBound])
        for line in ["showSearch = false", "searchText = \"\"", "searchFocused = false", "windowSkipEnd = 0", "windowCount = min(max(windowCount, Self.windowInitial), Self.windowMax)"] {
            XCTAssertTrue(body.contains(line), "closeSearch must do: \(line)")
        }
        XCTAssertTrue(src.contains("accessibilityIdentifier(\"chat-search-no-matches\")"), "zero hits say so instead of a blank transcript")
    }

    func testAResultRowOwnsTheTapsInsideIt() {
        XCTAssertTrue(ChatSearchClose.rowOwnsInnerTap(resultRow: true))
        XCTAssertFalse(ChatSearchClose.rowOwnsInnerTap(resultRow: false),
                       "no filter, no suppression: an avatar or a chip opens as it always has")
    }

    /// Every route from inside a row to something other than the reveal asks that one rule:
    /// the sender's avatar and the in-bubble name on a text row, the same two on a media
    /// row, the mention chip and the hidden spoiler. The rule takes nothing but "a filter is
    /// on", so an agent sender is covered by the same call as a person — the page that
    /// opened in the measurement was an agent's. An ordinary web link is left alone.
    func testEveryRouteOutOfAResultRowAsksThatOneRule() {
        let rows = transcript
        XCTAssertTrue(rows.contains("guard !ChatSearchClose.rowOwnsInnerTap(resultRow: searchHit) else { return }"),
                      "the row's decision point")
        XCTAssertEqual(rows.components(separatedBy: "model.toggleProfile(").count - 1, 1,
                       "a row opens a page through openProfile and nowhere else")
        XCTAssertEqual(rows.components(separatedBy: "= openProfile(").count - 1, 4,
                       "person and agent, text row and media row — four bindings, eight buttons")
        // The flag is a stored input on a compared view, not an environment read: the row is
        // used `.equatable()`, so an environment read would not re-run its body on a flip.
        XCTAssertTrue(rows.contains("var searchHit: Bool = false"))
        XCTAssertTrue(rows.contains("&& lhs.searchHit == rhs.searchHit"), "…and it is compared")
        let src = chatView
        XCTAssertTrue(src.contains(".equatable()"), "the row list compares rows rather than re-reading them")
        XCTAssertTrue(src.contains("searchHit: searchHitsActive && Self.isMessageBubble(row)"),
                      "the row gets exactly the flag that mounts the result tap")
        // The chip and the spoiler, through the transcript's OpenURLAction.
        XCTAssertTrue(src.contains("let rowOwnsIt = ChatSearchClose.rowOwnsInnerTap(resultRow: searchHitsActive)"))
        XCTAssertTrue(src.contains("if !rowOwnsIt { SpoilerState.shared.toggle(key) }"), "hidden spoiler")
        XCTAssertTrue(src.contains("if !rowOwnsIt { model.toggleProfile(target) }"), "mention chip")
        // Deliberately untouched: a link out to the web still goes to the system, and the
        // row's gesture stays simultaneous, so no tap changes shape.
        XCTAssertTrue(src.contains("guard let target = MentionTable.target(from: url, model: model) else { return .systemAction }"))
        XCTAssertTrue(src.contains("content.simultaneousGesture(TapGesture().onEnded { action() })"))
    }

    // `testARevealIsOneHopWithNoFollowUpCorrections` stood here and asserted that
    // `settleOnRevealed` was absent from `ChatView`. It was written to catch a reverted change
    // being quietly undone, and it did exactly that: #2446 reintroduces the correction ladder,
    // the merge into this branch is textually clean, and this test was the only thing that said
    // so. It is retired rather than fixed because the project then decided, deliberately and
    // with measurements behind it, that the ladder is correct — a reveal resolves `scrollTo`
    // against estimated heights, and one hop lands 13 to 35 rows out on a long transcript.
    // Asserting the absence of the chosen fix is no longer a thing anyone wants true.
}
