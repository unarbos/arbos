// Telegram-style profile pages in Arbos/Grok Bot dress: a big avatar, name,
// status line, a row of glass action tiles, an info card, a pill of tabs,
// then the tab's list. Three pages share the scaffold: a person
// (`UserProfileView`), a group chat (`GroupProfileView`, people groups and
// shared project chats), and an agent (`AgentProfileView`, its own file).
// `AppModel.showProfile(_:)` opens one from anywhere; the root presents it.

import ArbosFFI
import PhotosUI
import SwiftUI
import UIKit

/// Who a user page is about — from a chat member, a search result or a sender row.
struct ProfilePerson: Identifiable, Equatable, Hashable {
    var id: String
    var handle: String
    var name: String?
    var avatarUrl: String?
    /// §9 presence as last reported by the relay for this row.
    var lastSeenAt: Int64? = nil
    var online: Bool = false
    var avatarColorIndex: Int? = nil

    var displayName: String { name?.isEmpty == false ? name! : "@\(handle)" }

    init(id: String, handle: String, name: String?, avatarUrl: String?, lastSeenAt: Int64? = nil, online: Bool = false, avatarColorIndex: Int? = nil) {
        self.id = id; self.handle = handle; self.name = name; self.avatarUrl = avatarUrl; self.lastSeenAt = lastSeenAt; self.online = online; self.avatarColorIndex = avatarColorIndex
    }
    init(_ m: ChatMember) { self.init(id: m.id, handle: m.handle, name: m.name, avatarUrl: m.avatarUrl, lastSeenAt: m.lastSeenAt, online: m.online, avatarColorIndex: m.avatarColorIndex) }
    init(_ p: Person) { self.init(id: p.id, handle: p.handle, name: p.name, avatarUrl: p.avatarUrl, lastSeenAt: p.lastSeenAt, online: p.online, avatarColorIndex: p.avatarColorIndex) }
    init(_ p: ProjectMember) { self.init(id: p.id, handle: p.handle, name: p.name, avatarUrl: p.avatarUrl, lastSeenAt: p.lastSeenAt, online: p.online, avatarColorIndex: p.avatarColorIndex) }
}

/// What the root view presents.
enum ProfileTarget: Identifiable, Equatable, Hashable {
    case user(ProfilePerson)
    case group(chatId: String, projectId: String?)
    case agent(projectId: String)
    /// D88: a server's details (Channels | Members; admin rows for owner/admin).
    case server(serverId: String)
    /// D88 (#1317): a channel's own page — what the `# general` pill opens.
    case channel(channelId: String)

    /// Pages whose rows push a person over them (back chevron returns here):
    /// every kind — the agent page's "Run by …" owner link pushes too
    /// ([feedback agent-pane-run-by-link-ios-2026-10-25]).
    var pushesPeople: Bool { true }

    var id: String {
        switch self {
        case .user(let p): return "user:\(p.id)"
        case .group(let c, _): return "group:\(c)"
        case .agent(let p): return "agent:\(p)"
        case .server(let s): return "server:\(s)"
        case .channel(let c): return "channel:\(c)"
        }
    }
}

// MARK: - Scaffold pieces

/// Back button + optional trailing button over the header.
struct ProfileTopBar<Trailing: View>: View {
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    /// Override when `dismiss` is a no-op (Settings as a tab root). Sheets keep the default.
    var onBack: (() -> Void)? = nil
    /// Which page this bar fronts — its `ListTopFade` runs from the physical top edge under
    /// the bar ([feedback iphone-list-top-fade-2026-10-26]).
    var screen: ListTopFade.Screen = .agentPage
    @ViewBuilder var trailing: () -> Trailing

    var body: some View {
        ZStack(alignment: .top) {
            ListTopFade(screen: screen)
            bar
        }
    }

    private var bar: some View {
        HStack {
            Button { (onBack ?? { dismiss() })() } label: {
                GlassCircle { Image(systemName: "chevron.left").font(.system(size: 17, weight: .semibold)).foregroundStyle(theme.text) }
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Back")
            Spacer()
            trailing()
        }
        .padding(.horizontal, 16)
        .padding(.top, 16)
    }
}

/// The one row/card kit every pane page draws with (Jacob, 2026-10-20:
/// "make the settings, user and group panels consistent"). Settings groups,
/// profile cards and the agent editor all build on these, so a metric lives
/// in exactly one place:
///
/// * card: 16 pt continuous radius, `bubbleAssistant` fill (D80), hairline
///   between rows inset under the label;
/// * row: 48 pt minimum, 16 pt side padding, 14 pt gap; leading = a 30 pt
///   icon tile, a 36 pt picture, or nothing; trailing value in body size,
///   muted, tail-truncated; chevron 14 pt semibold placeholder-coloured;
/// * danger row: centred red label, same height;
/// * tabs: one capsule pill, `field` fill on the selected tab.
enum PaneKit {
    /// A pane page's content starts this far under the safe-area top (the bar is 16 + 48).
    static let pageTop: CGFloat = 80
    static let rowHeight: CGFloat = 48
    static let inset: CGFloat = 16
    /// Picture → name on a row: Grok Bot's sidebar / members rows measure 11–12 pt
    /// ink to ink (Mac refs 45, 39); the Mac's `identity::ROW_GAP`. Jacob, 2026-10-22:
    /// "names pulled closer to the avatar".
    static let gap: CGFloat = 10
    static let tile: CGFloat = 30
    static let avatar: CGFloat = 36
    /// The hero picture (a free constant: a static on the generic PaneHeader cannot be named without its type parameter).
    static let heroAvatar: CGFloat = 112
    static let radius: CGFloat = 16
    /// Extra room over the hero's action pill: with the hero's 10 pt stack gap the
    /// pill sits 16 under the "@handle" line (the Mac's `HERO_ACTION_GAP`).
    static let heroActionGap: CGFloat = 6
    /// Where a row's hairline starts for a given leading element.
    static func separatorInset(_ leading: PaneLeading) -> CGFloat {
        switch leading {
        case .none: return inset
        case .tile: return inset + tile + gap
        case .avatar, .glyph: return inset + avatar + gap
        }
    }
}

/// What sits at a row's leading edge.
enum PaneLeading {
    case none
    /// Apple-style 30 pt icon tile (settings rows); `dot` = machine state.
    case tile(String, dot: Color? = nil)
    /// A 36 pt picture (people, agents, groups).
    case avatar(AnyView)
    /// A 36 pt box with a glyph in it (Add members, files).
    case glyph(String)
}

/// The grouped card: one fill, one radius, hairlines inset under the label.
struct PaneCard<Rows: View>: View {
    @Environment(\.theme) private var theme
    var header: String? = nil
    var footer: String? = nil
    /// Where the hairlines start; a card of tile rows passes `.tile`.
    var separatorInset: CGFloat = PaneKit.inset
    @ViewBuilder var rows: () -> Rows

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let header {
                Text(header.uppercased())
                    .font(theme.font(Theme.tiny, weight: .medium))
                    .foregroundStyle(theme.muted)
                    .padding(.leading, PaneKit.inset)
            }
            _VariadicView.Tree(PaneCardLayout(separator: theme.separator, inset: separatorInset)) { rows() }
                .background(RoundedRectangle(cornerRadius: PaneKit.radius, style: .continuous).fill(theme.bubbleAssistant))
            if let footer {
                Text(footer)
                    .font(theme.font(Theme.tiny))
                    .foregroundStyle(theme.muted)
                    .padding(.horizontal, PaneKit.inset)
            }
        }
    }
}

private struct PaneCardLayout: _VariadicView_UnaryViewRoot {
    let separator: Color
    let inset: CGFloat

    @ViewBuilder
    func body(children: _VariadicView.Children) -> some View {
        let last = children.last?.id
        VStack(spacing: 0) {
            ForEach(children) { child in
                child
                if child.id != last {
                    Rectangle()
                        .fill(separator)
                        .frame(height: 1 / UIScreen.main.scale)
                        .padding(.leading, inset)
                }
            }
        }
    }
}

/// One row: leading, title (+ optional subtitle), optional trailing value,
/// chevron when it pushes a page.
struct PaneRow: View {
    @Environment(\.theme) private var theme
    var leading: PaneLeading = .none
    let title: String
    var subtitle: String? = nil
    var value: String? = nil
    var valueTint: Color? = nil
    var chevron: Bool = false
    var titleTint: Color? = nil
    /// A control at the trailing edge (a Toggle, a ✕); drawn after the value.
    var trailing: AnyView? = nil

    /// Mock b: names on people / agent / file rows are medium; mock g:
    /// settings labels are regular.
    private var titleWeight: Font.Weight {
        switch leading {
        case .avatar, .glyph: return .medium
        case .none, .tile: return .regular
        }
    }

    var body: some View {
        HStack(spacing: PaneKit.gap) {
            leadingView
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(theme.font(Theme.body, weight: titleWeight))
                    .foregroundStyle(titleTint ?? theme.text)
                    .lineLimit(1)
                if let subtitle, !subtitle.isEmpty {
                    Text(subtitle)
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                        .lineLimit(1)
                }
            }
            .layoutPriority(1)
            Spacer(minLength: 8)
            if let value, !value.isEmpty {
                // The value takes what the title leaves and clips at its end
                // (Apple's Settings), not in the middle.
                Text(value)
                    .font(theme.font(Theme.body))
                    .foregroundStyle(valueTint ?? theme.muted)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            if let trailing { trailing }
            if chevron {
                Image(systemName: "chevron.right")
                    .font(theme.font(14, weight: .semibold))
                    .foregroundStyle(theme.placeholder)
            }
        }
        .padding(.horizontal, PaneKit.inset)
        .padding(.vertical, 6)
        .frame(minHeight: PaneKit.rowHeight)
        .contentShape(Rectangle())
    }

    @ViewBuilder private var leadingView: some View {
        switch leading {
        case .none:
            EmptyView()
        case .tile(let icon, let dot):
            ZStack(alignment: .bottomTrailing) {
                RoundedRectangle(cornerRadius: 7.5, style: .continuous)
                    .fill(theme.isDark ? Color(hex: "#5C5C60")! : Color(hex: "#6E6E73")!)
                    .frame(width: PaneKit.tile, height: PaneKit.tile)
                    .overlay(
                        Image(systemName: icon)
                            .font(theme.font(15, weight: .semibold))
                            .foregroundStyle(.white)
                    )
                if let dot {
                    Circle().fill(dot).frame(width: 9, height: 9)
                        .overlay(Circle().strokeBorder(theme.bubbleAssistant, lineWidth: 1.5))
                        .offset(x: 3, y: 3)
                }
            }
        case .avatar(let view):
            view.frame(width: PaneKit.avatar, height: PaneKit.avatar)
        case .glyph(let icon):
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(theme.field)
                .frame(width: PaneKit.avatar, height: PaneKit.avatar)
                .overlay(Image(systemName: icon).font(theme.font(17, weight: .medium)).foregroundStyle(theme.muted))
        }
    }
}

/// The destructive action at the end of a pane: a centred red label.
struct PaneDangerRow: View {
    @Environment(\.theme) private var theme
    let title: String
    let action: () -> Void

    var body: some View { PaneActionRow(title: title, color: theme.danger, action: action) }
}

/// A centred one-label action in its own card, in the given colour — the
/// non-destructive twin of `PaneDangerRow` (Unblock in the accent colour).
struct PaneActionRow: View {
    @Environment(\.theme) private var theme
    let title: String
    let color: Color
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(theme.font(Theme.body))
                .foregroundStyle(color)
                .frame(maxWidth: .infinity)
                .frame(minHeight: PaneKit.rowHeight)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// A small caps title over a section of a pane ("Images · 3", "Files · 1").
struct PaneSectionTitle: View {
    @Environment(\.theme) private var theme
    let title: String
    init(_ title: String) { self.title = title }
    var body: some View {
        Text(title.uppercased())
            .font(theme.font(Theme.tiny, weight: .medium))
            .foregroundStyle(theme.muted)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, PaneKit.inset * 2)
    }
}

/// The one capsule tab pill (Profile | Settings, Links | Groups, Members | Library).
struct PaneTabs: View {
    @Environment(\.theme) private var theme
    let tabs: [String]
    @Binding var selected: String

    // Redesign mocks b/g (same kit as the Mac): bare labels, the current one
    // on a capsule, centred — no track, no border.
    var body: some View {
        HStack(spacing: 4) {
            ForEach(tabs, id: \.self) { t in
                Button { withAnimation(.snappy(duration: 0.2)) { selected = t } } label: {
                    Text(t)
                        .font(theme.font(Theme.body, weight: selected == t ? .semibold : .regular))
                        .foregroundStyle(selected == t ? theme.text : theme.muted)
                        .padding(.horizontal, 16).frame(height: 32)
                        .background(Capsule().fill(selected == t ? theme.bubbleAssistant : .clear))
                }
                .buttonStyle(.plain)
            }
        }
        .frame(maxWidth: .infinity)
    }
}

/// The hero: 112 pt picture, name, subtitle (@handle, member count, …).
struct PaneHeader<Avatar: View>: View {
    /// Kept for callers that spell the type (`PaneHeader<EmptyView>.avatarSize`); new code uses `PaneKit.heroAvatar`.
    static var avatarSize: CGFloat { PaneKit.heroAvatar }
    @Environment(\.theme) private var theme
    let name: String
    let subtitle: String
    /// SF Symbol after the name ("lock.fill" on a private channel).
    var badge: String? = nil
    /// A tap on the name (the agent pane: the owner's rename alert).
    var nameAction: (() -> Void)? = nil
    /// The subtitle's colour when it is not the muted default (a placeholder reads fainter).
    var subtitleTint: Color? = nil
    /// A tap on the subtitle (the agent pane's label: tap to edit it in place).
    var subtitleAction: (() -> Void)? = nil
    /// The subtitle is a link (the foreign agent's "Run by …"): a mention-chip pill,
    /// accent text, 14 % accent fill, 30 % while pressed ([feedback agent-pane-run-by-link-ios-2026-10-25]).
    var subtitleLink = false
    /// Drawn instead of the subtitle while it is being edited.
    var subtitleEditor: AnyView? = nil
    /// Drawn under the subtitle (the "Run by …" link under an agent's label).
    var subtitleBelow: AnyView? = nil
    /// The hero's one action pill under the subtitle (a person's Message,
    /// [feedback profile-message-button-ios-2026-10-25]); nil on panes without one.
    var action: AnyView? = nil
    @ViewBuilder var avatar: () -> Avatar

