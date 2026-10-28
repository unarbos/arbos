import SwiftUI
import XCTest
@testable import Arbos

/// Hardware-keyboard ⌘F toggles the Search-in-chat bar ([feedback
/// iphone-chat-search-cmd-f-2026-10-26] opened it; [feedback
/// iphone-chat-search-cmd-f-toggle-2026-10-27] closes it again, Jacob 2026-10-27).
/// Sender chip + keyword ([feedback app-01M61GXS849K96HRER4YD0ZZ6R]).
final class ChatSearchShortcutTests: XCTestCase {
    func testTheShortcutIsCommandF() {
        XCTAssertEqual(ChatSearchShortcut.key, KeyEquivalent("f"))
        XCTAssertEqual(ChatSearchShortcut.modifiers, .command)
    }

    func testCommandFOpensAClosedBarAndFocusesIt() {
        let effect = ChatSearchShortcut.apply(open: false)
        XCTAssertEqual(effect, .init(open: true, focus: true, animate: true))
    }

    /// Jacob 2026-10-27: "search should close with command F again" — a toggle,
    /// the same as the Mac's ⌘F ([feedback iphone-chat-search-cmd-f-toggle-2026-10-27]).
    func testCommandFOnAnOpenBarClosesIt() {
        let effect = ChatSearchShortcut.apply(open: true)
        XCTAssertFalse(effect.open, "⌘F on the open bar closes it like ✕ / Esc")
        XCTAssertFalse(effect.focus, "nothing to focus when closing")
    }

    func testToggleIsAnInvolution() {
        XCTAssertTrue(ChatSearchShortcut.apply(open: ChatSearchShortcut.apply(open: false).open).open == false)
        XCTAssertTrue(ChatSearchShortcut.apply(open: ChatSearchShortcut.apply(open: true).open).open == true)
    }

    /// The view wires the shortcut (and Esc) to the bar, not to the toggling disc.
    func testChatViewCarriesTheShortcutAndEscape() throws {
        let chatView = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Arbos/UI/ChatView.swift")
        let source = try String(contentsOf: chatView, encoding: .utf8)
        XCTAssertTrue(
            source.contains(".keyboardShortcut(ChatSearchShortcut.key, modifiers: ChatSearchShortcut.modifiers)"),
            "ChatView must bind ⌘F through ChatSearchShortcut")
        XCTAssertTrue(source.contains("Button(action: openSearchFromKeyboard)"), "⌘F toggles the bar")
        XCTAssertTrue(source.contains("if !effect.open {\n            closeSearch()"), "an open bar closes through closeSearch (✕ / Esc path)")
        XCTAssertTrue(source.contains(".keyboardShortcut(.cancelAction)"), "Esc closes the bar like the Mac's")
        XCTAssertTrue(source.contains("searchPinnedSender"), "closeSearch and the bar clear the pinned sender")
        XCTAssertTrue(source.contains("ChatSearchFilter.hits"), "visibleRows goes through the shared helper")
        XCTAssertTrue(source.contains("ChatSearchBar("), "bar UI is the extracted ChatSearchBar")
    }

    // MARK: - Sender + keyword ([feedback app-01M61GXS849K96HRER4YD0ZZ6R])

    private func sender(_ key: String, _ name: String, _ handle: String) -> ChatSearchFilter.Sender {
        .init(key: key, name: name, handle: handle)
    }

    func testKeywordHitsIgnoreCaseAndAccents() {
        XCTAssertTrue(
            ChatSearchFilter.hits(text: "Keep the Canvas infinite", senderKey: "a", keyword: "canvas", pinned: nil))
        XCTAssertTrue(
            ChatSearchFilter.hits(text: "the cánvas spec", senderKey: "a", keyword: "Canvas", pinned: nil))
        XCTAssertFalse(
            ChatSearchFilter.hits(text: "no match here", senderKey: "a", keyword: "canvas", pinned: nil))
        XCTAssertFalse(
            ChatSearchFilter.hits(text: "anything", senderKey: "a", keyword: "  ", pinned: nil),
            "nothing typed and no sender = no filter hit")
        XCTAssertFalse(ChatSearchFilter.isFiltering(keyword: "  ", pinned: nil))
    }

    func testSenderAloneFiltersThatPersonsMessages() {
        let affine = sender("agent:a1", "Affine", "affine")
        XCTAssertTrue(
            ChatSearchFilter.hits(text: "Three decisions", senderKey: "agent:a1", keyword: "", pinned: affine))
        XCTAssertFalse(
            ChatSearchFilter.hits(text: "Snap at 8", senderKey: "user:me", keyword: "", pinned: affine),
            "my message is not Affine's")
        XCTAssertTrue(ChatSearchFilter.isFiltering(keyword: "", pinned: affine))
    }

    func testSenderAndKeywordAndTogether() {
        let affine = sender("agent:a1", "Affine", "affine")
        let me = sender("user:me", "Jacob Steeves", "unarbos")
        XCTAssertTrue(
            ChatSearchFilter.hits(
                text: "move export to the share sheet", senderKey: "agent:a1", keyword: "export", pinned: affine),
            "export from Affine hits")
        XCTAssertFalse(
            ChatSearchFilter.hits(
                text: "Export keeps the background.", senderKey: "user:me", keyword: "export", pinned: affine),
            "export from me does not hit while Affine is pinned")
        XCTAssertTrue(
            ChatSearchFilter.hits(
                text: "Export keeps the background.", senderKey: "user:me", keyword: "export", pinned: me))
    }

    func testOfferResolvesNamesAndHandles() {
        let all = [
            sender("user:me", "Jacob Steeves", "unarbos"),
            sender("agent:a1", "Affine", "affine"),
            sender("user:sam", "Sam Rivera", "sam"),
        ]
        XCTAssertEqual(ChatSearchFilter.offer(typed: "aff", senders: all)?.name, "Affine")
        XCTAssertEqual(ChatSearchFilter.offer(typed: "@sam", senders: all)?.name, "Sam Rivera")
        XCTAssertEqual(
            ChatSearchFilter.offer(typed: "rivera", senders: all)?.name, "Sam Rivera",
            "word start")
        XCTAssertNil(ChatSearchFilter.offer(typed: "canvas", senders: all))
        XCTAssertNil(ChatSearchFilter.offer(typed: "", senders: all))
        XCTAssertNil(ChatSearchFilter.offer(typed: "   ", senders: all))
    }

    func testUserRowKeysToMe() {
        let me = "user:me"
        XCTAssertEqual(ChatSearchFilter.senderKey("user", me: me), me)
        XCTAssertEqual(ChatSearchFilter.senderKey("user:me", me: me), me)
        XCTAssertEqual(ChatSearchFilter.senderKey("", me: me), me)
        XCTAssertEqual(ChatSearchFilter.senderKey("user:sam", me: me), "user:sam")
        XCTAssertEqual(ChatSearchFilter.senderKey("agent:a1", me: me), "agent:a1")
    }
}
