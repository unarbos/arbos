import ImageIO
import PencilKit
import SwiftUI
import UIKit

// Crop and draw on a staged picture before it is sent (Scout's request in Arbonites;
// mockups approved by Jacob 2026-10-31 14:15 CR). Tapping a pending attachment in the
// composer opens this over the chat; Done replaces the bytes the composer will send.
//
// Apple's own drawing engine, in our own container: the Markup screen inside
// `QLPreviewController` brings back the system navigation bar that `SheetHeaderBar` exists
// to replace, has no crop of its own, and wants a file URL the composer has already had to
// work around once (`openPendingPreview`'s nil-then-reset). PencilKit is that screen's
// engine without its chrome.

// MARK: - The edit

/// A still picture's reversible edit, carried by the pending attachment rather than by the
/// sheet: Cancel is then a dismissal with nothing written, and needs no confirmation card.
///
/// The crop and the strokes are independent until Done. Strokes live in the picture's own
/// pixel space, so narrowing the crop leaves the ones outside it in place — kept, and
/// merely not rendered — and widening it brings them back.
///
/// The original is here only while the attachment is staged. A send clears the whole
/// pending list, so nothing recoverable travels with the message.
struct PhotoEdit: Equatable {
    /// The bytes as they were picked.
    var original: Data
    /// The kept region as a unit rect of the original, origin top-left.
    var crop: CGRect = PhotoEdit.whole
    /// `PKDrawing.dataRepresentation()` in the original's pixel space; empty = no strokes.
    var strokes: Data = Data()

    static let whole = CGRect(x: 0, y: 0, width: 1, height: 1)

    /// Nothing has been done to it: Done hands back the original bytes and drops the edit.
    var isWhole: Bool { crop == PhotoEdit.whole && strokes.isEmpty }

    var drawing: PKDrawing {
        guard !strokes.isEmpty else { return PKDrawing() }
        return (try? PKDrawing(data: strokes)) ?? PKDrawing()
    }

    /// An empty drawing is stored as no bytes at all, so `isWhole` can tell "never drew"
    /// from "drew and erased" without decoding anything.
    static func strokeData(_ drawing: PKDrawing) -> Data {
        drawing.strokes.isEmpty ? Data() : drawing.dataRepresentation()
    }
}

enum PhotoEditorMode: String, CaseIterable, Identifiable {
    case crop, draw
    var id: String { rawValue }

    var label: String {
        switch self {
        case .crop: return "Crop"
        case .draw: return "Draw"
        }
    }
}

/// A grab point on the crop rectangle: the four corners move two sides, the four edge bars
/// move one.
enum PhotoCropHandle: String, CaseIterable, Identifiable {
    case topLeading, top, topTrailing, leading, trailing, bottomLeading, bottom, bottomTrailing
    var id: String { rawValue }

    var movesLeading: Bool { self == .topLeading || self == .leading || self == .bottomLeading }
    var movesTrailing: Bool { self == .topTrailing || self == .trailing || self == .bottomTrailing }
    var movesTop: Bool { self == .topLeading || self == .top || self == .topTrailing }
    var movesBottom: Bool { self == .bottomLeading || self == .bottom || self == .bottomTrailing }
    var isCorner: Bool { (movesLeading || movesTrailing) && (movesTop || movesBottom) }

    var label: String {
        let down = movesTop ? "top" : (movesBottom ? "bottom" : "")
        let across = movesLeading ? "left" : (movesTrailing ? "right" : "")
        return "Crop \([down, across].filter { !$0.isEmpty }.joined(separator: " ")) \(isCorner ? "corner" : "edge")"
    }

    func point(of rect: CGRect) -> CGPoint {
        CGPoint(x: movesLeading ? rect.minX : (movesTrailing ? rect.maxX : rect.midX),
                y: movesTop ? rect.minY : (movesBottom ? rect.maxY : rect.midY))
    }
}

enum PhotoEditGeometry {
    /// The smallest a crop may get, as a fraction of each side.
    static let minimumSide: CGFloat = 0.12