    var body: some View {
        VStack(spacing: 10) {
            avatar().frame(width: PaneKit.heroAvatar, height: PaneKit.heroAvatar)
            if let nameAction {
                Button(action: nameAction) { nameLine }
                    .buttonStyle(.plain)
                    .accessibilityHint("Renames")
            } else {
                nameLine
            }
            if let subtitleEditor {
                subtitleEditor
            } else if let subtitleAction, subtitleLink {
                Button(action: subtitleAction) { Text(subtitle).lineLimit(1) }
                    .buttonStyle(ChipLinkStyle())
                    .accessibilityHint("Opens their profile")
            } else if let subtitleAction {
                Button(action: subtitleAction) { subtitleText }
                    .buttonStyle(.plain)
                    .accessibilityHint("Edits the label")
            } else {
                subtitleText
            }
            if let subtitleBelow {
                subtitleBelow
            }
            if let action {
                action.padding(.top, PaneKit.heroActionGap)
            }
        }
        .frame(maxWidth: .infinity)
    }

    private var nameLine: some View {
        HStack(spacing: 6) {
            Text(name)
                .font(theme.font(28, weight: .semibold))
                .foregroundStyle(theme.text)
                .lineLimit(1)
            if let badge {
                Image(systemName: badge).font(theme.font(15, weight: .semibold)).foregroundStyle(theme.muted)
            }
        }
        .padding(.horizontal, 24)
    }

    private var subtitleText: some View {
        Text(subtitle)
            .font(theme.font(Theme.small))
            .foregroundStyle(subtitleTint ?? theme.muted)
            .lineLimit(1)
            .padding(.horizontal, 32)
    }
}

/// A link drawn as a mention chip (the Mac's `run_by_link`): accent text on an accent
/// capsule at 14 %, 30 % while the finger is down, a 120 ms ease-out between them —
/// the chip pointer rules ([feedback mention-chip-hover-2026-10-24]).
struct ChipLinkStyle: ButtonStyle {
    @Environment(\.theme) private var theme
    static let restAlpha: Double = 0.14
    static let pressedAlpha: Double = 0.30

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(theme.font(Theme.small))
            .foregroundStyle(theme.accent)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(Capsule().fill(theme.accent.opacity(configuration.isPressed ? Self.pressedAlpha : Self.restAlpha)))
            .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
            .contentShape(Capsule())
    }
}

/// The hero's action pill ("Message" on a person's pane, [feedback
/// profile-message-button-ios-2026-10-25]): the iPhone's control family —
/// `Theme.control` 42 tall like the top bar's back disc, a capsule on the
/// `field` fill with its hairline rim, SF glyph + body-size semibold label in
/// `text`; pressed = the fill lifted, as the Mac's chip rule.
struct PanePillButton: View {
    @Environment(\.theme) private var theme
    let glyph: String
    let title: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Image(systemName: glyph).font(theme.font(17, weight: .semibold))
                Text(title).font(theme.font(Theme.body, weight: .semibold))
            }
            .foregroundStyle(theme.text)
            .padding(.horizontal, 22)
            .frame(height: Controls.height)
            .contentShape(Capsule())
        }
        .buttonStyle(PanePillStyle())
        .accessibilityIdentifier("profile-\(title.lowercased())")
    }
}

private struct PanePillStyle: ButtonStyle {
    @Environment(\.theme) private var theme
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background {
                ZStack {
                    if theme.isGlass { Capsule().fill(.ultraThinMaterial) }
                    ControlSurface(shape: Capsule(), pressed: configuration.isPressed)
                        .opacity(theme.isGlass ? ChatHeaderGlass.legacyTint : 1)
                }
            }
    }
}

/// The person pane's Message pill (the Mac's `profiles::user::shows_message`):
/// for another person I have not blocked — never my own pane, never once I
/// blocked them. The other direction (they blocked me) is in no payload the
/// client reads; the relay refuses the DM and `openDM` says so.
enum MessageButton {
    static let title = "Message"
    static func shows(isMe: Bool, blocked: Bool) -> Bool { !isMe && !blocked }
}

/// The crayon on an editable hero picture (own profile, people groups): a
/// small pencil disc at the bottom-right, same on every pane.
private struct PaneCrayon: ViewModifier {
    @Environment(\.theme) private var theme
    let on: Bool
    func body(content: Content) -> some View {
        content.overlay(alignment: .bottomTrailing) {
            if on {
                Image(systemName: "pencil")
                    .font(theme.font(13, weight: .semibold))
                    .foregroundStyle(theme.text)
                    .frame(width: 30, height: 30)
                    .background(Circle().fill(theme.field)) // control-size: exception rowIconTiles
                    .overlay(Circle().strokeBorder(theme.bg, lineWidth: 2))
            }
        }
    }
}

extension View {
    func paneCrayon(_ on: Bool = true) -> some View { modifier(PaneCrayon(on: on)) }
}

/// Jacob, 2026-10-25: a tap on a profile picture opens it full-size in the media viewer chat
/// photos use (tap / swipe-down closes, pinch zooms). With no picture (`url` empty — initials
/// or a leaf) the content is just drawn: nothing to view. Panes whose picture opens the avatar
/// picker instead get **View** inside the picker.
struct ViewablePicture<Content: View>: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let name: String
    let url: String?
    @ViewBuilder let content: () -> Content
    @State private var viewer: MediaViewerState?

    var body: some View {
        if let url, !url.isEmpty {
            Button {
                viewer = MediaViewerState(items: [MediaAlbum.picture(name: name, url: url)], index: 0)
            } label: {
                content().contentShape(Circle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("View picture")
            .accessibilityIdentifier("hero-view-picture")
            .onAppear {
                if DebugHooks.viewPicture {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
                        viewer = MediaViewerState(items: [MediaAlbum.picture(name: name, url: url)], index: 0)
                    }
                }
            }
            .fullScreenCover(item: $viewer) { v in
                MediaViewer(album: v.items, index: v.index, onClose: { viewer = nil })
                    .environmentObject(model)
                    .environment(\.theme, theme)
            }
        } else {
            content()
        }
    }
}

// ----- the shared pane kit (PaneKit) and the names the pages use ------------

/// A grouped card of rows (the kit's card; hairlines inset to the label).
struct ProfileCard<Content: View>: View {
    var separatorInset: CGFloat = PaneKit.inset
    @ViewBuilder var content: () -> Content
    var body: some View {
        PaneCard(separatorInset: separatorInset) { content() }
            .padding(.horizontal, PaneKit.inset)
    }
}

/// One-line action row inside a card: the kit's row (red = the danger row).
struct ProfileLinkRow: View {
    let title: String
    var destructive = false
    let action: () -> Void

    var body: some View {
        if destructive {
            PaneDangerRow(title: title, action: action)
        } else {
            Button(action: action) { PaneRow(title: title, chevron: true) }.buttonStyle(.plain)
        }
    }
}

/// The tab pill at the pane's insets (profile pages lay out edge to edge).
struct ProfileTabs: View {
    let tabs: [String]
    @Binding var selected: String
    var body: some View { PaneTabs(tabs: tabs, selected: $selected).padding(.horizontal, PaneKit.inset) }
}

/// Last-seen text: the relay's `online`/`last_seen_at` (§9) merged with the
/// presence frames seen this session; "last seen recently" when unknown.
func lastSeenLine(_ at: Int64?, online: Bool = false, now: Int64 = nowMs()) -> (text: String, online: Bool) {
    if online { return ("online", true) }
    guard let at, at > 0 else { return ("last seen recently", false) }
    let age = max(0, now - at) / 1000
    if age < 90 { return ("online", true) }
    if age < 3600 { return ("last seen \(age / 60) min ago", false) }
    if age < 86_400 {
        let d = Date(timeIntervalSince1970: Double(at) / 1000)
        return ("last seen today at \(d.formatted(date: .omitted, time: .shortened))", false)
    }
    if age < 7 * 86_400 { return ("last seen \(age / 86_400) day\(age / 86_400 == 1 ? "" : "s") ago", false) }
    return ("last seen a long time ago", false)
}

// MARK: - Media / files / voice / links from a chat's messages

struct ChatItem: Identifiable, Equatable {
    enum Kind { case media, file, voice, link }
    var id: String
    var kind: Kind
    var title: String
    var subtitle: String
    var url: URL?
    var at: Int64
    var fromHandle: String?
    var mime: String
    /// The viewer's entry for an attachment (media-viewer-2026-10-23); nil for links.
    var media: MediaItem? = nil
    /// Who shared it, for the Library's preview card ("Samir Haddad", "You", the
    /// agent's name) and their picture: a person's avatar URL and id, or the
    /// agent's project id ([feedback library-list-ios-2026-10-25]).
    var by: String = ""
    var fromAvatar: String? = nil
    var fromId: String? = nil
    var agentId: String? = nil
}

enum ChatMedia {
    private static let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)

    /// Only what was exchanged in the chat feeds the Library (Jacob, 2026-10-25,
    /// [feedback library-chat-only-ios-2026-10-25]): a hidden job row or an unnamed
    /// worker's spawn row is the agent's own machinery — the files it read, the
    /// pages it fetched — and never counts. The Mac's `shared_in_chat`.
    static func sharedInChat(_ m: Message) -> Bool { !m.hidden && !m.worker }

    /// The links a text shares: `http(s)://` typed out. The data detector also
    /// reads a bare "app.rs" or "calls.rs" as a Serbian site — a file name is not
    /// a link (the flood Jacob saw). The Mac's `urls_in`.
    static func links(in text: String) -> [URL] {
        guard let detector else { return [] }
        let ns = text as NSString
        return detector.matches(in: text, range: NSRange(location: 0, length: ns.length)).compactMap { match in
            guard let url = match.url, url.scheme?.hasPrefix("http") == true else { return nil }
            let typed = ns.substring(with: match.range).lowercased()
            return typed.hasPrefix("http://") || typed.hasPrefix("https://") ? url : nil
        }
    }

    /// Links in the Library are the ones **people** sent (Jacob's rule names links
    /// people sent and files the agent made — an agent's prose link, "source:
    /// https://…", is its research, not something it shared). The Mac's `library_links`.
    static func libraryLinks(_ m: Message) -> [URL] {
        guard sharedInChat(m), m.role == .user else { return [] }
        return m.parts.flatMap { part -> [URL] in
            if case .text(_, let text) = part { return links(in: text) }
            return []
        }
    }

    @MainActor
    static func items(messages: [Message], slot: String, model: AppModel) -> [ChatItem] {
        var out: [ChatItem] = []
        for m in messages where sharedInChat(m) {
            let by = MediaAlbum.senderName(m, projectName: { model.project($0)?.name })
            let agentId = m.sender.hasPrefix("agent:") ? String(m.sender.dropFirst(6))
                : m.sender.hasPrefix("project:") ? String(m.sender.dropFirst(8)) : nil
            let fromId = m.sender.hasPrefix("user:") ? String(m.sender.dropFirst(5)) : nil
            for part in m.parts {
                switch part {
                case .attachment(let id, let a):
                    // Same rules as the transcript (`Attachment.isImage`) so a photo
                    // lands under Media, not Files.
                    let kind: ChatItem.Kind = a.mime.hasPrefix("audio/") || a.kind == "audio" ? .voice
                        : (a.isImage || a.mime.hasPrefix("video/") || a.kind == "video") ? .media : .file
                    let url = a.blobRef.flatMap { model.blobURL(projectId: slot, key: $0) }
                    let media = MediaAlbum.kind(of: a).map {
                        MediaAlbum.item(id: "\(m.id)#\(id)", message: m, attachment: a, kind: $0, slot: slot,
                                        projectName: { model.project($0)?.name })
                    }
                    out.append(ChatItem(id: "\(m.id)#\(id)", kind: kind, title: a.name,
                                        subtitle: a.mime, url: url, at: m.createdAt, fromHandle: m.senderHandle, mime: a.mime,
                                        media: media, by: by, fromAvatar: m.senderAvatar, fromId: fromId, agentId: agentId))
                case .text(let id, let text):
                    // People's typed links only (`libraryLinks`): never the agent's prose.
                    guard m.role == .user else { continue }
                    for (ix, url) in links(in: text).enumerated() {
                        out.append(ChatItem(id: "\(m.id)#\(id)#\(ix)", kind: .link, title: url.host() ?? url.absoluteString,
                                            subtitle: url.absoluteString, url: url, at: m.createdAt, fromHandle: m.senderHandle, mime: "text/uri-list",
                                            by: by, fromAvatar: m.senderAvatar, fromId: fromId, agentId: agentId))
                    }
                default: continue
                }
            }
        }
        return out.sorted { $0.at > $1.at }
    }
}

