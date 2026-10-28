import SwiftUI
import XCTest
@testable import Arbos

/// Hardware-keyboard ⌘F toggles the Search-in-chat bar ([feedback
/// iphone-chat-search-cmd-f-2026-10-26] opened it; [feedback
/// iphone-chat-search-cmd-f-toggle-2026-10-27] closes it again, Jacob 2026-10-27).
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
    }
}
