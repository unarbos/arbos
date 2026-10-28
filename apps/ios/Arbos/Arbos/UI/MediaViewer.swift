import ArbosFFI
import AVKit
import Photos
import QuickLook
import SwiftUI
import UIKit

// The media viewer (feedback media-viewer-2026-10-23, #1381): one full-screen
// viewer for every chat kind, opened from an image / file bubble or a Library
// tile. Black backdrop; the album (the chat's pictures and videos in order)
// pages with a swipe; swipe down, ×, or the Close pill closes. Caption
// top-centre (who, when), counter top-right, Close · Download · Forward pills
// at the bottom ([feedback ABUo19aGqPpSnnw4Ha6F6H0] — the top × was easy to
// miss / not hittable over the paged image; Close matches Mac). Video plays
// inline after Play; a file shows its tile with Open (Quick Look). Forward is a
// sheet of DMs, agents and groups; one tap sends the item there as a new message.

/// One entry of the viewer's album.
struct MediaItem: Identifiable, Equatable {
    enum Kind: Equatable { case image, video, file }
    /// `<message id>#<part id>` — the transcript row's id.
    var id: String
    var name: String
    var mime: String
    var kind: Kind
    var blobRef: String?
    /// Blob slot (`<project id>` or `people:<chat id>`).
    var slot: String
    /// Who shared it ("Samir Haddad", "You", the agent's name).
    var by: String
    var at: Int64
    /// A profile picture (Jacob, 2026-10-25: tap the hero to view it): fetched from this URL
    /// instead of a chat blob — no Download / Forward, and `caption` under the name.
    var pictureURL: String? = nil
    var caption: String? = nil

    /// The line under the name: the caption of a profile picture, else when it was shared.
    var subline: String { caption ?? whenLabel }

    /// "Today at 2:14 PM", "Yesterday at 9:03 AM", "Oct 12 at 4:40 PM".
    var whenLabel: String {
        let date = Date(timeIntervalSince1970: Double(at) / 1000)
        let cal = Calendar.current
        let time = date.formatted(date: .omitted, time: .shortened)
        if cal.isDateInToday(date) { return "Today at \(time)" }
        if cal.isDateInYesterday(date) { return "Yesterday at \(time)" }
        if cal.component(.year, from: date) == cal.component(.year, from: Date()) {
            return "\(date.formatted(.dateTime.month(.abbreviated).day())) at \(time)"
        }
        return "\(date.formatted(date: .abbreviated, time: .omitted)) at \(time)"
    }

    /// Mac `AttachmentKind::label` for the file state's meta line.
    var kindLabel: String {
        let ext = (name as NSString).pathExtension.uppercased()
        switch kind {
        case .image: return "Image"
        case .video: return "Video"
        case .file: return ext.isEmpty ? "File" : (ext == "PDF" ? "PDF" : "\(ext) file")
        }
    }
}

enum MediaAlbum {
    /// The viewer kind of an attachment; `nil` for voice memos (they stay in their chip).
    static func kind(of a: Attachment) -> MediaItem.Kind? {
        let mime = a.mime.lowercased()
        let k = a.kind?.lowercased()
        if mime.hasPrefix("audio/") || k == "audio" { return nil }
        if a.isImage { return .image }
        if mime.hasPrefix("video/") || k == "video" { return .video }
        let ext = (a.name as NSString).pathExtension.lowercased()
        if ["mp4", "mov", "webm", "mkv", "m4v"].contains(ext) { return .video }
        return .file
    }

    /// Every picture and video of a chat in transcript order — what the viewer
    /// pages through. Files are not in it: a file opens on its own.
    /// `projectName` resolves an agent's project id for the caption.
    static func items(messages: [Message], slot: String, projectName: (String) -> String?) -> [MediaItem] {
        var out: [MediaItem] = []
        for m in messages where !m.voice {
            for part in m.parts {
                guard case .attachment(let id, let a) = part, let kind = kind(of: a), kind != .file else { continue }
                out.append(item(id: "\(m.id)#\(id)", message: m, attachment: a, kind: kind, slot: slot, projectName: projectName))
            }
        }
        return out
    }

    static func item(id: String, message m: Message, attachment a: Attachment, kind: MediaItem.Kind, slot: String,
                     projectName: (String) -> String?) -> MediaItem {
        MediaItem(id: id, name: a.name, mime: a.mime, kind: kind, blobRef: a.blobRef, slot: slot,
                  by: senderName(m, projectName: projectName), at: m.createdAt)
    }

    /// Who shared it, for the caption: name, else `@handle`, else "You" / the agent.
    static func senderName(_ m: Message, projectName: (String) -> String?) -> String {
        if let n = m.senderName, !n.isEmpty { return n }
        if let h = m.senderHandle, !h.isEmpty { return "@\(h)" }
        if m.role == .user { return "You" }
        if m.sender.hasPrefix("agent:"), let n = projectName(String(m.sender.dropFirst(6))) { return n }
        if m.sender.hasPrefix("project:"), let n = projectName(String(m.sender.dropFirst(8))) { return n }
        return m.role == .assistant ? "Agent" : "You"
    }

    /// `name`'s profile picture at `url`, as a one-page album (Jacob, 2026-10-25).
    static func picture(name: String, url: String) -> MediaItem {
        MediaItem(id: "picture#\(url)", name: "Profile picture", mime: "image/jpeg", kind: .image, blobRef: nil, slot: "",
                  by: name, at: 0, pictureURL: url, caption: "Profile picture")
    }