struct ChatItemsList: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let items: [ChatItem]
    let kind: ChatItem.Kind
    let empty: String
    @State private var viewer: MediaViewerState?

    private var filtered: [ChatItem] { items.filter { $0.kind == kind } }

    /// The grid is newest first; the viewer's album runs in chat order (Mac `media_grid`).
    private var album: [MediaItem] { filtered.reversed().compactMap(\.media) }

    var body: some View {
        if filtered.isEmpty {
            Text(empty).font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                .frame(maxWidth: .infinity).padding(.top, 36).padding(.horizontal, 32).multilineTextAlignment(.center)
        } else if kind == .media {
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 2), count: 3), spacing: 2) {
                ForEach(filtered) { item in
                    // Size from the grid column (fit), then paint the image in an
                    // overlay. `aspectRatio(..., .fill)` + AsyncImage used the
                    // image's intrinsic size and blew the ScrollView (white sheet).
                    Button { open(item) } label: {
                        Color.clear
                            .aspectRatio(1, contentMode: .fit)
                            .overlay {
                                ZStack {
                                    Rectangle().fill(theme.card)
                                    if let media = item.media, media.kind == .image {
                                        BlobImage(item: media)
                                    } else {
                                        Image(systemName: "film").font(.system(size: 22)).foregroundStyle(theme.muted)
                                    }
                                }
                            }
                            .clipped()
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("\(item.title), tap to view")
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
            .padding(.horizontal, 16)
            .fullScreenCover(item: $viewer) { v in
                MediaViewer(album: v.items, index: v.index, onClose: { viewer = nil })
                    .environmentObject(model)
                    .environment(\.theme, theme)
            }
        } else {
            ProfileCard(separatorInset: PaneKit.separatorInset(.glyph(""))) {
                ForEach(filtered) { item in
                    rowView(item)
                }
            }
            .fullScreenCover(item: $viewer) { v in
                MediaViewer(album: v.items, index: v.index, onClose: { viewer = nil })
                    .environmentObject(model)
                    .environment(\.theme, theme)
            }
        }
    }

    private func rowView(_ item: ChatItem) -> some View {
        Group {
            if item.kind == .file, item.media != nil {
                // A file opens in the viewer (Open → Quick Look, Download, Forward).
                Button { open(item) } label: { rowLabel(item) }
            } else if let url = item.url {
                SwiftUI.Link(destination: url) { rowLabel(item) }
            } else {
                rowLabel(item)
            }
        }
        .buttonStyle(.plain)
    }

    /// A tile opens the album at its place; a file row opens that file alone.
    private func open(_ item: ChatItem) {
        guard let media = item.media else { return }
        if kind == .media, let ix = album.firstIndex(where: { $0.id == media.id }) {
            viewer = MediaViewerState(items: album, index: ix)
        } else {
            viewer = MediaViewerState(items: [media], index: 0)
        }
    }

    private func rowLabel(_ item: ChatItem) -> some View {
        let when = Date(timeIntervalSince1970: Double(item.at) / 1000).formatted(date: .abbreviated, time: .omitted)
        return PaneRow(
            leading: .glyph(item.kind == .voice ? "waveform" : item.kind == .link ? "link" : "doc.fill"),
            title: item.title,
            subtitle: item.kind == .link ? item.subtitle : "\(item.fromHandle.map { "@\($0) · " } ?? "")\(when)"
        )
    }
}

// MARK: - User page

/// The person pane's footer row (#1545): Block user / Unblock derives from the relay's
/// block list (`AppModel.blocked`, `GET /blocks`), never from a local flag, so it is right
/// on pane re-open and app relaunch. The Mac twin is `app.rs` `blocked_entry` /
/// `user.rs` `block_row_label`.
enum BlockRow {
    /// The block-list entry for a person: same non-empty id, else the same handle
    /// (GitHub logins are case-insensitive).
    static func entry(in blocked: [Person], id: String, handle: String) -> Person? {
        blocked.first { b in
            (!b.id.isEmpty && b.id == id)
                || (!b.handle.isEmpty && b.handle.caseInsensitiveCompare(handle) == .orderedSame)
        }
    }

    static func title(blocked: Bool) -> String { blocked ? "Unblock" : "Block user" }
}

struct UserProfileView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let person: ProfilePerson
    @State private var tab = "Links"
    @State private var sharedProjects: [Project] = []
    @State private var confirmBlock = false
    /// Unblock asks first ([feedback blocked-state-ios-2026-10-26]).
    @State private var confirmUnblock = false

    /// Stable user id: prefer a fresh card, then a handle lookup (agent-owner rows
    /// sometimes open this page with `id == handle`), then the value we were given.
    private var peerId: String { fresh?.id ?? model.personId(handle: person.handle) ?? person.id }
    private var peerHandle: String { fresh?.handle ?? person.handle }
    private func isPeer(_ m: ChatMember) -> Bool {
        m.id == peerId || m.id == person.id
            || m.handle.caseInsensitiveCompare(peerHandle) == .orderedSame
    }
    private var dm: PeopleChat? {
        model.peopleChats.first { $0.kind == "dm" && $0.humans.contains(where: isPeer) }
    }
    @State private var fresh: Person?
    private var isMe: Bool { peerId == model.user?.id || person.id == model.user?.id }
    private var blockedEntry: Person? { BlockRow.entry(in: model.blocked, id: peerId, handle: peerHandle) }
    private var items: [ChatItem] {
        guard let dm else { return [] }
        return ChatMedia.items(messages: model.messages(of: dm.id), slot: "\(peopleSlot):\(dm.id)", model: model)
    }
    /// People groups we share — same rule as Mac: every non-DM space that lists this person.
    /// Match by id or handle; `isGroup` covers `kind == group` and multi-human chats whose kind lagged.
    private var sharedGroups: [PeopleChat] {
        model.peopleChats.filter { $0.isGroup && $0.humans.contains(where: isPeer) }
    }

    var body: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            ScrollView {
                VStack(spacing: 18) {
                    Color.clear.frame(height: PaneKit.pageTop)
                    // The shared pane (D93): picture, name, @handle, the Message pill
                    // (Jacob 2026-10-25; not on my own pane, not once I blocked them);
                    // Links | Groups (the Mac's set); one Block user row. No last-seen,
                    // no username field, no Add members.
                    ProfilePane(
                        hero: PaneHero(
                            picture: .image(AnyView(ViewablePicture(name: person.displayName, url: person.avatarUrl) {
                                PersonAvatar(url: person.avatarUrl, handle: person.handle, size: PaneKit.heroAvatar, name: person.name, id: person.id, colorIndex: person.avatarColorIndex)
                            })),
                            name: person.displayName,
                            subtitle: "@\(person.handle)",
                            // Blocked ([feedback blocked-state-ios-2026-10-26]): the chip under
                            // the @handle, and Unblock as the one action instead of Message.
                            subtitleBelow: blockedEntry != nil && !isMe ? AnyView(BlockedChip()) : nil,
                            action: blockedEntry != nil && !isMe
                                ? AnyView(unblockButton)
                                : (MessageButton.shows(isMe: isMe, blocked: blockedEntry != nil) ? AnyView(messageButton) : nil)
                        ),
                        tabs: PaneKinds.person,
                        selected: $tab,
                        body: tab == "Links"
                            ? [.custom(AnyView(ChatItemsList(items: items, kind: .link, empty: dm == nil ? "No chat with \(person.displayName) yet." : "No links shared yet.")))]
                            : groupsSections,
                        footer: isMe ? [] : [.card([AnyView(blockRow)])]
                    )
                    Color.clear.frame(height: 40)
                }
            }
            .scrollIndicators(.visible)
            ProfileTopBar(screen: .personPage) { EmptyView() }
        }
        .confirmationDialog("Block @\(person.handle)?", isPresented: $confirmBlock, titleVisibility: .visible) {
            Button("Block", role: .destructive) {
                // The pane stays open: the row flips to Unblock once the relay lists them.
                if let m = dm?.humans.first(where: isPeer) { model.block(m) } else { model.block(handle: peerHandle) }
            }
        } message: { Text("They cannot message you or add you to chats.") }
        .confirmationDialog(BlockedState.confirmTitle(handle: person.handle), isPresented: $confirmUnblock, titleVisibility: .visible) {
            Button(BlockedState.unblock) {
                // The pane stays open: the chip and the row go once the relay's list comes back.
                if let entry = blockedEntry { model.unblock(entry) }
            }
        } message: { Text(BlockedState.confirmNote) }
        .task {
            await model.refreshBlocks()
            await model.refreshPeople()
            let id = model.personId(handle: person.handle) ?? person.id
            fresh = await model.fetchUser(id)
            let who = ProfilePerson(
                id: fresh?.id ?? id,
                handle: fresh?.handle ?? person.handle,
                name: fresh?.name ?? person.name,
                avatarUrl: fresh?.avatarUrl ?? person.avatarUrl,
                lastSeenAt: fresh?.lastSeenAt ?? person.lastSeenAt,
                online: fresh?.online ?? person.online,
                avatarColorIndex: fresh?.avatarColorIndex ?? person.avatarColorIndex
            )
            sharedProjects = await model.sharedProjects(with: who)
        }
    }

    /// The one primary action (Jacob 2026-10-25): open the DM we have, or start one
    /// through the same `POST /chats/dm` path the New Message sheet uses (`openDM`
    /// navigates and shows "You can't message this person." on a relay refusal).
    private var messageButton: some View {
        PanePillButton(glyph: "bubble.left", title: MessageButton.title) {
            if let dm {
                dismiss()
                model.open(peopleChat: dm)
            } else {
                Task { if await model.openDM(handle: peerHandle) != nil { dismiss() } }
            }
        }
    }

    /// The hero's Unblock once I blocked them ([feedback blocked-state-ios-2026-10-26]).
    private var unblockButton: some View {
        PanePillButton(glyph: "nosign", title: BlockedState.unblock) { confirmUnblock = true }
            .accessibilityIdentifier("profile-unblock-pill")
    }

    /// Block user (red) or Unblock (accent), from the relay's block list; both ask first.
    @ViewBuilder private var blockRow: some View {
        if blockedEntry != nil {
            PaneActionRow(title: BlockRow.title(blocked: true), color: theme.accent) { confirmUnblock = true }
        } else {
            PaneDangerRow(title: BlockRow.title(blocked: false)) { confirmBlock = true }
        }
    }

    private var groupsSections: [PaneSection] {
        let groups = sharedGroups
        if groups.isEmpty && sharedProjects.isEmpty {
            return [.empty("No groups in common yet.")]
        }
        var rows: [AnyView] = groups.map { g in
            AnyView(
                Button { dismiss(); model.open(peopleChat: g) } label: {
                    PaneRow(leading: .glyph("person.2.fill"), title: g.title, subtitle: "\(g.humans.count) members", chevron: true)
                }
                .buttonStyle(.plain)
            )
        }
        rows += sharedProjects.map { p in
            AnyView(
                Button { dismiss(); model.open(project: p) } label: {
                    PaneRow(
                        leading: .avatar(AnyView(AgentAvatar(project: p, theme: theme, size: PaneKit.avatar, still: true))),
                        title: p.name,
                        subtitle: "shared project · @\(p.ownerHandle ?? model.user?.login ?? "")",
                        chevron: true
                    )
                }
                .buttonStyle(.plain)
            )
        }
        return [.card(rows, separatorInset: PaneKit.separatorInset(.glyph("")))]
    }

}

// MARK: - Group invite share

/// Present the system share sheet from the top UIKit controller.
/// Wrapping `UIActivityViewController` in a SwiftUI `.sheet` paints blank on
/// iPhone when the host is already a sheet (the group profile).
@MainActor
private func presentShareSheet(items: [Any]) {
    let activity = UIActivityViewController(activityItems: items, applicationActivities: nil)
    let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
    guard let scene = scenes.first(where: { $0.activationState == .foregroundActive }) ?? scenes.first,
          let root = (scene.windows.first(where: \.isKeyWindow) ?? scene.windows.first)?.rootViewController
    else { return }
    var top = root
    while let presented = top.presentedViewController { top = presented }
    top.present(activity, animated: true)
}

// MARK: - Group page

