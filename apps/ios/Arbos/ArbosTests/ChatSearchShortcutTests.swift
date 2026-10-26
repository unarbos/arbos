import SwiftUI
import XCTest
@testable import Arbos

/// Hardware-keyboard ⌘F opens the Search-in-chat bar ([feedback
/// iphone-chat-search-cmd-f-2026-10-26], Jacob 2026-10-26). Before this the bar
/// (#1829) opened only from the header's search disc; no `keyboardShortcut` existed
/// anywhere in the app.
final class ChatSearchShortcutTests: XCTestCase {
    func testTheShortcutIsCommandF() {
        XCTAssertEqual(ChatSearchShortcut.key, KeyEquivalent("f"))
        XCTAssertEqual(ChatSearchShortcut.modifiers, .command)
    }

    func testCommandFOpensAClosedBarAndFocusesIt() {
        let effect = ChatSearchShortcut.apply(open: false)
        XCTAssertEqual(effect, .init(open: true, focus: true, animate: true))
    }

    func testCommandFOnAnOpenBarRefocusesWithoutClosingOrFlicker() {
        let effect = ChatSearchShortcut.apply(open: true)
        XCTAssertTrue(effect.open, "Find's key never closes the bar — Esc and ✕ do")
        XCTAssertTrue(effect.focus)
        XCTAssertFalse(effect.animate, "an open bar must not slide in again")
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
        XCTAssertTrue(source.contains("Button(action: openSearchFromKeyboard)"), "⌘F opens or refocuses, never toggles")
        XCTAssertTrue(source.contains(".keyboardShortcut(.cancelAction)"), "Esc closes the bar like the Mac's")
    }
}