    /// The "Forwarded from <name>" line a forwarded attachment travels with (Mac
    /// `forwarded_from_text`); every client draws it as the card's hint, not a bubble.
    static let forwardedPrefix = "Forwarded from "

    static func forwardedFrom(_ text: String) -> String? {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return t.hasPrefix(forwardedPrefix) && !t.contains("\n") ? t : nil
    }
}

/// What a `fullScreenCover(item:)` presents: an album and the page to show.
struct MediaViewerState: Identifiable, Equatable {
    var id: String { "\(items.first?.id ?? "")#\(index)#\(items.count)" }
    var items: [MediaItem]
    var index: Int
}

/// A chat the Forward sheet can send to.
struct ForwardTarget: Identifiable, Equatable {
    enum Kind: Equatable { case people(PeopleChat), agent(Project) }
    var id: String
    var kind: Kind
    var title: String
    var subtitle: String
    var activity: Int64
}

// MARK: - Viewer

struct MediaViewer: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let album: [MediaItem]
    @State var index: Int
    let onClose: () -> Void
    /// Rig: open with the Forward sheet already up (`ARBOS_SHOW=media-forward`).
    var openForward = false
    /// The Library row's "Save": open and download at once (Photos / share sheet).
    var openDownload = false

    /// Fades the chrome and backdrop while a swipe-down is in flight (0…1).
    @State private var dismissProgress: CGFloat = 0
    @State private var showForward = false
    @State private var toast: String?
    @State private var busy = false
    @State private var shareURL: ShareFile?

    private var item: MediaItem? { album.indices.contains(index) ? album[index] : nil }
    private var canAct: Bool { item?.blobRef.map { !$0.hasPrefix("local:") } ?? false }

    var body: some View {
        ZStack {
            Color.black.opacity(1 - Double(dismissProgress) * 0.65).ignoresSafeArea()
            if album.count > 1 {
                TabView(selection: $index) {
                    ForEach(Array(album.enumerated()), id: \.element.id) { i, item in
                        page(item).tag(i)
                    }
                }
                .tabViewStyle(.page(indexDisplayMode: .never))
                .ignoresSafeArea()
            } else if let item {
                page(item).ignoresSafeArea()
            }
        }
        // Overlays sit above the UIKit page TabView (which can punch through a
        // ZStack sibling and eat the top × hits — feedback ABUo19aGqPpSnnw4Ha6F6H0).
        .overlay(alignment: .top) {
            topBar.opacity(1 - Double(dismissProgress))
        }
        .overlay(alignment: .bottom) {
            bottomBar.opacity(1 - Double(dismissProgress))
        }
        .statusBarHidden(false)
        .preferredColorScheme(.dark)
        .onAppear {
            if openForward { showForward = true }
            if openDownload { Task { await download() } }
            #if DEBUG
            if let ms = DebugHooks.viewerCloseMs {
                DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(ms)) { close() }
            }
            #endif
        }
        .sheet(isPresented: $showForward) {
            if let item {
                ForwardSheet(fromName: item.by, attachmentNote: true) { target in
                    showForward = false
                    model.forward(item, to: target)
                    flash("Forwarded to \(target.title)")
                }
                .environmentObject(model)
                .environment(\.theme, theme)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
            }
        }
        .sheet(item: $shareURL) { file in
            ActivityView(items: [file.url])
        }
        .accessibilityAction(.escape, close)
    }

    /// Dismiss the cover: clear the caller's item binding and the environment dismiss.
    private func close() {
        onClose()
        dismiss()
    }

    @ViewBuilder
    private func page(_ item: MediaItem) -> some View {
        switch item.kind {
        case .image:
            ZoomableImagePage(item: item, dismissProgress: $dismissProgress, onClose: close)
        case .video:
            VideoPage(item: item, dismissProgress: $dismissProgress, onClose: close)
        case .file:
            FilePage(item: item, dismissProgress: $dismissProgress, onClose: close)
        }
    }

    // ----- top bar, pills, dots ----------------------------------------------

    private var topBar: some View {
        HStack(alignment: .center, spacing: 12) {
            Button(action: close) {
                Image(systemName: "xmark")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(.white)
                    .frame(width: 36, height: 36)
                    .background(Circle().fill(Color.white.opacity(0.22)))
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Close")

            Spacer(minLength: 0)

            if let item {
                VStack(spacing: 2) {
                    Text(item.by).font(theme.font(15, weight: .semibold)).foregroundStyle(.white).lineLimit(1)
                    Text(item.subline).font(theme.font(13)).foregroundStyle(.white.opacity(0.67)).lineLimit(1)
                }
            }

            Spacer(minLength: 0)

            if album.count > 1 {
                Text("\(index + 1) of \(album.count)")
                    .font(theme.font(13, weight: .medium))
                    .foregroundStyle(.white.opacity(0.67))
                    .monospacedDigit()
                    .frame(minWidth: 36, alignment: .trailing)
            } else {
                Color.clear.frame(width: 36, height: 36)
            }
        }
        .padding(.horizontal, 16)
        .frame(height: 44)
        .padding(.top, 8)
        .padding(.bottom, 4)
    }

    private var bottomBar: some View {
        VStack(spacing: 0) {
            if let toast {
                Text(toast)
                    .font(theme.font(13, weight: .medium))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 14).padding(.vertical, 8)
                    .background(Capsule().fill(Color.white.opacity(0.18)))
                    .padding(.bottom, 10)
                    .transition(.opacity)
            }
            if album.count > 1 {
                HStack(spacing: 4) {
                    ForEach(album.indices, id: \.self) { i in
                        Circle().fill(Color.white.opacity(i == index ? 1 : 0.35)).frame(width: 6, height: 6)
                    }
                }
                .padding(.bottom, 12)
                .accessibilityHidden(true)
            }
            HStack(spacing: 12) {
                // Close beside Download / Forward — same as Mac; always exits even
                // when swipe-down or the top × lose the gesture race with zoom/paging.
                pill("Close", system: "xmark") { close() }
                // A profile picture is not a chat attachment: Close only.
                if item?.pictureURL == nil {
                    pill("Download", system: "arrow.down.to.line") { Task { await download() } }
                        .disabled(!canAct || busy)
                        .opacity(canAct ? 1 : 0.5)
                    pill("Forward", system: "arrowshape.turn.up.right") { showForward = true }
                        .disabled(!canAct || busy)
                        .opacity(canAct ? 1 : 0.5)
                }
            }
            .padding(.bottom, 14)
        }
        .animation(.easeOut(duration: 0.2), value: toast)
    }

    private func pill(_ label: String, system: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Image(systemName: system).font(.system(size: 14, weight: .semibold))
                Text(label).font(theme.font(15, weight: .medium))
            }
            .foregroundStyle(.white)
            .padding(.horizontal, 16)
            .frame(height: 38)
            .background(Capsule().fill(Color.white.opacity(0.16)))
        }
        .buttonStyle(.plain)
    }

    private func flash(_ text: String) {
        toast = text
        Task {
            try? await Task.sleep(nanoseconds: 1_800_000_000)
            if toast == text { toast = nil }
        }
    }

    // ----- Download -----------------------------------------------------------

    /// Pictures and videos go to Photos (add-only permission); anything else
    /// opens the share sheet so it can be saved to Files or sent on.
    @MainActor
    private func download() async {
        guard let item, canAct else { return }
        busy = true
        defer { busy = false }
        guard let (data, _) = await MediaFetch.bytes(for: item, model: model) else {
            flash("Couldn't download \(item.name)")
            return
        }
        switch item.kind {
        case .image, .video:
            let status = await PHPhotoLibrary.requestAuthorization(for: .addOnly)
            guard status == .authorized || status == .limited else {
                // No Photos access: fall back to the share sheet (Save to Files is in it).
                shareURL = MediaFetch.tempFile(data, name: item.name).map(ShareFile.init)
                return
            }
            do {
                let file = MediaFetch.tempFile(data, name: item.name)
                try await PHPhotoLibrary.shared().performChanges {
                    let req = PHAssetCreationRequest.forAsset()
                    if item.kind == .image {
                        req.addResource(with: .photo, data: data, options: nil)
                    } else if let file {
                        req.addResource(with: .video, fileURL: file, options: nil)
                    }
                }
                flash("Saved to Photos")
            } catch {
                flash("Couldn't save to Photos")
            }
        case .file:
            shareURL = MediaFetch.tempFile(data, name: item.name).map(ShareFile.init)
        }
    }
}

