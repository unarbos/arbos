// Emoji · Stickers · GIFs picker (ARB-544, Jacob 2026-10-28, option B): the `+` attach sheet
// gains a third row, "Emoji, stickers & GIFs", which opens this sheet. Mock:
// `media/feedback/emoji-sticker-gif/sheet-picker-mock-b.png`.
//
// - Emoji: search, Recents, one Unicode category at a time, a skin-tone swatch. A pick goes
//   into the composer draft; the sheet stays open.
// - Stickers: pack switcher (the bundled Fluent 3D starter first, then the relay's packs), a
//   4-column grid, the pack's licence line in the footer. A tap sends at once.
// - GIFs: "Search KLIPY" field (the provider's required attribution), term chips, a 2-column
//   grid, "Powered by KLIPY" + the privacy line in the footer. A tap sends at once. Without a
//   relay key: "GIFs are not set up yet"; over the hourly budget: "GIFs are busy, try again
//   shortly." — never a retry storm.
//
// Emoji glyphs are the system font (Apple Color Emoji): no art asset, no licence.

import SwiftUI

// MARK: - Picker data (relay shapes)

/// One GIF of the relay's `GET /gif/search` page: the grid shows `preview`, the message
/// carries `url` + attribution.
struct GifItem: Codable, Equatable, Identifiable {
    struct Preview: Codable, Equatable {
        var url: String
        var width: Int
        var height: Int
    }
    var id: String
    var title: String
    var width: Int
    var height: Int
    var url: String
    var preview: Preview
    var pageUrl: String
    /// The search that found it (for the provider's share signal); not on the wire.
    var query: String = ""

    enum CodingKeys: String, CodingKey { case id, title, width, height, url, preview, pageUrl }
}

struct GifPageDto: Decodable {
    var items: [GifItem]
    var next: String?
}

/// A page of GIF results, or why there is none.
enum GifPage: Equatable {
    case items([GifItem], next: String?)
    /// The relay has no provider key (`503 gif_unavailable`).
    case unavailable
    /// Our budget or the provider's rate limit (`429 gif_busy`).
    case busy
    case failed
}

/// One sticker pack of the relay's manifest (`GET /stickers/packs`).
struct StickerPack: Codable, Equatable, Identifiable {
    var id: String
    var name: String
    var publisher: String
    var license: String
    var licenseUrl: String
    var attribution: String
    var animated: Bool
    var pageUrl: String
    var mime: String
    var items: [StickerItem]
}

struct StickerItem: Codable, Equatable, Identifiable {
    var id: String
    var name: String
    var emoji: String
    var mime: String
    /// Relay path (`/stickers/{pack}/{id}`), or empty for a bundled starter sticker.
    var path: String
}

struct StickerPacksDto: Decodable {
    var packs: [StickerPack]
}

/// What one tap sends.
enum MediaPick {
    case sticker(pack: StickerPack, item: StickerItem, data: Data)
    case gif(GifItem)
}

// MARK: - Bundled data (Resources/Fun)

/// One emoji of `Fun/emoji.json` (built by `scripts/fun/build-emoji-and-packs.py`).
struct EmojiEntry: Decodable, Identifiable, Equatable {
    /// The character(s).
    var e: String
    /// CLDR short name, lower-case.
    var n: String
    /// Category id (`smileys`, `people`, …).
    var c: String
    /// Takes a skin tone.
    var t: Int?
    var id: String { e }
}

enum EmojiCatalog {
    private struct File: Decodable { var emoji: [EmojiEntry] }

    /// Category strip: id, label, glyph.
    static let categories: [(id: String, label: String, glyph: String)] = [
        ("smileys", "Smileys & People", "😀"),
        ("people", "People & Body", "👋"),
        ("nature", "Animals & Nature", "🐻"),
        ("food", "Food & Drink", "🍔"),
        ("travel", "Travel & Places", "🚗"),
        ("activity", "Activities", "⚽"),
        ("objects", "Objects", "💡"),
        ("symbols", "Symbols", "🔣"),
        ("flags", "Flags", "🏳️"),
    ]

    static let all: [EmojiEntry] = {
        guard let url = Bundle.main.url(forResource: "emoji", withExtension: "json", subdirectory: "Fun"),
              let data = try? Data(contentsOf: url),
              let file = try? JSONDecoder().decode(File.self, from: data) else { return [] }
        return file.emoji
    }()