struct GroupProfileView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let chatId: String
    /// nil for people groups; the project for a shared project chat.
    let projectId: String?
    @State private var tab = "Members"
    @State private var projectMembers: [ProjectMember] = []
    @State private var showAdd = false
    @State private var showEdit = false
    /// Rig only: the avatar picker opened straight from the pane (users go through Edit group).
    @State private var showAvatarPicker: AvatarPickerTab? = nil
    @State private var showActivity = false
    @State private var pendingRemoveId: String?
    @State private var pendingRemoveName: String?
    /// Ban confirm (remove and block rejoin), same shape as server member ban.
    @State private var pendingBanId: String?
    @State private var pendingBanName: String?
    /// Human member (not you) whose row was tapped: offer View profile / Send private message.
    @State private var memberActions: ProfilePerson?
    /// Footer confirm: leave (member) or delete (owner) this people group.
    @State private var confirmLeave = false
    @State private var confirmDelete = false
    /// Mute notifications for 1 hour / 8 hours / until unmuted.
    @State private var showMuteOptions = false
    /// People-group invite links (create / copy / share / revoke), same shape as server invites.
    @State private var groupInvites: [ServerInviteLink] = []
    @State private var inviteExpires = false
    @State private var inviteSingleUse = false
    @State private var inviteCopied = false
    /// True while minting an invite link before opening the system share sheet.
    @State private var sharingInvite = false

    private var people: PeopleChat? { model.peopleChat(chatId) }
    private var project: Project? { projectId.flatMap { model.project($0) } }
    private var title: String { project?.name ?? people?.title ?? "Group" }
    private var slot: String { projectId ?? "\(peopleSlot):\(chatId)" }
    private var items: [ChatItem] { ChatMedia.items(messages: model.messages(of: chatId), slot: slot, model: model) }

    private struct Row: Identifiable {
        var id: String
        var person: ProfilePerson
        var role: String
        var agent: Project?
    }

    private var rows: [Row] {
        if let people {
            return people.members.map { m in
                Row(id: m.id, person: ProfilePerson(m), role: m.role, agent: m.isAgent ? model.agent(member: m, chatId: people.id) : nil)
            }
        }
        var out = projectMembers.map { Row(id: $0.id, person: ProfilePerson($0), role: $0.role, agent: nil) }
        if let project { out.insert(Row(id: project.id, person: ProfilePerson(id: project.id, handle: mentionSlug(project.name), name: project.name, avatarUrl: nil), role: "agent", agent: project), at: 0) }
        return out
    }

    private var bannedRows: [Row] {
        guard let people else { return [] }
        return people.banned.map { m in
            Row(id: m.id, person: ProfilePerson(m), role: m.role, agent: nil)
        }
    }

    private var memberCount: Int { rows.filter { $0.agent == nil }.count }

    /// Owners (and admins, when a chat has that role) can remove other people.
    /// Check `me.role` and our row in `members` — parse defaults `myRole` to
    /// `"member"` when `me` is missing, which would otherwise hide remove.
    private var canManageMembers: Bool {
        if people?.myRole == "owner" || people?.myRole == "admin" { return true }
        let rowRole = people?.humans.first(where: { $0.id == model.user?.id })?.role
        return rowRole == "owner" || rowRole == "admin"
    }

    private var isGroupOwner: Bool {
        if people?.myRole == "owner" { return true }
        return people?.humans.first(where: { $0.id == model.user?.id })?.role == "owner"
    }

    /// My human row in this people group (for mute/ban rights).
    private var myMember: ChatMember? {
        people?.humans.first(where: { $0.id == model.user?.id })
    }

    /// Owner always; an admin only when the owner left mute/ban on for them.
    private var iCanMuteBan: Bool {
        if isGroupOwner { return true }
        guard people?.myRole == "admin" || myMember?.role == "admin" else { return false }
        return myMember?.canMuteBan ?? true
    }

    /// People: owners/admins remove anyone but themselves and other owners.
    /// Agents: only the agent's own owner (the relay's rule for
    /// `DELETE /chats/{id}/agents/{project}`), and only in people chats.
    private func canRemove(_ r: Row) -> Bool {
        if let agent = r.agent {
            guard people != nil, let me = model.user?.id else { return false }
            let member = people?.agents.first { $0.id == agent.id }
            return (member?.ownerId ?? agent.ownerId) == me
        }
        if isGroupOwner && r.role == "admin" { return true }
        if people?.myRole == "admin" || myMember?.role == "admin" {
            return r.person.id != model.user?.id && r.role == "member"
        }
        return canManageMembers && r.person.id != model.user?.id && r.role != "owner"
    }

    /// Mute / ban this human: same targets as remove, plus I must have mute/ban rights.
    private func canMuteBan(_ r: Row) -> Bool {
        r.agent == nil && canRemove(r) && iCanMuteBan
    }

    private var groupImageURL: String? {
        guard let people, let key = people.imageKey, !key.isEmpty else { return nil }
        return model.blobURL(projectId: "\(peopleSlot):\(chatId)", key: key)?.absoluteString
    }

    private var clusterMembers: [ChatMember] {
        guard let people else { return [] }
        let others = people.humans.filter { $0.id != model.user?.id }
        return others.isEmpty ? people.humans : others
    }

    /// Mute / Unmute above Leave — people groups only (relay `me.muted` + local duration).
    private var muteFooter: [PaneSection] {
        guard people?.isGroup == true, projectId == nil else { return [] }
        let muted = model.isPeopleChatMuted(chatId)
        let row = AnyView(Button {
            if muted { model.unmutePeopleChat(chatId) }
            else { showMuteOptions = true }
        } label: {
            PaneRow(
                leading: .tile(muted ? "bell" : "bell.slash"),
                title: muted ? "Unmute notifications" : "Mute notifications",
                subtitle: model.peopleMuteSubtitle(chatId)
            )
        }.buttonStyle(.plain))
        return [.card([row], separatorInset: PaneKit.separatorInset(.tile("")))]
    }

    /// Footer on every tab for a people group: Leave group chat for members,
    /// Delete group chat for the owner (same rule as the chat list).
    private var leaveFooter: [PaneSection] {
        guard let people, people.isGroup else { return [] }
        if model.ownsGroup(people) {
            return [.card([AnyView(ProfileLinkRow(title: "Delete group chat", destructive: true) {
                confirmDelete = true
            })])]
        }
        return [.card([AnyView(ProfileLinkRow(title: "Leave group chat", destructive: true) {
            confirmLeave = true
        })])]
    }

    /// Split from `body` so the type checker finishes (swift-core-check / Xcode 26).
    private var paneBodySections: [PaneSection] {
        tab == "Members" ? membersSections + groupInviteSections : librarySections
    }

    private var paneFooterSections: [PaneSection] { muteFooter + leaveFooter }

    private var memberSubtitle: String {
        "\(memberCount) member\(memberCount == 1 ? "" : "s")"
    }

    @ViewBuilder private var mainStack: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            ScrollView {
                VStack(spacing: 18) {
                    Color.clear.frame(height: PaneKit.pageTop)
                    // The shared pane (D93): picture (crayon when a people group — it
                    // opens the edit sheet), name, member count, Members | Library.
                    // Leave / Delete group chat in the footer (same rule as the chat
                    // list: members leave, owners delete).
                    ProfilePane(
                        hero: PaneHero(
                            picture: .image(AnyView(heroPicture)),
                            name: title,
                            subtitle: memberSubtitle
                        ),
                        tabs: PaneKinds.group,
                        selected: $tab,
                        body: paneBodySections,
                        footer: paneFooterSections
                    )
                    Color.clear.frame(height: 40)
                }
            }
            ProfileTopBar(screen: .groupPage) { EmptyView() }
        }
    }

    var body: some View {
        // Modifiers live in a helper so `body` itself stays a small expression.
        withSheets(withDialogs(mainStack))
    }

    private func withDialogs<Content: View>(_ content: Content) -> some View {
        content
            .confirmationDialog(
                "Mute notifications",
                isPresented: $showMuteOptions,
                titleVisibility: .visible
            ) {
                ForEach(ChatMuteDuration.allCases) { duration in
                    Button(duration.title) { model.mutePeopleChat(chatId, for: duration) }
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("No banners from this group for the time you pick.")
            }
            .confirmationDialog(
                pendingRemoveName.map { "Remove \($0) from \(title)?" } ?? "Remove from chat?",
                isPresented: Binding(
                    get: { pendingRemoveId != nil },
                    set: { if !$0 { pendingRemoveId = nil; pendingRemoveName = nil } }
                ),
                titleVisibility: .visible
            ) {
                Button("Remove", role: .destructive) {
                    if let id = pendingRemoveId {
                        if rows.first(where: { $0.id == id })?.agent != nil {
                            model.removeAgent(chatId: chatId, projectId: id)
                        } else {
                            model.removeMember(chatId: chatId, userId: id, ban: false)
                        }
                    }
                    pendingRemoveId = nil
                    pendingRemoveName = nil
                }
                Button("Cancel", role: .cancel) {
                    pendingRemoveId = nil
                    pendingRemoveName = nil
                }
            } message: {
                Text("They can be added again later.")
            }
            .confirmationDialog(
                pendingBanName.map { "Ban \($0) from \(title)?" } ?? "Ban from chat?",
                isPresented: Binding(
                    get: { pendingBanId != nil },
                    set: { if !$0 { pendingBanId = nil; pendingBanName = nil } }
                ),
                titleVisibility: .visible
            ) {
                Button("Ban", role: .destructive) {
                    if let id = pendingBanId {
                        model.removeMember(chatId: chatId, userId: id, ban: true)
                    }
                    pendingBanId = nil
                    pendingBanName = nil
                }
                Button("Cancel", role: .cancel) {
                    pendingBanId = nil
                    pendingBanName = nil
                }
            } message: {
                Text("They are removed and cannot rejoin.")
            }
            .confirmationDialog(
                "Leave \(title)?",
                isPresented: $confirmLeave,
                titleVisibility: .visible
            ) {
                Button("Leave group chat", role: .destructive) {
                    model.leavePeopleChat(chatId)
                    dismiss()
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("You leave the group. It stays for everyone else.")
            }
            .confirmationDialog(
                "Delete \(title)?",
                isPresented: $confirmDelete,
                titleVisibility: .visible
            ) {
                Button("Delete group chat", role: .destructive) {
                    model.deletePeopleChat(chatId)
                    dismiss()
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("Deletes the group for everyone in it.")
            }
            .confirmationDialog(
                memberActions.map { $0.displayName } ?? "Member",
                isPresented: Binding(
                    get: { memberActions != nil },
                    set: { if !$0 { memberActions = nil } }
                ),
                titleVisibility: .visible
            ) {
                Button("Send private message") {
                    guard let handle = memberActions?.handle else { return }
                    memberActions = nil
                    Task {
                        if await model.openDM(handle: handle) != nil { dismiss() }
                    }
                }
                Button("View profile") {
                    if let p = memberActions { model.showProfile(.user(p)) }
                    memberActions = nil
                }
                Button("Cancel", role: .cancel) { memberActions = nil }
            }
    }

    private func withSheets<Content: View>(_ content: Content) -> some View {
        content
            .sheet(isPresented: $showAdd) {
                if let project {
                    ProjectMembersSheet(project: project).environmentObject(model).environment(\.theme, theme)
                } else {
                    // The New group picker aimed at this group: its members show as
                    // already in, Add puts the picks into it (not into a new group).
                    // A sheet over the profile sheet, like GroupEditSheet: no
                    // presentation / colour-scheme modifiers here — they unseat
                    // the presenting sheet.
                    NavigationStack {
                        NewGroupView(onDone: { showAdd = false }, addTo: chatId)
                    }
                    .tint(theme.text)
                    .environmentObject(model)
                    .environment(\.theme, theme)
                }
            }
            .sheet(isPresented: $showEdit) {
                GroupEditSheet(chatId: chatId).environmentObject(model).environment(\.theme, theme)
            }
            .sheet(isPresented: $showActivity) {
                // Admin-only activity log (joined / removed / left / deleted messages).
                // A sheet over the profile sheet — no presentation modifiers here.
                GroupActivityView(chatId: chatId)
                    .environmentObject(model)
                    .environment(\.theme, theme)
            }
            .task {
                if let projectId { projectMembers = await model.projectMembers(projectId) }
                if projectId == nil, people?.kind == "group" {
                    groupInvites = await model.groupInvites(chatId)
                }
            }
            .onAppear {
                // Rig: `ARBOS_SHOW=removemember` arms the Remove confirm for the first other member.
                if DebugHooks.show == "removemember", let r = rows.first(where: { canRemove($0) }) {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
                        pendingRemoveId = r.person.id
                        pendingRemoveName = r.agent?.name ?? "@\(r.person.handle)"
                    }
                }
                // Rig: `ARBOS_SHOW=addmembers` lands on the Add members picker;
                // `ARBOS_PANEL_TAB=Library` lands on the Library tab (the media grid).
                if DebugHooks.show == "addmembers", project == nil {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { showAdd = true }
                }
                // Rig: `ARBOS_SHOW=groupavatar[-generate|-upload]` opens the avatar picker on that tab.
                if let show = DebugHooks.show, show.hasPrefix("groupavatar"), project == nil {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
                        showAvatarPicker = show.hasSuffix("generate") ? .generate : (show.hasSuffix("upload") ? .upload : .bot)
                    }
                }
                if let t = DebugHooks.panelTab, PaneKinds.group.contains(t) { tab = t }
            }
            .sheet(item: $showAvatarPicker) { initial in
                AvatarPickerSheet(target: .group(chatId), initialTab: initial).environmentObject(model).environment(\.theme, theme)
            }
    }

    /// Invite links for a people group (not a shared project chat): mint, copy, share, revoke.
    private var groupInviteSections: [PaneSection] {
        guard projectId == nil, people?.kind == "group" else { return [] }
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        var rows: [AnyView] = []
        if groupInvites.isEmpty {
            rows.append(AnyView(PaneRow(leading: .tile("link"), title: "No invite links",
                                        subtitle: "People you add by name need none")))
        }
        for inv in groupInvites {
            let short = inv.url.split(separator: "/").last.map(String.init) ?? inv.code
            rows.append(AnyView(
                PaneRow(leading: .tile("link"), title: "…/g/\(short)", subtitle: inv.terms(now: now),
                        trailing: AnyView(HStack(spacing: 4) {
                            if inv.usable {
                                Button {
                                    UIPasteboard.general.string = inv.url
                                    inviteCopied = true
                                    Task { @MainActor in
                                        try? await Task.sleep(for: .seconds(1.5))
                                        inviteCopied = false
                                    }
                                } label: {
                                    Image(systemName: inviteCopied ? "checkmark" : "doc.on.doc")
                                        .font(theme.font(15)).foregroundStyle(theme.muted)
                                        .frame(width: 30, height: 44).contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)
                                .accessibilityLabel("Copy link")
                                if let url = URL(string: inv.url) {
                                    ShareLink(item: url) {
                                        Image(systemName: "square.and.arrow.up")
                                            .font(theme.font(15)).foregroundStyle(theme.muted)
                                            .frame(width: 30, height: 44).contentShape(Rectangle())
                                    }
                                    .buttonStyle(.plain)
                                }
                            }
                            Button {
                                Task {
                                    if await model.revokeGroupInvite(chatId, inviteId: inv.id) {
                                        groupInvites.removeAll { $0.id == inv.id }
                                    }
                                }
                            } label: {
                                Image(systemName: "xmark.circle")
                                    .font(theme.font(17)).foregroundStyle(theme.muted)
                                    .frame(width: 30, height: 44).contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Revoke link")
                        }))
                .opacity(inv.usable ? 1 : 0.55)
            ))
        }
        let options: [AnyView] = [
            AnyView(PaneToggleRow(title: "Expires after 7 days", isOn: $inviteExpires)),
            AnyView(PaneToggleRow(title: "Single use", subtitle: "One person joins, then the link is spent", isOn: $inviteSingleUse)),
            AnyView(Button {
                Task {
                    if let inv = await model.createGroupInvite(chatId, expires: inviteExpires, singleUse: inviteSingleUse) {
                        groupInvites.insert(inv, at: 0)
                    }
                }
            } label: {
                PaneRow(leading: .glyph("plus"), title: "New invite link")
            }.buttonStyle(.plain)),
        ]
        return [
            .title("Invite links"),
            .card(rows, separatorInset: PaneKit.separatorInset(.tile(""))),
            .card(options),
            .footnote("Share a link so someone can join this group. Add people who already have an account from Add members above."),
        ]
    }

    /// The hero picture: the agent's leaf for a shared project; the group's
    /// cluster or picked image (tap → the edit sheet) for a people group.
    @ViewBuilder private var heroPicture: some View {
        if let project {
            ViewablePicture(name: project.name, url: project.imageURL) {
                AgentAvatar(project: project, theme: theme, size: PaneKit.heroAvatar, idle: true)
            }
        } else {
            Button { showEdit = true } label: {
                AvatarCluster(members: clusterMembers, size: 72, imageUrl: groupImageURL, leaf: people?.leafPick, neutralName: people?.isNeutralAvatar == true ? people?.title : nil)
                    .frame(width: PaneKit.heroAvatar, height: PaneKit.heroAvatar)
                    .paneCrayon()
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Edit group name, description and picture")
        }
    }

    /// Members: Add members, Share invite link (people groups), Activity for
    /// owners/admins, then every person (name, @handle) and agent as kit rows.
    /// Banned people (owner/admin) sit under Banned with Unban.
    private var membersSections: [PaneSection] {
        var list: [AnyView] = [
            AnyView(Button { showAdd = true } label: {
                PaneRow(leading: .glyph("person.badge.plus"), title: "Add members")
            }
            .buttonStyle(.plain)),
        ]
        // People groups: description lives in Edit group (same sheet as name + picture).
        if projectId == nil, people?.isGroup == true {
            let desc = people?.description ?? ""
            list.append(AnyView(Button { showEdit = true } label: {
                PaneRow(
                    leading: .glyph("text.alignleft"),
                    title: "Description",
                    value: desc.isEmpty ? "Add a description" : desc,
                    chevron: true
                )
            }
            .buttonStyle(.plain)
            .accessibilityLabel(desc.isEmpty ? "Add a description" : "Edit description")))
        }
        // People groups only (not a shared project): one-tap share of the invite link.
        if project == nil, people?.kind == "group" {
            list.append(AnyView(Button {
                shareGroupInvite()
            } label: {
                PaneRow(
                    leading: .glyph("square.and.arrow.up"),
                    title: sharingInvite ? "Preparing link…" : "Share invite link"
                )
            }
            .buttonStyle(.plain)
            .disabled(sharingInvite)
            .accessibilityLabel("Share invite link")))
        }
        // People groups only: the activity log is built from this chat's system lines.
        if projectId == nil, people?.isGroup == true, canManageMembers {
            list.append(AnyView(Button { showActivity = true } label: {
                PaneRow(leading: .glyph("list.bullet.rectangle"), title: "Activity", chevron: true)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Group activity")))
        }
        list += rows.map { r in AnyView(memberRow(r)) }
        var out: [PaneSection] = [.card(list, separatorInset: PaneKit.separatorInset(.glyph("")))]
        if !bannedRows.isEmpty {
            out.append(.title("Banned"))
            out.append(.card(bannedRows.map { r in AnyView(bannedRow(r)) },
                             separatorInset: PaneKit.separatorInset(.glyph(""))))
        }
        return out
    }

    /// A banned person: name, @handle · banned, Unban for whoever may mute/ban.
    private func bannedRow(_ r: Row) -> some View {
        let unban: AnyView? = iCanMuteBan ? AnyView(
            Button {
                model.unbanMember(chatId: chatId, userId: r.person.id)
            } label: {
                Text("Unban")
                    .font(theme.font(Theme.small, weight: .medium))
            }
            .buttonStyle(.bordered)
            .tint(theme.muted)
            .accessibilityLabel("Unban @\(r.person.handle)")
        ) : nil
        return Button {
            model.showProfile(.user(r.person))
        } label: {
            PaneRow(
                leading: .avatar(AnyView(PersonAvatar(url: r.person.avatarUrl, handle: r.person.handle, size: PaneKit.avatar, name: r.person.name, id: r.person.id, colorIndex: r.person.avatarColorIndex))),
                title: r.person.displayName,
                subtitle: "@\(r.person.handle) · banned",
                trailing: unban
            )
        }
        .buttonStyle(.borderless)
        .contextMenu {
            Button {
                model.showProfile(.user(r.person))
            } label: { Label("View profile", systemImage: "person.crop.circle") }
            if iCanMuteBan {
                Divider()
                Button {
                    model.unbanMember(chatId: chatId, userId: r.person.id)
                } label: { Label("Unban", systemImage: "hand.raised.slash") }
            }
        }
    }

    /// Reuse a usable invite URL, or mint one, then open the system share sheet.
    /// Pass the link as a `String`, not a `URL`: Telegram's share extension drops a
    /// bare URL activity item, so the recipient's chat gets an empty message.
    private func shareGroupInvite() {
        guard !sharingInvite else { return }
        sharingInvite = true
        Task {
            if let existing = groupInvites.first(where: { $0.usable }),
               URL(string: existing.url) != nil {
                presentShareSheet(items: [existing.url])
                sharingInvite = false
                return
            }
            if let inv = await model.createGroupInvite(
                chatId, expires: inviteExpires, singleUse: inviteSingleUse
            ) {
                groupInvites.insert(inv, at: 0)
                if URL(string: inv.url) != nil {
                    presentShareSheet(items: [inv.url])
                }
            }
            sharingInvite = false
        }
    }

    private func memberSubtitle(_ r: Row) -> String {
        if let a = r.agent {
            let member = people?.agents.first { $0.id == a.id }
            let ownerId = member?.ownerId ?? a.ownerId
            if ownerId == nil || ownerId == model.user?.id { return "Agent · run by you" }
            if let h = member?.ownerHandle ?? a.ownerHandle { return "Agent · run by @\(h)" }
            return "Agent"
        }
        return "@\(r.person.handle)"
    }

    private func memberRow(_ r: Row) -> some View {
        let isMe = r.agent == nil && r.person.id == model.user?.id
        let title = r.agent?.name ?? (isMe ? "\(r.person.displayName) (you)" : r.person.displayName)
        let avatar: AnyView = r.agent.map { AnyView(AgentAvatar(project: $0, theme: theme, size: PaneKit.avatar, still: true)) }
            ?? AnyView(PersonAvatar(url: r.person.avatarUrl, handle: r.person.handle, size: PaneKit.avatar, name: r.person.name, id: r.person.id, colorIndex: r.person.avatarColorIndex))
        let member = r.agent == nil ? people?.members.first(where: { !$0.isAgent && $0.id == r.person.id }) : nil
        let isSilenced = member?.isSilencedNow ?? false
        let label = r.agent?.name ?? "@\(r.person.handle)"
        let showManage = canRemove(r) || (isGroupOwner && r.agent == nil && r.person.id != model.user?.id && r.role != "owner")
        // Owners/admins: Mute (with duration) / Unmute, Remove, Ban; owner also
        // Make admin and mute/ban rights. Same ellipsis pattern as the server row.
        let manage: AnyView? = showManage ? AnyView(
            Menu {
                if r.agent == nil {
                    if canMuteBan(r) {
                        if isSilenced {
                            Button {
                                model.setMemberSilenced(chatId: chatId, userId: r.person.id, silenced: false)
                            } label: { Label("Unmute", systemImage: "speaker.wave.2") }
                        } else {
                            Menu {
                                ForEach(MemberMuteDuration.allCases) { duration in
                                    Button(duration.title) {
                                        model.setMemberSilenced(
                                            chatId: chatId, userId: r.person.id,
                                            silenced: true, duration: duration
                                        )
                                    }
                                }
                            } label: { Label("Mute", systemImage: "speaker.slash") }
                        }
                    }
                    if isGroupOwner {
                        if r.role == "member" {
                            Button {
                                model.setMemberRole(chatId: chatId, userId: r.person.id, role: "admin")
                            } label: { Label("Make admin", systemImage: "person.badge.key") }
                        } else if r.role == "admin" {
                            let allowed = member?.canMuteBan ?? true
                            Button {
                                model.setMemberCanMuteBan(
                                    chatId: chatId, userId: r.person.id, canMuteBan: !allowed
                                )
                            } label: {
                                Label(
                                    allowed ? "Revoke mute and ban" : "Allow mute and ban",
                                    systemImage: allowed ? "hand.raised.slash" : "hand.raised"
                                )
                            }
                            Button {
                                model.setMemberRole(chatId: chatId, userId: r.person.id, role: "member")
                            } label: { Label("Remove admin", systemImage: "person.badge.minus") }
                        }
                    }
                    if canRemove(r) {
                        Divider()
                        Button(role: .destructive) {
                            pendingRemoveId = r.person.id
                            pendingRemoveName = label
                        } label: { Label("Remove", systemImage: "person.badge.minus") }
                        if canMuteBan(r) {
                            Button(role: .destructive) {
                                pendingBanId = r.person.id
                                pendingBanName = label
                            } label: { Label("Ban", systemImage: "hand.raised") }
                        }
                    }
                } else if canRemove(r) {
                    Button(role: .destructive) {
                        pendingRemoveId = r.person.id
                        pendingRemoveName = label
                    } label: { Label("Remove", systemImage: "person.badge.minus") }
                }
            } label: {
                Image(systemName: "ellipsis.circle")
                    .font(theme.font(17))
                    .foregroundStyle(theme.muted)
                    .frame(width: 32, height: 44)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("Member options")
        ) : nil
        // borderless (not .plain): .plain steals long-press on recent iOS,
        // so the remove context menu never appears (same as HomeView rows).
        // Blocked ([feedback blocked-state-ios-2026-10-26]): the tag on the row, before the menu.
        let isBlocked = r.agent == nil && !isMe && BlockedState.isBlocked(id: r.person.id, handle: r.person.handle, in: model.blocked)
        let trailing: AnyView? = {
            switch (isBlocked, manage) {
            case (true, let manage?): return AnyView(HStack(spacing: 6) { BlockedTag(); manage })
            case (true, nil): return AnyView(BlockedTag())
            case (false, let manage): return manage
            }
        }()
        return Button {
            if let a = r.agent {
                model.showProfile(.agent(projectId: a.id))
            } else if isMe {
                model.showProfile(.user(r.person))
            } else {
                // Offer Send private message alongside View profile (#1455).
                memberActions = r.person
            }
        } label: {
            PaneRow(
                leading: .avatar(avatar),
                title: title,
                subtitle: memberRowSubtitle(r, member: member, isSilenced: isSilenced),
                trailing: trailing
            )
        }
        .buttonStyle(.borderless)
        // Long-press: View profile, Send private message, Mute (with duration) /
        // Unmute, owner admin rights, Remove, Ban.
        .contextMenu {
            if r.agent == nil && !isMe {
                Button {
                    let handle = r.person.handle
                    Task {
                        if await model.openDM(handle: handle) != nil { dismiss() }
                    }
                } label: { Label("Send private message", systemImage: "bubble.left") }
            }
            Button {
                if let a = r.agent { model.showProfile(.agent(projectId: a.id)) } else { model.showProfile(.user(r.person)) }
            } label: { Label("View profile", systemImage: "person.crop.circle") }
            if showManage {
                Divider()
                if r.agent == nil {
                    if canMuteBan(r) {
                        if isSilenced {
                            Button {
                                model.setMemberSilenced(chatId: chatId, userId: r.person.id, silenced: false)
                            } label: { Label("Unmute", systemImage: "speaker.wave.2") }
                        } else {
                            ForEach(MemberMuteDuration.allCases) { duration in
                                Button("Mute · \(duration.title)") {
                                    model.setMemberSilenced(
                                        chatId: chatId, userId: r.person.id,
                                        silenced: true, duration: duration
                                    )
                                }
                            }
                        }
                    }
                    if isGroupOwner {
                        if r.role == "member" {
                            Button {
                                model.setMemberRole(chatId: chatId, userId: r.person.id, role: "admin")
                            } label: { Label("Make admin", systemImage: "person.badge.key") }
                        } else if r.role == "admin" {
                            let allowed = member?.canMuteBan ?? true
                            Button {
                                model.setMemberCanMuteBan(
                                    chatId: chatId, userId: r.person.id, canMuteBan: !allowed
                                )
                            } label: {
                                Label(
                                    allowed ? "Revoke mute and ban" : "Allow mute and ban",
                                    systemImage: allowed ? "hand.raised.slash" : "hand.raised"
                                )
                            }
                            Button {
                                model.setMemberRole(chatId: chatId, userId: r.person.id, role: "member")
                            } label: { Label("Remove admin", systemImage: "person.badge.minus") }
                        }
                    }
                    if canRemove(r) {
                        Button(role: .destructive) {
                            pendingRemoveId = r.person.id
                            pendingRemoveName = label
                        } label: { Label("Remove from group", systemImage: "person.badge.minus") }
                        if canMuteBan(r) {
                            Button(role: .destructive) {
                                pendingBanId = r.person.id
                                pendingBanName = label
                            } label: { Label("Ban from group", systemImage: "hand.raised") }
                        }
                    }
                } else if canRemove(r) {
                    Button(role: .destructive) {
                        pendingRemoveId = r.person.id
                        pendingRemoveName = label
                    } label: { Label("Remove from group", systemImage: "xmark") }
                }
            }
        }
    }

    private func memberRowSubtitle(_ r: Row, member: ChatMember?, isSilenced: Bool) -> String {
        var parts: [String] = [memberSubtitle(r)]
        if r.role == "admin" {
            let rights = (member?.canMuteBan ?? true) ? "can mute & ban" : "no mute & ban"
            parts.append(rights)
        }
        if isSilenced {
            if let until = member?.silencedUntil {
                let date = Date(timeIntervalSince1970: TimeInterval(until) / 1000)
                let f = DateFormatter()
                f.doesRelativeDateFormatting = true
                f.dateStyle = .short
                f.timeStyle = .short
                parts.append("Muted until \(f.string(from: date))")
            } else {
                parts.append("Muted")
            }
        }
        return parts.joined(separator: " · ")
    }

    /// Library: one list of everything shared here by day — the Mac's
    /// `profiles/library.rs` ([feedback library-list-ios-2026-10-25]; links as in
    /// the agent pane, [feedback group-library-links-2026-10-23]).
    private var librarySections: [PaneSection] {
        LibraryList.sections(items: items, empty: "Nothing shared here yet.")
    }

}

// MARK: - Group activity (admins)

/// Admin-only log of a people group's membership and deletion events.
/// Membership lines come from the relay (#1349). Deletion lines are written by
/// the deleting device with a private marker so the main transcript stays calm.
enum GroupActivity {
    /// Invisible separator — Transcript skips these; Activity keeps them.
    static let auditPrefix = "\u{2063}"

    enum Kind: String, CaseIterable {
        case deleted
        case joined
        case removed
        case left

        var title: String {
            switch self {
            case .deleted: return "Deleted messages"
            case .joined: return "Joined"
            case .removed: return "Removed"
            case .left: return "Left"
            }
        }
    }

    struct Event: Identifiable, Equatable {
        var id: String
        var kind: Kind
        var text: String
        var at: Int64
    }

    /// System-line text for a message deleted in a people group (hidden in the chat).
    static func deletionNotice(who: String) -> String {
        "\(auditPrefix)\(who) deleted a message"
    }

    static func isAuditLine(_ text: String) -> Bool {
        text.hasPrefix(auditPrefix)
    }

    static func displayText(_ raw: String) -> String {
        isAuditLine(raw) ? String(raw.dropFirst()) : raw
    }

    /// Newest first. Only membership notices and deletion audits — not call lines.
    static func events(from messages: [Message]) -> [Event] {
        var out: [Event] = []
        out.reserveCapacity(messages.count / 8)
        for m in messages where m.role == .system {
            let raw = m.plainText.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !raw.isEmpty, let kind = classify(raw) else { continue }
            out.append(Event(id: m.id, kind: kind, text: displayText(raw), at: m.createdAt))
        }
        out.sort { $0.at > $1.at }
        return out
    }

    static func classify(_ text: String) -> Kind? {
        if isAuditLine(text) { return .deleted }
        // Relay membership notices (#1349): "A added B", "A removed B", "A left".
        if text.hasSuffix(" left"), !text.contains(" added "), !text.contains(" removed ") {
            return .left
        }
        if text.contains(" removed ") { return .removed }
        if text.contains(" added ") { return .joined }
        return nil
    }

    static func whenLabel(_ at: Int64, now: Int64 = nowMs()) -> String {
        guard at > 0 else { return "" }
        let date = Date(timeIntervalSince1970: Double(at) / 1000)
        let age = max(0, now - at)
        if age < 86_400_000 {
            return date.formatted(date: .omitted, time: .shortened)
        }
        return date.formatted(date: .abbreviated, time: .shortened)
    }
}

/// Owners and admins: deleted messages, joins, removes, and leaves for one group.
struct GroupActivityView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let chatId: String

    private var events: [GroupActivity.Event] {
        GroupActivity.events(from: model.messages(of: chatId))
    }

    var body: some View {
        SettingsPage(title: "Activity") {
            if events.isEmpty {
                SettingsGroup {
                    Text("Joins, removals, leaves and deleted messages will show up here.")
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 12)
                }
            } else {
                ForEach(GroupActivity.Kind.allCases, id: \.self) { kind in
                    GroupActivitySection(kind: kind, events: events.filter { $0.kind == kind })
                }
            }
        }
    }
}

private struct GroupActivitySection: View {
    let kind: GroupActivity.Kind
    let events: [GroupActivity.Event]

    var body: some View {
        if !events.isEmpty {
            SettingsGroup(header: kind.title) {
                ForEach(events) { event in
                    GroupActivityRow(event: event)
                }
            }
        }
    }
}

private struct GroupActivityRow: View {
    @Environment(\.theme) private var theme
    let event: GroupActivity.Event

    private var when: String { GroupActivity.whenLabel(event.at) }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(event.text)
                .font(theme.font(Theme.small))
                .foregroundStyle(theme.text)
                .lineLimit(2)
            if !when.isEmpty {
                Text(when)
                    .font(theme.font(Theme.tiny))
                    .foregroundStyle(theme.muted)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 16)
        .padding(.vertical, 9)
    }
}

// MARK: - Server page (D88)

/// A server's details on the shared pane kit (rule 5): disc, name, member
/// count, Channels | Members, the invite link (rule 6), then the one action at
/// the end. Owners and admins get the admin rows (rename, add / rename /
/// delete a channel, "Admins only post", roles, remove, ban, reset link);
/// members see the same page read-only with Leave.
struct ServerProfileView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let serverId: String
    @State private var tab = "Channels"
    @State private var showRename = false
    @State private var renameDraft = ""
    @State private var showAvatarPicker = false
    @State private var avatarPickerTab: AvatarPickerTab = .bot
    @State private var showNewChannel = false
    @State private var renamingChannel: ServerChannel?
    @State private var channelDraft = ""
    @State private var deletingChannel: ServerChannel?
    @State private var removingMember: ServerMember?
    @State private var banningMember: ServerMember?
    @State private var confirmLeave = false
    @State private var confirmDelete = false
    @State private var confirmReset = false
    @State private var copied = false
    @State private var loadedMembers = false
    @State private var showAddMembers = false
    /// Settings › Invite links › New link options (owner).
    @State private var inviteExpires = false
    @State private var inviteSingleUse = false

    private var server: Server? { model.server(serverId) }
    private var isAdmin: Bool { server?.isAdmin ?? false }
    private var isOwner: Bool { server?.isOwner ?? false }
    private var memberCount: Int { max(server?.memberCount ?? 0, server?.members.count ?? 0) }

    var body: some View {
        serverDialogs(memberDialogs(channelDialogs(frame)))
            .sheet(isPresented: $showNewChannel) {
                NewChannelSheet(serverId: serverId).environmentObject(model).environment(\.theme, theme)
            }
            .sheet(isPresented: $showAddMembers) {
                // The groups' picker, people only; a sheet over the profile sheet (no
                // presentation modifiers here — they unseat the presenting sheet).
                NavigationStack {
                    NewGroupView(onDone: { showAddMembers = false }, addToServer: serverId)
                }
                .tint(theme.text)
                .environmentObject(model)
                .environment(\.theme, theme)
            }
            .task {
                if let t = DebugHooks.panelTab { tab = t }
                await model.refreshServer(serverId)
                loadedMembers = true
            }
    }

    /// The page without its dialogs: split out so the compiler type-checks each part alone.
    /// Shared profile pane (D93): hero, Channels | Members | Settings, Leave / Delete in the footer.
    private var frame: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            ScrollView {
                VStack(spacing: 18) {
                    Color.clear.frame(height: PaneKit.pageTop)
                    if let server {
                        ProfilePane(
                            hero: hero(server),
                            tabs: PaneKinds.server,
                            selected: $tab,
                            body: tab == "Channels" ? channelSections(server)
                                : tab == "Members" ? memberSections(server)
                                : settingsSections(server),
                            footer: [.card([AnyView(actions(server))])]
                        )
                    } else {
                        Text("This server is no longer available.")
                            .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                            .padding(.top, 80)
                    }
                    Color.clear.frame(height: 40)
                }
            }
            ProfileTopBar(screen: .serverPage) { EmptyView() }
        }
    }

    private func hero(_ server: Server) -> PaneHero {
        let channels = server.liveChannels.count
        return PaneHero(
            picture: .image(AnyView(heroPicture(server))),
            name: server.name,
            subtitle: "\(memberCount) member\(memberCount == 1 ? "" : "s") · \(channels) channel\(channels == 1 ? "" : "s")"
        )
    }

    @ViewBuilder
    private func heroPicture(_ server: Server) -> some View {
        Group {
            if isAdmin {
                // D92 amendment: the avatar picker (Bot | Generate | Upload) for the server's
                // picture; Rename lives on the Settings tab.
                Button { showAvatarPicker = true } label: {
                    ServerAvatar(server: server, size: PaneHeader<EmptyView>.avatarSize)
                        .paneCrayon()
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Change server picture")
            } else {
                ViewablePicture(name: server.name, url: server.imageKey.flatMap { model.blobURL(projectId: "servers:\(server.id)", key: $0)?.absoluteString }) {
                    ServerAvatar(server: server, size: PaneHeader<EmptyView>.avatarSize)
                }
            }
        }
    }

    /// Footer (every tab): Leave server — or Delete server for the owner.
    @ViewBuilder
    private func actions(_ server: Server) -> some View {
        if isOwner {
            ProfileLinkRow(title: "Delete server", destructive: true) { confirmDelete = true }
        } else {
            ProfileLinkRow(title: "Leave server", destructive: true) { confirmLeave = true }
        }
    }

    /// Settings (D93 order): identity (Rename) → behaviour (Open server) → relations (invite link).
    /// Settings (D93 order): identity (Rename) → behaviour (Open server) → relations (the
    /// owner's invite links — demoted to a fallback on 2026-10-22; admins see none).
    private func settingsSections(_ server: Server) -> [PaneSection] {
        guard isAdmin else { return [.empty("Nothing to set up here yet.")] }
        var out: [PaneSection] = [
            .card([AnyView(ProfileLinkRow(title: "Rename server") {
                renameDraft = server.name
                showRename = true
            })]),
            .custom(AnyView(openCard(server))),
        ]
        if isOwner {
            out += inviteSections(server)
        }
        return out
    }

    /// The owner's invite links: each with its terms, Copy / Share and revoke; then the two
    /// options and "New invite link".
    private func inviteSections(_ server: Server) -> [PaneSection] {
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        var rows: [AnyView] = []
        if server.invites.isEmpty {
            rows.append(AnyView(PaneRow(leading: .tile("link"), title: "No invite links",
                                        subtitle: "People you add by name need none")))
        }
        for inv in server.invites {
            let short = inv.url.split(separator: "/").last.map(String.init) ?? inv.code
            rows.append(AnyView(
                PaneRow(leading: .tile("link"), title: "…/s/\(short)", subtitle: inv.terms(now: now),
                        trailing: AnyView(HStack(spacing: 4) {
                            if inv.usable {
                                Button {
                                    UIPasteboard.general.string = inv.url
                                    copied = true
                                    Task { @MainActor in
                                        try? await Task.sleep(for: .seconds(1.5))
                                        copied = false
                                    }
                                } label: {
                                    Image(systemName: copied ? "checkmark" : "doc.on.doc")
                                        .font(theme.font(15)).foregroundStyle(theme.muted)
                                        .frame(width: 30, height: 44).contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)
                                .accessibilityLabel("Copy link")
                                if let url = URL(string: inv.url) {
                                    ShareLink(item: url) {
                                        Image(systemName: "square.and.arrow.up")
                                            .font(theme.font(15)).foregroundStyle(theme.muted)
                                            .frame(width: 30, height: 44).contentShape(Rectangle())
                                    }
                                    .buttonStyle(.plain)
                                }
                            }
                            Button {
                                Task { await model.revokeServerInvite(serverId, inviteId: inv.id) }
                            } label: {
                                Image(systemName: "xmark.circle")
                                    .font(theme.font(17)).foregroundStyle(theme.muted)
                                    .frame(width: 30, height: 44).contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Revoke link")
                        }))
                .opacity(inv.usable ? 1 : 0.55)
            ))
        }
        let options: [AnyView] = [
            AnyView(PaneToggleRow(title: "Expires after 7 days", isOn: $inviteExpires)),
            AnyView(PaneToggleRow(title: "Single use", subtitle: "One person joins, then the link is spent", isOn: $inviteSingleUse)),
            AnyView(Button {
                Task { await model.createServerInvite(serverId, expires: inviteExpires, singleUse: inviteSingleUse) }
            } label: {
                PaneRow(leading: .glyph("plus"), title: "New invite link")
            }.buttonStyle(.plain)),
        ]
        return [
            .title("Invite links"),
            .card(rows, separatorInset: PaneKit.separatorInset(.tile(""))),
            .card(options),
            .footnote("A fallback for people who are not on Arbos yet: the link opens Arbos and joins them. Add people who already have an account from the Members tab."),
        ]
    }

    private func channelDialogs<V: View>(_ v: V) -> some View {
        v
            .sheet(isPresented: $showAvatarPicker) {
                AvatarPickerSheet(target: .server(serverId), initialTab: avatarPickerTab).environmentObject(model).environment(\.theme, theme)
            }
            .onAppear {
                // Rig: `ARBOS_SHOW=serveravatar[-generate|-upload]` opens the avatar picker on that tab.
                if let show = DebugHooks.show, show.hasPrefix("serveravatar") {
                    avatarPickerTab = show.hasSuffix("generate") ? .generate : (show.hasSuffix("upload") ? .upload : .bot)
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { showAvatarPicker = true }
                }
            }
            .alert("Rename server", isPresented: $showRename) {
                TextField("Server name", text: $renameDraft)
                Button("Save") { Task { await model.renameServer(serverId, name: renameDraft) } }
                Button("Cancel", role: .cancel) { }
            }
            .alert("Rename channel", isPresented: Binding(
                get: { renamingChannel != nil },
                set: { if !$0 { renamingChannel = nil } }
            )) {
                TextField("Channel name", text: $channelDraft)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                Button("Save") {
                    if let c = renamingChannel { Task { await model.renameChannel(c, name: channelDraft) } }
                    renamingChannel = nil
                }
                Button("Cancel", role: .cancel) { renamingChannel = nil }
            } message: {
                Text("Letters, digits and dashes.")
            }
            .confirmationDialog(
                deletingChannel.map { "Delete #\($0.name)?" } ?? "Delete channel?",
                isPresented: Binding(
                    get: { deletingChannel != nil },
                    set: { if !$0 { deletingChannel = nil } }
                ),
                titleVisibility: .visible
            ) {
                Button("Delete channel", role: .destructive) {
                    if let c = deletingChannel { Task { await model.deleteChannel(c) } }
                    deletingChannel = nil
                }
                Button("Cancel", role: .cancel) { deletingChannel = nil }
            } message: {
                Text("Its messages go for everyone in the server.")
            }
    }

    private func memberDialogs<V: View>(_ v: V) -> some View {
        v
            .confirmationDialog(
                removingMember.map { "Remove @\($0.handle)?" } ?? "Remove member?",
                isPresented: Binding(
                    get: { removingMember != nil },
                    set: { if !$0 { removingMember = nil } }
                ),
                titleVisibility: .visible
            ) {
                Button("Remove", role: .destructive) {
                    if let m = removingMember { Task { await model.removeServerMember(serverId: serverId, userId: m.id, ban: false) } }
                    removingMember = nil
                }
                Button("Cancel", role: .cancel) { removingMember = nil }
            } message: {
                Text("They can join again with the invite link.")
            }
            .confirmationDialog(
                banningMember.map { "Ban @\($0.handle)?" } ?? "Ban member?",
                isPresented: Binding(
                    get: { banningMember != nil },
                    set: { if !$0 { banningMember = nil } }
                ),
                titleVisibility: .visible
            ) {
                Button("Ban", role: .destructive) {
                    if let m = banningMember { Task { await model.removeServerMember(serverId: serverId, userId: m.id, ban: true) } }
                    banningMember = nil
                }
                Button("Cancel", role: .cancel) { banningMember = nil }
            } message: {
                Text("They are removed and the invite link no longer works for them.")
            }
    }

    private func serverDialogs<V: View>(_ v: V) -> some View {
        v
            .confirmationDialog("Leave \(server?.name ?? "this server")?", isPresented: $confirmLeave, titleVisibility: .visible) {
                Button("Leave", role: .destructive) {
                    Task {
                        if await model.leaveServer(serverId) { dismiss() }
                    }
                }
            } message: { Text("You can come back with an invite link.") }
            .confirmationDialog("Delete \(server?.name ?? "this server")?", isPresented: $confirmDelete, titleVisibility: .visible) {
                Button("Delete server", role: .destructive) {
                    Task {
                        if await model.deleteServer(serverId) { dismiss() }
                    }
                }
            } message: { Text("Every channel and message goes, for everyone.") }
            .confirmationDialog("Reset the invite link?", isPresented: $confirmReset, titleVisibility: .visible) {
                Button("Reset link", role: .destructive) { Task { await model.resetServerInvite(serverId) } }
            } message: { Text("The old link stops working. People already in stay.") }
    }

    // MARK: Channels

    private func channelSections(_ server: Server) -> [PaneSection] {
        let inset = PaneKit.separatorInset(.glyph(""))
        var rows: [AnyView] = []
        if isAdmin {
            // Rule 7: owners/admins add channels; members never see this row.
            rows.append(AnyView(Button { showNewChannel = true } label: {
                PaneRow(leading: .glyph("plus"), title: "Add channel")
            }.buttonStyle(.plain)))
        }
        rows += server.liveChannels.map { c in AnyView(channelRow(c, in: server)) }
        var out: [PaneSection] = rows.isEmpty ? [.empty("No channels yet")] : [.card(rows, separatorInset: inset)]
        if !server.archivedChannels.isEmpty {
            // Channels I archived (row menu → Archive): Unarchive brings one back to my list.
            out.append(.title("Archived"))
            out.append(.card(server.archivedChannels.map { c in
                AnyView(PaneRow(leading: .glyph("archivebox"), title: c.name,
                                subtitle: model.channelRestricted(c) ? "Admins only post" : nil,
                                trailing: AnyView(
                                    Button("Unarchive") { Task { await model.archiveChannel(c, false) } }
                                        .font(theme.font(Theme.small, weight: .medium))
                                        .buttonStyle(.bordered)
                                        .tint(theme.muted)
                                )))
            }, separatorInset: inset))
        }
        return out
    }

    private func channelRow(_ c: ServerChannel, in server: Server) -> some View {
        let restricted = model.channelRestricted(c)
        let isGeneral = c.id == server.generalChannelId || (server.generalChannelId == nil && c.name == "general")
        let menu: AnyView? = isAdmin ? AnyView(
            Menu {
                Button {
                    channelDraft = c.name
                    renamingChannel = c
                } label: { Label("Rename", systemImage: "pencil") }
                Button {
                    Task { await model.setChannelRestricted(c, !restricted) }
                } label: {
                    Label("Admins only post", systemImage: restricted ? "checkmark" : "lock")
                }
                if !isGeneral {
                    Divider()
                    Button(role: .destructive) { deletingChannel = c } label: { Label("Delete channel", systemImage: "trash") }
                }
            } label: {
                Image(systemName: "ellipsis.circle")
                    .font(theme.font(17))
                    .foregroundStyle(theme.muted)
                    .frame(width: 32, height: 44)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("Channel options")
        ) : nil
        return Button {
            dismiss()
            model.open(channel: c)
        } label: {
            PaneRow(leading: .glyph("number"), title: c.name,
                    subtitle: restricted ? "Admins only post" : "Everyone posts",
                    trailing: menu)
        }
        .buttonStyle(.borderless)
    }

    // MARK: Members

    private func memberSections(_ server: Server) -> [PaneSection] {
        var rows: [AnyView] = []
        if isAdmin {
            // The main way in (Jacob 2026-10-22): pick people by name, the groups' picker.
            rows.append(AnyView(Button { showAddMembers = true } label: {
                PaneRow(leading: .glyph("person.badge.plus"), title: "Add members")
            }.buttonStyle(.plain)))
        }
        if server.members.isEmpty, !loadedMembers {
            return [.custom(AnyView(HStack { Spacer(); ProgressView().tint(theme.muted); Spacer() }.padding(.top, 36)))]
        }
        rows += sortedMembers(server).map { m in AnyView(memberRow(m)) }
        if rows.isEmpty { return [.empty("No members to show.")] }
        return [.card(rows, separatorInset: PaneKit.separatorInset(.glyph("")))]
    }

    /// Owner first, then admins, then members; each group by name.
    private func sortedMembers(_ server: Server) -> [ServerMember] {
        func rank(_ r: String) -> Int { r == "owner" ? 0 : r == "admin" ? 1 : 2 }
        return server.members.sorted {
            if rank($0.role) != rank($1.role) { return rank($0.role) < rank($1.role) }
            return $0.displayName.localizedCaseInsensitiveCompare($1.displayName) == .orderedAscending
        }
    }

    private func roleLabel(_ role: String) -> String {
        switch role {
        case "owner": return "Owner"
        case "admin": return "Admin"
        default: return "Member"
        }
    }

    private func memberRow(_ m: ServerMember) -> some View {
        let isMe = m.id == model.user?.id
        let title = isMe ? "\(m.displayName) (you)" : m.displayName
        // Owners manage everyone else; admins manage members only.
        let canManage = !isMe && m.role != "owner" && (isOwner || (isAdmin && m.role == "member"))
        let menu: AnyView? = canManage ? AnyView(
            Menu {
                if isOwner {
                    if m.role == "admin" {
                        Button { Task { await model.setServerRole(serverId: serverId, userId: m.id, role: "member") } } label: {
                            Label("Make member", systemImage: "person")
                        }
                    } else {
                        Button { Task { await model.setServerRole(serverId: serverId, userId: m.id, role: "admin") } } label: {
                            Label("Make admin", systemImage: "person.badge.key")
                        }
                    }
                    Divider()
                }
                Button(role: .destructive) { removingMember = m } label: { Label("Remove", systemImage: "person.badge.minus") }
                Button(role: .destructive) { banningMember = m } label: { Label("Ban", systemImage: "hand.raised") }
            } label: {
                Image(systemName: "ellipsis.circle")
                    .font(theme.font(17))
                    .foregroundStyle(theme.muted)
                    .frame(width: 32, height: 44)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("Member options")
        ) : nil
        let person = ProfilePerson(id: m.id, handle: m.handle, name: m.name, avatarUrl: m.avatarUrl,
                                   lastSeenAt: m.lastSeenAt, online: m.online, avatarColorIndex: m.avatarColorIndex)
        return Button {
            model.showProfile(.user(person))
        } label: {
            PaneRow(
                leading: .avatar(AnyView(PersonAvatar(url: m.avatarUrl, handle: m.handle, size: PaneKit.avatar, name: m.name, id: m.id, colorIndex: m.avatarColorIndex))),
                title: title,
                subtitle: "@\(m.handle) · \(roleLabel(m.role))",
                trailing: menu
            )
        }
        .buttonStyle(.borderless)
    }

    // MARK: Open + invite (rule 6)

    private func openCard(_ server: Server) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            PaneCard(header: "Discovery", footer: "When open, anyone on Arbos can find this server by name and join without an invite link.") {
                PaneRow(
                    title: "Open server",
                    subtitle: "Searchable · no invite needed",
                    trailing: AnyView(
                        Toggle("", isOn: Binding(
                            get: { server.isOpen },
                            set: { on in Task { await model.setServerOpen(serverId, open: on) } }
                        ))
                        .labelsHidden()
                        .tint(theme.online)
                        .accessibilityLabel("Open server")
                    )
                )
            }
        }
        .padding(.horizontal, PaneKit.inset)
    }

}

/// Finds the SwiftUI-hosted `UITextField` and makes a double-tap select the
/// whole string. The system default only selects one word, which is awkward for
/// comma-separated / hyphenated auto names like "a, b-c, d".
private struct SelectAllOnDoubleTap: UIViewRepresentable {
    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> UIView {
        let view = ProbeView()
        view.coordinator = context.coordinator
        return view
    }

    func updateUIView(_ uiView: UIView, context: Context) {
        context.coordinator.install(from: uiView)
    }

    final class ProbeView: UIView {
        weak var coordinator: Coordinator?
        override func didMoveToWindow() {
            super.didMoveToWindow()
            coordinator?.install(from: self)
        }
    }

    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        private weak var textField: UITextField?

        func install(from probe: UIView) {
            guard textField == nil, probe.window != nil else { return }
            guard let tf = Self.nearestTextField(from: probe) else { return }
            let gr = UITapGestureRecognizer(target: self, action: #selector(doubleTapped(_:)))
            gr.numberOfTapsRequired = 2
            gr.delegate = self
            tf.addGestureRecognizer(gr)
            textField = tf
        }

        @objc private func doubleTapped(_ gr: UITapGestureRecognizer) {
            guard let tf = gr.view as? UITextField else { return }
            if !tf.isFirstResponder { tf.becomeFirstResponder() }
            // After UIKit's word-select runs, expand to the full string.
            DispatchQueue.main.async { tf.selectAll(nil) }
        }

        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
            true
        }

        private static func nearestTextField(from view: UIView) -> UITextField? {
            var node: UIView? = view.superview
            while let current = node {
                if let tf = current as? UITextField { return tf }
                if let tf = findTextField(in: current, skipping: view) { return tf }
                node = current.superview
            }
            return nil
        }

        private static func findTextField(in root: UIView, skipping: UIView) -> UITextField? {
            for sub in root.subviews where sub !== skipping {
                if let tf = sub as? UITextField { return tf }
                if let tf = findTextField(in: sub, skipping: skipping) { return tf }
            }
            return nil
        }
    }
}

/// Name the group, set an optional description, and optionally pick a picture.
/// Saves with `PATCH /chats/{id}`.
struct GroupEditSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let chatId: String
    @State private var name = ""
    @State private var descriptionText = ""
    @State private var showAvatarPicker = false
    @State private var saving = false
    @FocusState private var nameFocused: Bool

    private var people: PeopleChat? { model.peopleChat(chatId) }

    private var clusterMembers: [ChatMember] {
        guard let people else { return [] }
        let others = people.humans.filter { $0.id != model.user?.id }
        return others.isEmpty ? people.humans : others
    }

    private var existingImageURL: String? {
        guard let people, let key = people.imageKey, !key.isEmpty else { return nil }
        return model.blobURL(projectId: "\(peopleSlot):\(chatId)", key: key)?.absoluteString
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 22) {
                    // The picture: the avatar picker sheet (D92 amendment — Bot = the group's
                    // leaf, Upload = a photo; changes apply on Set avatar).
                    Button { showAvatarPicker = true } label: {
                        ZStack(alignment: .bottomTrailing) {
                            AvatarCluster(members: clusterMembers, size: 72, imageUrl: existingImageURL, leaf: people?.leafPick, neutralName: people?.isNeutralAvatar == true ? people?.title : nil)
                                .frame(width: 112, height: 112)
                            GlassCircle(size: 36) { // control-size: exception rowIconTiles
                                Image(systemName: "camera.fill")
                                    .font(theme.font(14, weight: .semibold))
                                    .foregroundStyle(theme.text)
                            }
                            .offset(x: 4, y: 4)
                        }
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Change group picture")

                    VStack(alignment: .leading, spacing: 8) {
                        Text("Group name").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                        TextField("", text: $name, prompt: Text("Name this group").foregroundStyle(theme.placeholder))
                            .font(theme.font(Theme.body))
                            .foregroundStyle(theme.text)
                            .autocorrectionDisabled()
                            .focused($nameFocused)
                            .background(SelectAllOnDoubleTap())
                            .controlField()
                    }
                    .padding(.horizontal, 16)

                    VStack(alignment: .leading, spacing: 8) {
                        Text("Description").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                        TextField("", text: $descriptionText, prompt: Text("What this group is for").foregroundStyle(theme.placeholder), axis: .vertical)
                            .font(theme.font(Theme.body))
                            .foregroundStyle(theme.text)
                            .lineLimit(3...6)
                            .controlFieldGrowing()
                    }
                    .padding(.horizontal, 16)

                    Text("A name, description and picture help everyone recognise this chat. Leave the name blank to use member handles.")
                        .font(theme.font(Theme.tiny))
                        .foregroundStyle(theme.muted)
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, 28)

                    Color.clear.frame(height: 24)
                }
                .padding(.top, 28)
            }
            .background(theme.bg)
            .navigationTitle("Edit group")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(theme.bg, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") { dismiss() }.foregroundStyle(theme.text)
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button { Task { await save() } } label: {
                        Text(saving ? "Saving…" : "Save")
                            .font(theme.font(Theme.small, weight: .semibold))
                            .foregroundStyle(theme.onPrimary)
                            .padding(.horizontal, 12).frame(height: 32)
                            .background(Capsule().fill(theme.primary))
                    }
                    .buttonStyle(.plain)
                    .disabled(saving)
                    .opacity(saving ? 0.55 : 1)
                }
            }
            .onAppear {
                name = people?.title ?? ""
                descriptionText = people?.description ?? ""
                nameFocused = true
            }
            .sheet(isPresented: $showAvatarPicker) {
                AvatarPickerSheet(target: .group(chatId)).environmentObject(model).environment(\.theme, theme)
            }
        }
    }

    private func save() async {
        saving = true
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let desc = String(descriptionText.trimmingCharacters(in: .whitespacesAndNewlines).prefix(200))
        let ok = await model.patchPeopleChat(chatId, title: trimmed, description: desc)
        saving = false
        if ok { dismiss() }
    }
}