// MARK: - Pages

/// Shared swipe-down-to-dismiss for every page kind: a vertical drag at 1×
/// follows the finger, fades the chrome, and closes past 120 pt.
///
/// Uses a UIKit pan that only *begins* when the drag is clearly vertical, so a
/// horizontal album swipe is never claimed by dismiss. A SwiftUI `DragGesture`
/// recognizes after ~12 pt in any direction and freezes paging even when its
/// handlers then no-op ([feedback app-01M5ZFF6EW6QE7RB7M5EQ4F2NP]).
private struct DismissDrag: ViewModifier {
    @Binding var progress: CGFloat
    let onClose: () -> Void
    var enabled: Bool = true
    @State private var offset: CGFloat = 0

    func body(content: Content) -> some View {
        content
            .offset(y: offset)
            .scaleEffect(max(0.85, 1 - offset / 1000))
            .background(
                VerticalDismissProbe(enabled: enabled, offset: $offset, progress: $progress, onClose: onClose)
            )
    }
}

/// Installs a vertical-only pan on the page host. The probe itself never takes
/// hits (`isUserInteractionEnabled = false`); the recognizer lives on the
/// enclosing page view so Play / Open / double-tap still work.
private struct VerticalDismissProbe: UIViewRepresentable {
    var enabled: Bool
    @Binding var offset: CGFloat
    @Binding var progress: CGFloat
    var onClose: () -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(offset: $offset, progress: $progress, onClose: onClose)
    }

    func makeUIView(context: Context) -> Probe {
        let probe = Probe()
        probe.isUserInteractionEnabled = false
        probe.backgroundColor = .clear
        probe.coordinator = context.coordinator
        return probe
    }

    func updateUIView(_ uiView: Probe, context: Context) {
        context.coordinator.enabled = enabled
        context.coordinator.offset = $offset
        context.coordinator.progress = $progress
        context.coordinator.onClose = onClose
        DispatchQueue.main.async { uiView.attachIfNeeded() }
    }

    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        var enabled = true
        var offset: Binding<CGFloat>
        var progress: Binding<CGFloat>
        var onClose: () -> Void
        /// How far down this drag has already moved the page (for the end threshold).
        private var dragOffset: CGFloat = 0

        init(offset: Binding<CGFloat>, progress: Binding<CGFloat>, onClose: @escaping () -> Void) {
            self.offset = offset
            self.progress = progress
            self.onClose = onClose
        }

        @objc func handle(_ pan: UIPanGestureRecognizer) {
            guard enabled else { return }
            let t = pan.translation(in: pan.view)
            switch pan.state {
            case .changed:
                guard t.y > 0, t.y >= abs(t.x) * 1.2 else { return }
                dragOffset = t.y
                offset.wrappedValue = t.y
                progress.wrappedValue = min(1, t.y / 400)
            case .ended, .cancelled:
                if dragOffset > 120, t.y > abs(t.x) * 1.2 {
                    onClose()
                } else {
                    withAnimation(.easeOut(duration: 0.2)) {
                        offset.wrappedValue = 0
                        progress.wrappedValue = 0
                    }
                }
                dragOffset = 0
            default:
                break
            }
        }

        func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
            guard enabled, let pan = gestureRecognizer as? UIPanGestureRecognizer else { return false }
            let v = pan.velocity(in: pan.view)
            return v.y > 0 && v.y >= abs(v.x) * 1.2
        }

        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer,
                               shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
            true
        }
    }

    final class Probe: UIView {
        weak var coordinator: Coordinator?
        private weak var host: UIView?
        private var pan: UIPanGestureRecognizer?

        override func didMoveToWindow() {
            super.didMoveToWindow()
            attachIfNeeded()
        }

        deinit {
            if let pan { pan.view?.removeGestureRecognizer(pan) }
        }

        func attachIfNeeded() {
            guard window != nil, let coordinator else { return }
            guard let found = Self.pageHost(from: self) else { return }
            if host === found, pan != nil { return }
            if let pan { pan.view?.removeGestureRecognizer(pan) }
            let gr = UIPanGestureRecognizer(target: coordinator, action: #selector(Coordinator.handle(_:)))
            gr.delegate = coordinator
            gr.cancelsTouchesInView = false
            found.addGestureRecognizer(gr)
            host = found
            pan = gr
        }

        /// First ancestor large enough to be the page (the probe is zero-sized).
        private static func pageHost(from view: UIView) -> UIView? {
            var node: UIView? = view.superview
            while let current = node {
                if current.bounds.width > 100, current.bounds.height > 100 { return current }
                node = current.superview
            }
            return view.superview
        }
    }
}