    /// `content` aspect-fitted in `bounds`, centred.
    static func fit(content: CGSize, in bounds: CGSize) -> CGRect {
        guard content.width > 0, content.height > 0, bounds.width > 0, bounds.height > 0 else { return .zero }
        let scale = min(bounds.width / content.width, bounds.height / content.height)
        let size = CGSize(width: content.width * scale, height: content.height * scale)
        return CGRect(x: (bounds.width - size.width) / 2, y: (bounds.height - size.height) / 2,
                      width: size.width, height: size.height)
    }

    /// The whole picture's frame, in viewport space, that puts `crop` — a unit rect of the
    /// picture — aspect-fitted and centred in `bounds`.
    ///
    /// Crop mode passes `PhotoEdit.whole` and gets the plain fit. Draw mode passes the crop,
    /// so the kept region fills the viewport and the strokes outside it keep their place off
    /// screen instead of being rescaled into view.
    static func pictureFrame(picture: CGSize, crop: CGRect, in bounds: CGSize) -> CGRect {
        guard picture.width > 0, picture.height > 0, crop.width > 0, crop.height > 0 else { return .zero }
        let cropped = CGSize(width: picture.width * crop.width, height: picture.height * crop.height)
        let fitted = fit(content: cropped, in: bounds)
        guard fitted.width > 0 else { return .zero }
        let scale = fitted.width / cropped.width
        return CGRect(x: fitted.minX - crop.minX * picture.width * scale,
                      y: fitted.minY - crop.minY * picture.height * scale,
                      width: picture.width * scale, height: picture.height * scale)
    }

    /// `crop` in viewport space, given the whole picture's frame there.
    static func rect(of crop: CGRect, in picture: CGRect) -> CGRect {
        CGRect(x: picture.minX + crop.minX * picture.width, y: picture.minY + crop.minY * picture.height,
               width: crop.width * picture.width, height: crop.height * picture.height)
    }

    /// `handle` dragged by `delta` (a fraction of the picture's sides), clamped inside the
    /// picture and never under `minimum` on either side. A handle never moves both sides of
    /// an axis, so the clamps cannot fight each other.
    static func resized(_ crop: CGRect, handle: PhotoCropHandle, by delta: CGSize,
                        minimum: CGFloat = minimumSide) -> CGRect {
        var left = crop.minX, right = crop.maxX, top = crop.minY, bottom = crop.maxY
        if handle.movesLeading { left = min(max(0, left + delta.width), right - minimum) }
        if handle.movesTrailing { right = max(min(1, right + delta.width), left + minimum) }
        if handle.movesTop { top = min(max(0, top + delta.height), bottom - minimum) }
        if handle.movesBottom { bottom = max(min(1, bottom + delta.height), top + minimum) }
        return CGRect(x: left, y: top, width: right - left, height: bottom - top)
    }
}

enum PhotoEditorEligibility {
    /// Crop and draw are for stills. A video or an animated picture keeps the Quick Look
    /// preview it has today, and the test is the **frame count**, not the mime type: an
    /// animated GIF and a still one are both `image/gif`, and the same count catches an
    /// animated WebP or HEIC sequence without naming either.
    ///
    /// A count of exactly one is also the readability test, which is why this runs no
    /// decode: it is asked once per composer render, and `UIImage(data:)` there would
    /// decode every staged picture on every keystroke.
    static func canEdit(mime: String, frames: Int) -> Bool {
        mime.lowercased().hasPrefix("image/") && frames == 1
    }

    /// The mime prefix is checked first so a staged PDF or video never reaches ImageIO: this
    /// is asked once per chip per composer render, and most chips are not pictures at all.
    static func canEdit(mime: String, data: Data) -> Bool {
        guard mime.lowercased().hasPrefix("image/") else { return false }
        return canEdit(mime: mime, frames: frameCount(of: data))
    }

    /// Frames in the encoded bytes; 0 when nothing can read them.
    static func frameCount(of data: Data) -> Int {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return 0 }
        return CGImageSourceGetCount(source)
    }
}

