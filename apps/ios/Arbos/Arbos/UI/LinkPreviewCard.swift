import SwiftUI

/// Link previews ([feedback link-embeds-ios-2026-10-27], #2066): the card a `link_preview`
/// part draws inside its bubble — the code card's chrome (`bg` at 60 % over the bubble, the
/// hairline, radius 12, 10 pt padding, the bubble's inner width). Two densities, decided by the
/// relay: **compact** = 56 pt art on the left, title · subtitle · footer on the right;
/// **expanded** = the picture on top at 16:9 (▶ and the duration on a video), the text under
/// it. Every card opens out: a tap is the link's tap (`openURL`, where `LinkConfirm` slots in).
/// Remove preview lives in the long-press menu (`MessageMenu`), for everyone. The footer's
/// small square is the shared link-icon tile (`LinkTile`): the host's bundled brand mark, else
/// the relay's `siteIcon` picture on the part, else the relay's host icon, else the host's
/// initial — the card itself fetches nothing from the site.
struct LinkPreviewCard: View {
    @Environment(\.theme) private var theme
    @Environment(\.openURL) private var openURL
    @EnvironmentObject private var model: AppModel
    /// Host icons for the tile: the card redraws when one arrives.
    @ObservedObject private var linkIcons = LinkIcons.shared
    let preview: LinkPreview
    let chatId: Id
    let messageId: Id
    /// My own (grey) bubble: white ink, lighter muted tones.
    let own: Bool
    @State private var picture: UIImage?
    /// The relay's `siteIcon` for the part, from the chat's blob slot.
    @State private var siteIcon: UIImage?

    static let radius: CGFloat = 12
    static let pad: CGFloat = 10
    static let art: CGFloat = 56
    static let artRadius: CGFloat = 6
    static let playDisc: CGFloat = 48
    static let tile: CGFloat = 14
    /// A card-only bubble keeps this even inset around the card instead of the text pads.
    static let bubbleInset: CGFloat = 6

    private var ink: Color { own ? theme.userText : theme.text }
    private var muted: Color { own ? theme.userText.opacity(0.66) : theme.muted }
    private var faint: Color { own ? theme.userText.opacity(0.5) : theme.placeholder }

    var body: some View {
        // Not a `Button` (`rowTap`): the reply swipe must start on the card too
        // ([feedback swipe-reply-own-quoted-2026-10-28]).
        VStack(alignment: .leading, spacing: 0) {
            if preview.expanded { media }
            HStack(alignment: .top, spacing: 10) {
                if !preview.expanded, !preview.isPost, let image = preview.image {
                    artView(image)
                }
                text
            }
            .padding(Self.pad)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: Self.radius, style: .continuous).fill(theme.bg.opacity(0.6)))
        .overlay(RoundedRectangle(cornerRadius: Self.radius, style: .continuous).stroke(theme.separator, lineWidth: 0.5))
        .clipShape(RoundedRectangle(cornerRadius: Self.radius, style: .continuous))
        .rowTap {
            guard let url = preview.openURL else { return }
            openURL(url)
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("link-card")
        .accessibilityLabel(preview.oneLine)
        .accessibilityHint("Opens the link")
        .task(id: preview.image?.ref) { await load() }
    }

    // MARK: pieces

    private var media: some View {
        ZStack {
            Rectangle().fill(theme.muted.opacity(0.14))
            if let picture {
                Image(uiImage: picture).resizable().scaledToFill()
            }
            if preview.type == "video" {
                Circle()
                    .fill(Color.black.opacity(0.62))
                    .overlay(Circle().stroke(Color.white.opacity(0.18), lineWidth: 1))
                    .frame(width: Self.playDisc, height: Self.playDisc)
                    .overlay(
                        Image(systemName: "play.fill")
                            .font(.system(size: 20, weight: .semibold))
                            .foregroundStyle(.white)
                            .offset(x: 2)
                    )
                    .accessibilityIdentifier("link-play")
            }
        }
        .aspectRatio(16 / 9, contentMode: .fit)
        .clipped()
        .overlay(alignment: .bottomTrailing) {
            if let d = preview.duration {
                Text(d)
                    .font(theme.font(12, weight: .medium))
                    .monospacedDigit()
                    .foregroundStyle(.white)
                    .padding(.horizontal, 5).padding(.vertical, 2)
                    .background(RoundedRectangle(cornerRadius: 4, style: .continuous).fill(Color.black.opacity(0.72)))
                    .padding(8)
                    .accessibilityIdentifier("link-duration")
            }
        }
        .accessibilityIdentifier("link-media")
    }

    private func artView(_ image: LinkPreview.Picture) -> some View {
        ZStack {
            RoundedRectangle(cornerRadius: Self.artRadius, style: .continuous).fill(theme.muted.opacity(0.14))
            if let picture {
                Image(uiImage: picture).resizable().scaledToFill()
            }
        }
        .frame(width: Self.art, height: Self.art)
        .clipShape(RoundedRectangle(cornerRadius: Self.artRadius, style: .continuous))
        .accessibilityIdentifier("link-art")
    }

    /// The host the card stands for: `siteHost` from the relay, else the URL's host.
    private var host: String {
        if let h = preview.siteHost, !h.isEmpty { return h }
        let h = preview.openURL?.host ?? ""
        return h.hasPrefix("www.") ? String(h.dropFirst(4)) : h
    }