/// Full-screen image: pinch to zoom, drag to pan when zoomed, double-tap to toggle.
private struct ZoomableImagePage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let item: MediaItem
    @Binding var dismissProgress: CGFloat
    let onClose: () -> Void

    @State private var image: UIImage?
    @State private var failed = false
    @State private var scale: CGFloat = 1
    @State private var baseScale: CGFloat = 1
    @State private var offset: CGSize = .zero
    @State private var baseOffset: CGSize = .zero

    private let minScale: CGFloat = 1
    private let maxScale: CGFloat = 5

    var body: some View {
        // Fit the whole picture in the viewport at 1× (aspect-fit). Without an
        // explicit size, a large photo can lay out at its pixel size inside the
        // paged TabView and look like a zoomed-in crop.
        GeometryReader { geo in
            ZStack {
                if let image {
                    Image(uiImage: image)
                        .resizable()
                        .scaledToFit()
                        .frame(width: geo.size.width, height: geo.size.height)
                        .scaleEffect(scale)
                        .offset(offset)
                        .gesture(magnify)
                        // Pan only while zoomed: an always-on DragGesture steals the
                        // album's horizontal page swipe at 1× and freezes it.
                        .simultaneousGesture(pan, including: scale > minScale + 0.01 ? .all : .subviews)
                        .onTapGesture(count: 2, perform: toggleZoom)
                        .modifier(DismissDrag(progress: $dismissProgress, onClose: onClose, enabled: scale <= minScale + 0.01))
                        .accessibilityLabel("Image")
                        .accessibilityHint("Pinch to zoom, double tap to zoom in or out, swipe down to close")
                } else if failed {
                    VStack(spacing: 12) {
                        Image(systemName: "photo").font(.system(size: 44)).foregroundStyle(.white.opacity(0.35))
                        Text("No preview").font(theme.font(13)).foregroundStyle(.white.opacity(0.67))
                    }
                    .frame(width: geo.size.width, height: geo.size.height)
                    .modifier(DismissDrag(progress: $dismissProgress, onClose: onClose))
                } else {
                    ProgressView().tint(.white)
                        .frame(width: geo.size.width, height: geo.size.height)
                }
            }
        }
        .task(id: item.blobRef ?? item.pictureURL) {
            if let (data, _) = await MediaFetch.bytes(for: item, model: model), let img = UIImage(data: data) {
                image = img
            } else {
                failed = true
            }
        }
    }

    private var magnify: some Gesture {
        MagnifyGesture()
            .onChanged { v in scale = min(max(baseScale * v.magnification, minScale), maxScale) }
            .onEnded { _ in
                baseScale = scale
                if scale <= minScale + 0.01 { withAnimation(.easeOut(duration: 0.2)) { reset() } }
            }
    }

    private var pan: some Gesture {
        DragGesture()
            .onChanged { v in
                guard scale > minScale else { return }
                offset = CGSize(width: baseOffset.width + v.translation.width, height: baseOffset.height + v.translation.height)
            }
            .onEnded { _ in if scale > minScale { baseOffset = offset } }
    }

    private func toggleZoom() {
        withAnimation(.easeOut(duration: 0.2)) {
            if scale > minScale { reset() } else { scale = 2.5; baseScale = 2.5 }
        }
    }

    private func reset() {
        scale = minScale; baseScale = minScale
        offset = .zero; baseOffset = .zero
    }
}

