import ArbosFFI
import XCTest
@testable import Arbos

/// Search-in-chat by who wrote a message ([feedback search-in-chat-mac-2026-10-26],
/// Jacob 2026-10-26: "find things in a chat by key word, user name etc."). The Mac
/// had the chip and the phone had only a keyword field, so the "user name" half was
/// absent here. Mirrors `arbos_core::chat_search::tests`, through ArbosFFI — these
/// tests assert the phone asks the shared rules, not that Swift agrees with them.
final class ChatSearchSenderTests: XCTestCase {
    private func sender(_ key: String, _ name: String, _ handle: String) -> ChatSearchSender {
        ChatSearchSender(key: key, name: name, handle: handle, avatarUrl: nil)
    }

    private var jacob: ChatSearchSender { sender("user:me", "Jacob Steeves", "unarbos") }
    private var affine: ChatSearchSender { sender("agent:a1", "Affine", "affine") }
    private var sam: ChatSearchSender { sender("user:sam", "Sam Rivera", "sam") }

    /// One row's sender, in transcript order: Jacob, Affine, Jacob, Sam, Affine.
    private var posted: [ChatSearchSender] { [jacob, affine, jacob, sam, affine] }

    // MARK: who the chip can be

    /// The pool a typed name is offered from: everyone who posted, one entry each,
    /// in the order they first appear.
    func testThePoolIsEveryoneWhoPostedOnceInFirstAppearanceOrder() {
        let who = ChatSearchSenderPin.posted(posted)
        XCTAssertEqual(who.map(\.key), ["user:me", "agent:a1", "user:sam"])
        XCTAssertEqual(who.map(\.name), ["Jacob Steeves", "Affine", "Sam Rivera"])
        XCTAssertTrue(ChatSearchSenderPin.posted([]).isEmpty)
    }

    // MARK: the offer

    private func offered(_ typed: String, pinned: ChatSearchSender? = nil) -> String? {
        ChatSearchSenderPin.offer(typed: typed, in: ChatSearchSenderPin.posted(posted),
                                 pinned: pinned)?.name
    }

    /// Exact, prefix and word-start on the display name and the @handle, which is
    /// the shared matcher's order and not a rule of this file's own.
    func testATypedNameOrHandleOffersOneSender() {
        XCTAssertEqual(offered("aff"), "Affine")
        XCTAssertEqual(offered("@sam"), "Sam Rivera")
        XCTAssertEqual(offered("rivera"), "Sam Rivera", "word start inside the name")
        XCTAssertEqual(offered("Jacob S"), "Jacob Steeves", "two words, in order")
        XCTAssertEqual(offered("SAM"), "Sam Rivera", "case folds")
    }

    /// Diacritics fold on a name here exactly as they do in every picker.
    func testANameFoldsItsAccents() {
        let zoe = [sender("user:z", "Zoë Álvarez", "zoe")]
        XCTAssertEqual(ChatSearchSenderPin.offer(typed: "zoe alv", in: zoe, pinned: nil)?.key,
                       "user:z")
        XCTAssertEqual(ChatSearchSenderPin.offer(typed: "álvarez", in: zoe, pinned: nil)?.key,
                       "user:z")
    }

    /// A keyword nobody is named is not a sender, and an empty field offers nothing.
    func testAWordNobodyIsNamedIsNotAnOffer() {
        XCTAssertNil(offered("canvas"))
        XCTAssertNil(offered(""))
        XCTAssertNil(offered("   "))
    }

    /// Jacob has not ruled on whether typing should swap a pinned sender, so the
    /// phone does what the Mac does: with a chip up there is no offer at all, and
    /// the only way to change one is to remove it. Pinned in the shared rules, so
    /// this cannot be answered differently on one client.
    func testAPinnedSenderIsNotSwappedByTypingAnotherName() {
        XCTAssertNil(offered("aff", pinned: sam))
        XCTAssertNil(offered("jacob", pinned: sam))
        XCTAssertNil(offered("sam", pinned: sam), "not even their own name")
    }

    // MARK: which rows the results hold

    private func hits(keyword: String, pinned: ChatSearchSender?, row: String,
                      inText: Bool) -> Bool {
        ChatSearchSenderPin.rowHits(keyword: keyword, pinned: pinned,
                                    rowSenderKey: row, keywordInText: inText)
    }

    /// A chip alone takes every one of that sender's messages — the point of the
    /// feature, and the case a keyword-only search cannot express.
    func testAPinnedSenderAloneTakesAllOfTheirMessages() {
        XCTAssertTrue(hits(keyword: "", pinned: sam, row: "user:sam", inText: false))
        XCTAssertFalse(hits(keyword: "", pinned: sam, row: "user:me", inText: true))
        XCTAssertFalse(hits(keyword: "", pinned: sam, row: "agent:a1", inText: true))
    }

    /// A chip and a keyword AND together.
    func testAChipAndAKeywordNarrowEachOther() {
        XCTAssertTrue(hits(keyword: "canvas", pinned: sam, row: "user:sam", inText: true))
        XCTAssertFalse(hits(keyword: "canvas", pinned: sam, row: "user:sam", inText: false),
                       "their message without the word is not a hit")
        XCTAssertFalse(hits(keyword: "canvas", pinned: sam, row: "user:me", inText: true),
                       "the word from someone else is not a hit")
    }

