import SwiftUI

/// Staged pictures and files, previewed inside the composer the way Grok does it
/// ([feedback attachment-in-composer-2026-11-09]; Jacob, 2026-11-09, shot 6 against our shot 7).
///
/// Ours used to put a capsule chip in a row *above* the composer: a 28 pt thumbnail, the
/// file's name as text, and a ✕ — and it changed the placeholder to "Add a description…" while
/// it was there. Grok keeps the attachment inside the box: a large rounded-square thumbnail of
/// the picture itself at the top-left, a white ✕ badge on its corner, the ordinary placeholder
/// underneath, and the button row still pinned to the bottom of a box that has grown into a
/// tall card. The name is not drawn at all — the picture is the label.
enum ComposerAttachment {
    /// The thumbnail's side. Grok's is a little over a third of the box's width on a 6.9"
    /// phone; 110 pt matches it and leaves the ✕ clear of the box's rounded corner.
    static let side: CGFloat = 110
    /// The ✕ badge, which overhangs the thumbnail's top-right corner rather than sitting inside
    /// it, so it never covers the part of the picture a reader is checking.
    static let badge: CGFloat = 28
    static let badgeGlyph: CGFloat = 13
    /// Between thumbnails when more than one is staged.
    static let gap: CGFloat = 8
    static let radius: CGFloat = 18

    /// The badge hangs half of itself outside the thumbnail on both axes, so the row needs that
    /// much room above it and beside the last one or the badge is clipped by the box.
    static var overhang: CGFloat { badge / 2 }
}

/// One staged attachment: the picture, or a glyph for something that has no picture.
struct ComposerAttachmentTile: View {
    @Environment(\.theme) private var theme
    let attachment: AppModel.PendingAttachment
    /// Tapping the picture opens it; only pictures and videos can be opened.
    let canOpen: Bool
    let onOpen: () -> Void
    let onRemove: () -> Void

    private var image: UIImage? {
        attachment.mime.hasPrefix("image/") ? UIImage(data: attachment.data) : nil
    }

    private var glyph: String {
        if attachment.mime.hasPrefix("image/") { return "photo" }
        if attachment.mime.hasPrefix("audio/") { return "waveform" }
        if attachment.mime.hasPrefix("video/") { return "film" }
        return "doc"
    }

    private var shape: RoundedRectangle {
        RoundedRectangle(cornerRadius: ComposerAttachment.radius, style: .continuous)
    }

    var body: some View {
        Button(action: onOpen) {
            ZStack {
                if let image {
                    Image(uiImage: image)
                        .resizable()
                        .scaledToFill()
                } else {
                    theme.bubbleAssistant
                    Image(systemName: glyph)
                        .font(theme.font(28))
                        .foregroundStyle(theme.icon)
                }
            }
            .frame(width: ComposerAttachment.side, height: ComposerAttachment.side)
            .clipShape(shape)
        }
        .buttonStyle(.plain)
        .disabled(!canOpen)
        .accessibilityLabel(canOpen ? "\(attachment.name), tap to open" : attachment.name)
        .accessibilityIdentifier("composer-attachment")
        .overlay(alignment: .topTrailing) {
            Button(action: onRemove) {
                ZStack {
                    // White in both appearances, like Grok's: the badge sits on the
                    // picture, not on the app's background, so it takes its contrast from
                    // the photo rather than from the theme.
                    Circle().fill(.white).shadow(color: .black.opacity(0.25), radius: 2, y: 1)
                    Image(systemName: "xmark")
                        .font(.system(size: ComposerAttachment.badgeGlyph, weight: .bold))
                        .foregroundStyle(.black)
                }
                .frame(width: ComposerAttachment.badge, height: ComposerAttachment.badge)
                .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Remove attachment")
            .accessibilityIdentifier("composer-remove-attachment")
            .offset(x: ComposerAttachment.overhang, y: -ComposerAttachment.overhang)
        }
        // Room for the half of the badge that hangs outside the picture.
        .padding(.top, ComposerAttachment.overhang)
        .padding(.trailing, ComposerAttachment.overhang)
    }
}

/// The staged row inside the composer. More than one sits side by side and scrolls, because
/// three 110 pt tiles are already wider than a phone.
struct ComposerAttachmentRow: View {
    let pending: [AppModel.PendingAttachment]
    let canOpen: (AppModel.PendingAttachment) -> Bool
    let onOpen: (AppModel.PendingAttachment) -> Void
    let onRemove: (UUID) -> Void

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(alignment: .top, spacing: ComposerAttachment.gap) {
                ForEach(pending) { a in
                    ComposerAttachmentTile(
                        attachment: a,
                        canOpen: canOpen(a),
                        onOpen: { onOpen(a) },
                        onRemove: { onRemove(a.id) }
                    )
                }
            }
        }
        // The row is as tall as a tile plus the badge's overhang; without this the scroll view
        // takes whatever height is going and the text below it drifts.
        .frame(height: ComposerAttachment.side + ComposerAttachment.overhang)
    }
}