    /// Fitzpatrick modifiers, in swatch order (0 = none).
    static let tones: [Character] = ["\u{1F3FB}", "\u{1F3FC}", "\u{1F3FD}", "\u{1F3FE}", "\u{1F3FF}"]
    static let toneSwatches: [Color] = [
        Color(red: 0.95, green: 0.76, blue: 0.49), Color(red: 0.97, green: 0.85, blue: 0.72),
        Color(red: 0.88, green: 0.67, blue: 0.41), Color(red: 0.78, green: 0.53, blue: 0.26),
        Color(red: 0.55, green: 0.33, blue: 0.14), Color(red: 0.29, green: 0.18, blue: 0.10),
    ]

    /// `entry` with skin tone `tone` (1–5) when it takes one: the modifier goes right after
    /// the first scalar (a ZWJ sequence keeps the rest), replacing a FE0F there.
    static func withTone(_ entry: EmojiEntry, tone: Int) -> String {
        guard (entry.t ?? 0) != 0, tone > 0, tone <= tones.count else { return entry.e }
        var scalars = Array(entry.e.unicodeScalars)
        guard let first = scalars.first else { return entry.e }
        scalars.removeFirst()
        if scalars.first == "\u{FE0F}" { scalars.removeFirst() }
        var out = String.UnicodeScalarView()
        out.append(first)
        out.append(contentsOf: tones[tone - 1].unicodeScalars)
        out.append(contentsOf: scalars)
        return String(out)
    }

    /// Emoji whose name starts with `query`, then those with a word starting with it.
    static func search(_ query: String, limit: Int = 64) -> [EmojiEntry] {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !q.isEmpty else { return [] }
        var starts: [EmojiEntry] = []
        var words: [EmojiEntry] = []
        for e in all {
            if e.n.hasPrefix(q) {
                starts.append(e)
            } else if e.n.split(whereSeparator: { $0 == " " || $0 == "-" || $0 == ":" }).contains(where: { $0.hasPrefix(q) }) {
                words.append(e)
            }
            if starts.count >= limit { break }
        }
        return Array((starts + words).prefix(limit))
    }
}

/// The bundled Fluent 3D starter (48 stickers at 160 px, `Fun/starter/`).
enum StarterPack {
    private struct Index: Decodable {
        struct Item: Decodable { var id: String; var name: String; var emoji: String; var file: String }
        var pack: String
        var items: [Item]
    }

    static let packId = "fluent-3d"

    static let pack: StickerPack = {
        var pack = StickerPack(id: packId, name: "Fluent 3D", publisher: "Microsoft", license: "MIT",
                               licenseUrl: "https://github.com/microsoft/fluentui-emoji/blob/main/LICENSE",
                               attribution: "Fluent Emoji 3D · Microsoft · MIT", animated: false,
                               pageUrl: "https://github.com/microsoft/fluentui-emoji", mime: "image/png", items: [])
        guard let url = Bundle.main.url(forResource: "index", withExtension: "json", subdirectory: "Fun/starter"),
              let data = try? Data(contentsOf: url),
              let index = try? JSONDecoder().decode(Index.self, from: data) else { return pack }
        pack.items = index.items.map { StickerItem(id: $0.id, name: $0.name, emoji: $0.emoji, mime: "image/png", path: "") }
        return pack
    }()

    static func data(for item: StickerItem) -> Data? {
        guard let url = Bundle.main.url(forResource: item.id, withExtension: "png", subdirectory: "Fun/starter") else { return nil }
        return try? Data(contentsOf: url)
    }

    static func image(for item: StickerItem) -> UIImage? {
        data(for: item).flatMap(UIImage.init(data:))
    }
}

// MARK: - The sticker row in the transcript

