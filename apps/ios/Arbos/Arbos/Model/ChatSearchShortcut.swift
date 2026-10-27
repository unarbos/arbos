// Hardware-keyboard ⌘F on the iPhone opens the Search-in-chat bar
// ([feedback iphone-chat-search-cmd-f-2026-10-26], Jacob 2026-10-26: "bring back
// command F to find things in a chat"). The bar itself exists since #1829; only the
// header's search disc raised it, so a hardware keyboard on the phone had no way in.
// The Mac twin is `ui/shell.rs` `OpenChatSearch` (⌘F there too).

import SwiftUI

enum ChatSearchShortcut {
    /// ⌘F — the Find key on every Apple platform.
    static let key: KeyEquivalent = "f"
    static let modifiers: EventModifiers = .command

    /// What one ⌘F does to the bar.
    struct Effect: Equatable {
        /// The bar is shown afterwards (always: Find's key never closes the bar —
        /// Esc and the ✕ do).
        let open: Bool
        /// The field takes the cursor (always, so a second ⌘F while the bar is up
        /// brings the cursor back from the composer).
        let focus: Bool
        /// Slide the bar in only when it was closed; an open bar must not flicker.
        let animate: Bool
    }

    static func apply(open: Bool) -> Effect {
        Effect(open: true, focus: true, animate: !open)
    }
}
