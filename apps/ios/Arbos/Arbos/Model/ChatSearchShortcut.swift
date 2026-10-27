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