/// A picker sticker in the thread: the picture alone, fixed square, no bubble (Mac 128 pt,
/// iPhone 160 pt). Tap opens the viewer like any picture.
struct StickerBubble: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    let item: MediaItem
    let chatId: Id

    static let size: CGFloat = 160

    @State private var image: UIImage?
    @State private var failed = false
    @State private var viewer: MediaViewerState?

    var body: some View {
        ZStack {
            if let image {
                Image(uiImage: image).resizable().scaledToFit()
            } else if failed {
                Image(systemName: "photo").font(theme.font(28)).foregroundStyle(theme.muted)
            } else {
                RoundedRectangle(cornerRadius: Theme.bubbleRadius, style: .continuous)
                    .fill(theme.bubbleAssistant.opacity(0.6))
                    .frame(width: Self.size * 0.6, height: Self.size * 0.6)
            }
        }
        .frame(width: Self.size, height: Self.size)
        .contentShape(Rectangle())
        // A plain tap, no drag gesture of its own: the system's interactive edge pop
        // (#2239) owns horizontal swipes, same as `ImageBubble`.
        .onTapGesture {
            viewer = MediaOpen.state(for: item, chatId: chatId, model: model)
        }
        .accessibilityIdentifier("sticker-bubble")
        .accessibilityLabel("Sticker")
        .task(id: item.cacheKey) { await load() }
        .fullScreenCover(item: $viewer) { v in
            MediaViewer(album: v.items, index: v.index, onClose: { viewer = nil })
                .environmentObject(model)
                .environment(\.theme, theme)
        }
    }

    private func load() async {
        if let ref = item.cacheKey, let hit = ChatImageCache.decoded(for: ref) {
            image = hit.image
            return
        }
        guard let (data, _) = await MediaFetch.bytes(for: item, model: model),
              let decoded = await ChatImageCache.decode(data, maxPixels: CGSize(width: 512, height: 512)) else {
            failed = true
            return
        }
        if let ref = item.cacheKey { ChatImageCache.store(decoded, for: ref) }
        image = decoded.image
    }
}

// MARK: - The picker sheet

enum FunTab: String, CaseIterable, Identifiable {
    case emoji = "Emoji"
    case stickers = "Stickers"
    case gifs = "GIFs"
    var id: String { rawValue }
}