// MARK: - Shared profile pane (D93) — one component for every entity
// (docs/group-chat-redesign.md § "Shared profile pane"; the Mac twin is profiles::pane).

/// The hero: picture (image or glyph tile), name, optional badge, subtitle.
struct PaneHero {
    enum Picture {
        case image(AnyView)
        /// A glyph tile ("number" for channels, "archivebox"…).
        case glyph(String)
    }
    let picture: Picture
    let name: String
    /// A tap on the name (the agent pane: the owner's rename alert).
    var nameAction: (() -> Void)? = nil
    /// SF Symbol after the name ("lock.fill" on a private channel).
    var badge: String? = nil
    let subtitle: String
    /// The subtitle's colour when it is not the muted default (a placeholder reads fainter).
    var subtitleTint: Color? = nil
    /// A tap on the subtitle (the agent pane's label: tap to edit it in place).
    var subtitleAction: (() -> Void)? = nil
    /// The subtitle is a link — the foreign agent's "Run by …" (a chip pill).
    var subtitleLink = false
    /// Drawn instead of the subtitle while it is being edited.
    var subtitleEditor: AnyView? = nil
    /// Drawn under the subtitle (the "Run by …" link under an agent's label).
    var subtitleBelow: AnyView? = nil
    /// The hero's one action pill under the subtitle (a person's Message); nil
    /// on panes with no primary action and on a person you cannot message.
    var action: AnyView? = nil
}