    /// The footer's tile: the shared link tile — brand mark, else the part's `siteIcon`
    /// picture, else the relay's host icon, else the host's letter.
    private var tileSlot: some View {
        let picture: (image: UIImage, tone: LinkIconTone)? = {
            if LinkBrand.of(host: host) != nil { return nil }
            if let siteIcon { return (image: siteIcon, tone: LinkIconTone.of(siteIcon)) }
            return linkIcons.picture(for: host)
        }()
        return LinkTile(url: preview.openURL, host: host, picture: picture, size: Self.tile)
            .accessibilityIdentifier("link-tile")
    }

    @ViewBuilder
    private var text: some View {
        VStack(alignment: .leading, spacing: 2) {
            if preview.isPost {
                HStack(spacing: 9) {
                    Circle()
                        .fill(theme.accent.opacity(0.35))
                        .frame(width: 32, height: 32)
                        .overlay(Text(initials).font(theme.font(12, weight: .semibold)).foregroundStyle(.white))
                    VStack(alignment: .leading, spacing: 0) {
                        Text(preview.title).font(theme.font(Theme.small, weight: .semibold)).foregroundStyle(ink).lineLimit(1)
                        if let s = preview.subtitle {
                            Text(s).font(theme.font(Theme.tiny)).foregroundStyle(muted).lineLimit(1)
                        }
                    }
                }
                if let d = preview.description {
                    Text(d).font(theme.font(Theme.small)).foregroundStyle(ink).lineLimit(8).padding(.top, 6)
                }
            } else if preview.isGitHubItem {
                if let s = preview.subtitle {
                    HStack(spacing: 6) {
                        tileSlot
                        Text(s).font(theme.font(Theme.tiny)).foregroundStyle(muted).lineLimit(1)
                    }
                    .padding(.bottom, 2)
                }
                Text(preview.title).font(theme.font(Theme.small, weight: .semibold)).foregroundStyle(ink).lineLimit(2)
                HStack(spacing: 6) {
                    if let s = preview.state { statePill(s) }
                    if let d = preview.description {
                        Text(d).font(theme.font(Theme.tiny)).foregroundStyle(muted).lineLimit(1)
                    }
                }
                .padding(.top, 4)
            } else {
                Text(preview.title)
                    .font(theme.font(Theme.small, weight: .semibold))
                    .foregroundStyle(ink)
                    .lineLimit(preview.expanded ? 2 : 1)
                if let s = preview.subtitle {
                    Text(s).font(theme.font(Theme.tiny)).foregroundStyle(muted).lineLimit(1)
                }
                if preview.expanded || preview.subtitle == nil, let d = preview.description {
                    Text(d).font(theme.font(Theme.tiny)).foregroundStyle(muted).lineLimit(2).padding(.top, 2)
                }
            }
            footer.padding(.top, 4)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityIdentifier("link-text")
    }

    private var initials: String {
        preview.title.split(separator: " ").prefix(2).compactMap { $0.first }.map { String($0).uppercased() }.joined()
    }

    private var footer: some View {
        var parts: [String] = []
        if !preview.providerName.isEmpty { parts.append(preview.providerName) }
        parts.append(contentsOf: preview.facts)
        return HStack(spacing: 5) {
            tileSlot
            if let diff = preview.diff {
                Text("+\(diff.additions)").foregroundStyle(theme.online).monospacedDigit()
                // error-ui: exception diff-deletions-count
                Text("−\(diff.deletions)").foregroundStyle(theme.danger).monospacedDigit()
                if !parts.isEmpty { Text("·").opacity(0.7) }
            }
            Text(parts.joined(separator: " · ")).lineLimit(1).truncationMode(.tail)
        }
        .font(theme.font(12))
        .foregroundStyle(faint)
        .accessibilityIdentifier("link-footer")
    }

    private func statePill(_ state: String) -> some View {
        let (label, color): (String, Color) = {
            switch state {
            case "open": return ("Open", theme.online)
            case "merged": return ("Merged", Color(red: 0.54, green: 0.34, blue: 0.9))
            case "draft": return ("Draft", theme.muted)
            default: return ("Closed", theme.danger)
            }
        }()
        return Text(label)
            .font(theme.font(12, weight: .semibold))
            .foregroundStyle(color)
            .padding(.horizontal, 7).padding(.vertical, 2)
            .background(Capsule().fill(color.opacity(0.18)))
            .accessibilityIdentifier("link-state")
    }

    // MARK: picture

    /// The preview picture is a blob in the chat's slot, loaded with the device bearer like
    /// an attachment (`MediaFetch`); nothing is fetched from the site itself.
    private func load() async {
        if let icon = preview.siteIcon, !icon.ref.isEmpty, !preview.slot.isEmpty, LinkBrand.of(host: host) == nil {
            let item = MediaItem(id: "\(messageId)#li", name: icon.ref, mime: icon.mime, kind: .image,
                                 blobRef: icon.ref, slot: preview.slot, by: "", at: 0)
            if let (data, _) = await MediaFetch.bytes(for: item, model: model), let img = UIImage(data: data) {
                siteIcon = img
            }
        }
        guard let image = preview.image, !image.ref.isEmpty, !preview.slot.isEmpty else { return }
        let item = MediaItem(id: "\(messageId)#lp", name: image.ref, mime: image.mime, kind: .image,
                             blobRef: image.ref, slot: preview.slot, by: "", at: 0)
        if let (data, _) = await MediaFetch.bytes(for: item, model: model), let img = UIImage(data: data) {
            picture = img
        }
    }
}
