// Telegram-style avatars (D79): a coloured gradient circle with initials.
// `AgentAvatar` draws a project (name initials, colour from its id or the
// owner's pick); people go through `PersonAvatar` (picture first). The
// palette, derivation and initials rule live in Shared/AvatarPalette.swift
// so the notification extension draws the identical circle.

import ArbosFFI
import SwiftUI

/// The circle with initials. Size is the diameter; text ≈ 42 % of it.
struct InitialsAvatar: View {
    @Environment(\.theme) private var theme
    let initials: String
    let colorIndex: Int
    var size: CGFloat = 48

    init(name: String?, handle: String?, id: String, override: Int? = nil, size: CGFloat = 48) {
        self.initials = AvatarPalette.initials(name: name, handle: handle)
        self.colorIndex = AvatarPalette.clamp(override) ?? AvatarPalette.index(for: id)
        self.size = size
    }

    init(initials: String, colorIndex: Int, size: CGFloat = 48) {
        self.initials = initials
        self.colorIndex = colorIndex
        self.size = size
    }

    var body: some View {
        ZStack {
            Circle().fill(AvatarPalette.gradient(colorIndex, dark: theme.isDark))
            Text(initials)
                .font(.system(size: size * AvatarPalette.initialsScale, weight: .semibold))
                .foregroundStyle(.white)
                .lineLimit(1)
                .minimumScaleFactor(0.6)
                // Optical centre: cap-height text sits a hair high in a circle.
                .offset(y: size * 0.01)
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

extension AvatarPalette {
    static func gradient(_ index: Int, dark: Bool) -> LinearGradient {
        let s = stops(index, dark: dark)
        return LinearGradient(colors: [Color(uiColor: s.top), Color(uiColor: s.bottom)], startPoint: .top, endPoint: .bottom)
    }
}

/// An agent: its D92 leaf on its autumn disc while the relay flag is on (else
/// initials on its D79 colour), with the optional machine dot. `idle` sways the
/// leaf (profile heroes); a `working` start plays one gust; `still` keeps it
/// quiet. Reduce Motion stops both.
struct AgentAvatar: View {
    @ObservedObject private var flag = LeafAvatarsFlag.shared
    let project: Project
    let theme: Theme
    var size: CGFloat = Theme.avatar
    var dot: Color? = nil
    var working: Bool = false
    var still: Bool = false
    var idle: Bool = false
    @State private var gust = 0

    /// Jacob's rule (2026-10-26): every surface draws an agent through ONE resolver, keyed by
    /// agent id and avatar version — never the copy it happened to be handed. The caller's
    /// `project` names the agent (id, name, machine dot); what is drawn comes from the model's
    /// newest copy (`AppModel.newestAgentAvatar`): a stale record or roster copy on some screen
    /// (the mention table's raw `model.projects` entry, a roster row's `project`) can no longer
    /// show a different picture than the row next to it.
    private var shown: Project {
        guard let model = AppModel.current, let facts = model.newestAgentAvatar(project.id) else { return project }
        var p = project
        p.imageKey = facts.imageKey
        p.avatarLeaf = facts.avatarLeaf
        p.avatarLeafColor = facts.avatarLeafColor
        p.avatarVersion = facts.version
        p.imageURL = model.agentPictureURL(id: project.id, imageKey: facts.imageKey, version: facts.version)
        return p
    }

    var body: some View {
        let project = shown
        ZStack(alignment: .bottomTrailing) {
            if let url = project.imageURL, !url.isEmpty {
                // D92 amendment: the uploaded / generated picture wins over the leaf. While the
                // picture loads the disc is the neutral initial — never the leaf, which would
                // read as "the agent's avatar changed" for a moment (Jacob, 2026-10-26).
                AgentPicture(url: url, size: size) {
                    NeutralAvatar(name: project.name, size: size)
                }
            } else if LeafArt.isNeutral(project.avatarLeaf) {
                // Jacob, 2026-10-25: created with no leaf — the name's initial on a neutral disc.
                NeutralAvatar(name: project.name, size: size)
            } else if flag.on {
                LeafAvatar(pick: LeafArt.pick(project), size: size, idle: idle && !still, gust: gust)
                    .onChange(of: working) { _, now in
                        if now && !still { gust += 1 }
                    }
            } else {
                InitialsAvatar(name: project.name, handle: mentionSlug(project.name), id: project.id,
                               override: project.effectiveAvatarColorIndex, size: size)
            }
            if let dot {
                // Grok's status badge is 12 pt on a 48 pt row avatar; smaller avatars scale.
                let d = min(size * 0.36, 12)
                Circle()
                    .fill(dot)
                    .frame(width: d, height: d)
                    .overlay(Circle().strokeBorder(theme.bg, lineWidth: 2))
                    .offset(x: size * 0.08, y: size * 0.08)
            }
        }
        .frame(width: size, height: size)
    }
}

/// An agent's uploaded / generated picture, through `AvatarImageCache` (one download per
/// URL, URLCache behind it). While a *new* URL loads (the owner just changed the picture:
/// the version in the URL moved) the picture last shown stays up — the leaf placeholder is
/// drawn only when nothing has ever loaded. Jacob, 2026-10-25: the leaf flashing between the
/// old and the new picture read as "sometimes the old pfp, sometimes the new one".
struct AgentPicture<Placeholder: View>: View {
    let url: String
    let size: CGFloat
    @ViewBuilder let placeholder: () -> Placeholder
    @State private var image: UIImage?

    private var displayed: UIImage? {
        AvatarImageCache.shared.image(for: url) ?? image
    }

    var body: some View {
        ZStack {
            if let displayed {
                Image(uiImage: displayed).resizable().scaledToFill()
            } else {
                placeholder()
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .task(id: url) {
            if let cached = AvatarImageCache.shared.image(for: url) {
                image = cached
                return
            }
            if let loaded = await AvatarImageCache.shared.load(url) {
                image = loaded
            }
        }
    }
}

/// A people group's picture — the resolver's group case ([feedback group-row-avatar-ios-2026-10-27]).
/// Every surface that lists a group (home row, a person's Groups tab, search, mention chips,
/// Forward to) draws this and never builds its own URL or picks its own glyph. Keyed by chat id
/// + image key: the key names the upload, so a changed picture is a new URL (nothing stale can
/// win), and what is drawn is the model's copy of the chat — the one `setGroupImageKey`, the
/// relay re-reads and pushes land in — not the value a list happened to hold. The uploaded or
/// generated picture wins; else the neutral initial or leaf the group was created with; else
/// the placeholder: the stacked member heads when `members` are given, otherwise the two-person
/// glyph on a disc. The model is passed, not read from the environment: mention chips render
/// their caps through `ImageRenderer`, where no environment object exists.
struct GroupAvatar: View {
    @ObservedObject private var flag = LeafAvatarsFlag.shared
    @Environment(\.theme) private var theme
    let chat: PeopleChat
    let model: AppModel
    var size: CGFloat = Theme.avatar
    /// Members for the stacked-heads placeholder (Forward to); nil draws the glyph disc.
    var members: [ChatMember]? = nil

    /// The group's picture URL, or nil when it has none: the tokenised blob route under the
    /// `people:` slot, which `AvatarImageCache` sizes and caches by URL.
    static func pictureURL(_ chat: PeopleChat, model: AppModel) -> String? {
        guard chat.isGroup, let key = chat.imageKey, !key.isEmpty else { return nil }
        return model.blobURL(projectId: "\(peopleSlot):\(chat.id)", key: key)?.absoluteString
    }

    /// Picture URL, then neutral initial, then leaf, then placeholder — the order every
    /// surface agrees on.
    static func resolution(_ chat: PeopleChat, model: AppModel, leafArtOn: Bool) -> GroupAvatarResolution {
        if let url = pictureURL(chat, model: model) { return .picture(url) }
        if chat.isNeutralAvatar { return .neutral }
        if let leaf = chat.leafPick, leafArtOn { return .leaf(leaf) }
        return .placeholder
    }

    private var shown: PeopleChat { model.peopleChat(chat.id) ?? chat }

    var body: some View {
        let chat = shown
        switch GroupAvatar.resolution(chat, model: model, leafArtOn: flag.on) {
        case .picture(let url):
            // While the picture loads the disc is the neutral initial — never a stale copy.
            AgentPicture(url: url, size: size) { NeutralAvatar(name: chat.title, size: size) }
        case .neutral:
            NeutralAvatar(name: chat.title, size: size)
        case .leaf(let pick):
            LeafAvatar(pick: pick, size: size)
        case .placeholder:
            if let members, !members.isEmpty {
                AvatarCluster(members: members, size: size)
            } else {
                ZStack {
                    Circle().fill(theme.card)
                    Image(systemName: "person.2.fill")
                        .font(theme.font(size * 0.36, weight: .medium))
                        .foregroundStyle(theme.accent)
                }
                .frame(width: size, height: size)
                .accessibilityHidden(true)
            }
        }
    }
}

/// What `GroupAvatar` draws for a chat, in priority order.
enum GroupAvatarResolution {
    case picture(String)
    case neutral
    case leaf(LeafPick)
    case placeholder
}

/// Small coloured dot for machine / workspace state.
func machineDotColor(_ state: BoxState, theme: Theme) -> Color {
    switch state {
    case .awake: return theme.online
    case .waking: return theme.warn
    case .hibernatedLight, .hibernatedDeep, .stoppedByUser: return theme.muted.opacity(0.6)
    case .error: return theme.danger
    }
}
