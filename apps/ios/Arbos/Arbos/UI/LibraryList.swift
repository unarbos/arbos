import SwiftUI

// The Library tab as Grok Bot's, the Mac's `profiles/library.rs` twin (Jacob,
// 2026-10-25, "make library list based with hover pop out";
// [feedback library-list-ios-2026-10-25]): ONE list of everything shared in
// the chats — images, videos, files, voice notes and links together, newest
// first — grouped by day (TODAY · YESTERDAY · the weekday · OCT 18) inside the
// pane's cards. A row is the kit's 48 pt row with a 36 pt thumbnail or type
// tile, the name on one line and the relative time ("13h"); a day shows five
// rows then "Show more" that unfolds the rest in place. A long press is the
// system context menu with the preview on top (the hover card's twin: the
// picture or a big type tile, "Image", the full name wrapping, the sender's
// picture and "Arbos · Yesterday 10:06 PM") over Open · Forward · Save /
// Open link · Copy link. A tap opens the viewer (links open in Safari).
// Mock: `media/feedback/library-list/mock-iphone-{list,preview}.png`.

enum LibraryList {
    /// A day shows this many rows before "Show more" (the Mac's `SHOW_MORE_AFTER`).
    static let showMoreAfter = 5
    /// The thumbnail / type tile (the kit's 36 pt leading) and its corners.
    static let thumb: CGFloat = PaneKit.avatar
    static let thumbRadius: CGFloat = 8
    /// The link tile in the long-press preview's picture frame (the 36 pt globe it replaces, as a tile).
    static let previewTile: CGFloat = 72
    /// The preview card: width, picture height, the sender's picture.
    static let previewWidth: CGFloat = 344
    static let previewImageHeight: CGFloat = 206
    static let previewAvatar: CGFloat = 20
    /// Title lines in the card before the ellipsis.
    static let previewTitleLines = 3

    /// One day of the list.
    struct Day: Equatable {
        var label: String
        var items: [ChatItem]
    }

    /// The day's label — the home list's date words: Today, Yesterday, the
    /// weekday inside a week, "Oct 18" after that ("Oct 18, 2025" in another year).
    static func dayLabel(_ date: Date, today: Date = Date(), calendar: Calendar = .current) -> String {
        if calendar.isDate(date, inSameDayAs: today) { return "Today" }
        if let y = calendar.date(byAdding: .day, value: -1, to: today), calendar.isDate(date, inSameDayAs: y) {
            return "Yesterday"
        }
        let start = calendar.startOfDay(for: date)
        let todayStart = calendar.startOfDay(for: today)
        let days = calendar.dateComponents([.day], from: start, to: todayStart).day ?? 0
        let f = DateFormatter()
        f.calendar = calendar
        f.locale = Locale(identifier: "en_US_POSIX")
        if (0..<7).contains(days) {
            f.dateFormat = "EEEE"
        } else if calendar.component(.year, from: date) == calendar.component(.year, from: today) {
            f.dateFormat = "MMM d"
        } else {
            f.dateFormat = "MMM d, yyyy"
        }
        return f.string(from: date)
    }

    /// Items → days, newest first, each day's items newest first.
    static func days(_ items: [ChatItem], now: Date = Date(), calendar: Calendar = .current) -> [Day] {
        var out: [Day] = []
        for item in items.sorted(by: { $0.at > $1.at }) {
            let label = dayLabel(Date(timeIntervalSince1970: Double(item.at) / 1000), today: now, calendar: calendar)
            if out.last?.label == label {
                out[out.count - 1].items.append(item)
            } else {
                out.append(Day(label: label, items: [item]))
            }
        }
        return out
    }

    /// "now", "42m", "13h", "2d", "3w", "4mo", "1y" — the Mac's `rel_time`.
    static func relTime(_ atMs: Int64, now: Int64) -> String {
        let s = max(0, now - atMs) / 1000
        let m = s / 60, h = m / 60, d = h / 24
        if m == 0 { return "now" }
        if h == 0 { return "\(m)m" }
        if d == 0 { return "\(h)h" }
        if d < 7 { return "\(d)d" }
        if d < 30 { return "\(d / 7)w" }
        if d < 365 { return "\(d / 30)mo" }
        return "\(d / 365)y"
    }