/// One block of a tab's content, top to bottom.
enum PaneSection {
    /// Small caps title over a card ("Images · 4", "Archived").
    case title(String)
    /// Rows; the card draws separators between them only.
    case card([AnyView], separatorInset: CGFloat = PaneKit.inset)
    /// Centred muted line for an empty tab.
    case empty(String)
    case footnote(String)
    /// Escape hatch — say why on the component's issue.
    case custom(AnyView)
}

/// The blocks of a pane, top to bottom — the structure every kind shares
/// (Jacob, 2026-10-22 22:32 UTC: same structure, not the same tab count).
enum PaneBlock: Equatable {
    case hero
    /// Only when the kind has more than one tab.
    case tabPill
    /// The selected tab's sections.
    case options
    /// Pinned after the options (Block user, Delete agent, Sign out…).
    case footer
}

/// The tab set of every kind, the same labels and order as the Mac page of
/// that kind (`profiles/*.rs` `TABS`; the Mac's `pane::tests` holds the twin).
/// Agent: Grok Bot's Details · Library · Computer (Jacob 2026-10-24, the Mac's #1679;
/// iPhone [feedback iphone-agent-pane-tabs-2026-10-25]).
enum PaneKinds {
    static let person = ["Links", "Groups"]
    static let ownProfile = ["Profile", "Settings"]
    static let agent = ["Details", "Library", "Computer"]
    static let group = ["Members", "Library"]
    static let server = ["Channels", "Members", "Settings"]
    static let channel = ["Profile", "Settings"]
    static let all: [(String, [String])] = [
        ("person", person), ("own profile", ownProfile), ("agent", agent),
        ("group", group), ("server", server), ("channel", channel),
    ]
}

