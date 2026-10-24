import SwiftUI

/// The message bubble's menu under the reaction capsule (`MessageActions.swift`):
/// **Copy, Reply, Forward, Reply in DM, Pin / Unpin (group chats when allowed),
/// Delete** (#1370 + #1472 + pin feedback); reactions live in the capsule, never
/// here (Jacob, 2026-10-23). Forward opens the same chat picker the media viewer
/// uses; Reply in DM opens a private chat with the human who wrote the message.
/// Pin is group-only and gated by the admin's "Who can pin" setting. Non-message
/// rows get `copyOnly` as a plain `.contextMenu`.
enum MessageMenuItem: CaseIterable, Equatable {
    case copy
    case reply
    case forward
    case replyInDM
    case pin
    case unpin
    case delete

    var label: String {
        switch self {
        case .copy: return "Copy"
        case .reply: return "Reply"
        case .forward: return "Forward"
        case .replyInDM: return "Reply in DM"
        case .pin: return "Pin"
        case .unpin: return "Unpin"
        case .delete: return "Delete"
        }
    }

    var systemImage: String {
        switch self {
        case .copy: return "doc.on.doc"
        case .reply: return "arrowshape.turn.up.left"
        case .forward: return "arrowshape.turn.up.right"
        case .replyInDM: return "bubble.left"
        case .pin: return "pin"
        case .unpin: return "pin.slash"
        case .delete: return "trash"
        }
    }

    var isDestructive: Bool { self == .delete }
}

enum MessageMenu {
    /// The items, in order, for every real message bubble (no pin — pin is
    /// added only in group chats when the member may pin).
    static let items: [MessageMenuItem] = [.copy, .reply, .forward, .replyInDM, .delete]

    /// Rows that are not a message in a shared doc (channels, date / system
    /// lines) can only be copied.
    static let copyOnly: [MessageMenuItem] = [.copy]

    /// Full bubble menu: optional Pin / Unpin in groups, Delete only when the
    /// relay would allow it (`MessageDeleteRule.mayDelete`) — a member without
    /// delete rights still gets Copy / Reply / Forward / Reply in DM.
    static func items(
        forMessage: Bool,
        canPin: Bool = false,
        isPinned: Bool = false,
        canDelete: Bool = true
    ) -> [MessageMenuItem] {
        guard forMessage else { return copyOnly }
        var out: [MessageMenuItem] = [.copy, .reply, .forward, .replyInDM]
        if canPin {
            out.append(isPinned ? .unpin : .pin)
        }
        if canDelete {
            out.append(.delete)
        }
        return out
    }
}

/// Who may delete a message for everyone — the relay's rule (`arbos-relay/src/moderation.rs`),
/// mirrored so the menu never offers a Delete the relay would refuse: your own always; in a
/// people group the owner (and admins) delete anyone's; an agent's message is its owner's to
/// delete; in a project chat the owner deletes anything, a guest their own. System lines are a
/// moderator's only.
enum MessageDeleteRule {
    static func mayDelete(sender: String, meId: String?, moderates: Bool, agentOwnerIds: [String: String]) -> Bool {
        if moderates { return true }
        guard let meId else { return false }
        if sender == "user" || sender == "user:\(meId)" { return true }
        if sender.hasPrefix("agent:") {
            let pid = String(sender.dropFirst(6))
            return agentOwnerIds[pid] == meId
        }
        return false
    }
}

/// The menu's buttons, for `.contextMenu { MessageMenuContent(...) }`.
struct MessageMenuContent: View {
    let items: [MessageMenuItem]
    let action: (MessageMenuItem) -> Void

    var body: some View {
        ForEach(items, id: \.self) { item in
            Button(role: item.isDestructive ? .destructive : nil) {
                action(item)
            } label: {
                Label(item.label, systemImage: item.systemImage)
            }
        }
    }
}

/// "Delete message?" — what the confirm says before a bubble is deleted
/// (feedback app-01M5MXQ90MEEWF6C8W090VCZHF). Delete is always for everyone:
/// the menu only offered it where `MessageDeleteRule` holds. A moderator
/// deleting someone else's message sees whose it is in the title.
enum MessageDeleteCopy {
    static let body = "This removes it from the chat for everyone. This can’t be undone."

    static func title(mine: Bool, sender: String) -> String {
        let name = sender.trimmingCharacters(in: .whitespacesAndNewlines)
        if mine || name.isEmpty { return "Delete message?" }
        return "Delete \(name)’s message?"
    }
}