    /// No chip: the keyword decides alone, exactly as it does today.
    func testWithoutAChipTheKeywordDecidesAlone() {
        XCTAssertTrue(hits(keyword: "canvas", pinned: nil, row: "user:me", inText: true))
        XCTAssertFalse(hits(keyword: "canvas", pinned: nil, row: "user:me", inText: false))
    }

    /// Nothing typed and nothing pinned is not a search, so no row is a hit — this
    /// is what keeps the unfiltered transcript unfiltered.
    func testAnEmptyQueryHitsNothing() {
        XCTAssertFalse(hits(keyword: "", pinned: nil, row: "user:me", inText: true))
    }

    /// The sender is an identity, never the name on the chip: two people may share
    /// a display name, and a message carries `user:<id>` / `agent:<id>`.
    func testTheSenderIsComparedByIdentityAndNotByItsName() {
        XCTAssertFalse(hits(keyword: "", pinned: sam, row: "Sam Rivera", inText: true))
        let twin = sender("user:other", "Sam Rivera", "sam2")
        XCTAssertFalse(hits(keyword: "", pinned: sam, row: twin.key, inText: true),
                       "the same name is not the same sender")
    }

    // MARK: ⌫ on the field

    /// ⌫ with the caret in an empty field removes the chip, one press, no
    /// select-first step — `MentionUITextView.deleteBackward`'s convention, and the
    /// Mac's since `f3632d286`. With text in the field the key stays the field's,
    /// so a keyword is never traded for the chip, and it is never a close.
    func testBackspaceOnTheEmptyFieldRemovesThePinnedSender() {
        XCTAssertEqual(ChatSearchBackspace.apply(fieldEmpty: true, pinned: true), .unpinSender)
        XCTAssertEqual(ChatSearchBackspace.apply(fieldEmpty: false, pinned: true), .field,
                       "text to delete: the key is the field's")
        XCTAssertEqual(ChatSearchBackspace.apply(fieldEmpty: true, pinned: false), .field,
                       "nothing pinned and nothing typed: a no-op, not a close")
        XCTAssertEqual(ChatSearchBackspace.apply(fieldEmpty: false, pinned: false), .field)
    }

    // MARK: the chip's size

    /// The chip is a control inside the bar and its whole body removes the sender,
    /// so the target is the bar's height — over Apple's 44 pt minimum, which the
    /// 36 pt capsule alone would miss. The ✕ glyph beside the search field was
    /// already found too small once ([feedback iphone-search-close-2026-10-28]).
    func testTheChipsTapTargetClearsTheMinimum() {
        XCTAssertGreaterThanOrEqual(Theme.searchChipHit, 44)
        XCTAssertEqual(Theme.searchChipHit, Controls.height)
        XCTAssertLessThanOrEqual(Theme.searchChipHeight, Controls.height,
                                 "the capsule fits inside the bar")
        XCTAssertEqual(Theme.searchChipLeading,
                       (Theme.searchChipHeight - Theme.searchChipAvatar) / 2,
                       "the picture is inset evenly, like an avatar in a title pill")
    }

    /// With a chip up the field still has room to type in on the narrowest phone
    /// this ships to (375 pt). The Mac's bar is 520 pt wide and never has to care;
    /// this is the one number the phone cannot copy, which is why the label
    /// truncates instead of pushing the ✕ off the bar.
    func testTheFieldKeepsRoomBesideTheChipOnTheNarrowestPhone() {
        let bar: CGFloat = 375 - 2 * Controls.inset
        let chip = Theme.searchChipLeading + Theme.searchChipAvatar + Theme.searchChipGap
            + Theme.searchChipMaxLabel + Theme.searchChipTrailing
        let taken = Controls.textInset + Controls.glyph + Theme.searchChipGap + chip
            + ChatSearchClose.closeHitWidth
        XCTAssertGreaterThanOrEqual(bar - taken, 60,
                                    "field left: \(bar - taken) pt of \(bar)")
    }

    // MARK: the guard

    private func chatSearchSource() throws -> String {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Arbos")
        return try String(contentsOf: root.appendingPathComponent("Model/ChatSearchSenderPin.swift"),
                          encoding: .utf8)
    }

    /// The guard that matters, in the shape `HomeSearchTests` uses for the pickers:
    /// the sender half routes through the shared rules and holds no comparison of
    /// its own. Two of this surface's bugs were one rule written twice — a folding
    /// table and a reveal — so a second copy is the class of defect to prevent,
    /// not any one instance of it.
    func testTheSenderHalfRoutesThroughTheSharedRulesAndNowhereElse() throws {
        let src = try chatSearchSource()
        for call in ["chatSearchSenders(", "chatSearchOffer(", "chatSearchRowHits("] {
            XCTAssertTrue(src.contains(call), "the shared rule \(call) is not called")
        }
        for adHoc in ["lowercased()", "localizedCaseInsensitiveContains(", "HomeSearch.fold(",
                      "folding(options:", ".hasPrefix(", "EntitySearch."] {
            XCTAssertFalse(src.contains(adHoc),
                           "ad-hoc comparison \(adHoc): the sender half must ask the shared rules")
        }
    }
}
