// Servers on the phone (D88). A server is one more row on Home, drawn like a
// chat row; tapping it slides into its channel list (this file). A channel
// tap opens that room in the normal ChatView. Everything here reuses the
// home-row metrics and the Hidden chats page header so nothing new has to be learned.

import ArbosFFI
import SwiftUI

/// A server in the home list: the D79 initials disc, the name, the newest line
/// across its channels, the time, the unread dot (rule 1 — no counts or badges).
struct ServerRow: View {
    @Environment(\.theme) private var theme
    let server: Server
    let preview: String
    let time: String
    let unread: Bool

    var body: some View {
        HStack(spacing: 15) {
            ServerAvatar(server: server, size: Theme.rowAvatar)
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline) {
                    Text(server.name)
                        .font(theme.font(Theme.body, weight: .semibold))
                        .foregroundStyle(theme.text)
                        .lineLimit(1)
                    Spacer()
                    Text(time)
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                        .lineLimit(1)
                        .fixedSize()
                }
                HStack(spacing: 6) {
                    Text(preview)
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                    if unread {
                        Circle().fill(theme.unread).frame(width: Theme.unreadDot, height: Theme.unreadDot)
                    }
                }
            }
        }
        .padding(.vertical, Theme.rowVPad)
        .contentShape(Rectangle())
    }
}

/// A channel in the server's list: the same row as a chat, with a `#` disc for
/// the avatar, a small lock when only admins post, the unread dot (rule 2).
struct ChannelRow: View {
    @Environment(\.theme) private var theme
    let channel: ServerChannel
    let restricted: Bool
    let preview: String
    let time: String
    let unread: Bool

    var body: some View {
        HStack(spacing: 15) {
            ZStack {
                Circle().fill(theme.card).frame(width: Theme.rowAvatar, height: Theme.rowAvatar)
                Text("#")
                    .font(theme.font(22, weight: .semibold))
                    .foregroundStyle(theme.accent)
            }
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(channel.name)
                        .font(theme.font(Theme.body, weight: .semibold))
                        .foregroundStyle(theme.text)
                        .lineLimit(1)
                    if restricted {
                        Image(systemName: "lock.fill")
                            .font(theme.font(11, weight: .semibold))
                            .foregroundStyle(theme.muted)
                            .accessibilityLabel("Only admins post")
                    }
                    Spacer()
                    Text(time)
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                        .lineLimit(1)
                        .fixedSize()
                }
                HStack(spacing: 6) {
                    Text(preview)
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                    if unread {
                        Circle().fill(theme.unread).frame(width: Theme.unreadDot, height: Theme.unreadDot)
                    }
                }
            }
        }
        .padding(.vertical, Theme.rowVPad)
        .contentShape(Rectangle())
    }
}

