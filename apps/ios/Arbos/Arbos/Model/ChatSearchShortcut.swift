// Hardware-keyboard ⌘F on the iPhone toggles the Search-in-chat bar
// ([feedback iphone-chat-search-cmd-f-2026-10-26] opened it, Jacob 2026-10-26;
// [feedback iphone-chat-search-cmd-f-toggle-2026-10-27] closes it again). The bar
// itself exists since #1829. The Mac twin is `ui/shell.rs` `chat_search::Open`.

import SwiftUI

enum ChatSearchShortcut {
    /// ⌘F — the Find key on every Apple platform.
    static let key: KeyEquivalent = "f"
    static let modifiers: EventModifiers = .command

    /// What one ⌘F does to the bar. A toggle ([feedback
    /// iphone-chat-search-cmd-f-toggle-2026-10-27], Jacob 2026-10-27: "search
    /// should close with command F again"): closed → open and focus the field;
    /// open → close it exactly like ✕ / Esc. The Mac does the same.
    struct Effect: Equatable {
        /// The bar is shown afterwards.
        let open: Bool
        /// The field takes the cursor (only when opening).
        let focus: Bool
        /// Slide the bar in / out.
        let animate: Bool
    }

    static func apply(open: Bool) -> Effect {
        Effect(open: !open, focus: !open, animate: true)
    }
}

/// How the Search-in-chat bar closes ([feedback iphone-search-close-2026-10-28], Jacob
/// 2026-10-28: "The x button doesn't work on the search on iPhone. Also clicking away from
/// the search should close it."). The ✕, a tap on the messages area and a swipe on an
/// empty-query list all close through one path (`closeSearch`: bar gone, query cleared,
/// keyboard down); a tap on a result closes and then reveals that message. The Mac bar
/// follows the same rule since [feedback search-toggle-click-outside-mac-2026-10-27].
enum ChatSearchClose {
    /// The ✕ cell: 44 wide, the bar's full height (the glyph alone was 17 pt).
    static let closeHitWidth: CGFloat = 44

    /// A tap anywhere on the transcript (empty chrome or a non-result row) closes the bar.
    static func tapOutsideCloses(open: Bool) -> Bool { open }

    /// A swipe on the list closes the bar only while nothing is typed: with a keyword the
    /// list is the result list, and scrolling results must keep the bar.
    static func scrollCloses(open: Bool, query: String) -> Bool {
        open && query.trimmingCharacters(in: .whitespaces).isEmpty
    }

    /// A tap on a row while a keyword filters the list: close, then reveal that message.
    static func resultTapReveals(open: Bool, query: String) -> Bool {
        open && !query.trimmingCharacters(in: .whitespaces).isEmpty
    }
}
