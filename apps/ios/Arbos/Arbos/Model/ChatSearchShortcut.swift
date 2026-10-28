// Hardware-keyboard ⌘F on the iPhone toggles the Search-in-chat bar
// ([feedback iphone-chat-search-cmd-f-2026-10-26] opened it, Jacob 2026-10-26;
// [feedback iphone-chat-search-cmd-f-toggle-2026-10-27] closes it again). The bar
// itself exists since #1829. The Mac twin is `ui/shell.rs` `chat_search::Open`.
//
// Sender chip + keyword ([feedback app-01M61GXS849K96HRER4YD0ZZ6R]): the bar
// filters by who wrote a message as well as by text, mirroring
// `crates/arbos-mac/src/chat_search.rs` (`Query::hits_row`, `offer_sender`).

import ArbosFFI
import Foundation
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

/// Search-in-chat query rules ([feedback app-01M61GXS849K96HRER4YD0ZZ6R]): a
/// keyword and an optional pinned sender. Pure so the tests reach it; the Mac
/// twin is `crates/arbos-mac/src/chat_search.rs`.
enum ChatSearchFilter {
    /// Someone who can write in the chat: the chip's name and handle, and the
    /// key a message's sender is compared by.
    struct Sender: Equatable, Hashable, Identifiable {
        /// Stable identity (`user:<id>`, `agent:<id>`) — never shown.
        var key: String
        var name: String
        /// Without the `@`; may be empty (an agent without a slug).
        var handle: String
        var id: String { key }
    }

    /// True while there is something to search for: a keyword and/or a pinned sender.
    static func isFiltering(keyword: String, pinned: Sender?) -> Bool {
        !EntitySearch.fold(keyword).isEmpty || pinned != nil
    }

    /// Does this message hit the query? With a pinned sender the message key
    /// must equal its key, AND the keyword is empty or the folded text contains
    /// the folded keyword. Empty keyword + a sender = all of that person's
    /// messages. Nothing typed and no sender = no hit (the view shows every row).
    static func hits(text: String, senderKey: String, keyword: String, pinned: Sender?) -> Bool {
        let foldedKeyword = EntitySearch.fold(keyword)
        if foldedKeyword.isEmpty && pinned == nil { return false }
        if let pinned, senderKey != pinned.key { return false }
        return foldedKeyword.isEmpty || EntitySearch.fold(text).contains(foldedKeyword)
    }

    /// The sender the typed text names, if any: best `EntitySearch.match` on
    /// name and handle (exact > prefix > word-start, name before handle). Nil
    /// for empty text or no match. Leading `@` is dropped by the matcher.
    static func offer(typed: String, senders: [Sender]) -> Sender? {
        if EntitySearch.normalize(typed).isEmpty { return nil }
        return senders
            .compactMap { s -> (UInt8, Sender)? in
                guard let m = EntitySearch.match(name: s.name, handle: s.handle, query: typed) else {
                    return nil
                }
                return (m.order, s)
            }
            .min(by: { $0.0 < $1.0 })
            .map(\.1)
    }

    /// Key a message by `Message.sender`; my own rows (`"user"` or `user:<my id>`)
    /// count as me (`me` is `user:<id>`).
    static func senderKey(_ sender: String, me: String?) -> String {
        if sender == "user" || sender.isEmpty { return me ?? "user" }
        if let me, sender == me { return me }
        return sender
    }

    /// Senders for the menu from a people chat's members: me included; agents
    /// without a project skipped (they cannot reply — same cut as member chips).
    static func senders(members: [ChatMember]) -> [Sender] {
        members.compactMap { m in
            if m.isAgent && m.project == nil { return nil }
            let handle = m.isAgent ? mentionSlug(m.project?.name ?? m.handle) : m.handle
            return Sender(key: m.member, name: m.displayName, handle: handle)
        }
    }
}