/// The server's channel list — the pane that slides in from Home (rule 2):
/// back circle, the server's disc + name in the centre pill (tap → server
/// details, rule 5), `+` for owners/admins (rule 7), then the channel rows.
struct ServerChannelsView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let serverId: String
    @State private var showNewChannel = false
    /// Long-press menu → Delete: the channel asking "Delete #name?".
    @State private var deletingChannel: ServerChannel?

    private var server: Server? { model.server(serverId) }

    var body: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            ScrollViewReader { proxy in
            List {
                Color.clear.frame(height: HomeChrome.barOnlyListTop)
                    .listRowInsets(EdgeInsets())
                    .listRowSeparator(.hidden)
                    .listRowBackground(Color.clear)
                    .accessibilityHidden(true)
                if let server {
                    if server.liveChannels.isEmpty {
                        Text("No channels yet")
                            .font(theme.font(Theme.small))
                            .foregroundStyle(theme.muted)
                            .frame(maxWidth: .infinity)
                            .padding(.top, 80)
                            .listRowInsets(EdgeInsets())
                            .listRowSeparator(.hidden)
                            .listRowBackground(Color.clear)
                    }
                    ForEach(server.liveChannels) { channel in
                        Button { model.open(channel: channel) } label: {
                            ChannelRow(channel: channel,
                                       restricted: model.channelRestricted(channel),
                                       preview: model.channelPreview(channel),
                                       time: HomeView.timeLabel(ms: model.channelActivity(channel)),
                                       unread: model.channelUnread(channel))
                        }
                        .buttonStyle(.borderless)
                        // Long-press (the Mac's right-click): Archive for everyone; Delete for
                        // owners/admins, never on #general; Delete asks first.
                        .contextMenu {
                            Button {
                                Task { await model.archiveChannel(channel, true) }
                            } label: { Label("Archive", systemImage: "archivebox") }
                            if server.isAdmin, channel.id != server.general?.id {
                                Divider()
                                Button(role: .destructive) { deletingChannel = channel } label: {
                                    Label("Delete", systemImage: "trash")
                                }
                            }
                        }
                        .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 22))
                        .listRowSeparator(.hidden)
                        .listRowBackground(theme.bg)
                    }
                }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .background(theme.bg)
            .refreshable { await model.refreshServers() }
            .rigListScroll(proxy, ids: server?.liveChannels.map(\.id) ?? [])
            }
            ListTopFade(screen: .serverChannels)
            header
        }
        // Custom back chevron; swipe right (edge pop or anywhere) goes back to Home —
        // `BackSwipe.swift`, the same rules as every chat screen.
        .backSwipeToPop()
        .sheet(isPresented: $showNewChannel) {
            NewChannelSheet(serverId: serverId).environmentObject(model).environment(\.theme, theme)
        }
        .confirmationDialog(
            deletingChannel.map { "Delete #\($0.name)?" } ?? "Delete channel?",
            isPresented: Binding(get: { deletingChannel != nil }, set: { if !$0 { deletingChannel = nil } }),
            titleVisibility: .visible
        ) {
            Button("Delete", role: .destructive) {
                if let c = deletingChannel { Task { _ = await model.deleteChannel(c) } }
                deletingChannel = nil
            }
            Button("Cancel", role: .cancel) { deletingChannel = nil }
        } message: {
            Text("Deletes the channel and its messages for everyone in the server.")
        }
        // `AppModel.forgetServer` empties the path when the server goes (deleted, left, removed).
    }

    /// Same header as Hidden chats: back circle left, the title pill centred.
    private var header: some View {
        HStack {
            Button { dismiss() } label: {
                GlassCircle {
                    Image(systemName: "chevron.left")
                        .font(theme.font(Controls.glyph, weight: .semibold))
                        .foregroundStyle(theme.text)
                }
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Back")
            .accessibilityIdentifier("server-back:\(serverId)")
            Spacer()
            GlassPill {
                HStack(spacing: Theme.headerPillGap) {
                    if let server {
                        ServerAvatar(server: server, size: Controls.avatar)
                    }
                    Text(server?.name ?? "Server")
                        .font(.system(size: Theme.body, weight: .semibold))
                        .foregroundStyle(theme.text)
                        .lineLimit(1)
                }
                .padding(.leading, Controls.avatarInset)
                .padding(.trailing, Controls.textInset)
                .frame(height: Controls.height)
                .contentShape(Capsule())
                .onTapGesture { model.showProfile(.server(serverId: serverId)) }
                .accessibilityAddTraits(.isButton)
                .accessibilityHint("Opens the server details")
            }
            Spacer()
            if server?.isAdmin == true {
                Button { showNewChannel = true } label: {
                    GlassCircle {
                        Image(systemName: "plus")
                            .font(theme.font(Controls.glyph, weight: .regular))
                            .foregroundStyle(theme.text)
                    }
                }
                .buttonStyle(.plain)
                .accessibilityLabel("New channel")
            } else {
                // Members cannot add channels; keep the pill centred.
                Color.clear.frame(width: Theme.control, height: Theme.control)
            }
        }
        .controlBar()
    }
}

