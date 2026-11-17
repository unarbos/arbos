import SwiftUI
import UniformTypeIdentifiers

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

    /// What Edit is offered on (tester report, 2026-11-17: "cut a vid or pic within the app
    /// before sending it"): a video (trim) or a still picture (crop). A GIF is left alone, a
    /// crop would keep one frame of it.
    static func canEdit(_ a: AppModel.PendingAttachment) -> Bool {
        let mime = a.mime.lowercased()
        return mime.hasPrefix("video/") || (mime.hasPrefix("image/") && mime != "image/gif")
    }

    /// The edited file takes the original's place in the row; an attachment removed while its
    /// editor was open is not brought back.
    static func replacing(_ id: UUID, with edited: AppModel.PendingAttachment,
                          in list: [AppModel.PendingAttachment]) -> [AppModel.PendingAttachment] {
        list.map { $0.id == id ? edited : $0 }
    }
}

/// One staged attachment: the picture, or a glyph for something that has no picture.
struct ComposerAttachmentTile: View {
    @Environment(\.theme) private var theme
    let attachment: AppModel.PendingAttachment
    /// Tapping the picture opens it; only pictures and videos can be opened.
    let canOpen: Bool
    let onOpen: () -> Void
    let onRemove: () -> Void
    /// Crop or trim it before it is sent; `nil` where there is nothing to edit.
    var onEdit: (() -> Void)? = nil

    /// A video's first frame and length (tester report, 2026-11-17).
    @State private var poster: VideoThumb.Frame?

    private var image: UIImage? {
        attachment.mime.hasPrefix("image/") ? UIImage(data: attachment.data) : poster?.image
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
            .overlay {
                if let poster {
                    Image(systemName: "play.fill")
                        .font(.system(size: 22, weight: .semibold))
                        .foregroundStyle(.white)
                        .shadow(color: .black.opacity(0.5), radius: 3)
                    Text(VideoThumb.durationLabel(poster.seconds))
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(.white)
                        .shadow(color: .black.opacity(0.7), radius: 2)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomTrailing)
                        .padding(8)
                }
            }
            .clipShape(shape)
        }
        .buttonStyle(.plain)
        .disabled(!canOpen)
        .accessibilityLabel(canOpen ? "\(attachment.name), tap to open" : attachment.name)
        .accessibilityIdentifier("composer-attachment")
        .task(id: attachment.id) {
            guard attachment.mime.lowercased().hasPrefix("video/") else { return }
            let side = ComposerAttachment.side * 3
            poster = await VideoThumb.frame(from: attachment.data, name: attachment.name,
                                            maxPixels: CGSize(width: side, height: side))
        }
        .overlay(alignment: .bottomLeading) {
            if let onEdit {
                Button(action: onEdit) {
                    ZStack {
                        Circle().fill(.white).shadow(color: .black.opacity(0.25), radius: 2, y: 1)
                        Image(systemName: attachment.mime.lowercased().hasPrefix("video/") ? "scissors" : "crop")
                            .font(.system(size: ComposerAttachment.badgeGlyph, weight: .bold))
                            .foregroundStyle(.black)
                    }
                    .frame(width: ComposerAttachment.badge, height: ComposerAttachment.badge)
                    .contentShape(Circle())
                }
                .buttonStyle(.plain)
                .padding(6)
                .accessibilityLabel("Edit attachment")
                .accessibilityIdentifier("composer-edit-attachment")
            }
        }
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
    let onEdit: (AppModel.PendingAttachment) -> Void

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(alignment: .top, spacing: ComposerAttachment.gap) {
                ForEach(pending) { a in
                    ComposerAttachmentTile(
                        attachment: a,
                        canOpen: canOpen(a),
                        onOpen: { onOpen(a) },
                        onRemove: { onRemove(a.id) },
                        onEdit: ComposerAttachment.canEdit(a) ? { onEdit(a) } : nil
                    )
                }
            }
        }
        // The row is as tall as a tile plus the badge's overhang; without this the scroll view
        // takes whatever height is going and the text below it drifts.
        .frame(height: ComposerAttachment.side + ComposerAttachment.overhang)
    }
}

/// Edit a staged attachment before it is sent (tester report, 2026-11-17): the system trimmer
/// for a video, a crop screen for a picture. `onDone(nil)` is Cancel, and anything that cannot
/// be opened: the original stays as it was.
struct AttachmentEditor: View {
    let attachment: AppModel.PendingAttachment
    let onDone: (AppModel.PendingAttachment?) -> Void