    /// "Today 9:39 PM", "Yesterday 10:06 PM", "Mon 3:02 PM", "Oct 3, 11:18 AM" —
    /// the Mac's `sidebar::stamp_label`, for the card's sender line.
    static func stamp(_ atMs: Int64, now: Date = Date(), calendar: Calendar = .current) -> String {
        let date = Date(timeIntervalSince1970: Double(atMs) / 1000)
        let f = DateFormatter()
        f.calendar = calendar
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "h:mm a"
        let time = f.string(from: date)
        if calendar.isDate(date, inSameDayAs: now) { return "Today \(time)" }
        if let y = calendar.date(byAdding: .day, value: -1, to: now), calendar.isDate(date, inSameDayAs: y) {
            return "Yesterday \(time)"
        }
        let days = calendar.dateComponents([.day], from: calendar.startOfDay(for: date), to: calendar.startOfDay(for: now)).day ?? 0
        if (0..<7).contains(days) {
            f.dateFormat = "EEE"
            return "\(f.string(from: date)) \(time)"
        }
        f.dateFormat = calendar.component(.year, from: date) == calendar.component(.year, from: now) ? "MMM d" : "MMM d, yyyy"
        return "\(f.string(from: date)), \(time)"
    }

    /// How many of a day's `count` rows show: all when unfolded or short, else five.
    static func visibleRows(_ count: Int, expanded: Bool) -> Int {
        expanded || count <= showMoreAfter ? count : showMoreAfter
    }

    /// "PDF", "ZIP", "XLSX" — the upper-cased extension when it reads as one.
    static func fileExtension(_ name: String) -> String? {
        guard let dot = name.lastIndex(of: "."), dot < name.index(before: name.endIndex) else { return nil }
        let ext = String(name[name.index(after: dot)...]).uppercased()
        guard (2...4).contains(ext.count), ext.allSatisfy({ $0.isLetter || $0.isNumber }) else { return nil }
        return ext
    }

    /// "Image" · "Video" · "PDF" (the extension) · "File" · "Voice" · "Link".
    static func kindLabel(_ item: ChatItem) -> String {
        switch item.kind {
        case .media: return item.media?.kind == .video ? "Video" : "Image"
        case .file: return fileExtension(item.title) ?? "File"
        case .voice: return "Voice"
        case .link: return "Link"
        }
    }

    /// What the long press offers: attachments open, forward, save; links open and copy.
    static func menuItems(_ item: ChatItem) -> [String] {
        switch item.kind {
        case .media: return ["Open", "Forward", "Save to Photos"]
        case .file: return ["Open", "Forward", "Save"]
        case .voice, .link: return ["Open link", "Copy link"]
        }
    }

    /// The plan of the tab, pure for the test: per day its label, how many rows
    /// show and whether it has a Show more row.
    static func plan(_ items: [ChatItem], expanded: Set<String> = [], now: Date = Date(), calendar: Calendar = .current)
        -> [(label: String, shown: Int, more: Bool)] {
        days(items, now: now, calendar: calendar).map { day in
            (day.label, visibleRows(day.items.count, expanded: expanded.contains(day.label)), day.items.count > showMoreAfter)
        }
    }

    /// `https://www.arbos.life/launch/` → `arbos.life/launch`.
    static func bareURL(_ url: URL) -> String {
        var s = url.absoluteString
        for p in ["https://", "http://"] where s.hasPrefix(p) { s.removeFirst(p.count) }
        if s.hasPrefix("www.") { s.removeFirst(4) }
        while s.hasSuffix("/") { s.removeLast() }
        return s
    }

    /// The tab's sections: a label and a card per day (the pane body draws them).
    @MainActor
    static func sections(items: [ChatItem], empty: String) -> [PaneSection] {
        if items.isEmpty { return [.empty(empty)] }
        // The viewer's album runs in chat order over every attachment; a row
        // opens the viewer at its own place in it.
        let album = items.filter { $0.media != nil }.sorted { $0.at < $1.at }.compactMap(\.media)
        return days(items).flatMap { day -> [PaneSection] in
            [.title(day.label), .custom(AnyView(LibraryDayCard(day: day, album: album).id(day.label)))]
        }
    }
}

