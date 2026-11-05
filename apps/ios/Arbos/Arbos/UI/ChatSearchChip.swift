// Search-in-chat's sender chips, the Mac's two ([feedback
// search-in-chat-mac-2026-10-26]): the pinned chip the results are filtered by,
// and the chip a typed name offers before it is pinned.
//
// Every rule behind them is in `ChatSearchSenderPin`, which asks the shared Rust
// (`arbos_core::chat_search`). These are the views and nothing else — no
// comparison, no ranking, no decision about what a name means.

import ArbosFFI
import SwiftUI

/// The pinned sender: an accent capsule reading "From Sam Rivera", whose whole
/// body removes it. The Mac's chip is one click too, and the target here is the
/// bar's height rather than the capsule's 36, since 36 misses Apple's 44 pt
/// minimum — the ✕ beside this very field was already found too small once
/// ([feedback iphone-search-close-2026-10-28]).
struct ChatSearchPinnedChip: View {
    @Environment(\.theme) private var theme
    let sender: ChatSearchSender
    let remove: () -> Void

    var body: some View {
        Button(action: remove) {
            HStack(spacing: Theme.searchChipGap) {
                PersonAvatar(url: sender.avatarUrl, handle: sender.handle,
                             size: Theme.searchChipAvatar, name: sender.name, id: sender.key)
                Text(ChatSearchSenderPin.chipLabel(sender))
                    .font(theme.font(Theme.tiny, weight: .medium))
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: Theme.searchChipMaxLabel, alignment: .leading)
                Image(systemName: "xmark")
                    .font(.system(size: 10, weight: .semibold))
            }
            .foregroundStyle(theme.accent)
            .padding(.leading, Theme.searchChipLeading)
            .padding(.trailing, Theme.searchChipTrailing)
            .frame(height: Theme.searchChipHeight)
            .background(Capsule().fill(theme.accent.opacity(0.18)))
            // The capsule is 36 and the gesture wants 44: the hit frame is the
            // bar's height around it, which is also why the chip can sit in a
            // 48 pt bar without the field losing its own target.
            .frame(height: Theme.searchChipHit)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Remove \(sender.name) from the search")
        .accessibilityIdentifier("chat-search-sender")
    }
}

/// The sender a typed name offers, before it is pinned: the same capsule in the
/// text's own colour with an outline, tapped to pin. The Mac offers ⇥ as well;
/// this phone has no ⇥, so the tap is the whole gesture.
struct ChatSearchOfferChip: View {
    @Environment(\.theme) private var theme
    let sender: ChatSearchSender
    let pin: () -> Void

    var body: some View {
        Button(action: pin) {
            HStack(spacing: Theme.searchChipGap) {
                PersonAvatar(url: sender.avatarUrl, handle: sender.handle,
                             size: Theme.searchChipAvatar, name: sender.name, id: sender.key)
                Text(ChatSearchSenderPin.chipLabel(sender))
                    .font(theme.font(Theme.tiny, weight: .medium))
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: Theme.searchChipMaxLabel, alignment: .leading)
            }
            .foregroundStyle(theme.text)
            .padding(.leading, Theme.searchChipLeading)
            .padding(.trailing, Theme.searchChipTrailing)
            .frame(height: Theme.searchChipHeight)
            .background(
                Capsule().fill(theme.text.opacity(0.08))
                    .overlay(Capsule().strokeBorder(theme.text.opacity(0.18), lineWidth: 1)))
            .frame(height: Theme.searchChipHit)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Search only \(sender.name)")
        .accessibilityIdentifier("chat-search-offer")
    }
}