/// The picker: three tabs in the pane tab-pill style, opened from the attach sheet's third row.
struct FunPickerSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    let chatId: Id
    /// An emoji pick: goes into the composer draft (the sheet stays open).
    let onEmoji: (String) -> Void
    /// A sticker / GIF pick: sent at once by the caller; the sheet closes.
    let onSend: (MediaPick) -> Void

    @State private var tab: FunTab = .emoji
    /// `PaneTabs` binds to the label; mirrored into `tab`.
    @State private var tabLabel: String = FunTab.emoji.rawValue
    @State private var query = ""
    @State private var category = 0
    @State private var tone = 0
    @AppStorage("fun.recentEmoji") private var recentsRaw = ""
    // Stickers
    @State private var relayPacks: [StickerPack]? = nil
    @State private var packsFailed = false
    @State private var selectedPack: String = StarterPack.packId
    @State private var stickerPages = 1
    @State private var stickerImages: [String: UIImage] = [:]
    @State private var stickerData: [String: Data] = [:]
    // GIFs
    @State private var gifPage: GifPage? = nil
    @State private var gifQuery = ""
    @State private var gifTask: Task<Void, Never>? = nil
    @State private var gifPreviews: [String: UIImage] = [:]
    @State private var loadingMore = false

    private static let recentsCap = 16
    private static let stickerPage = 48
    private static let gifChips = ["Trending", "happy", "lol", "yes", "no", "love", "dance", "wow", "thanks"]

    private var recents: [String] { recentsRaw.isEmpty ? [] : recentsRaw.components(separatedBy: "\u{1F}") }

    var body: some View {
        VStack(spacing: 0) {
            header
            switch tab {
            case .emoji: emojiTab
            case .stickers: stickersTab
            case .gifs: gifsTab
            }
        }
        .background(theme.bg)
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .presentationBackground(theme.bg)
        .onChange(of: tabLabel) { _, new in
            guard let t = FunTab(rawValue: new), t != tab else { return }
            tab = t
            query = ""
            if t == .stickers { Task { await loadPacks() } }
            if t == .gifs, gifPage == nil { searchGifs("") }
        }
    }

    private var header: some View {
        HStack {
            PaneTabs(tabs: FunTab.allCases.map(\.rawValue), selected: $tabLabel, hitAbove: 0, hitBelow: 0)
                .frame(maxWidth: 260)
            Spacer()
            Button { dismiss() } label: {
                GlassCircle { Image(systemName: "xmark").font(.system(size: 15, weight: .medium)).foregroundStyle(theme.text) }
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Close")
        }
        .padding(.horizontal, 16)
        .padding(.top, 14)
        .padding(.bottom, 8)
    }

    private func searchField(_ placeholder: String) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").font(.system(size: 15)).foregroundStyle(theme.muted)
            TextField(placeholder, text: $query)
                .font(theme.font(Theme.body))
                .foregroundStyle(theme.text)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.search)
                .accessibilityIdentifier("fun-search")
            if !query.isEmpty {
                Button { query = "" } label: {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(theme.muted)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
            }
        }
        // The control family's field (Controls.height, ControlSurface, the family rim).
        .controlField()
        .padding(.horizontal, 16)
    }

    private func footer<L: View, R: View>(@ViewBuilder left: () -> L, @ViewBuilder right: () -> R) -> some View {
        HStack {
            left()
            Spacer()
            right()
        }
        .font(theme.font(Theme.tiny))
        .foregroundStyle(theme.muted)
        .padding(.horizontal, 16)
        .frame(height: 36)
        .overlay(alignment: .top) { Rectangle().fill(theme.fieldBorder).frame(height: 1) }
    }

    private func sectionLabel(_ text: String) -> some View {
        Text(text.uppercased())
            .font(theme.font(11, weight: .semibold))
            .foregroundStyle(theme.muted)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.top, 10)
            .padding(.bottom, 2)
    }

    // MARK: Emoji

    private var emojiTab: some View {
        let cols = Array(repeating: GridItem(.flexible(), spacing: 0), count: 8)
        let q = query.trimmingCharacters(in: .whitespaces)
        return VStack(spacing: 0) {
            searchField("Search emoji")
            ScrollView {
                LazyVStack(spacing: 0) {
                    if !q.isEmpty {
                        let hits = EmojiCatalog.search(q)
                        sectionLabel(hits.isEmpty ? "No emoji match" : "Results")
                        emojiGrid(hits.map { EmojiCatalog.withTone($0, tone: tone) }, columns: cols)
                    } else {
                        if !recents.isEmpty {
                            sectionLabel("Recents")
                            emojiGrid(recents, columns: cols)
                        }
                        let cat = EmojiCatalog.categories[min(category, EmojiCatalog.categories.count - 1)]
                        sectionLabel(cat.label)
                        emojiGrid(EmojiCatalog.all.filter { $0.c == cat.id }.map { EmojiCatalog.withTone($0, tone: tone) },
                                  columns: cols)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 8)
            }
            footer {
                HStack(spacing: 2) {
                    ForEach(Array(EmojiCatalog.categories.enumerated()), id: \.offset) { i, cat in
                        Button {
                            category = i
                            query = ""
                        } label: {
                            Text(cat.glyph)
                                .font(.system(size: 17))
                                .opacity(i == category && q.isEmpty ? 1 : 0.5)
                                .frame(width: 30, height: 30)
                                .background(RoundedRectangle(cornerRadius: 6).fill(i == category && q.isEmpty ? theme.field : .clear))
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel(cat.label)
                    }
                }
            } right: {
                Button {
                    tone = (tone + 1) % (EmojiCatalog.tones.count + 1)
                } label: {
                    HStack(spacing: 4) {
                        Text("Tone")
                        Circle().fill(EmojiCatalog.toneSwatches[min(tone, EmojiCatalog.toneSwatches.count - 1)])
                            .frame(width: 14, height: 14)
                            .overlay(Circle().strokeBorder(theme.muted.opacity(0.4), lineWidth: 1))
                    }
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Skin tone")
            }
        }
    }

    private func emojiGrid(_ glyphs: [String], columns: [GridItem]) -> some View {
        LazyVGrid(columns: columns, spacing: 0) {
            ForEach(Array(glyphs.enumerated()), id: \.offset) { _, glyph in
                Button { pickEmoji(glyph) } label: {
                    Text(glyph).font(.system(size: 28)).frame(height: 42).frame(maxWidth: .infinity)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(glyph)
            }
        }
    }

    private func pickEmoji(_ glyph: String) {
        var list = recents.filter { $0 != glyph }
        list.insert(glyph, at: 0)
        recentsRaw = list.prefix(Self.recentsCap).joined(separator: "\u{1F}")
        Haptics.pick()
        onEmoji(glyph)
    }

    // MARK: Stickers

    private var allPacks: [StickerPack] {
        var packs = relayPacks ?? []
        if !packs.contains(where: { $0.id == StarterPack.packId }) { packs.insert(StarterPack.pack, at: 0) }
        return packs
    }

    private var stickersTab: some View {
        let packs = allPacks
        let pack = packs.first { $0.id == selectedPack } ?? packs.first ?? StarterPack.pack
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        let shown = pack.items.filter { q.isEmpty || $0.name.contains(q) }
        let page = Array(shown.prefix(Self.stickerPage * stickerPages))
        let cols = Array(repeating: GridItem(.flexible(), spacing: 6), count: 4)
        return VStack(spacing: 0) {
            HStack(spacing: 6) {
                ForEach(packs) { p in
                    Button {
                        selectedPack = p.id
                        stickerPages = 1
                    } label: {
                        ZStack {
                            RoundedRectangle(cornerRadius: 12, style: .continuous)
                                .fill(p.id == pack.id ? theme.field : .clear)
                            packIcon(p)
                        }
                        .frame(width: 52, height: 52)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(p.name)
                }
                Spacer()
            }
            .padding(.horizontal, 12)
            .padding(.top, 4)
            searchField("Search stickers")
                .padding(.top, 8)
            ScrollView {
                LazyVGrid(columns: cols, spacing: 6) {
                    ForEach(page) { item in
                        Button { pickSticker(pack: pack, item: item) } label: {
                            ZStack {
                                if let img = stickerImage(pack: pack, item: item) {
                                    Image(uiImage: img).resizable().scaledToFit().frame(width: 84, height: 84)
                                } else {
                                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                                        .fill(theme.field).frame(width: 52, height: 52)
                                        .task(id: item.id) { await loadSticker(pack: pack, item: item) }
                                }
                            }
                            .frame(maxWidth: .infinity)
                            .frame(height: 96)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel(item.name)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 8)
                if shown.count > page.count {
                    Button("More") { stickerPages += 1 }
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                        .padding(.vertical, 10)
                }
            }
            footer {
                Text(pack.attribution)
            } right: {
                Text(relayPacks == nil && !packsFailed ? "Loading packs…" : "Tap to send")
            }
        }
        .task { await loadPacks() }
    }

    @ViewBuilder
    private func packIcon(_ p: StickerPack) -> some View {
        if let first = p.items.first, let img = stickerImage(pack: p, item: first) {
            Image(uiImage: img).resizable().scaledToFit().frame(width: 36, height: 36)
        } else if let first = p.items.first {
            Text(first.emoji).font(.system(size: 26))
        } else {
            Image(systemName: "face.smiling").font(.system(size: 22)).foregroundStyle(theme.muted)
        }
    }

    private func stickerKey(pack: StickerPack, item: StickerItem) -> String { "\(pack.id)/\(item.id)" }

    private func stickerImage(pack: StickerPack, item: StickerItem) -> UIImage? {
        if let img = stickerImages[stickerKey(pack: pack, item: item)] { return img }
        if pack.id == StarterPack.packId, StarterPack.pack.items.contains(where: { $0.id == item.id }) {
            return StarterPack.image(for: item)
        }
        return nil
    }

    private func loadPacks() async {
        guard relayPacks == nil, !packsFailed, !model.isFixture else { return }
        if let packs = await model.relay.stickerPacks() {
            relayPacks = packs
        } else {
            packsFailed = true
        }
    }

    private func loadSticker(pack: StickerPack, item: StickerItem) async {
        let key = stickerKey(pack: pack, item: item)
        guard stickerImages[key] == nil, !item.path.isEmpty else { return }
        guard let data = await model.relay.stickerData(path: item.path), let img = UIImage(data: data) else { return }
        stickerData[key] = data
        stickerImages[key] = img
    }

    private func pickSticker(pack: StickerPack, item: StickerItem) {
        let key = stickerKey(pack: pack, item: item)
        let starterData = pack.id == StarterPack.packId ? StarterPack.data(for: item) : nil
        guard let data = stickerData[key] ?? starterData else { return }
        Haptics.send()
        onSend(.sticker(pack: pack, item: item, data: data))
        dismiss()
    }

    // MARK: GIFs

    private var gifsTab: some View {
        VStack(spacing: 0) {
            // KLIPY's required attribution: the search placeholder.
            searchField("Search KLIPY")
                .onChange(of: query) { _, new in
                    guard tab == .gifs else { return }
                    gifTask?.cancel()
                    gifTask = Task {
                        try? await Task.sleep(nanoseconds: 400_000_000)
                        guard !Task.isCancelled else { return }
                        searchGifs(new)
                    }
                }
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(Self.gifChips, id: \.self) { chip in
                        let term = chip == "Trending" ? "" : chip
                        let on = gifQuery.caseInsensitiveCompare(term) == .orderedSame
                        Button {
                            query = term
                            searchGifs(term)
                        } label: {
                            Text(chip)
                                .font(theme.font(Theme.tiny, weight: .medium))
                                .foregroundStyle(on ? theme.bg : theme.text)
                                .padding(.horizontal, 12)
                                .frame(height: 30)
                                .background(Capsule().fill(on ? theme.text : theme.field))
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 16)
            }
            .padding(.top, 10)
            gifBody
            footer {
                HStack(spacing: 6) {
                    RoundedRectangle(cornerRadius: 2).fill(Color(red: 0.42, green: 0.36, blue: 0.9)).frame(width: 10, height: 14)
                    Text("Powered by KLIPY").fontWeight(.semibold).foregroundStyle(theme.text)
                }
            } right: {
                Text("Searches are sent to KLIPY.")
            }
        }
        .task { if gifPage == nil { searchGifs("") } }
    }

    @ViewBuilder
    private var gifBody: some View {
        switch gifPage {
        case .none:
            Spacer()
            ProgressView().tint(theme.muted)
            Spacer()
        case .unavailable:
            note("GIFs are not set up yet.", detail: "The server has no GIF provider key.")
        case .busy:
            note("GIFs are busy, try again shortly.", detail: nil)
        case .failed:
            note("GIFs could not be loaded.", detail: "Check the connection and try again.")
        case .items(let items, _) where items.isEmpty:
            note("No GIFs for that.", detail: nil)
        case .items(let items, let next):
            let colW = (UIScreen.main.bounds.width - 32 - 6) / 2
            ScrollView {
                HStack(alignment: .top, spacing: 6) {
                    ForEach(0..<2, id: \.self) { col in
                        LazyVStack(spacing: 6) {
                            ForEach(Array(items.enumerated()).filter { $0.offset % 2 == col }, id: \.element.id) { _, gif in
                                gifTile(gif, width: colW)
                            }
                        }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 10)
                if next != nil {
                    Button(loadingMore ? "Loading…" : "More") { loadMoreGifs() }
                        .disabled(loadingMore)
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                        .padding(.vertical, 10)
                }
            }
        }
    }

    private func note(_ line: String, detail: String?) -> some View {
        VStack(spacing: 4) {
            Spacer()
            Text(line).font(theme.font(Theme.small)).foregroundStyle(theme.muted)
            if let detail { Text(detail).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted.opacity(0.8)) }
            Spacer()
        }
        .frame(maxWidth: .infinity)
        .padding(.horizontal, 24)
        .multilineTextAlignment(.center)
    }

    private func gifTile(_ gif: GifItem, width: CGFloat) -> some View {
        let ratio = gif.preview.width > 0 && gif.preview.height > 0
            ? CGFloat(gif.preview.height) / CGFloat(gif.preview.width) : 1
        let h = min(max(width * ratio, 70), 260)
        return Button {
            Haptics.send()
            var picked = gif
            picked.query = gifQuery
            onSend(.gif(picked))
            dismiss()
        } label: {
            ZStack {
                RoundedRectangle(cornerRadius: 10, style: .continuous).fill(theme.field)
                if let img = gifPreviews[gif.id] {
                    Image(uiImage: img).resizable().scaledToFill()
                } else {
                    ProgressView().tint(theme.muted)
                        .task(id: gif.id) { await loadPreview(gif) }
                }
            }
            .frame(width: width, height: h)
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(gif.title)
    }

    private func loadPreview(_ gif: GifItem) async {
        guard gifPreviews[gif.id] == nil, let url = model.gifMediaURL(gif.preview.url) else { return }
        guard let (data, _) = await MediaFetch.data(url, headers: model.authHeaders) else { return }
        // The grid shows the first frame; the thread animates the sent GIF.
        if let img = UIImage(data: data) { gifPreviews[gif.id] = img }
    }

    private func searchGifs(_ text: String) {
        let q = text.trimmingCharacters(in: .whitespacesAndNewlines)
        gifQuery = q
        gifPage = nil
        Task {
            let page = await model.relay.gifSearch(query: q, pos: nil)
            guard gifQuery == q else { return }
            gifPage = page
        }
    }

    private func loadMoreGifs() {
        guard case .items(let items, let next) = gifPage, let pos = next, !loadingMore else { return }
        loadingMore = true
        let q = gifQuery
        Task {
            let page = await model.relay.gifSearch(query: q, pos: pos)
            loadingMore = false
            guard gifQuery == q else { return }
            if case .items(let more, let n) = page {
                gifPage = .items(items + more, next: n)
            } else {
                gifPage = .items(items, next: nil)
            }
        }
    }
}
