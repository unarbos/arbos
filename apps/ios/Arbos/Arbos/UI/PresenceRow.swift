// The one presence row over the composer ([feedback
// typing-indicator-left-2026-10-23], Jacob 2026-10-23): who is typing (people)
// or working (agents) in the open chat, as a stack of round profile pictures
// and ONE line of text — "Sam is typing…", "Lium and Home are working" — that
// starts at the chat column's left edge like the bubbles. Mirrors the Mac's
// `ui/presence.rs`; it replaces the "<name> is working" transcript row.

import SwiftUI

enum PresenceActivity: Equatable {
    case typing
    case working
}

struct PresenceParticipant: Identifiable {
    let id: String
    let name: String
    let activity: PresenceActivity
    let picture: AnyView
}

/// The row's words, shared with the tests. Same rules as the Mac
/// (`presence::sentence`): one clause per activity, joined with " · ";
/// a single worker shows the harness's `detail` or "is waiting for the machine".
enum PresenceLine {
    /// "Sam Okafor" → "Sam"; "@sam" and one-word names stay as they are.
    static func firstName(_ name: String) -> String {
        name.split(separator: " ").first.map(String.init) ?? name
    }

    static func join(_ names: [String]) -> String {
        switch names.count {
        case 0: return ""
        case 1: return names[0]
        case 2: return "\(names[0]) and \(names[1])"
        case 3: return "\(names[0]), \(names[1]) and \(names[2])"
        default: return "\(names[0]), \(names[1]) and \(names.count - 2) others"
        }
    }

    static func sentence(typing: [String], working: [String], detail: String? = nil, queued: Bool = false) -> String {
        var clauses: [String] = []
        if !typing.isEmpty {
            clauses.append("\(join(typing)) \(typing.count == 1 ? "is" : "are") typing…")
        }
        if !working.isEmpty {
            if working.count == 1, queued {
                clauses.append("\(working[0]) is waiting for the machine")
            } else if working.count == 1, let d = detail?.trimmingCharacters(in: .whitespacesAndNewlines), !d.isEmpty {
                clauses.append(d)
            } else {
                clauses.append("\(join(working)) \(working.count == 1 ? "is" : "are") working")
            }
        }
        return clauses.joined(separator: " · ")
    }
}

struct PresenceRow: View {
    @Environment(\.theme) private var theme
    let participants: [PresenceParticipant]
    var detail: String? = nil
    var queued: Bool = false

    /// Round picture size: the avatar column's disc — the sender's picture beside a
    /// people-chat bubble, centred on the composer's `+` (`Theme.memberAvatar`).
    static let avatar: CGFloat = Theme.memberAvatar
    /// Where the first picture starts inside the 16 pt chat edge (= the `+` column,
    /// which moves with the keyboard — read from the environment).
    @Environment(\.composerInset) private var composerInset
    var inset: CGFloat { Theme.avatarColumnInset(composerInset: composerInset) }
    /// How far each further picture tucks under the one before it.
    static let overlap: CGFloat = 8
    /// Pictures → text: the picture → bubble gap, so the text starts where the bubble body starts.
    static let gap: CGFloat = Theme.memberAvatarGap
    /// Pictures shown before "+N".
    static let maxPictures = 3
    static let height: CGFloat = avatar + 6

    /// One line. Alone, a participant keeps the full display name; from two on,
    /// first names ("Mira and Sam are typing…"), so the line stays one line.
    var text: String {
        let several = participants.count > 1
        let name: (PresenceParticipant) -> String = { several ? PresenceLine.firstName($0.name) : $0.name }
        return PresenceLine.sentence(
            typing: participants.filter { $0.activity == .typing }.map(name),
            working: participants.filter { $0.activity == .working }.map(name),
            detail: detail,
            queued: queued
        )
    }

    private var ring: some View {
        Circle().strokeBorder(theme.bg, lineWidth: 1).padding(-1)
    }

    private var shown: [PresenceParticipant] { Array(participants.prefix(Self.maxPictures)) }
    private var more: Int { max(0, participants.count - Self.maxPictures) }

    var body: some View {
        HStack(spacing: Self.gap) {
            HStack(spacing: -Self.overlap) {
                // The picture is the bubble picture's exact box; with several, a ring in
                // the page colour sits just *outside* each one so the overlaps read.
                ForEach(shown) { p in
                    p.picture
                        .frame(width: Self.avatar, height: Self.avatar)
                        .clipShape(Circle())
                        .overlay { if participants.count > 1 { ring } }
                        .accessibilityIdentifier("presence-picture")
                }
                if more > 0 {
                    ZStack {
                        Circle().fill(theme.field)
                        Text("+\(more)")
                            .font(theme.font(10, weight: .medium))
                            .foregroundStyle(theme.muted)
                    }
                    .frame(width: Self.avatar, height: Self.avatar)
                    .overlay(ring)
                    .accessibilityIdentifier("presence-more")
                }
            }
            .frame(height: Self.avatar)
            WorkingStatusText(text: text, color: queued ? theme.warn : theme.muted)
                .lineLimit(1)
                .id(text)
            Spacer(minLength: 0)
        }
        .padding(.leading, inset)
        .frame(height: Self.height)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(text)
        .accessibilityIdentifier("presence-row")
        .animation(.easeInOut(duration: 0.25), value: text)
    }
}