    var body: some View {
        if attachment.mime.lowercased().hasPrefix("video/") {
            VideoTrimmer(attachment: attachment, onDone: onDone).ignoresSafeArea()
        } else if let image = UIImage(data: attachment.data) {
            PhotoCropper(image: image, onCancel: { onDone(nil) }) { unit in
                // PNG stays PNG (a screenshot, transparency); anything else goes out as JPEG.
                let png = attachment.mime.lowercased() == "image/png"
                let cropped = PhotoCrop.cropped(image, to: unit)
                guard let data = png ? cropped.pngData() : cropped.jpegData(compressionQuality: 0.9) else { return onDone(nil) }
                let stem = (attachment.name as NSString).deletingPathExtension
                onDone(.init(name: "\(stem).\(png ? "png" : "jpg")", mime: png ? "image/png" : "image/jpeg", data: data))
            }
        } else {
            Color.black.ignoresSafeArea().onAppear { onDone(nil) }
        }
    }
}

/// `UIVideoEditorController`: the system's start / end trim. It re-encodes to a QuickTime file.
/// The editor is presented from a plain controller: SwiftUI cannot host it directly (it has
/// no view of its own to embed, and the app aborts).
private struct VideoTrimmer: UIViewControllerRepresentable {
    let attachment: AppModel.PendingAttachment
    let onDone: (AppModel.PendingAttachment?) -> Void

    func makeUIViewController(context: Context) -> Host { Host(attachment, onDone) }
    func updateUIViewController(_ host: Host, context: Context) {}

    final class Host: UIViewController, UIVideoEditorControllerDelegate, UINavigationControllerDelegate {
        private let attachment: AppModel.PendingAttachment
        private let onDone: (AppModel.PendingAttachment?) -> Void
        private var shown = false
        /// The system calls the save delegate twice on some releases; only the first counts.
        private var finished = false

        init(_ attachment: AppModel.PendingAttachment, _ onDone: @escaping (AppModel.PendingAttachment?) -> Void) {
            self.attachment = attachment
            self.onDone = onDone
            super.init(nibName: nil, bundle: nil)
            view.backgroundColor = .black
        }

        required init?(coder: NSCoder) { nil }

        override func viewDidAppear(_ animated: Bool) {
            super.viewDidAppear(animated)
            guard !shown else { return }
            shown = true
            guard let file = MediaFetch.tempFile(attachment.data, name: attachment.name),
                  UIVideoEditorController.canEditVideo(atPath: file.path) else { return finish(nil) }
            let editor = UIVideoEditorController()
            editor.delegate = self
            editor.videoQuality = .typeHigh
            editor.videoPath = file.path
            editor.modalPresentationStyle = .fullScreen
            present(editor, animated: false)
        }

        private func finish(_ edited: AppModel.PendingAttachment?) {
            guard !finished else { return }
            finished = true
            dismiss(animated: false) { [onDone] in onDone(edited) }
        }

        func videoEditorController(_ editor: UIVideoEditorController, didSaveEditedVideoToPath path: String) {
            let file = URL(fileURLWithPath: path)
            defer { try? FileManager.default.removeItem(at: file) }
            guard let data = try? Data(contentsOf: file), !data.isEmpty else { return finish(nil) }
            let ext = file.pathExtension.isEmpty ? "mov" : file.pathExtension.lowercased()
            let stem = (attachment.name as NSString).deletingPathExtension
            finish(.init(name: "\(stem).\(ext)",
                         mime: UTType(filenameExtension: ext)?.preferredMIMEType ?? "video/quicktime", data: data))
        }

        func videoEditorController(_ editor: UIVideoEditorController, didFailWithError error: Error) { finish(nil) }
        func videoEditorControllerDidCancel(_ editor: UIVideoEditorController) { finish(nil) }
    }
}

/// The crop rectangle's arithmetic, in the picture's unit square (0…1 on both axes) so it
/// does not depend on how large the picture is drawn.
enum PhotoCrop {
    /// The smallest crop, as a share of the picture's side.
    static let minSide: CGFloat = 0.1

    /// Drag one corner by `delta`; the opposite corner stays put and the rectangle stays inside
    /// the picture. `aspect` is the picture's width over its height, for a square in pixels.
    static func dragging(_ r: CGRect, right: Bool, bottom: Bool, by delta: CGSize, square: Bool, aspect: CGFloat) -> CGRect {
        let fx = right ? r.minX : r.maxX, fy = bottom ? r.minY : r.maxY
        let mx = (right ? r.maxX : r.minX) + delta.width, my = (bottom ? r.maxY : r.minY) + delta.height
        var w = right ? min(max(mx, fx + minSide), 1) - fx : fx - max(min(mx, fx - minSide), 0)
        var h = bottom ? min(max(my, fy + minSide), 1) - fy : fy - max(min(my, fy - minSide), 0)
        if square {
            let side = min(w * aspect, h)
            (w, h) = (side / aspect, side)
        }
        return CGRect(x: right ? fx : fx - w, y: bottom ? fy : fy - h, width: w, height: h)
    }

