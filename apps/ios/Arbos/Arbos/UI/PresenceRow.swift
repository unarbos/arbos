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
    /// The profile the picture opens (`ProfileToggle`, [feedback
    /// profile-pane-toggle-ios-2026-10-26]): the person or the agent; `nil`
    /// for a face the chat cannot name, which stays inert.
    var target: ProfileTarget? = nil
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
    /// Duplex §2.4: open background jobs in the chat. With `onTasks` set the row carries
    /// the tasks chip ("2 tasks") after its words, the box's "· N tasks" suffix leaves the
    /// line (the chip says the count), and a tap on the row opens the tasks sheet.
    var taskCount: Int = 0
    var onTasks: (() -> Void)? = nil
    /// Job tabs: a tap on the words opens the newest job's tab (the chip keeps the list).
    var onWords: (() -> Void)? = nil
    /// A tap on a picture: the one profile action (`AppModel.toggleProfile`); the
    /// label beside it says what the tap does ("Open Sam's profile" / "Close profile").
    var onPicture: ((ProfileTarget) -> Void)? = nil
    var pictureShowing: ((ProfileTarget) -> Bool)? = nil

    /// Round picture size: Grok's status-row picture ([feedback
    /// iphone-composer-grok-parity-2026-10-25]: 76 px — smaller than the `+` disc, no
    /// longer the bubble's 42 pt disc).
    static let avatar: CGFloat = Theme.presenceAvatar
    /// Where the first picture starts inside the 16 pt chat edge: 6 pt inside the `+`
    /// disc's left edge, which moves with the keyboard — read from the environment.
    @Environment(\.composerInset) private var composerInset
    var inset: CGFloat { Theme.presenceInset(composerInset: composerInset) }
    /// How far each further picture tucks under the one before it.
    static let overlap: CGFloat = 8
    /// Pictures → text: Grok's 42 px.
    static let gap: CGFloat = Theme.presenceGap
    /// Pictures shown before "+N".
    static let maxPictures = 3
    /// The row is exactly the picture tall; ChatView adds Grok's 68 px below it.
    static let height: CGFloat = avatar
    /// The words: Grok's 17 pt body, muted.
    static let textSize: CGFloat = Theme.body

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

    /// The words drawn: with a chip on the row the count moves into the chip.
    var shownText: String { hasChip ? Job.stripCountSuffix(text) : text }
    var hasChip: Bool { onTasks != nil && taskCount > 0 }

    /// The chip: Grok has no counterpart (its status line is a plain shimmer); it follows
    /// the mention chip — the accent on a 14 % accent capsule, the tiny type.
    static let chipHeight: CGFloat = 20
    static let chipPadX: CGFloat = 8
    static let chipAlpha: Double = 0.14

    private var ring: some View {
        Circle().strokeBorder(theme.bg, lineWidth: 1).padding(-1)
    }

    /// The face as a profile target when the row can name it and the chat
    /// handed over the action; a plain picture otherwise.
    @ViewBuilder
    private func picture(_ p: PresenceParticipant) -> some View {
        if let target = p.target, let onPicture {
            Button { onPicture(target) } label: { p.picture }
                .buttonStyle(.plain)
                .accessibilityLabel(ProfileToggle.label(name: p.name, showing: pictureShowing?(target) ?? false))
        } else {
            p.picture
        }
    }

    private var shown: [PresenceParticipant] { Array(participants.prefix(Self.maxPictures)) }
    private var more: Int { max(0, participants.count - Self.maxPictures) }

    var body: some View {
        HStack(spacing: Self.gap) {
            HStack(spacing: -Self.overlap) {
                // The picture is the bubble picture's exact box; with several, a ring in
                // the page colour sits just *outside* each one so the overlaps read.
                ForEach(shown) { p in
                    picture(p)
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
            WorkingStatusText(text: shownText, color: queued ? theme.warn : theme.muted, size: Self.textSize)
                .lineLimit(1)
                .id(shownText)
                .contentShape(Rectangle())
                .onTapGesture { (onWords ?? onTasks)?() }
                .accessibilityIdentifier(onWords != nil ? "presence-link" : "presence-text")
            if hasChip {
                tasksChip
                    .contentShape(Capsule())
                    .onTapGesture { onTasks?() }
            }
            Spacer(minLength: 0)
        }
        .padding(.leading, inset)
        .frame(height: Self.height)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(text)
        .accessibilityHint(hasChip ? "Opens the tasks" : "")
        .accessibilityIdentifier("presence-row")
        .animation(.easeInOut(duration: 0.25), value: shownText)
    }

    private var tasksChip: some View {
        HStack(spacing: 3) {
            Text(Job.countLabel(taskCount))
                .font(theme.font(11.5, weight: .medium))
            Image(systemName: "chevron.up")
                .font(.system(size: 9, weight: .semibold))
        }
        .foregroundStyle(theme.accent)
        .padding(.horizontal, Self.chipPadX)
        .frame(height: Self.chipHeight)
        .background(Capsule().fill(theme.accent.opacity(Self.chipAlpha)))
        .accessibilityIdentifier("tasks-chip")
    }
}

// MARK: - Tasks sheet (duplex §2.4, D3 / D7; feedback tasks-chip-drawer-ios-2026-10-25)

/// The agent's open background jobs, one card row each: title over "<step> · m:ss",
/// **Steer** (running jobs: the composer becomes a reply to the job's ask — no command of
/// its own) and **Stop** (`stop { jobId }`, that job only). **Stop all** in the bar is the
/// Stop button's `stop {}`: the turn and every job of the chat. The Mac's `ui::tasks`
/// drawer, as a bottom sheet. Finished jobs are not here — their outcome is a bubble.
struct TasksSheet: View {
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let jobs: [Job]
    let onSteer: (Job) -> Void
    let onStop: (Job) -> Void
    let onStopAll: () -> Void
    /// A tap on the row (not its buttons) opens the job's tab (`JobTabs`).
    var onOpen: ((Job) -> Void)? = nil

    /// A running row's spinner (the agent pane's Coding row, 15 pt) / a queued row's pause mark.
    static let glyph: CGFloat = 15
    static let rowPadY: CGFloat = 10
    /// Steer / Stop capsules.
    static let buttonHeight: CGFloat = 28
    static let buttonPadX: CGFloat = 10

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 0) {
                    ForEach(Array(jobs.enumerated()), id: \.element.id) { i, job in
                        row(job)
                        if i < jobs.count - 1 {
                            Rectangle().fill(theme.separator).frame(height: 0.5)
                                .padding(.leading, SheetRow.paddingX + Self.glyph + 12)
                        }
                    }
                }
                .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(theme.card))
                .padding(.horizontal, SheetRow.cardInset)
                .padding(.top, 8)
                Color.clear.frame(height: 24)
            }
            .background(theme.bg)
            .navigationTitle("Tasks")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(theme.bg, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Done") { dismiss() }.foregroundStyle(theme.text)
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Stop all") { onStopAll() }
                        .foregroundStyle(theme.danger)
                        .accessibilityIdentifier("tasks-stop-all")
                }
            }
        }
        .accessibilityIdentifier("tasks-sheet")
    }

    private func row(_ job: Job) -> some View {
        HStack(spacing: 12) {
            Group {
                switch job.state {
                case .running:
                    ProgressView().tint(theme.accent).controlSize(.small)
                case .queued:
                    Image(systemName: "pause.fill")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(theme.placeholder)
                }
            }
            .frame(width: Self.glyph, height: Self.glyph)
            VStack(alignment: .leading, spacing: 2) {
                // The title may take two lines on a phone; the step truncates but the
                // elapsed clock never does.
                HStack(spacing: 4) {
                    Text(job.title)
                        .font(theme.font(Theme.small, weight: .medium))
                        .foregroundStyle(theme.text)
                        .lineLimit(2)
                    if onOpen != nil {
                        Image(systemName: "chevron.right")
                            .font(.system(size: 10, weight: .semibold))
                            .foregroundStyle(theme.placeholder)
                    }
                }
                HStack(spacing: 0) {
                    Text(job.statusLine)
                        .lineLimit(1)
                        .truncationMode(.tail)
                    Text(" · \(job.elapsedLabel)")
                        .fixedSize()
                        .accessibilityIdentifier("tasks-row-elapsed")
                }
                .font(theme.font(Theme.tiny))
                .foregroundStyle(job.state == .queued ? theme.placeholder : theme.muted)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Spacer(minLength: 8)
            if job.canSteer {
                capsule("Steer", color: theme.text) { onSteer(job) }
                    .accessibilityIdentifier("tasks-steer")
            }
            capsule("Stop", color: theme.danger) { onStop(job) }
                .accessibilityIdentifier("tasks-stop")
        }
        .padding(.horizontal, SheetRow.paddingX)
        .padding(.vertical, Self.rowPadY)
        .contentShape(Rectangle())
        .onTapGesture { onOpen?(job) }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(job.title), \(job.statusLine), \(job.elapsedLabel)")
        .accessibilityIdentifier("tasks-row")
    }

    private func capsule(_ label: String, color: Color, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label)
                .font(theme.font(Theme.tiny, weight: .medium))
                .foregroundStyle(color)
                .padding(.horizontal, Self.buttonPadX)
                .frame(height: Self.buttonHeight)
                .background(Capsule().fill(theme.bubbleAssistant))
        }
        .buttonStyle(.plain)
    }
}
