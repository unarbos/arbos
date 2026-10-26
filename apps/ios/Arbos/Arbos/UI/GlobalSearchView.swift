// The iPhone's bottom search bar and its results (Jacob, 2026-10-25, [feedback
// iphone-bottom-search-bar-2026-10-25]): the Telegram-style tab bar is gone;
// one capsule sits at the bottom of Home — nothing beside it — and rides the
// keyboard. Typing replaces the chat list with the Mac palette's sections
// (People & agents · Chats · Servers & channels · Messages · Files, media &
// links); a tap opens the chat, the message in its chat, the person's DM, the
// channel, or the Library item. The × inside the capsule clears and closes;
// dragging the results dismisses the keyboard.

import SwiftUI

/// The capsule at the bottom of Home: magnifier, field, × while active.
struct HomeSearchBar: View {
    @Environment(\.theme) private var theme
    @Binding var query: String
    var focused: FocusState<Bool>.Binding
    /// True while the bar is focused or holds a query (the results are up).
    let active: Bool
    let onClose: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .font(theme.font(Controls.glyph, weight: .medium))
                .foregroundStyle(theme.icon)
            TextField("", text: $query, prompt: Text(HomeChrome.searchPlaceholder).foregroundStyle(theme.placeholder))
                .font(theme.font(Theme.body))
                .foregroundStyle(theme.text)
                .focused(focused)
                .submitLabel(.search)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .accessibilityIdentifier("home-search-field")
            if active {
                Button(action: onClose) {
                    Image(systemName: "xmark.circle.fill")
                        .font(theme.font(17))
                        .foregroundStyle(theme.icon)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
                .transition(.opacity)
            }
        }
        // The family's field (#1810 bar = the composer's field: 48, capsule, text at 17, rim).
        .controlField()
        .onTapGesture { focused.wrappedValue = true }
        .accessibilityIdentifier("home-bottom-search")
    }
}

/// The results list under the bar while a query is typed.
struct GlobalSearchResults: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    /// The typed text (for the empty copy) and its folded form (for matching).
    let query: String
    let folded: String
    /// Called after a row opened something (the home clears the bar).
    let onOpen: () -> Void
    @State private var viewer: MediaViewerState?

    private var groups: [GlobalSearch.Group] {
        GlobalSearch.arrange(model.globalSearchEntries(query: folded), queryIsEmpty: folded.isEmpty)
    }

    var body: some View {
        let groups = self.groups
        List {
            Color.clear.frame(height: 66)
                .listRowInsets(EdgeInsets())
                .listRowSeparator(.hidden)
                .listRowBackground(Color.clear)
                .accessibilityHidden(true)
            if groups.isEmpty {
                Text(folded.isEmpty ? "Search chats, people, agents, servers, messages and files" : GlobalSearch.emptyCopy(for: query))
                    .font(theme.font(Theme.small))
                    .foregroundStyle(theme.muted)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: .infinity)
                    .padding(.horizontal, 36)
                    .padding(.top, 80)
                    .listRowInsets(EdgeInsets())
                    .listRowSeparator(.hidden)
                    .listRowBackground(Color.clear)
                    .accessibilityIdentifier("home-search-empty")
            }
            ForEach(groups, id: \.section) { group in
                Section {
                    ForEach(group.rows) { row in
                        Button { open(row) } label: {
                            GlobalSearchRow(entry: row)
                        }
                        .buttonStyle(.borderless)
                        .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 22))
                        .listRowSeparator(.hidden)
                        .listRowBackground(theme.bg)
                        .accessibilityIdentifier("search-row")
                    }
                } header: {
                    Text(group.section.label)
                        .font(theme.font(Theme.tiny, weight: .medium))
                        .foregroundStyle(theme.muted)
                        .textCase(nil)
                        .padding(.leading, 4)
                        .accessibilityIdentifier("search-section-\(group.section.label)")
                }
            }
            Color.clear.frame(height: 8)
                .listRowInsets(EdgeInsets())
                .listRowSeparator(.hidden)
                .listRowBackground(Color.clear)
                .accessibilityHidden(true)
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(theme.bg)
        .scrollDismissesKeyboard(.interactively)
        .task {
            // Rig: `ARBOS_SEARCH_OPEN=<n>` opens the n-th row once the rows are there.
            guard let n = DebugHooks.searchOpen, n > 0 else { return }
            try? await Task.sleep(nanoseconds: 1_200_000_000)
            let rows = GlobalSearch.flatten(self.groups)
            if rows.indices.contains(n - 1) { open(rows[n - 1]) }
        }
        .fullScreenCover(item: $viewer) { v in
            MediaViewer(album: v.items, index: v.index, onClose: { viewer = nil })
                .environmentObject(model)
                .environment(\.theme, theme)
        }
        .accessibilityIdentifier("home-search-results")
    }

    private func open(_ entry: GlobalSearch.Entry) {
        if let item = model.open(searchTarget: entry.target) {
            // A picture / file: the Library item in the viewer; links already opened.
            if let media = item.media {
                viewer = MediaViewerState(items: [media], index: 0)
            } else if let url = item.url {
                UIApplication.shared.open(url)
            }
            return
        }
        onOpen()
    }
}