/// One day's card: the rows, "Show more" past five, the viewer.
struct LibraryDayCard: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let day: LibraryList.Day
    let album: [MediaItem]
    @State private var expanded = false
    @State private var viewer: MediaViewerState?
    @State private var viewerForward = false
    @State private var viewerDownload = false

    private var shown: Int { LibraryList.visibleRows(day.items.count, expanded: expanded) }

    var body: some View {
        ProfileCard(separatorInset: PaneKit.inset + LibraryList.thumb + PaneKit.gap) {
            ForEach(day.items.prefix(shown)) { item in
                LibraryRow(item: item, open: { open(item) }, forward: { open(item, forward: true) },
                           save: { open(item, download: true) })
            }
            if day.items.count > LibraryList.showMoreAfter {
                Button {
                    withAnimation(.easeOut(duration: 0.18)) { expanded.toggle() }
                } label: {
                    HStack(spacing: 6) {
                        Text(expanded ? "Show less" : "Show more")
                        Image(systemName: expanded ? "chevron.up" : "chevron.down")
                            .font(theme.font(12, weight: .semibold))
                    }
                    .font(theme.font(Theme.body))
                    .foregroundStyle(theme.muted)
                    .padding(.horizontal, PaneKit.inset)
                    .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("library-show-more")
            }
        }
        .fullScreenCover(item: $viewer) { v in
            MediaViewer(album: v.items, index: v.index, onClose: { viewer = nil },
                        openForward: viewerForward, openDownload: viewerDownload)
                .environmentObject(model)
                .environment(\.theme, theme)
        }
    }

    /// A tap opens the album at the row's place; a link opens in Safari.
    private func open(_ item: ChatItem, forward: Bool = false, download: Bool = false) {
        if let media = item.media {
            viewerForward = forward
            viewerDownload = download
            let ix = album.firstIndex(where: { $0.id == media.id })
            viewer = ix.map { MediaViewerState(items: album, index: $0) } ?? MediaViewerState(items: [media], index: 0)
        } else if let url = item.url {
            UIApplication.shared.open(url)
        }
    }
}

/// A row: thumbnail / type tile, the name on one line, the relative time; the
/// long press shows the preview card over the row's actions.
struct LibraryRow: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let item: ChatItem
    let open: () -> Void
    let forward: () -> Void
    let save: () -> Void

    var body: some View {
        Button(action: open) {
            HStack(spacing: PaneKit.gap) {
                LibraryThumb(item: item)
                Text(item.kind == .link ? (item.url.map(LibraryList.bareURL) ?? item.title) : item.title)
                    .font(theme.font(Theme.body, weight: .medium))
                    .foregroundStyle(theme.text)
                    .lineLimit(1)
                    .truncationMode(.tail)
                Spacer(minLength: 8)
                Text(LibraryList.relTime(item.at, now: Int64(Date().timeIntervalSince1970 * 1000)))
                    .font(theme.font(Theme.small))
                    .foregroundStyle(theme.muted)
                    .monospacedDigit()
            }
            .padding(.horizontal, PaneKit.inset)
            .padding(.vertical, 6)
            .frame(minHeight: PaneKit.rowHeight)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(item.title), \(LibraryList.kindLabel(item))")
        .contextMenu {
            switch item.kind {
            case .media, .file:
                Button(action: open) { Label("Open", systemImage: "arrow.up.right.square") }
                Button(action: forward) { Label("Forward", systemImage: "arrowshape.turn.up.right") }
                Button(action: save) {
                    Label(item.kind == .media ? "Save to Photos" : "Save", systemImage: "arrow.down.to.line")
                }
            case .voice, .link:
                Button(action: open) { Label("Open link", systemImage: "safari") }
                if let url = item.url {
                    Button { UIPasteboard.general.string = url.absoluteString } label: {
                        Label("Copy link", systemImage: "link")
                    }
                }
            }
        } preview: {
            LibraryPreview(item: item)
                .environmentObject(model)
                .environment(\.theme, theme)
        }
    }
}