enum PhotoEditRender {
    /// The picture as a scale-1, `.up` bitmap. An EXIF-rotated photo would otherwise be
    /// cropped and inked against a different frame than the one on screen.
    static func normalised(_ image: UIImage) -> UIImage {
        guard image.imageOrientation != .up || image.scale != 1 else { return image }
        let size = CGSize(width: (image.size.width * image.scale).rounded(),
                          height: (image.size.height * image.scale).rounded())
        let format = UIGraphicsImageRendererFormat.default()
        format.scale = 1
        format.opaque = false
        return UIGraphicsImageRenderer(size: size, format: format).image { _ in
            image.draw(in: CGRect(origin: .zero, size: size))
        }
    }

    /// `crop` in the picture's own pixels, whole-numbered so the output's size is the one
    /// the rectangle framed.
    static func pixelCrop(_ crop: CGRect, picture: CGSize) -> CGRect {
        CGRect(x: (crop.minX * picture.width).rounded(), y: (crop.minY * picture.height).rounded(),
               width: max(1, (crop.width * picture.width).rounded()),
               height: max(1, (crop.height * picture.height).rounded()))
    }

    /// The crop region with the strokes inside it on top. Strokes outside the crop are not
    /// drawn — they stay in `PhotoEdit.strokes` and come back if the crop widens again.
    static func flattened(_ picture: UIImage, crop: CGRect, drawing: PKDrawing) -> UIImage {
        let base = normalised(picture)
        let box = pixelCrop(crop, picture: base.size)
        let format = UIGraphicsImageRendererFormat.default()
        format.scale = 1
        format.opaque = false
        return UIGraphicsImageRenderer(size: box.size, format: format).image { _ in
            base.draw(in: CGRect(origin: CGPoint(x: -box.minX, y: -box.minY), size: base.size))
            guard !drawing.strokes.isEmpty else { return }
            drawing.image(from: box, scale: 1).draw(in: CGRect(origin: .zero, size: box.size))
        }
    }

    /// PNG stays PNG; anything else leaves as JPEG, with the name's extension following the
    /// bytes, because the relay's blob key is built from the name.
    static func encoded(_ image: UIImage, name: String, mime: String) -> (data: Data, mime: String, name: String)? {
        if mime.lowercased() == "image/png", let png = image.pngData() {
            return (png, "image/png", name)
        }
        guard let jpeg = image.jpegData(compressionQuality: 0.9) else { return nil }
        return (jpeg, "image/jpeg", renamed(name, ext: "jpg"))
    }

    static func renamed(_ name: String, ext: String) -> String {
        let base = (name as NSString).deletingPathExtension
        return base.isEmpty ? "photo.\(ext)" : "\(base).\(ext)"
    }
}

/// What the cover presents, and what comes back from it.
struct PhotoEditorTarget: Identifiable, Equatable {
    /// The pending attachment this edits.
    let id: UUID
    var name: String
    var mime: String
    var edit: PhotoEdit
}

struct PhotoEditResult: Equatable {
    let id: UUID
    var name: String
    var mime: String
    var data: Data
    /// `nil` once the edit is back to the original: the attachment carries no edit at all
    /// rather than an identity one.
    var edit: PhotoEdit?
}

/// The editor's floating controls take the media viewer's plate (`ViewerChrome`), which is
/// the surface-aware recipe. `AvatarSheetPalette` and its family are fixed dark and would
/// draw Glass and Matte identically.
enum PhotoEditorChrome {
    static let bar: CGFloat = 36
    static let segmentPad: CGFloat = 3
    static let inset: CGFloat = Controls.inset
    static let top: CGFloat = 10
    /// Dim over the cut-away region; the thirds grid and the edge over the kept one.
    static let dim: Double = 0.55
    static let grid: Double = 0.22
    static let edge: Double = 0.55
    static let handle: CGFloat = 22
    static let handleBar: CGFloat = 3
    /// A handle's hit area, which is larger than the bracket drawn in it.
    static let handleHit: CGFloat = 44
    /// How far back Undo reaches. PencilKit's picker carries no undo at compact width, so
    /// the sheet keeps its own history of whole drawings — which an erase can undo too.
    static let undoDepth = 24
}

