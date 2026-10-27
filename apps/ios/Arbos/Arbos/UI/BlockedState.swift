import SwiftUI

// The Blocked state, one look everywhere (Jacob 2026-10-26, [feedback
// blocked-state-ios-2026-10-26]: "Today you can't tell in the app that you've blocked
// someone"). The Mac's `ui/blocked.rs` twin: a **Blocked** tag (the DM header pill, member
// rows), the sheet hero's **Blocked** chip, the **Unblock** confirm every surface asks
// before it lifts a block, the bar that stands in for a blocked DM's composer ("You blocked
// X · Unblock"), and the words the chat shows when the relay refuses a post because of a
// block. The block list is the relay's (`AppModel.blocked`, `GET /blocks`); nothing here
// keeps a local flag. Sizes come from the `Controls` tokens.

enum BlockedState {
    static let label = "Blocked"
    static let unblock = "Unblock"
    static let confirmNote = "They'll be able to message you and see you in groups again."

    static func confirmTitle(handle: String) -> String { "Unblock @\(handle)?" }

    /// "N messages hidden because you blocked @x" for the transcript's folded line.
    static func hiddenLine(count: Int, who: String) -> String {
        "\(count == 1 ? "1 message" : "\(count) messages") hidden because you blocked \(who)"
    }

    /// "You blocked X" for the composer bar.
    static func composerLine(name: String) -> String { "You blocked \(name)" }

    /// Is this person on my block list (by id, else by handle — the relay keys blocks by
    /// user id; rows that only carry a handle still match)?
    static func isBlocked(id: String, handle: String, in blocked: [Person]) -> Bool {
        BlockRow.entry(in: blocked, id: id, handle: handle) != nil
    }

    /// The other party's name for a sentence: the display name, else `@handle`.
    static func who(name: String?, handle: String?) -> String? {
        if let name = name?.trimmingCharacters(in: .whitespaces), !name.isEmpty { return name }
        if let handle, !handle.isEmpty { return "@\(handle)" }
        return nil
    }

    /// The plain sentence for a refused push, keyed on the relay's `reason`
    /// (`not_available` + `blocked_by` / `blocks_member` / `silenced`; nil from an older
    /// relay keeps the generic line). The Mac's `PushRefusal::sentence` twin.
    static func refusalSentence(reason: String?, name: String?, handle: String?) -> String {
        let who = who(name: name, handle: handle)
        switch (reason, who) {
        case ("blocked_by", let who?): return "\(who) has blocked you in this chat."
        case ("blocked_by", nil): return "Someone in this chat has blocked you."
        case ("blocks_member", let who?):
            return "Couldn't post: you've blocked \(who), who is in this chat. Unblock to post."
        case ("blocks_member", nil):
            return "Couldn't post: you've blocked someone in this chat. Unblock to post."
        case ("silenced", _): return "You're muted in this chat, so your messages aren't being delivered."
        default: return "Messages in this chat aren't being delivered right now. We'll keep trying."
        }
    }

    /// Only my own block has an Unblock to offer.
    static func offersUnblock(reason: String?) -> Bool { reason == "blocks_member" }
}

/// The small capsule after a name: a DM header pill, a member row.
struct BlockedTag: View {
    @Environment(\.theme) private var theme

    static let height: CGFloat = 18

    var body: some View {
        Text(BlockedState.label)
            .font(theme.font(11, weight: .semibold))
            // error-ui: exception status-badge
            .foregroundStyle(theme.danger)
            .padding(.horizontal, 7)
            .frame(height: Self.height)
            .background(Capsule().fill(theme.danger.opacity(0.14)))
            .accessibilityIdentifier("blocked-tag")
            .accessibilityLabel("Blocked")
    }
}

/// The sheet hero's chip under the @handle: glyph + "Blocked".
struct BlockedChip: View {
    @Environment(\.theme) private var theme

    static let height: CGFloat = 26

    var body: some View {
        HStack(spacing: 5) {
            Image(systemName: "nosign").font(.system(size: 12, weight: .semibold))
            Text(BlockedState.label).font(theme.font(Theme.tiny, weight: .semibold))
        }
        // error-ui: exception status-badge
        .foregroundStyle(theme.danger)
        .padding(.leading, 9)
        .padding(.trailing, 11)
        .frame(height: Self.height)
        .background(Capsule().fill(theme.danger.opacity(0.14)))
        .accessibilityIdentifier("blocked-chip")
        .accessibilityLabel("Blocked")
    }
}

/// The composer's stand-in for a DM with someone I blocked: "You blocked X" with an Unblock
/// button at the right — the composer field's height and capsule (`Controls`), its rim.
struct BlockedComposerBar: View {
    @Environment(\.theme) private var theme
    let name: String
    let onUnblock: () -> Void

    var body: some View {
        HStack(spacing: Controls.gap) {
            Image(systemName: "nosign")
                .font(.system(size: 15, weight: .medium))
                .foregroundStyle(theme.muted)
            Text(BlockedState.composerLine(name: name))
                .font(theme.font(Theme.small))
                .foregroundStyle(theme.muted)
                .lineLimit(1)
                .accessibilityIdentifier("composer-blocked-text")
            Spacer(minLength: 0)
            Button(action: onUnblock) {
                Text(BlockedState.unblock)
                    .font(theme.font(Theme.small, weight: .semibold))
                    .foregroundStyle(theme.accent)
                    .padding(.horizontal, 12)
                    .frame(height: Controls.height - 16)
                    .background(Capsule().fill(theme.bubbleAssistant))
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("composer-unblock")
            .accessibilityLabel("Unblock")
        }
        .padding(.leading, Controls.textInset)
        .padding(.trailing, 8)
        .frame(maxWidth: .infinity)
        .frame(height: Controls.height)
        .background(ControlSurface(shape: Capsule()))
        .accessibilityIdentifier("composer-blocked")
    }
}