/// Video: a dark poster with Play; the file is fetched with the device bearer
/// (AVPlayer cannot send `Authorization`) and plays inline with the system controls.
private struct VideoPage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let item: MediaItem
    @Binding var dismissProgress: CGFloat
    let onClose: () -> Void

    @State private var player: AVPlayer?
    @State private var loading = false
    @State private var failed = false

    var body: some View {
        ZStack {
            if let player {
                VideoPlayer(player: player)
                    .aspectRatio(16 / 9, contentMode: .fit)
                    .onAppear { player.play() }
                    .onDisappear { player.pause() }
            } else {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(Color.white.opacity(0.06))
                    .aspectRatio(16 / 9, contentMode: .fit)
                    .overlay {
                        Image(systemName: "film").font(.system(size: 44)).foregroundStyle(.white.opacity(0.25))
                    }
                    .overlay {
                        VStack(spacing: 14) {
                            Button { Task { await load() } } label: {
                                ZStack {
                                    Circle().fill(Color.black.opacity(0.47)).frame(width: 72, height: 72)
                                    if loading {
                                        ProgressView().tint(.white)
                                    } else {
                                        Image(systemName: "play.fill").font(.system(size: 30, weight: .semibold))
                                            .foregroundStyle(.white).offset(x: 3)
                                    }
                                }
                            }
                            .buttonStyle(.plain)
                            .disabled(loading || item.blobRef == nil)
                            .accessibilityLabel("Play")
                            Text(failed ? "Couldn't load the video" : (item.blobRef == nil ? "Still uploading" : item.name))
                                .font(theme.font(13)).foregroundStyle(.white.opacity(0.67)).lineLimit(1)
                        }
                    }
                    .padding(.horizontal, 16)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .modifier(DismissDrag(progress: $dismissProgress, onClose: onClose, enabled: player == nil))
    }

    @MainActor
    private func load() async {
        loading = true
        defer { loading = false }
        guard let (data, _) = await MediaFetch.bytes(for: item, model: model),
              let file = MediaFetch.tempFile(data, name: item.name) else {
            failed = true
            return
        }
        player = AVPlayer(url: file)
    }
}

/// A file / PDF / document: its tile, name, kind, and Open → Quick Look.
private struct FilePage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let item: MediaItem
    @Binding var dismissProgress: CGFloat
    let onClose: () -> Void

    @State private var previewURL: URL?
    @State private var loading = false
    @State private var failed = false

    private var ext: String? {
        let e = (item.name as NSString).pathExtension.uppercased()
        return (2...4).contains(e.count) ? e : nil
    }

    var body: some View {
        VStack(spacing: 10) {
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .fill(Color.white.opacity(0.11))
                .frame(width: 100, height: 130)
                .overlay {
                    if let ext {
                        Text(ext).font(theme.font(15, weight: .semibold)).foregroundStyle(.white)
                    } else {
                        Image(systemName: "doc").font(.system(size: 36)).foregroundStyle(.white.opacity(0.8))
                    }
                }
            Text(item.name).font(theme.font(17, weight: .semibold)).foregroundStyle(.white)
                .lineLimit(2).multilineTextAlignment(.center).padding(.top, 8).padding(.horizontal, 32)
            Text(failed ? "Couldn't load the file" : item.kindLabel).font(theme.font(13)).foregroundStyle(.white.opacity(0.67))
            Button { Task { await open() } } label: {
                HStack(spacing: 8) {
                    if loading {
                        ProgressView().tint(.white).controlSize(.small)
                    } else {
                        Image(systemName: "eye").font(.system(size: 14, weight: .semibold))
                    }
                    Text(loading ? "Opening…" : "Open").font(theme.font(15, weight: .medium))
                }
                .foregroundStyle(.white)
                .padding(.horizontal, 16)
                .frame(height: 38)
                .background(Capsule().fill(Color.white.opacity(0.16)))
            }
            .buttonStyle(.plain)
            .disabled(loading || item.blobRef == nil)
            .padding(.top, 14)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .modifier(DismissDrag(progress: $dismissProgress, onClose: onClose))
        .quickLookPreview($previewURL)
    }

    @MainActor
    private func open() async {
        if let previewURL {
            // `.quickLookPreview` only presents when the binding becomes non-nil.
            self.previewURL = nil
            Task { @MainActor in self.previewURL = previewURL }
            return
        }
        loading = true
        defer { loading = false }
        guard let (data, _) = await MediaFetch.bytes(for: item, model: model),
              let file = MediaFetch.tempFile(data, name: item.name) else {
            failed = true
            return
        }
        previewURL = file
    }
}

// MARK: - Forward

/// Chat picker for Forward: media viewer and the bubble long-press menu share it.
struct ForwardSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    /// Who wrote the original (shown in the attachment-style footer note).
    let fromName: String
    /// Media forward: footer mentions the “Forwarded from …” line the attachment travels with.
    var attachmentNote = false
    let onPick: (ForwardTarget) -> Void
    @State private var query = ""

    private var targets: [ForwardTarget] { model.forwardTargets(matching: query) }

    private var footer: String {
        if attachmentNote {
            return "Sends it as a new message, “\(MediaAlbum.forwardedPrefix)\(fromName)”."
        }
        return "Sends the text as a new message."
    }

    var body: some View {
        NavigationStack {
            List {
                if targets.isEmpty {
                    Text(query.trimmingCharacters(in: .whitespaces).isEmpty ? "No chats yet." : "No chat matches “\(query)”.")
                        .font(theme.font(Theme.small)).foregroundStyle(theme.muted)
                        .frame(maxWidth: .infinity).listRowBackground(Color.clear)
                }
                ForEach(targets) { target in
                    Button { onPick(target) } label: {
                        HStack(spacing: 14) {
                            avatar(target)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(target.title).font(theme.font(Theme.body, weight: .medium)).foregroundStyle(theme.text).lineLimit(1)
                                Text(target.subtitle).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted).lineLimit(1)
                            }
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .listRowBackground(theme.bg)
                }
                Text(footer)
                    .font(theme.font(Theme.tiny)).foregroundStyle(theme.muted)
                    .listRowBackground(Color.clear)
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .background(theme.bg)
            .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always), prompt: "Search")
            .navigationTitle("Forward to")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
            }
        }
    }

    @ViewBuilder
    private func avatar(_ target: ForwardTarget) -> some View {
        switch target.kind {
        case .agent(let p):
            AgentAvatar(project: p, theme: theme, size: 44, still: true)
        case .people(let c):
            if c.isGroup {
                AvatarCluster(members: c.humans, size: 44, leaf: c.leafPick, neutralName: c.isNeutralAvatar ? c.title : nil)
            } else {
                let other = c.counterpart(me: model.user?.id)
                PersonAvatar(url: other?.avatarUrl, handle: other?.handle ?? "?", size: 44, name: other?.name, id: other?.id, colorIndex: other?.avatarColorIndex)
            }
        }
    }
}