// MARK: - The sheet

struct PhotoEditorSheet: View {
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let target: PhotoEditorTarget
    let onDone: (PhotoEditResult) -> Void

    static let title = "Edit photo"

    @State private var mode: PhotoEditorMode = .crop
    @State private var crop: CGRect
    @State private var drawing = PKDrawing()
    @State private var history: [PKDrawing] = []
    @State private var picture: UIImage?
    /// Bumped whenever the sheet itself writes `drawing` (load, undo, Original), which is
    /// what tells the canvas to take it; a stroke the user made never bumps it, so the
    /// canvas is never written back into mid-gesture.
    @State private var revision = 0

    init(target: PhotoEditorTarget, onDone: @escaping (PhotoEditResult) -> Void) {
        self.target = target
        self.onDone = onDone
        _crop = State(initialValue: target.edit.crop)
    }

    private var canReset: Bool { crop != PhotoEdit.whole || !drawing.strokes.isEmpty }

    var body: some View {
        GeometryReader { geo in
            stage(viewport: geo.size)
        }
        .background(Color.black)
        .sheetHeader(title: Self.title, leading: .cancel, onLeading: { dismiss() },
                     action: "Done", actionStyle: .pill, onAction: done)
        // `PKCanvasView` is a scroll view: a swipe-down dismissal races a stroke that starts
        // near the bottom of the picture, and the loser is the stroke.
        .interactiveDismissDisabled(true)
        .preferredColorScheme(.dark)
        // No identifier on the root: `accessibilityIdentifier` on a container replaces the
        // identifier of every element inside it, so one here left Crop, Draw, Undo, Original
        // and all eight crop handles answering to the container's name and to nothing else.
        .onAppear(perform: load)
    }

    @ViewBuilder
    private func stage(viewport: CGSize) -> some View {
        let size = picture?.size ?? .zero
        let frame = PhotoEditGeometry.pictureFrame(picture: size, crop: mode == .crop ? PhotoEdit.whole : crop,
                                                   in: viewport)
        let cropRect = PhotoEditGeometry.rect(of: crop, in: frame)
        ZStack {
            Color.black
            if let picture, !frame.isEmpty {
                layers(picture: picture, frame: frame, cropRect: cropRect, viewport: viewport)
            }
            if mode == .crop, !frame.isEmpty {
                PhotoCropOverlay(crop: $crop, picture: frame, viewport: viewport)
            }
        }
        .frame(width: viewport.width, height: viewport.height)
        .clipped()
        .overlay(alignment: .top) { controls }
    }

    private func layers(picture: UIImage, frame: CGRect, cropRect: CGRect, viewport: CGSize) -> some View {
        // Crop mode shows every stroke, dimmed outside the rectangle, so what is about to be
        // cut away is visible before Done. Draw mode clips to the crop, which is also what
        // keeps a stroke from being made where it could never be seen.
        let clip = mode == .draw ? cropRect : CGRect(origin: .zero, size: viewport)
        return ZStack {
            Image(uiImage: picture)
                .resizable()
                .frame(width: frame.width, height: frame.height)
                .position(x: frame.midX, y: frame.midY)
            PhotoInkCanvas(drawing: $drawing, revision: revision, active: mode == .draw,
                           scale: frame.width / max(picture.size.width, 1), onBeginStroke: pushHistory)
                .frame(width: frame.width, height: frame.height)
                .allowsHitTesting(mode == .draw)
                .accessibilityIdentifier("photo-editor-canvas")
                .position(x: frame.midX, y: frame.midY)
        }
        .frame(width: viewport.width, height: viewport.height)
        .compositingGroup()
        .clipShape(PhotoRectShape(rect: clip))
    }

    private var controls: some View {
        ZStack {
            modePicker
            HStack(spacing: Controls.gap) {
                originalPill
                Spacer(minLength: 0)
                undoDisc
            }
        }
        .padding(.horizontal, PhotoEditorChrome.inset)
        .padding(.top, PhotoEditorChrome.top)
    }

