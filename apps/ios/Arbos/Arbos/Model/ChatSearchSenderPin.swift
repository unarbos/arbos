// Search-in-chat by who wrote a message ([feedback search-in-chat-mac-2026-10-26],
// Jacob 2026-10-26: "bring back command F to find things in a chat by key word,
// user name etc."). The Mac got both halves; the phone's bar had the keyword and
// nothing else, so searching a chat by user name was simply absent here.
//
// Every rule below is the Mac's, reached through ArbosFFI
// (`arbos_core::chat_search`) rather than restated in Swift: which rows a pinned
// sender keeps, how a pinned sender composes with the keyword, who a typed name
// offers, and that a pinned sender is never swapped by typing. Two of tonight's
// bugs were a rule written twice — a folding table and a reveal — so this file
// deliberately holds no comparison of its own.

import ArbosFFI
import Foundation

/// The sender half of Search-in-chat. Thin by design: every function here is one
/// call into the shared rules, so the phone and the Mac cannot answer the same
/// question differently.
enum ChatSearchSenderPin {
    /// Everyone who posted in the chat, one entry per identity, in the order
    /// they first appear — the pool a typed name is offered from. Pass one
    /// entry per loaded row, newest or oldest first as the transcript has them.
    static func posted(_ senders: [ChatSearchSender]) -> [ChatSearchSender] {
        chatSearchSenders(senders: senders)
    }

    /// The sender the typed text names, best match first (exact > prefix >
    /// word-start, display name before @handle) — the matcher every picker,
    /// @mention completion and the name half of home search already use, so a
    /// name cannot rank one way in the chip and another way in the app.
    ///
    /// Nothing is offered while `pinned` is set: a pinned sender is changed by
    /// removing the chip, never by typing over it. That is the Mac's behaviour,
    /// and whether typing should swap it is an open question — asked in the
    /// shared rules so neither client can settle it alone.
    static func offer(typed: String, in senders: [ChatSearchSender],
                      pinned: ChatSearchSender?) -> ChatSearchSender? {
        chatSearchOffer(typed: typed, senders: senders, pinned: pinned)
    }

    /// Does one row belong in the results? `keywordInText` is the keyword's own
    /// verdict on this row's body, which stays the phone's to decide
    /// (`HomeSearch.fold`, the fold its app-wide message search uses); the
    /// sender is compared as an identity and is never folded.
    static func rowHits(keyword: String, pinned: ChatSearchSender?,
                        rowSenderKey: String, keywordInText: Bool) -> Bool {
        chatSearchRowHits(keyword: keyword, senderKey: pinned?.key,
                          rowSenderKey: rowSenderKey, keywordInText: keywordInText)
    }

    /// What the chip reads: "From Sam Rivera", the Mac's wording.
    static func chipLabel(_ sender: ChatSearchSender) -> String { "From \(sender.name)" }
}

/// What ⌫ means in the Search-in-chat field.
///
/// The convention is the one this app already uses for a removable token on both
/// clients: remove at once, with no select-first step — the Mac's composer
/// attachment strip ([feedback composer-attachment-removal-2026-10-27]), the
/// Mac's own search chip (`f3632d286`), and on this phone
/// `MentionUITextView.deleteBackward`, which hands the key to its owner only
/// when the field has nothing to delete for.
///
/// With text in the field the key stays the field's, even with the caret at the
/// start where it deletes nothing, so a keyword is never traded for the chip.
/// There is never a second token to reach: at most one sender is pinned, and
/// the offered chip exists only while something is typed.
enum ChatSearchBackspace {
    enum Effect: Equatable {
        /// Take the pinned sender off and search again without it.
        case unpinSender
        /// The field's own ⌫ — which deletes nothing when the field is empty,
        /// so this is also how "nothing pinned, nothing typed" does nothing.
        /// It is never a close: ⌫ does not dismiss the bar.
        case field
    }

    static func apply(fieldEmpty: Bool, pinned: Bool) -> Effect {
        fieldEmpty && pinned ? .unpinSender : .field
    }
}