// MARK: - Helpers

enum MediaFetch {
    /// The bytes of a viewer item: the relay blob with the device bearer, or —
    /// in the simulator fixture — a generated picture / PDF for `fixture-*` keys.
    @MainActor
    static func bytes(for item: MediaItem, model: AppModel) async -> (Data, String)? {
        // A profile picture: its public URL (the relay's `/avatars/…`, GitHub's CDN, or a
        // tokened blob URL) — never with the device bearer, which must not reach another host.
        if let picture = item.pictureURL {
            guard let url = URL(string: picture), url.scheme?.hasPrefix("http") == true else { return nil }
            return await data(url, headers: [:])
        }
        guard let ref = item.blobRef, !ref.hasPrefix("local:") else { return nil }
        #if DEBUG
        if model.isFixture, ref.hasPrefix("fixture-") {
            return FixtureMedia.bytes(ref: ref, item: item)
        }
        #endif
        guard let url = model.blobURL(projectId: item.slot, key: ref) else { return nil }
        return await data(url, headers: model.authHeaders)
    }

    /// GET with the device bearer; `nil` on any failure.
    static func data(_ url: URL, headers: [String: String]) async -> (Data, String)? {
        var req = URLRequest(url: url)
        for (k, v) in headers { req.setValue(v, forHTTPHeaderField: k) }
        guard let (data, response) = try? await URLSession.shared.data(for: req),
              let http = response as? HTTPURLResponse, (200..<400).contains(http.statusCode), !data.isEmpty else {
            return nil
        }
        return (data, http.mimeType ?? "application/octet-stream")
    }

    /// `tmp/arbos-media/<name>` so Quick Look, AVPlayer and the share sheet see a real file.
    static func tempFile(_ data: Data, name: String) -> URL? {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("arbos-media", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let safe = (name as NSString).lastPathComponent
        let file = dir.appendingPathComponent(safe.isEmpty ? "file" : safe)
        do {
            try data.write(to: file, options: .atomic)
            return file
        } catch {
            return nil
        }
    }
}

/// A picture loaded through `MediaFetch.bytes` (auth + fixture aware) — the
/// Library grid's tile and anything else that cannot use `AsyncImage`.
struct BlobImage: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let item: MediaItem
    @State private var image: UIImage?
    @State private var failed = false

    var body: some View {
        ZStack {
            if let image {
                Image(uiImage: image).resizable().scaledToFill()
            } else if failed {
                Image(systemName: "photo").font(.system(size: 22)).foregroundStyle(theme.muted)
            } else {
                ProgressView().tint(theme.muted)
            }
        }
        .task(id: item.blobRef ?? item.pictureURL) {
            if let (data, _) = await MediaFetch.bytes(for: item, model: model), let img = UIImage(data: data) {
                image = img
            } else {
                failed = true
            }
        }
    }
}

#if DEBUG
/// Simulator fixture media: a gradient picture seeded by the blob key (the Mac
/// fixture's `fixture_png`), or a one-page PDF naming the file.
enum FixtureMedia {
    static func bytes(ref: String, item: MediaItem) -> (Data, String)? {
        switch item.kind {
        case .image where item.name.contains("Screenshot"):
            return pagePNG().map { ($0, "image/png") }
        case .image where item.name.hasPrefix("IMG_"):
            return phoneShotPNG().map { ($0, "image/png") }
        case .image:
            return png(seed: ref).map { ($0, "image/png") }
        case .file:
            return (pdf(title: item.name), "application/pdf")
        case .video:
            return nil
        }
    }