struct ProfilePane: View {
    @Environment(\.theme) private var theme
    let hero: PaneHero
    /// Same labels and order as the Mac page; zero or one tab shows no pill.
    let tabs: [String]
    @Binding var selected: String
    /// The selected tab's content.
    let sections: [PaneSection]
    /// Pinned after the body: Block user, Leave group, Sign out…
    var footer: [PaneSection] = []

    /// What `body` lays out for `tabs` tabs with `footerSections` pinned parts:
    /// hero first, then the pill (2+ tabs), then the options, then the footer.
    static func layout(tabs: Int, footerSections: Int) -> [PaneBlock] {
        var out: [PaneBlock] = [.hero]
        if tabs > 1 { out.append(.tabPill) }
        out.append(.options)
        if footerSections > 0 { out.append(.footer) }
        return out
    }

    init(hero: PaneHero, tabs: [String], selected: Binding<String>, body: [PaneSection], footer: [PaneSection] = []) {
        self.hero = hero
        self.tabs = tabs
        self._selected = selected
        self.sections = body
        self.footer = footer
    }

    /// How far the content fades out under the pinned hero + tabs (the chat
    /// header's treatment, #1336; Jacob Oct 22: sticky header, no hard cut).
    /// = the stack spacing, so the fade ends exactly where the first card
    /// starts at rest and only scrolled rows pass through it.
    static let fadeBand: CGFloat = 18

