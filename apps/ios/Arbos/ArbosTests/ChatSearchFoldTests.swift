import XCTest
@testable import Arbos

/// Search-in-chat compared the typed keyword with `localizedCaseInsensitiveContains`,
/// which folds case and nothing else, so "canvas" did not find "cánvas" on the phone
/// while the Mac's in-chat search, the Mac's app-wide search and the phone's OWN
/// app-wide search all found it ([feedback iphone-chat-search-accent-folding-2026-11-05]).
/// One client disagreeing with itself: the three folding sites compare folded text and
/// this one call site did not. It goes through `HomeSearch.fold` now — the comparison the
/// app-wide search already uses on message bodies — so both halves fold, the unaccented
/// keyword against accented prose and the accented keyword against unaccented prose.
final class ChatSearchFoldTests: XCTestCase {
    private func matches(_ text: String, _ typed: String) -> Bool {
        ChatSession.searchMatches(text, keyword: ChatSession.searchKeyword(typed))
    }

    /// Jacob's case: the keyword carries no accent, the message does.
    func testAnUnaccentedKeywordFindsAccentedProse() {
        XCTAssertTrue(matches("Noted — the cánvas spec is updated.", "canvas"))
        XCTAssertTrue(matches("Zoë sent the deck", "zoe"))
        XCTAssertTrue(matches("ask José about the launch", "jose"))
    }

    /// The half people forget, and the same bug: the keyword carries the accent, the
    /// message does not.
    func testAnAccentedKeywordFindsUnaccentedProse() {
        XCTAssertTrue(matches("Keep the canvas infinite", "cánvas"))
        XCTAssertTrue(matches("Zoe sent the deck", "Zoë"))
        XCTAssertTrue(matches("ask Jose about the launch", "José"))
    }

    /// What the old comparison did get right stays right.
    func testCaseStillFoldsAndAMissIsStillAMiss() {
        XCTAssertTrue(matches("Keep the Canvas infinite", "canvas"))
        XCTAssertTrue(matches("keep the canvas infinite", "CANVAS"))
        XCTAssertFalse(matches("Snap at 8. Export keeps the background.", "canvas"))
        XCTAssertFalse(matches("", "canvas"))
    }

    /// No keyword, no filter — anything that folds away keeps every row.
    func testAKeywordThatFoldsAwayMatchesEverything() {
        for blank in ["", "   ", "\n", " \t ", "\u{00A0}"] {
            XCTAssertTrue(ChatSession.searchKeyword(blank).isEmpty,
                          "\(blank.debugDescription) is not a keyword")
            XCTAssertTrue(matches("anything at all", blank), blank.debugDescription)
        }
    }

    /// The point of the fix: the in-chat filter and the app-wide search answer the same
    /// question about the same message now. `HomeSearch.snippet` is the app-wide
    /// message-body predicate — a hit when it returns a snippet.
    func testTheInChatFilterAgreesWithTheAppWideMessageSearch() {
        let bodies = [
            "Noted — the cánvas spec is updated.",
            "Keep the canvas infinite; export keeps the background.",
            "Zoë Martín joined the room",
            "Snap at 8.",
        ]
        for typed in ["canvas", "cánvas", "CANVAS", "zoe", "Zoë", "martin", "snap", "zzqx"] {
            let keyword = ChatSession.searchKeyword(typed)
            for body in bodies {
                XCTAssertEqual(
                    ChatSession.searchMatches(body, keyword: keyword),
                    HomeSearch.snippet(body, q: keyword) != nil,
                    "\(typed.debugDescription) against \(body.debugDescription)")
            }
        }
    }

    /// The source guard, because the odd one out was a call site and not a rule: the
    /// filter routes through the shared fold and the un-folded comparison is gone.
    func testChatViewComparesThroughTheSharedFoldAndNowhereElse() throws {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Arbos/UI/ChatView.swift")
        let src = try String(contentsOf: url, encoding: .utf8)
        XCTAssertFalse(src.contains("localizedCaseInsensitiveContains("),
                       "the in-chat filter must not compare un-folded text")
        XCTAssertTrue(src.contains("static func searchKeyword(_ typed: String) -> String { HomeSearch.fold(typed) }"),
                      "the keyword is folded in one place")
        XCTAssertTrue(src.contains("keyword.isEmpty || HomeSearch.fold(text).contains(keyword)"),
                      "both halves of the comparison go through the same fold")
        // The filter now asks the shared rules whether a row hits, since the
        // keyword is only half of it ([feedback search-in-chat-mac-2026-10-26]
        // — "key word, user name etc."). What this assertion is for is
        // unchanged: the keyword's verdict on a body is still `searchMatches`
        // and still folded, so no second comparison crept into the filter.
        XCTAssertTrue(src.contains("keywordInText: Self.searchMatches(m_text($0), keyword: q)"),
                      "the row filter gets the keyword's verdict from it")
        XCTAssertTrue(src.contains("showSearch && !Self.searchKeyword(searchText).isEmpty"),
                      "the hit wash and the filter read the same keyword")
    }
}