    private static func png(seed: String) -> Data? {
        var h: UInt32 = 0x811c_9dc5
        for b in seed.utf8 { h = (h ^ UInt32(b)) &* 0x0100_0193 }
        let (w, hgt) = (640, 400)
        let renderer = UIGraphicsImageRenderer(size: CGSize(width: w, height: hgt))
        let img = renderer.image { ctx in
            let cg = ctx.cgContext
            let r0 = CGFloat(60 + Int(h & 0x3F)) / 255, g0 = CGFloat(70 + Int((h >> 8) & 0x3F)) / 255, b0 = CGFloat(120 + Int((h >> 16) & 0x3F)) / 255
            let colors = [UIColor(red: r0, green: g0, blue: min(1, b0 + 60 / 255), alpha: 1).cgColor,
                          UIColor(red: min(1, r0 + 110 / 255), green: min(1, g0 + 90 / 255), blue: b0, alpha: 1).cgColor]
            if let grad = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: colors as CFArray, locations: [0, 1]) {
                cg.drawLinearGradient(grad, start: .zero, end: CGPoint(x: w, y: hgt), options: [])
            }
            cg.setStrokeColor(UIColor(white: 0.96, alpha: 1).cgColor)
            cg.setLineWidth(12)
            cg.stroke(CGRect(x: 0, y: 0, width: w, height: hgt))
        }
        return img.pngData()
    }

    /// A 640 × 400 "page screenshot": white, empty top 22 %, title bar, line, blue
    /// button, short line — the shape of Jacob's shot (the Mac fixture's `fixture_page_png`).
    private static func pagePNG() -> Data? {
        let (w, h) = (640, 400)
        let img = UIGraphicsImageRenderer(size: CGSize(width: w, height: h), format: {
            let f = UIGraphicsImageRendererFormat(); f.scale = 1; return f
        }()).image { ctx in
            let cg = ctx.cgContext
            cg.setFillColor(UIColor.white.cgColor); cg.fill(CGRect(x: 0, y: 0, width: w, height: h))
            cg.setFillColor(UIColor(white: 0.09, alpha: 1).cgColor); cg.fill(CGRect(x: 40, y: 88, width: 260, height: 28))
            cg.setFillColor(UIColor(white: 0.38, alpha: 1).cgColor); cg.fill(CGRect(x: 40, y: 134, width: 290, height: 12))
            cg.setFillColor(UIColor(red: 26 / 255, green: 115 / 255, blue: 232 / 255, alpha: 1).cgColor)
            cg.fill(CGRect(x: 40, y: 176, width: 140, height: 40))
            cg.setFillColor(UIColor(white: 0.38, alpha: 1).cgColor); cg.fill(CGRect(x: 40, y: 240, width: 150, height: 12))
        }
        return img.pngData()
    }

    /// A 390 × 844 portrait "phone screenshot": dark chat page with a header pill and a
    /// few bubbles — the tall picture of [feedback image-caption-bubble-ios-2026-10-25],
    /// whose caption must not wrap at the picture's ~150 pt fitted width.
    private static func phoneShotPNG() -> Data? {
        let (w, h) = (390, 844)
        let img = UIGraphicsImageRenderer(size: CGSize(width: w, height: h), format: {
            let f = UIGraphicsImageRendererFormat(); f.scale = 1; return f
        }()).image { ctx in
            let cg = ctx.cgContext
            func fillPath(_ path: UIBezierPath) { cg.addPath(path.cgPath); cg.fillPath() }
            cg.setFillColor(UIColor(white: 0.07, alpha: 1).cgColor); cg.fill(CGRect(x: 0, y: 0, width: w, height: h))
            // Status bar + header pill.
            cg.setFillColor(UIColor(white: 0.9, alpha: 1).cgColor)
            cg.fill(CGRect(x: 32, y: 22, width: 36, height: 12))
            cg.setFillColor(UIColor(white: 0.16, alpha: 1).cgColor)
            fillPath(UIBezierPath(roundedRect: CGRect(x: 110, y: 62, width: 170, height: 44), cornerRadius: 22))
            // Bubbles: three incoming on the left, two outgoing on the right.
            let rows: [(CGFloat, CGFloat, CGFloat, Bool)] = [(150, 230, 70, false), (236, 180, 48, false), (300, 210, 92, true),
                                                              (408, 250, 120, false), (544, 160, 48, true), (608, 240, 96, false)]
            for (y, bw, bh, mine) in rows {
                let x: CGFloat = mine ? CGFloat(w) - 18 - bw : 18
                cg.setFillColor((mine ? UIColor(white: 0.32, alpha: 1) : UIColor(white: 0.14, alpha: 1)).cgColor)
                fillPath(UIBezierPath(roundedRect: CGRect(x: x, y: y, width: bw, height: bh), cornerRadius: 18))
                cg.setFillColor(UIColor(white: mine ? 0.85 : 0.6, alpha: 1).cgColor)
                var ly = y + 16
                while ly + 8 < y + bh - 10 {
                    cg.fill(CGRect(x: x + 14, y: ly, width: bw - 28 - CGFloat((Int(ly) * 7) % 40), height: 8))
                    ly += 20
                }
            }
            // Composer.
            cg.setFillColor(UIColor(white: 0.14, alpha: 1).cgColor)
            fillPath(UIBezierPath(roundedRect: CGRect(x: 70, y: CGFloat(h) - 86, width: 240, height: 52), cornerRadius: 26))
            fillPath(UIBezierPath(ovalIn: CGRect(x: 14, y: CGFloat(h) - 86, width: 52, height: 52)))
            cg.setFillColor(UIColor.white.cgColor)
            fillPath(UIBezierPath(ovalIn: CGRect(x: CGFloat(w) - 66, y: CGFloat(h) - 86, width: 52, height: 52)))
        }
        return img.pngData()
    }

    private static func pdf(title: String) -> Data {
        let page = CGRect(x: 0, y: 0, width: 612, height: 792)
        return UIGraphicsPDFRenderer(bounds: page).pdfData { ctx in
            ctx.beginPage()
            let attrs: [NSAttributedString.Key: Any] = [.font: UIFont.boldSystemFont(ofSize: 28), .foregroundColor: UIColor.black]
            title.draw(at: CGPoint(x: 56, y: 72), withAttributes: attrs)
            let body: [NSAttributedString.Key: Any] = [.font: UIFont.systemFont(ofSize: 14), .foregroundColor: UIColor.darkGray]
            "Fixture document for the screenshot rig.".draw(at: CGPoint(x: 56, y: 120), withAttributes: body)
        }
    }
}
#endif