    private var modePicker: some View {
        HStack(spacing: 0) {
            ForEach(PhotoEditorMode.allCases) { m in
                Button { mode = m } label: {
                    Text(m.label)
                        .font(theme.font(Theme.tiny, weight: .semibold))
                        .foregroundStyle(.white.opacity(mode == m ? 1 : 0.6))
                        .padding(.horizontal, 14)
                        .frame(height: PhotoEditorChrome.bar - 2 * PhotoEditorChrome.segmentPad)
                        .background {
                            if mode == m { Capsule().fill(Color.white.opacity(0.16)) }
                        }
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("photo-editor-mode-\(m.rawValue)")
                .accessibilityAddTraits(mode == m ? [.isButton, .isSelected] : [.isButton])
            }
        }
        .padding(PhotoEditorChrome.segmentPad)
        .background(ViewerChromeBackground(shape: Capsule(), interactive: true))
    }

    private var originalPill: some View {
        Button(action: reset) {
            Text("Original")
                .font(theme.font(Theme.tiny, weight: .medium))
                .foregroundStyle(.white)
                .padding(.horizontal, 14)
                .frame(height: PhotoEditorChrome.bar)
                .background(ViewerChromeBackground(shape: Capsule(), interactive: true))
        }
        .buttonStyle(.plain)
        .disabled(!canReset)
        .opacity(canReset ? 1 : 0.45)
        .accessibilityIdentifier("photo-editor-original")
        .accessibilityLabel("Back to the original")
    }

    @ViewBuilder
    private var undoDisc: some View {
        if mode == .draw {
            Button(action: undo) {
                Image(systemName: "arrow.uturn.backward")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(.white)
                    .frame(width: PhotoEditorChrome.bar, height: PhotoEditorChrome.bar)
                    .background(ViewerChromeBackground(shape: Circle(), interactive: true))
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .disabled(history.isEmpty)
            .opacity(history.isEmpty ? 0.45 : 1)
            .accessibilityIdentifier("photo-editor-undo")
            .accessibilityLabel("Undo")
        } else {
            Color.clear.frame(width: PhotoEditorChrome.bar, height: PhotoEditorChrome.bar)
        }
    }

    // MARK: Actions

    private func load() {
        if picture == nil { picture = UIImage(data: target.edit.original).map(PhotoEditRender.normalised) }
        drawing = target.edit.drawing
        revision += 1
    }

    private func pushHistory() {
        history.append(drawing)
        if history.count > PhotoEditorChrome.undoDepth { history.removeFirst() }
    }

    private func undo() {
        guard let previous = history.popLast() else { return }
        drawing = previous
        revision += 1
    }

    /// Original clears both halves of the edit — the crop and every stroke, including the
    /// ones outside the crop.
    private func reset() {
        crop = PhotoEdit.whole
        drawing = PKDrawing()
        history = []
        revision += 1
    }

    private func done() {
        defer { dismiss() }
        guard let picture else { return }
        var edit = target.edit
        edit.crop = crop
        edit.strokes = PhotoEdit.strokeData(drawing)
        guard !edit.isWhole else {
            onDone(PhotoEditResult(id: target.id, name: target.name, mime: target.mime,
                                   data: edit.original, edit: nil))
            return
        }
        let flat = PhotoEditRender.flattened(picture, crop: crop, drawing: drawing)
        guard let out = PhotoEditRender.encoded(flat, name: target.name, mime: target.mime) else { return }
        onDone(PhotoEditResult(id: target.id, name: out.name, mime: out.mime, data: out.data, edit: edit))
    }
}

/// A clip at a rect already in the clipped view's own coordinates.
private struct PhotoRectShape: Shape {
    let rect: CGRect
    func path(in _: CGRect) -> Path { Path(rect) }
}

// MARK: - Crop

private struct PhotoCropOverlay: View {
    @Binding var crop: CGRect
    /// The whole picture's frame in this view's coordinates.
    let picture: CGRect
    let viewport: CGSize
    @State private var dragFrom: CGRect?

    var body: some View {
        let rect = PhotoEditGeometry.rect(of: crop, in: picture)
        ZStack {
            dim(rect)
            grid(rect)
            Rectangle()
                .stroke(Color.white.opacity(PhotoEditorChrome.edge), lineWidth: 1)
                .frame(width: rect.width, height: rect.height)
                .position(x: rect.midX, y: rect.midY)
            ForEach(PhotoCropHandle.allCases) { handle in
                mark(handle, in: rect)
            }
            ForEach(PhotoCropHandle.allCases) { handle in
                grab(handle, in: rect)
            }
        }
        // No identifier on this container either: it would replace the identifier of all
        // eight handles inside it, exactly as the root's did to everything in the sheet.
        .frame(width: viewport.width, height: viewport.height)
    }

    private func dim(_ rect: CGRect) -> some View {
        let bands = [CGRect(x: 0, y: 0, width: viewport.width, height: rect.minY),
                     CGRect(x: 0, y: rect.maxY, width: viewport.width, height: viewport.height - rect.maxY),
                     CGRect(x: 0, y: rect.minY, width: rect.minX, height: rect.height),
                     CGRect(x: rect.maxX, y: rect.minY, width: viewport.width - rect.maxX, height: rect.height)]
        return ForEach(bands.indices, id: \.self) { i in
            Rectangle()
                .fill(Color.black.opacity(PhotoEditorChrome.dim))
                .frame(width: max(0, bands[i].width), height: max(0, bands[i].height))
                .position(x: bands[i].midX, y: bands[i].midY)
        }
    }

    private func grid(_ rect: CGRect) -> some View {
        ForEach(1...2, id: \.self) { i in
            let f = CGFloat(i) / 3
            ZStack {
                Rectangle().fill(Color.white.opacity(PhotoEditorChrome.grid))
                    .frame(width: 1, height: rect.height)
                    .position(x: rect.minX + rect.width * f, y: rect.midY)
                Rectangle().fill(Color.white.opacity(PhotoEditorChrome.grid))
                    .frame(width: rect.width, height: 1)
                    .position(x: rect.midX, y: rect.minY + rect.height * f)
            }
        }
    }

    private func mark(_ handle: PhotoCropHandle, in rect: CGRect) -> some View {
        let length = PhotoEditorChrome.handle
        let thick = PhotoEditorChrome.handleBar
        let p = handle.point(of: rect)
        let sx: CGFloat = handle.movesLeading ? 1 : -1
        let sy: CGFloat = handle.movesTop ? 1 : -1
        return ZStack {
            if handle.isCorner {
                bar(length, thick, p.x + sx * length / 2, p.y + sy * thick / 2)
                bar(thick, length, p.x + sx * thick / 2, p.y + sy * length / 2)
            } else if handle.movesTop || handle.movesBottom {
                bar(length, thick, p.x, p.y + sy * thick / 2)
            } else {
                bar(thick, length, p.x + sx * thick / 2, p.y)
            }
        }
    }

    private func bar(_ w: CGFloat, _ h: CGFloat, _ x: CGFloat, _ y: CGFloat) -> some View {
        Rectangle().fill(Color.white).frame(width: w, height: h).position(x: x, y: y)
    }

    private func grab(_ handle: PhotoCropHandle, in rect: CGRect) -> some View {
        let p = handle.point(of: rect)
        // The gesture and the identifier go on the 44 pt square, BEFORE `.position`:
        // `.position` returns a view that fills the space offered to it, so a gesture or an
        // identifier attached after it would belong to the whole viewport — eight handles
        // would then overlap completely, and the element's own centre would be the middle of
        // the screen rather than the corner it marks.
        return Color.clear
            .frame(width: PhotoEditorChrome.handleHit, height: PhotoEditorChrome.handleHit)
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 1)
                    .onChanged { value in
                        guard picture.width > 0, picture.height > 0 else { return }
                        let from = dragFrom ?? crop
                        if dragFrom == nil { dragFrom = crop }
                        let delta = CGSize(width: value.translation.width / picture.width,
                                           height: value.translation.height / picture.height)
                        crop = PhotoEditGeometry.resized(from, handle: handle, by: delta)
                    }
                    .onEnded { _ in dragFrom = nil }
            )
            // A clear view with a drag on it is not an accessibility element by itself, and
            // an identifier alone does not make one: without this the handles were invisible
            // to VoiceOver and unaddressable by name, however plainly they were drawn.
            .accessibilityElement()
            .accessibilityLabel(handle.label)
            .accessibilityIdentifier("photo-editor-handle-\(handle.rawValue)")
            .position(x: p.x, y: p.y)
    }
}