/// Rule 7: a new channel — a name (letters, digits, dashes) and announcement mode
/// (admins post; members can like, not reply).
struct NewChannelSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let serverId: String
    @State private var name = ""
    @State private var restricted = false
    @State private var creating = false
    @State private var errorText: String?
    @FocusState private var focused: Bool

    private var slug: String { AppModel.channelSlug(name) }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 22) {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("Channel name").font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                        HStack(spacing: 6) {
                            Text("#").font(theme.font(Theme.body, weight: .semibold)).foregroundStyle(theme.muted)
                            TextField("", text: $name, prompt: Text("announcements").foregroundStyle(theme.placeholder))
                                .font(theme.font(Theme.body))
                                .foregroundStyle(theme.text)
                                .textInputAutocapitalization(.never)
                                .autocorrectionDisabled()
                                .focused($focused)
                                .submitLabel(.done)
                                .onSubmit { create() }
                        }
                        .controlField()
                        if !name.isEmpty, slug != name {
                            Text(slug.isEmpty ? "Use letters, digits and dashes." : "Will be #\(slug)")
                                .font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                        }
                    }
                    .padding(.horizontal, 16)

                    ProfileCard {
                        PaneRow(title: "Announcement channel",
                                subtitle: restricted
                                    ? "Admins post · members can like, not reply"
                                    : "Everyone posts",
                                trailing: AnyView(Toggle("", isOn: $restricted).labelsHidden().tint(theme.primary)))
                    }

                    if let errorText {
                        Text(errorText)
                            .font(theme.font(Theme.small))
                            .foregroundStyle(theme.danger)
                            .multilineTextAlignment(.center)
                            .padding(.horizontal, 20)
                    }
                    Color.clear.frame(height: 24)
                }
                .padding(.top, 28)
            }
            .background(theme.bg)
            .navigationTitle("New channel")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(theme.bg, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") { dismiss() }.foregroundStyle(theme.text)
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button { create() } label: {
                        Text(creating ? "Creating…" : "Create")
                            .font(theme.font(Theme.small, weight: .semibold))
                            .foregroundStyle(theme.onPrimary)
                            .padding(.horizontal, 12).frame(height: 32)
                            .background(Capsule().fill(theme.primary))
                    }
                    .buttonStyle(.plain)
                    .disabled(slug.isEmpty || creating)
                    .opacity(slug.isEmpty || creating ? 0.45 : 1)
                }
            }
            .onAppear { focused = true }
        }
        .tint(theme.text)
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .presentationBackground(theme.bg)
        .presentationCornerRadius(28)
        .preferredColorScheme(theme.isDark ? .dark : .light)
    }

    private func create() {
        guard !slug.isEmpty, !creating else { return }
        creating = true
        errorText = nil
        Task {
            if let c = await model.createChannel(serverId: serverId, name: slug, restricted: restricted) {
                dismiss()
                model.open(channel: c)
            } else {
                // Copy onto this screen: RootView's alert is behind the sheet.
                errorText = model.notice?.text ?? "Could not create the channel."
                model.notice = nil
            }
            creating = false
        }
    }
}