    var body: some View {
        // Hero + tab pill pin at the top of the scroll view (a pinned section
        // header); the sections scroll beneath and fade out at the join.
        LazyVStack(spacing: 18, pinnedViews: [.sectionHeaders]) {
            Section {
                ForEach(Array(sections.enumerated()), id: \.offset) { _, s in section(s) }
                if !footer.isEmpty {
                    Color.clear.frame(height: 1)
                    ForEach(Array(footer.enumerated()), id: \.offset) { _, s in section(s) }
                }
            } header: {
                VStack(spacing: 18) {
                    PaneHeader(name: hero.name, subtitle: hero.subtitle, badge: hero.badge, nameAction: hero.nameAction,
                               subtitleTint: hero.subtitleTint, subtitleAction: hero.subtitleAction, subtitleLink: hero.subtitleLink,
                               subtitleEditor: hero.subtitleEditor, subtitleBelow: hero.subtitleBelow, action: hero.action) {
                        switch hero.picture {
                        case .image(let v): v
                        case .glyph(let glyph):
                            RoundedRectangle(cornerRadius: PaneKit.heroAvatar * 0.28, style: .continuous)
                                .fill(theme.field)
                                .frame(width: PaneKit.heroAvatar, height: PaneKit.heroAvatar)
                                .overlay(Image(systemName: glyph).font(.system(size: PaneKit.heroAvatar * 0.42, weight: .medium)).foregroundStyle(theme.text))
                        }
                    }
                    if Self.layout(tabs: tabs.count, footerSections: footer.count).contains(.tabPill) {
                        ProfileTabs(tabs: tabs, selected: $selected)
                    }
                }
                .padding(.bottom, 4)
                .background(alignment: .bottom) {
                    // Opaque behind the pinned block, then a fade band below it so
                    // rows pass under softly.
                    VStack(spacing: 0) {
                        theme.bg
                        LinearGradient(colors: [theme.bg, theme.bg.opacity(0)], startPoint: .top, endPoint: .bottom)
                            .frame(height: Self.fadeBand)
                    }
                    .padding(.bottom, -Self.fadeBand)
                    .padding(.top, -200)
                }
            }
        }
    }

    @ViewBuilder
    private func section(_ s: PaneSection) -> some View {
        switch s {
        case .title(let t):
            // 8 pt down to the list it names (the kit's title spacing).
            PaneSectionTitle(t).padding(.bottom, -10)
        case .card(let rows, let inset):
            ProfileCard(separatorInset: inset) {
                ForEach(Array(rows.enumerated()), id: \.offset) { _, r in r }
            }
        case .empty(let t):
            Text(t).font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                .frame(maxWidth: .infinity).padding(.top, 36).padding(.horizontal, 32).multilineTextAlignment(.center)
        case .footnote(let t):
            // Hugs the card above it (8 pt, the card footer's gap; the Mac's kit::footnote).
            Text(t).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, PaneKit.inset + 16)
                .padding(.top, -10)
        case .custom(let v):
            v
        }
    }
}

/// Title + optional subtitle with a switch; the row flips through `isOn`.
struct PaneToggleRow: View {
    @Environment(\.theme) private var theme
    let title: String
    var subtitle: String? = nil
    @Binding var isOn: Bool
    var body: some View {
        PaneRow(leading: .none, title: title, subtitle: subtitle, trailing: AnyView(
            // The Mac switch is green when on (`glass::switch` → `theme.online`); same here.
            Toggle("", isOn: $isOn).labelsHidden().tint(theme.online)
        ))
    }
}

/// A paragraph inside a card (a channel topic, an agent's description).
struct PaneTextRow: View {
    @Environment(\.theme) private var theme
    let text: String
    var body: some View {
        Text(text).font(theme.font(Theme.body)).foregroundStyle(theme.text)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, PaneKit.inset).padding(.vertical, 12)
    }
}

// MARK: - Channel (D88 rule 13) on the shared profile pane (D93)

/// A channel's page: `#` glyph tile, the name (never with a leading `#`) with a lock badge when
/// admins-only, `N members · <server>`; **Profile** (topic, members, privacy, who posts, the
/// server, mute) | **Settings** (owner/admin: Name, Topic, Notifications, Privacy, Admins only
/// post). Footer on both tabs: Archive (everyone), Delete channel (owner/admin; asks first).
struct ChannelProfileView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let channelId: String
    @State private var tab = "Profile"
    @State private var showRename = false
    @State private var renameDraft = ""
    @State private var showTopic = false
    @State private var topicDraft = ""
    @State private var confirmDelete = false

    private var channel: ServerChannel? { model.channel(channelId) }
    private var server: Server? { channel.flatMap { model.server($0.serverId) } }
    private var isAdmin: Bool { server?.isAdmin ?? false }

    var body: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            ScrollView {
                VStack(spacing: 18) {
                    Color.clear.frame(height: PaneKit.pageTop)
                    if let channel, let server {
                        let restricted = model.channelRestricted(channel)
                        let count = max(server.memberCount, server.members.count)
                        ProfilePane(
                            hero: PaneHero(picture: .glyph("number"), name: channel.name,
                                           badge: restricted ? "lock.fill" : nil,
                                           subtitle: "\(count) member\(count == 1 ? "" : "s") · \(server.name)"),
                            tabs: PaneKinds.channel,
                            selected: $tab,
                            body: tab == "Profile"
                                ? profileSections(channel, server, restricted: restricted, count: count)
                                : settingsSections(channel, server, restricted: restricted),
                            footer: footerSections(channel, server)
                        )
                    } else {
                        Text("This channel is no longer available.")
                            .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                            .padding(.top, 80)
                    }
                    Color.clear.frame(height: 40)
                }
            }
            ProfileTopBar(screen: .channelPage) { EmptyView() }
        }
        .task {
            if let t = model.openServerHookTab { tab = t; model.openServerHookTab = nil }
            if let channel { await model.refreshServer(channel.serverId) }
        }
        .alert("Rename channel", isPresented: $showRename) {
            TextField("Channel name", text: $renameDraft)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            Button("Save") { if let c = channel { Task { _ = await model.renameChannel(c, name: renameDraft) } } }
            Button("Cancel", role: .cancel) { }
        } message: {
            Text("Letters, digits and dashes.")
        }
        .alert("Topic", isPresented: $showTopic) {
            TextField("What this channel is for", text: $topicDraft)
            Button("Save") { if let c = channel { Task { await model.setChannelTopic(c, topic: topicDraft) } } }
            Button("Cancel", role: .cancel) { }
        }
        .confirmationDialog(
            channel.map { "Delete #\($0.name)?" } ?? "Delete channel?",
            isPresented: $confirmDelete, titleVisibility: .visible
        ) {
            Button("Delete", role: .destructive) {
                if let c = channel { Task { if await model.deleteChannel(c) { dismiss() } } }
            }
            Button("Cancel", role: .cancel) { }
        } message: {
            Text("Deletes the channel and its messages for everyone in the server.")
        }
    }

    private func privacyLabel(_ restricted: Bool) -> String {
        restricted ? "Everyone reads · admins post" : "Everyone in the server"
    }

    private func serverRow(_ server: Server) -> AnyView {
        AnyView(Button {
            model.showProfile(.server(serverId: server.id))
        } label: {
            PaneRow(leading: .avatar(AnyView(
                InitialsAvatar(name: server.name, handle: nil, id: server.id,
                               override: server.avatarColorIndex, size: PaneKit.avatar)
            )), title: server.name, subtitle: "Server", chevron: true)
        }.buttonStyle(.plain))
    }

    private func muteRow(_ channel: ServerChannel, title: String, subtitle: String?) -> AnyView {
        AnyView(PaneRow(title: title, subtitle: subtitle, trailing: AnyView(
            Toggle("", isOn: Binding(
                get: { model.isChannelMuted(channel) },
                set: { _ in model.toggleChannelMute(channel) }
            )).labelsHidden().tint(theme.online)
        )))
    }

    private func profileSections(_ channel: ServerChannel, _ server: Server, restricted: Bool, count: Int) -> [PaneSection] {
        var about: [AnyView] = []
        if let topic = channel.topic, !topic.trimmingCharacters(in: .whitespaces).isEmpty {
            about.append(AnyView(PaneTextRow(text: topic)))
        }
        about.append(AnyView(Button {
            model.showProfile(.server(serverId: server.id))
        } label: {
            PaneRow(title: "Members", value: "\(count)", chevron: true)
        }.buttonStyle(.plain)))
        about.append(AnyView(PaneRow(title: "Privacy", value: privacyLabel(restricted))))
        about.append(AnyView(PaneRow(title: "Posting", value: restricted ? "Admins only" : "Everyone")))
        return [
            .card(about),
            .card([serverRow(server)], separatorInset: PaneKit.separatorInset(.avatar(AnyView(EmptyView())))),
            .card([muteRow(channel, title: "Mute channel", subtitle: "No banners from this channel on this phone")]),
        ]
    }

    private func settingsSections(_ channel: ServerChannel, _ server: Server, restricted: Bool) -> [PaneSection] {
        var identity: [AnyView] = []
        if isAdmin {
            identity.append(AnyView(Button {
                renameDraft = channel.name
                showRename = true
            } label: { PaneRow(title: "Name", value: channel.name, chevron: true) }.buttonStyle(.plain)))
            identity.append(AnyView(Button {
                topicDraft = channel.topic ?? ""
                showTopic = true
            } label: { PaneRow(title: "Topic", value: channel.topic ?? "Add a topic", chevron: true) }.buttonStyle(.plain)))
        } else {
            identity.append(AnyView(PaneRow(title: "Name", value: channel.name)))
            identity.append(AnyView(PaneRow(title: "Topic", value: channel.topic ?? "None")))
        }
        var behaviour: [AnyView] = [
            muteRow(channel, title: "Notifications", subtitle: model.isChannelMuted(channel) ? "Muted on this phone" : "Banners on this phone"),
            AnyView(PaneRow(title: "Privacy", value: privacyLabel(restricted))),
        ]
        if isAdmin {
            behaviour.append(AnyView(PaneRow(title: "Admins only post", subtitle: "Members read", trailing: AnyView(
                Toggle("", isOn: Binding(
                    get: { restricted },
                    set: { on in Task { await model.setChannelRestricted(channel, on) } }
                )).labelsHidden().tint(theme.online)
            ))))
        } else {
            behaviour.append(AnyView(PaneRow(title: "Posting", value: restricted ? "Admins only" : "Everyone")))
        }
        var out: [PaneSection] = [
            .card(identity),
            .card(behaviour),
            .card([serverRow(server)], separatorInset: PaneKit.separatorInset(.avatar(AnyView(EmptyView())))),
        ]
        if isAdmin { out.append(.footnote("Name and topic change for everyone in the server.")) }
        return out
    }

    private func footerSections(_ channel: ServerChannel, _ server: Server) -> [PaneSection] {
        let archived = channel.archived
        var rows: [AnyView] = [AnyView(Button {
            Task { await model.archiveChannel(channel, !archived) }
        } label: {
            PaneRow(leading: .tile(archived ? "tray.and.arrow.up" : "archivebox"),
                    title: archived ? "Unarchive channel" : "Archive channel",
                    subtitle: archived ? "Back in your channel list" : "Hidden from your list; the server page brings it back")
        }.buttonStyle(.plain))]
        if isAdmin, channel.id != server.general?.id {
            rows.append(AnyView(PaneDangerRow(title: "Delete channel") { confirmDelete = true }))
        }
        return [.card(rows, separatorInset: PaneKit.separatorInset(.tile("")))]
    }
}