// MARK: - Ink

/// `PKCanvasView` with Apple's own tool picker, trimmed to pen, highlighter and eraser —
/// the colour well is the picker's own and rides with the inking tools. iOS 17 has no
/// `PKToolPicker(toolItems:)`, so there the canvas keeps one pen and shows no picker.
private struct PhotoInkCanvas: UIViewRepresentable {
    @Binding var drawing: PKDrawing
    let revision: Int
    /// Draw mode: the picker is up and the canvas takes touches.
    let active: Bool
    /// View points per picture pixel. Strokes are stored in the picture's pixels, so the
    /// canvas is written through this scale and read back through its inverse.
    let scale: CGFloat
    let onBeginStroke: () -> Void

    func makeUIView(context: Context) -> PKCanvasView {
        let canvas = PKCanvasView()
        canvas.backgroundColor = .clear
        canvas.isOpaque = false
        // Without `.anyInput` a finger only draws on a device that has never seen a pencil.
        canvas.drawingPolicy = .anyInput
        canvas.isScrollEnabled = false
        canvas.alwaysBounceVertical = false
        canvas.alwaysBounceHorizontal = false
        canvas.tool = PKInkingTool(.pen, color: .systemRed, width: 8)
        canvas.delegate = context.coordinator
        return canvas
    }

