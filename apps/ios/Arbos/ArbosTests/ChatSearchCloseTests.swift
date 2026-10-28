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
}
