import SwiftUI

/// The message bubble's menu under the reaction capsule (`MessageActions.swift`):
/// **Copy, Reply, View thread, Forward, Reply in DM, Edit (own messages), Delete**
/// (#1370 + #1472; View thread when the message has replies); reactions live in
/// the capsule, never here (Jacob, 2026-10-23). Forward opens the same chat
/// picker the media viewer uses; Reply in DM opens a private chat with the human
/// who wrote the message. Non-message rows get `copyOnly` as a plain `.contextMenu`.
enum MessageMenuItem: CaseIterable, Equatable {
    case copy
    case reply
    case viewThread
    case forward
    case replyInDM
    case edit
    /// The message shows a link preview card and the user may delete the message: drop the
    /// card for everyone ([feedback link-embeds-ios-2026-10-27]).
    case removePreview
    case delete

    var label: String {
        switch self {
        case .copy: return "Copy"
        case .reply: return "Reply"
        case .viewThread: return "View thread"
        case .forward: return "Forward"
        case .replyInDM: return "Reply in DM"
        case .edit: return "Edit"
        case .removePreview: return "Remove preview"
        case .delete: return "Delete"
        }
    }

    var systemImage: String {
        switch self {
        case .copy: return "doc.on.doc"
        case .reply: return "arrowshape.turn.up.left"
        case .viewThread: return "bubble.left.and.bubble.right"
        case .forward: return "arrowshape.turn.up.right"
        case .replyInDM: return "bubble.left"
        case .edit: return "pencil"
        case .removePreview: return "link"
        case .delete: return "trash"
        }
    }

    var isDestructive: Bool { self == .delete }
}

enum MessageMenu {
    /// The items, in order, for every real message bubble (Edit / Delete / View
    /// thread are gated per row via `items(forMessage:canEdit:canDelete:hasReplies:)`).
    static let items: [MessageMenuItem] = [.copy, .reply, .forward, .replyInDM, .delete]

    /// Same as `items`, plus View thread after Reply when the message has replies
    /// (no Edit — that still comes from `canEdit` on the row).
    static let itemsWithThread: [MessageMenuItem] = [.copy, .reply, .viewThread, .forward, .replyInDM, .delete]

    /// Rows that are not a message in a shared doc (channels, date / system
    /// lines) can only be copied.
    static let copyOnly: [MessageMenuItem] = [.copy]

    static func items(forMessage: Bool) -> [MessageMenuItem] {
        items(forMessage: forMessage, canEdit: false, canDelete: true, hasReplies: false)
    }

    /// View thread when `hasReplies`; Edit only on the user's own text; Delete only
    /// when the relay would allow it (`MessageDeleteRule.mayDelete`); Remove preview only on a
    /// message that shows a link card (`hasPreview`) the user may delete.
    static func items(forMessage: Bool, canEdit: Bool = false, canDelete: Bool = true,
                      hasReplies: Bool = false, hasPreview: Bool = false) -> [MessageMenuItem] {
        guard forMessage else { return copyOnly }
        var out: [MessageMenuItem] = [.copy, .reply]
        if hasReplies { out.append(.viewThread) }
        out.append(contentsOf: [.forward, .replyInDM])
        if canEdit { out.append(.edit) }
        if canDelete && hasPreview { out.append(.removePreview) }
        if canDelete { out.append(.delete) }
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

/// The menu's buttons, for `.contextMenu { MessageMenuContent(...) }` and the
/// long-press overlay under the reaction capsule.
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

/// Long-press → View thread: the held message and every reply in its chain.
struct MessageThreadSheet: View {
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let root: Message
    let replies: [Message]

    private var title: String {
        replies.count == 1 ? "1 reply" : "\(replies.count) replies"
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    threadRow(root, showCount: true)
                    ForEach(replies) { message in
                        threadRow(message, showCount: false)
                    }
                }
                .padding(16)
            }
            .background(theme.bg)
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                        .foregroundStyle(theme.text)
                }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .presentationBackground(theme.bg)
        .preferredColorScheme(theme.isDark ? .dark : .light)
    }

    private func threadRow(_ message: Message, showCount: Bool) -> some View {
        let mine = message.role == .user
        return VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(senderLabel(message))
                    .font(theme.font(Theme.tiny, weight: .semibold))
                    .foregroundStyle(mine ? theme.userText.opacity(0.7) : theme.muted)
                    .lineLimit(1)
                Spacer(minLength: 0)
                if showCount, !replies.isEmpty {
                    Text("\(replies.count)")
                        .font(theme.font(11, weight: .semibold))
                        .foregroundStyle(mine ? theme.userText.opacity(0.7) : theme.muted)
                        .accessibilityLabel("\(replies.count) \(replies.count == 1 ? "reply" : "replies")")
                }
            }
            Text(bodyText(message))
                .font(theme.font(Theme.body))
                .foregroundStyle(mine ? theme.userText : theme.text)
                .frame(maxWidth: .infinity, alignment: .leading)
                .textSelection(.enabled)
        }
        .padding(.horizontal, Theme.bubbleHPad)
        .padding(.vertical, Theme.bubbleVPad)
        .background(
            RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous)
                .fill(mine ? theme.bubbleUser : theme.bubbleAssistant)
        )
    }

    private func senderLabel(_ message: Message) -> String {
        if let handle = message.senderHandle?.trimmingCharacters(in: .whitespaces), !handle.isEmpty {
            return "@\(handle)"
        }
        if let name = message.senderName?.trimmingCharacters(in: .whitespaces), !name.isEmpty {
            return name
        }
        switch message.role {
        case .user: return "You"
        case .assistant, .project: return "Agent"
        case .system: return "System"
        }
    }

    private func bodyText(_ message: Message) -> String {
        let text = message.plainText.trimmingCharacters(in: .whitespacesAndNewlines)
        if !text.isEmpty { return text }
        for part in message.parts {
            if case .attachment(_, let a) = part { return a.name }
        }
        return "Message"
    }
}