/// The 36 pt leading: the picture (aspect-fill), a film glyph for a video, the
/// extension on a tile for files, a waveform for voice notes, the link glyph.
struct LibraryThumb: View {
    @Environment(\.theme) private var theme
    let item: ChatItem

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: LibraryList.thumbRadius, style: .continuous).fill(theme.field)
            switch item.kind {
            case .media:
                if let media = item.media, media.kind == .image {
                    BlobImage(item: media)
                } else {
                    Image(systemName: "play.fill").font(theme.font(13, weight: .semibold)).foregroundStyle(theme.muted)
                }
            case .file:
                if let ext = LibraryList.fileExtension(item.title) {
                    Text(ext).font(theme.font(11, weight: .semibold)).foregroundStyle(theme.muted)
                } else {
                    Image(systemName: "doc.fill").font(theme.font(15, weight: .medium)).foregroundStyle(theme.muted)
                }
            case .voice:
                Image(systemName: "waveform").font(theme.font(15, weight: .medium)).foregroundStyle(theme.muted)
            case .link:
                // The one link tile ([feedback link-icons-ios-adopt-2026-10-27]).
                LinkRowTile(url: item.url, size: LibraryList.thumb)
            }
        }
        .frame(width: LibraryList.thumb, height: LibraryList.thumb)
        .clipShape(RoundedRectangle(cornerRadius: LibraryList.thumbRadius, style: .continuous))
    }
}

/// The long-press preview: the picture (or the big type tile), "Image", the
/// full name, the sender with picture and "Arbos · Yesterday 10:06 PM".
struct LibraryPreview: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let item: ChatItem

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ZStack {
                Rectangle().fill(theme.field)
                switch item.kind {
                case .media:
                    if let media = item.media, media.kind == .image {
                        BlobImage(item: media)
                    } else {
                        Image(systemName: "play.fill").font(.system(size: 36)).foregroundStyle(theme.muted)
                    }
                case .file:
                    if let ext = LibraryList.fileExtension(item.title) {
                        Text(ext).font(theme.font(26, weight: .semibold)).foregroundStyle(theme.muted)
                    } else {
                        Image(systemName: "doc.fill").font(.system(size: 36)).foregroundStyle(theme.muted)
                    }
                case .voice:
                    Image(systemName: "waveform").font(.system(size: 36)).foregroundStyle(theme.muted)
                case .link:
                    LinkRowTile(url: item.url, size: LibraryList.previewTile)
                }
            }
            .frame(width: LibraryList.previewWidth, height: LibraryList.previewImageHeight)
            .clipped()
            VStack(alignment: .leading, spacing: 3) {
                Text(LibraryList.kindLabel(item))
                    .font(theme.font(Theme.tiny))
                    .foregroundStyle(theme.muted)
                Text(item.kind == .link ? (item.url.map(LibraryList.bareURL) ?? item.title) : item.title)
                    .font(theme.font(16, weight: .semibold))
                    .foregroundStyle(theme.text)
                    .lineLimit(LibraryList.previewTitleLines)
                    .fixedSize(horizontal: false, vertical: true)
                HStack(spacing: 8) {
                    senderPicture
                    (Text(item.by.isEmpty ? (item.fromHandle.map { "@\($0)" } ?? "") : item.by)
                        .font(theme.font(Theme.tiny, weight: .medium)).foregroundColor(theme.text)
                        + Text(" · \(LibraryList.stamp(item.at))")
                        .font(theme.font(Theme.tiny)).foregroundColor(theme.muted))
                        .lineLimit(1)
                }
                .padding(.top, 5)
            }
            .padding(.horizontal, 14)
            .padding(.top, 12)
            .padding(.bottom, 14)
        }
        .frame(width: LibraryList.previewWidth)
        .background(theme.bubbleAssistant)
        .accessibilityIdentifier("library-preview")
    }

    @ViewBuilder private var senderPicture: some View {
        if let id = item.agentId, let project = model.project(id) {
            AgentAvatar(project: project, theme: theme, size: LibraryList.previewAvatar, still: true)
        } else {
            PersonAvatar(url: item.fromAvatar, handle: item.fromHandle ?? "", size: LibraryList.previewAvatar,
                         name: item.by.isEmpty ? nil : item.by, id: item.fromId)
        }
    }
}