/// The system share sheet (Download for non-media files, and the Photos fallback).
private struct ActivityView: UIViewControllerRepresentable {
    let items: [Any]
    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: items, applicationActivities: nil)
    }
    func updateUIViewController(_ vc: UIActivityViewController, context: Context) {}
}

/// A downloaded file for the share sheet (`sheet(item:)` needs an `Identifiable`).
struct ShareFile: Identifiable, Equatable {
    let url: URL
    var id: String { url.absoluteString }
}

// MARK: - Model side

extension AppModel {
    /// The chats the Forward sheet offers: every DM, group and agent that is
    /// not archived, through the shared matcher on title, @handle and kind line
    /// (`EntitySearch`), best match first and most recent inside a tie.
    /// Channels are left out — they carry no attachments yet (Matrix `m.text`).
    func forwardTargets(matching query: String) -> [ForwardTarget] {
        var out: [ForwardTarget] = []
        for c in peopleChats where !isArchived(peopleChatId: c.id) {
            let title: String
            let subtitle: String
            if c.isGroup {
                title = c.title.isEmpty ? "Group" : c.title
                let n = c.humans.count + c.agents.count
                subtitle = "Group · \(n) member\(n == 1 ? "" : "s")"
            } else {
                let other = c.counterpart(me: user?.id)
                title = other?.displayName ?? c.title
                subtitle = "@\(other?.handle ?? c.title)"
            }
            let at = max(c.lastActivityAt, messages(of: c.id).last?.createdAt ?? 0)
            out.append(ForwardTarget(id: "c:\(c.id)", kind: .people(c), title: title, subtitle: subtitle, activity: at))
        }
        for p in projects where !isArchived(p.id) {
            guard primaryChat(of: p.id) != nil else { continue }
            let at = chats(of: p.id).map(\.lastMessageAt).max() ?? 0
            out.append(ForwardTarget(id: "p:\(p.id)", kind: .agent(p), title: p.name, subtitle: "Agent", activity: at))
        }
        out.sort { $0.activity > $1.activity }
        return EntitySearch.filter(out, query: query) { t in
            if t.subtitle.hasPrefix("@") {
                return SearchCandidate(name: t.title, handle: String(t.subtitle.dropFirst()), title: nil)
            }
            return SearchCandidate(name: t.title, handle: "", title: t.subtitle)
        }
    }

    /// Forward a viewer item: fetch its bytes, then send them to `target` as a
    /// new attachment with the "Forwarded from <name>" line (same path as a
    /// picked photo — the blob is copied into the destination's slot).
    func forward(_ item: MediaItem, to target: ForwardTarget) {
        guard let ref = item.blobRef else { return }
        let text = MediaAlbum.forwardedPrefix + item.by
        #if DEBUG
        if isFixture {
            // No blob store to copy between: the row lands with the same key.
            let chatId: Id
            switch target.kind {
            case .people(let c): chatId = c.id
            case .agent(let p): guard let chat = primaryChat(of: p.id) else { return }; chatId = chat.id
            }
            fixtureAppend(chatId: chatId, text: text,
                          attachments: [Attachment(name: item.name, mime: item.mime, blobRef: ref,
                                                   kind: Self.attachmentKind(mime: item.mime, name: item.name))])
            return
        }
        #endif
        guard let url = blobURL(projectId: item.slot, key: ref) else { return }
        Task { @MainActor in
            guard let (data, _) = await MediaFetch.data(url, headers: authHeaders) else {
                fail(UserError.fromText("forward \(item.name)", "the file could not be fetched"))
                return
            }
            let pending = PendingAttachment(name: item.name, mime: item.mime, data: data)
            switch target.kind {
            case .people(let c):
                sendPeople(chatId: c.id, text: text, replyTo: nil, attachments: [pending])
            case .agent(let p):
                guard let chat = primaryChat(of: p.id) else { return }
                send(chatId: chat.id, text: text, attachments: [pending])
            }
        }
    }

    /// Bubble long-press → Forward: send the message's text to `target` as a
    /// new message (no reply quote — it is a fresh send in that chat) carrying
    /// `forwardedFrom: by`, so every client draws "Forwarded from <by>" on it
    /// (feedback app-01M5MXRZ7609S70P9S9P2RZDDF).
    func forwardText(_ text: String, from by: String, to target: ForwardTarget) {
        let body = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !body.isEmpty else { return }
        let name = by.trimmingCharacters(in: .whitespacesAndNewlines)
        let forwardedFrom = name.isEmpty ? nil : name
        switch target.kind {
        case .people(let c):
            sendPeople(chatId: c.id, text: body, replyTo: nil, forwardedFrom: forwardedFrom)
        case .agent(let p):
            guard let chat = primaryChat(of: p.id) else { return }
            send(chatId: chat.id, text: body, forwardedFrom: forwardedFrom)
        }
    }
}