/// Rule 8: "Join a server" — paste an invite link (or its code), or search an
/// open server by name and join without a link (`POST /servers/join`).
struct JoinServerView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let onDone: () -> Void
    @State private var link = ""
    @State private var joining = false
    @State private var searching = false
    @State private var openHits: [Server] = []
    @State private var errorText: String?
    @FocusState private var focused: Bool

    /// The code at the end of `https://…/join/<code>` or `arbos://server/<code>`, or the bare code.
    private var code: String {
        let raw = link.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !raw.isEmpty else { return "" }
        if raw.contains("/"), let last = raw.split(separator: "/").last {
            return String(last).trimmingCharacters(in: CharacterSet(charactersIn: "#?&"))
        }
        return raw
    }

    /// Looks like a pasted invite (URL or the relay's 10-char code), not a server name to search.
    private var looksLikeInvite: Bool {
        let raw = link.trimmingCharacters(in: .whitespacesAndNewlines)
        if raw.contains("/") || raw.contains("://") { return true }
        // Bare invite codes are 10 lowercase alphanumerics (see relay `invite_code`).
        return raw.count == 10
            && raw == raw.lowercased()
            && raw.unicodeScalars.allSatisfy { CharacterSet.alphanumerics.contains($0) }
    }

    var body: some View {
        VStack(spacing: 18) {
            GlassCircle(size: 92) { // control-size: exception illustrationDiscs
                Image(systemName: "link").font(.system(size: 34, weight: .medium)).foregroundStyle(theme.text)
            }
            .padding(.top, 28)
            Text("Paste an invite link, or search an open server")
                .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 24)
            TextField("", text: $link, prompt: Text("Invite link or server name").foregroundStyle(theme.placeholder))
                .font(theme.font(Theme.body)).foregroundStyle(theme.text)
                .multilineTextAlignment(.center)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .keyboardType(.URL)
                .focused($focused)
                .submitLabel(.join)
                .onSubmit(join)
                .onChange(of: link) { _, _ in searchOpen() }
                .controlField()
                .padding(.horizontal, 24)
            Button(action: join) {
                Text(joining ? "Joining…" : "Join")
                    .font(theme.font(Theme.body, weight: .semibold)).foregroundStyle(theme.onPrimary)
                    .frame(maxWidth: .infinity).frame(height: 50)
                    .background(Capsule().fill(theme.primary))
            }
            .buttonStyle(.plain)
            .disabled(code.isEmpty || joining || (!looksLikeInvite && openHits.isEmpty))
            .opacity(code.isEmpty || joining || (!looksLikeInvite && openHits.isEmpty) ? 0.45 : 1)
            .padding(.horizontal, 24)
            if let errorText {
                Text(errorText)
                    .font(theme.font(Theme.small)).foregroundStyle(theme.danger)
                    .multilineTextAlignment(.center).padding(.horizontal, 36)
            }
            if !openHits.isEmpty {
                VStack(spacing: 0) {
                    ForEach(Array(openHits.enumerated()), id: \.element.id) { i, s in
                        Button { joinOpen(s) } label: { ServerPickRow(server: s) }
                            .buttonStyle(.plain)
                        if i < openHits.count - 1 {
                            Rectangle().fill(theme.separator).frame(height: 0.5).padding(.leading, 72)
                        }
                    }
                }
                .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(theme.card))
                .padding(.horizontal, 24)
            } else if searching {
                ProgressView().tint(theme.muted)
            }
            Spacer()
        }
        .background(theme.bg)
        .navigationTitle("Join a server")
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(theme.bg, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .onAppear { focused = true }
    }

    private func searchOpen() {
        let q = link.trimmingCharacters(in: .whitespacesAndNewlines)
        guard q.count >= 2, !looksLikeInvite else { openHits = []; searching = false; return }
        searching = true
        Task {
            try? await Task.sleep(nanoseconds: 250_000_000)
            guard q == link.trimmingCharacters(in: .whitespacesAndNewlines) else { return }
            openHits = await model.searchOpenServers(q)
            if q == link.trimmingCharacters(in: .whitespacesAndNewlines) { searching = false }
        }
    }

    private func join() {
        guard !joining else { return }
        if looksLikeInvite {
            guard !code.isEmpty else { return }
            joining = true
            errorText = nil
            Task {
                if await model.joinServer(code: code) {
                    onDone()
                } else {
                    errorText = model.notice?.text ?? "Could not join the server."
                    model.notice = nil
                }
                joining = false
            }
            return
        }
        if let first = openHits.first { joinOpen(first) }
    }

    private func joinOpen(_ s: Server) {
        guard !joining else { return }
        joining = true
        errorText = nil
        Task {
            if await model.joinOpenServer(s.id) {
                onDone()
            } else {
                errorText = model.notice?.text ?? "Could not join the server."
                model.notice = nil
            }
            joining = false
        }
    }
}