    func updateUIView(_ canvas: PKCanvasView, context: Context) {
        context.coordinator.onChange = { drawing = $0 }
        context.coordinator.onBegin = onBeginStroke
        context.coordinator.apply(drawing, revision: revision, scale: scale, to: canvas)
        context.coordinator.setPicker(visible: active, on: canvas)
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator: NSObject, PKCanvasViewDelegate {
        var onChange: (PKDrawing) -> Void = { _ in }
        var onBegin: () -> Void = {}
        private var picker: PKToolPicker?
        private var pickerVisible = false
        private var appliedRevision: Int?
        private var appliedScale: CGFloat = 0
        /// Set while this coordinator writes the canvas, so PencilKit's change callback does
        /// not push that write back into `@State` in the middle of a view update.
        private var writing = false

        func apply(_ drawing: PKDrawing, revision: Int, scale: CGFloat, to canvas: PKCanvasView) {
            guard scale > 0, revision != appliedRevision || scale != appliedScale else { return }
            appliedRevision = revision
            appliedScale = scale
            writing = true
            canvas.drawing = drawing.transformed(using: CGAffineTransform(scaleX: scale, y: scale))
            writing = false
        }

        func setPicker(visible: Bool, on canvas: PKCanvasView) {
            if picker == nil, #available(iOS 18.0, *) {
                let made = PKToolPicker(toolItems: [PKToolPickerInkingItem(type: .pen),
                                                    PKToolPickerInkingItem(type: .marker),
                                                    PKToolPickerEraserItem(type: .bitmap)])
                made.addObserver(canvas)
                picker = made
            }
            guard let picker, visible != pickerVisible else { return }
            pickerVisible = visible
            picker.setVisible(visible, forFirstResponder: canvas)
            if visible { canvas.becomeFirstResponder() } else { canvas.resignFirstResponder() }
        }

        func canvasViewDidBeginUsingTool(_ canvasView: PKCanvasView) {
            onBegin()
        }

        func canvasViewDrawingDidChange(_ canvasView: PKCanvasView) {
            guard !writing, appliedScale > 0 else { return }
            let inverse = CGAffineTransform(scaleX: 1 / appliedScale, y: 1 / appliedScale)
            onChange(canvasView.drawing.transformed(using: inverse))
        }
    }
}
