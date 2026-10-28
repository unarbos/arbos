// Telegram-style avatars (D79): a coloured gradient circle with initials.
// `AgentAvatar` draws a project (name initials, colour from its id or the
// owner's pick); people go through `PersonAvatar` (picture first). The
// palette, derivation and initials rule live in Shared/AvatarPalette.swift
// so the notification extension draws the identical circle.

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
        Group {
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
        }
        .frame(width: size, height: size)
        .presenceBadge(dot, size: size)
    }
}

// MARK: - Presence badge

/// The one status badge on a picture (Jacob 2026-10-27, [feedback
/// presence-dot-layering-ios-2026-10-27]; `docs/mac-design-rules.md` § Avatars): the dot
/// sits **in front of** the picture, past its bottom-right corner, inside a ring that is
/// **cut out of the picture** — so whatever surface is behind (the page, a card, the
/// header pill's material, Liquid Glass) shows through as the ring, and no colour has to
/// be guessed per surface. Same numbers as the Mac's `ui::badge`: dot = size / 4 clamped
/// 6–12 (12 on the 48 pt home circle, 8 on the 32 pt pill picture), ring 2 pt, the ring
/// box 2 pt past the picture's edge. Every picture-with-dot goes through
/// [`View.presenceBadge(_:size:)`]; `PresenceBadgeTests` fails the gate on a corner
/// dot drawn by hand.
enum PresenceBadge {
    static let ringWidth: CGFloat = 2
    static let overhang: CGFloat = 2
    static let dotMin: CGFloat = 6
    static let dotMax: CGFloat = 12

    struct Metrics: Equatable {
        let dot: CGFloat
        let ring: CGFloat
        let overhang: CGFloat
    }

    static func metrics(_ size: CGFloat) -> Metrics {
        let dot = min(max((size / 4).rounded(), dotMin), dotMax)
        return Metrics(dot: dot, ring: dot + 2 * ringWidth, overhang: overhang)
    }

    /// Cuts the ring out of `content` (a `size` square) and lays the dot over it.
    struct Modifier: ViewModifier {
        let color: Color?
        let size: CGFloat

        @ViewBuilder
        func body(content: Content) -> some View {
            if let color {
                let m = PresenceBadge.metrics(size)
                content
                    .mask {
                        ZStack(alignment: .bottomTrailing) {
                            // Wider than the picture: a leaf's sway or a shadow must not be clipped.
                            Rectangle().padding(-size)
                            Circle()
                                .frame(width: m.ring, height: m.ring)
                                .offset(x: m.overhang, y: m.overhang)
                                .blendMode(.destinationOut)
                        }
                        .compositingGroup()
                    }
                    .overlay(alignment: .bottomTrailing) {
                        Circle()
                            .fill(color)
                            .frame(width: m.dot, height: m.dot)
                            .offset(x: m.overhang - PresenceBadge.ringWidth, y: m.overhang - PresenceBadge.ringWidth)
                            .accessibilityHidden(true)
                    }
            } else {
                content
            }
        }
    }
}

extension View {
    /// A status dot (machine state) on this `size`-pt picture: in front of it, inside a
    /// ring cut out of the picture. `nil` draws the bare picture in the same box.
    func presenceBadge(_ color: Color?, size: CGFloat) -> some View {
        modifier(PresenceBadge.Modifier(color: color, size: size))
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

/// Small coloured dot for machine / workspace state.
func machineDotColor(_ state: BoxState, theme: Theme) -> Color {
    switch state {
    case .awake: return theme.online
    case .waking: return theme.warn
    case .hibernatedLight, .hibernatedDeep, .stoppedByUser: return theme.muted.opacity(0.6)
    case .error: return theme.danger
    }
}