/// One result: a 36 pt picture or glyph tile, the name, the subtitle.
struct GlobalSearchRow: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let entry: GlobalSearch.Entry

    var body: some View {
        HStack(spacing: PaneKit.gap + 2) {
            picture
                .frame(width: PaneKit.avatar, height: PaneKit.avatar)
            VStack(alignment: .leading, spacing: 2) {
                Text(entry.title)
                    .font(theme.font(Theme.body, weight: .medium))
                    .foregroundStyle(theme.text)
                    .lineLimit(1)
                Text(entry.subtitle)
                    .font(theme.font(Theme.tiny))
                    .foregroundStyle(theme.muted)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
        }
        .padding(.vertical, 9)
        .contentShape(Rectangle())
    }

    @ViewBuilder
    private var picture: some View {
        switch entry.picture {
        case .project(let id):
            if let p = model.project(id) ?? model.foreignAgent(id) {
                AgentAvatar(project: p, theme: theme, size: PaneKit.avatar, still: true)
            } else {
                glyphTile("sparkles")
            }
        case .peopleChat(let id):
            if let c = model.peopleChat(id) {
                if c.isGroup {
                    // The home row's group picture, at the pane's 36 pt (`PeopleRow`).
                    if let key = c.imageKey, !key.isEmpty,
                       let url = model.blobURL(projectId: "\(peopleSlot):\(c.id)", key: key)?.absoluteString {
                        AvatarCluster(members: [], size: PaneKit.avatar, imageUrl: url)
                    } else if c.isNeutralAvatar {
                        NeutralAvatar(name: c.title, size: PaneKit.avatar)
                    } else if let leaf = c.leafPick, LeafAvatarsFlag.shared.on {
                        LeafAvatar(pick: leaf, size: PaneKit.avatar)
                    } else {
                        ZStack {
                            Circle().fill(theme.card)
                            Image(systemName: "person.2.fill")
                                .font(theme.font(14, weight: .medium))
                                .foregroundStyle(theme.accent)
                        }
                    }
                } else {
                    let other = c.counterpart(me: model.user?.id)
                    PersonAvatar(url: other?.avatarUrl, handle: other?.handle ?? "?", size: PaneKit.avatar, name: other?.name, id: other?.id, colorIndex: other?.avatarColorIndex)
                }
            } else {
                glyphTile("person.2.fill")
            }
        case .server(let id):
            if let s = model.servers.first(where: { $0.id == id }) {
                ServerAvatar(server: s, size: PaneKit.avatar)
            } else {
                glyphTile("number")
            }
        case .person(let url, let handle, let name, let id, let colorIndex):
            PersonAvatar(url: url, handle: handle, size: PaneKit.avatar, name: name, id: id, colorIndex: colorIndex)
        case .thumbnail(let item):
            ZStack {
                RoundedRectangle(cornerRadius: LibraryList.thumbRadius, style: .continuous).fill(theme.field)
                BlobImage(item: item)
            }
            .clipShape(RoundedRectangle(cornerRadius: LibraryList.thumbRadius, style: .continuous))
        case .none:
            glyphTile(glyph)
        }
    }

    /// The kind's SF Symbol when there is no picture.
    private var glyph: String {
        switch entry.kind {
        case .agent: return "sparkles"
        case .person: return "person.crop.circle"
        case .group: return "person.2.fill"
        case .server: return "square.grid.2x2"
        case .channel: return "number"
        case .message: return "text.bubble"
        case .media: return "photo"
        case .file: return "doc.fill"
        case .link: return "link"
        }
    }

    private func glyphTile(_ name: String) -> some View {
        ZStack {
            RoundedRectangle(cornerRadius: LibraryList.thumbRadius, style: .continuous).fill(theme.field)
            Image(systemName: name)
                .font(theme.font(15, weight: .medium))
                .foregroundStyle(theme.muted)
        }
    }
}