    /// Slide the whole rectangle, stopping at the picture's edges.
    static func moving(_ r: CGRect, by delta: CGSize) -> CGRect {
        CGRect(x: min(max(r.minX + delta.width, 0), 1 - r.width), y: min(max(r.minY + delta.height, 0), 1 - r.height),
               width: r.width, height: r.height)
    }

    /// The largest square (in pixels) inside `r`, on the same centre.
    static func squared(_ r: CGRect, aspect: CGFloat) -> CGRect {
        let side = min(r.width * aspect, r.height)
        return CGRect(x: r.midX - side / aspect / 2, y: r.midY - side / 2, width: side / aspect, height: side)
    }

    /// Drawn rather than cut from the `CGImage`, so the picture's orientation is applied.
    static func cropped(_ image: UIImage, to unit: CGRect) -> UIImage {
        let size = CGSize(width: max(1, (image.size.width * unit.width).rounded()),
                          height: max(1, (image.size.height * unit.height).rounded()))
        let format = UIGraphicsImageRendererFormat.default()
        format.scale = image.scale
        return UIGraphicsImageRenderer(size: size, format: format).image { _ in
            image.draw(at: CGPoint(x: -image.size.width * unit.minX, y: -image.size.height * unit.minY))
        }
    }
}

/// Crop a picture: drag a corner to resize, drag inside to move, Free or Square.
/// Deliberately small: no zoom, no rotation.
private struct PhotoCropper: View {
    let image: UIImage
    let onCancel: () -> Void
    let onDone: (CGRect) -> Void

    @State private var crop = CGRect(x: 0, y: 0, width: 1, height: 1)
    @State private var square = false
    /// The rectangle when the current drag began.
    @State private var start: CGRect?

    private var aspect: CGFloat { image.size.width / max(image.size.height, 1) }

    var body: some View {
        VStack(spacing: 12) {
            HStack {
                Button("Cancel", action: onCancel)
                Spacer()
                Picker("Shape", selection: $square) {
                    Text("Free").tag(false)
                    Text("Square").tag(true)
                }
                .pickerStyle(.segmented)
                .frame(width: 160)
                Spacer()
                Button("Done") { onDone(crop) }.bold()
                    .accessibilityIdentifier("crop-done")
            }
            .padding(.horizontal, 16)
            GeometryReader { g in
                // The picture, fitted; everything inside is laid out in its points.
                let k = min(g.size.width / image.size.width, g.size.height / image.size.height)
                let fit = CGSize(width: image.size.width * k, height: image.size.height * k)
                let box = CGRect(x: crop.minX * fit.width, y: crop.minY * fit.height,
                                 width: crop.width * fit.width, height: crop.height * fit.height)
                ZStack(alignment: .topLeading) {
                    Image(uiImage: image).resizable().frame(width: fit.width, height: fit.height)
                    Path { p in
                        p.addRect(CGRect(origin: .zero, size: fit))
                        p.addRect(box)
                    }
                    .fill(Color.black.opacity(0.6), style: FillStyle(eoFill: true))
                    .allowsHitTesting(false)
                    Rectangle().strokeBorder(.white, lineWidth: 1)
                        .contentShape(Rectangle())
                        .frame(width: box.width, height: box.height)
                        .offset(x: box.minX, y: box.minY)
                        .gesture(drag(fit) { PhotoCrop.moving($0, by: $1) })
                    ForEach(0..<4, id: \.self) { i in
                        let (right, bottom) = (i % 2 == 1, i >= 2)
                        Circle().fill(.white)
                            .frame(width: 22, height: 22)
                            .padding(11)
                            .contentShape(Rectangle())
                            .offset(x: (right ? box.maxX : box.minX) - 22, y: (bottom ? box.maxY : box.minY) - 22)
                            .gesture(drag(fit) {
                                PhotoCrop.dragging($0, right: right, bottom: bottom, by: $1, square: square, aspect: aspect)
                            })
                            .accessibilityIdentifier("crop-handle-\(i)")
                    }
                }
                .frame(width: fit.width, height: fit.height)
                .position(x: g.size.width / 2, y: g.size.height / 2)
            }
            .padding(24)
        }
        .padding(.top, 8)
        .background(Color.black.ignoresSafeArea())
        .tint(.white)
        .preferredColorScheme(.dark)
        .onChange(of: square) { _, on in
            if on { crop = PhotoCrop.squared(crop, aspect: aspect) }
        }
    }

    /// One drag, measured on the screen and applied to the rectangle the drag started from.
    private func drag(_ fit: CGSize, _ apply: @escaping (CGRect, CGSize) -> CGRect) -> some Gesture {
        DragGesture(minimumDistance: 0, coordinateSpace: .global)
            .onChanged { v in
                let from = start ?? crop
                start = from
                crop = apply(from, CGSize(width: v.translation.width / fit.width, height: v.translation.height / fit.height))
            }
            .onEnded { _ in start = nil }
    }
}
