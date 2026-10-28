import SwiftUI
import UIKit

// The one link-icon tile ([feedback link-icons-ios-tile-2026-10-27]; the Mac's
// `ui/link_tile.rs`, same table in `docs/mac-design-rules.md` § Link icons): every
// place the iPhone shows a site's icon on a tile — a person's Links tab, the
// Library's Links rows, search results' links, the link-embed cards — draws it
// from here, so size, corner, fill and the mark's clear space are decided once
// and read the same in the dark and in the light.
//
// Jacob's report (2026-10-27, Mac dark): the Spotify row had a crisp green brand
// mark, the github.com row a blurry 16 px favicon stretched over the tile, black
// octocat on a dark grey square. Three rules:
//
// 1. A known host draws a bundled vector mark (`brand-<slug>` in the asset
//    catalog, one filled path, template rendering), never a fetched picture; a
//    black-and-white brand takes the theme's text colour (white on the dark tile,
//    near-black on the light), a brand with a colour of its own keeps it.
// 2. An unknown host draws the relay's high-resolution icon edge to edge; a
//    transparent dark mark gets a light fill in the dark theme (a light mark a
//    dark fill in the light), by its `LinkIconTone`.
// 3. No icon → the host's first letter in the muted text colour on the plain tile.

/// A brand whose mark ships with the app. Sources (the marks' press pages, whose
/// terms we follow: unaltered shape, mono or the published colour, clear space):
/// GitHub github.com/logos · X about.x.com brand toolkit · YouTube brand resources ·
/// Spotify developer.spotify.com/documentation/design · Apple Music identity
/// guidelines · SoundCloud press.soundcloud.com · Vimeo press.vimeo.com ·
/// Loom loom.com/press · Bluesky bsky.social press FAQ · Reddit redditinc.com/brand ·
/// Google about.google/brand-resource-center · Figma using-the-figma-brand ·
/// Notion notion.so/press · Linear linear.app/brand · Wikipedia Wikimedia Commons ·
/// Arbos: our own mark (`site/assets/logo/arbos-mark.svg`).
enum LinkBrand: String, CaseIterable {
    case github, x, youtube, spotify, appleMusic = "apple-music", soundcloud, vimeo, loom, bluesky, reddit
    case googleDocs = "google-docs", googleSheets = "google-sheets", googleSlides = "google-slides", googleMaps = "google-maps"
    case figma, notion, linear, wikipedia, arbos

    /// How the mark takes colour on the tile.
    enum Ink: Equatable {
        /// A black-and-white brand: the theme's text colour.
        case mono
        /// The brand's own colour (sRGB hex), the same on both tiles.
        case color(UInt32)
    }

    /// The brand for a URL — host first, then the path for hosts that serve several
    /// products (`docs.google.com/document` vs `/spreadsheets` vs `/presentation`,
    /// `google.com/maps`). `nil` for every other site.
    static func of(url: URL) -> LinkBrand? {
        guard let rawHost = url.host?.lowercased() else { return nil }
        let host = rawHost.hasPrefix("www.") ? String(rawHost.dropFirst(4)) : rawHost
        let path = url.path.lowercased().drop(while: { $0 == "/" })
        if host == "docs.google.com" {
            if path.hasPrefix("spreadsheets") { return .googleSheets }
            if path.hasPrefix("presentation") { return .googleSlides }
            return .googleDocs
        }
        if host == "google.com", path.hasPrefix("maps") { return .googleMaps }
        return of(host: host)
    }

    /// The brand for a bare host, `www.` or not. `docs.google.com` alone reads as
    /// Google Docs; `of(url:)` tells the three Google editors apart.
    static func of(host rawHost: String) -> LinkBrand? {
        let lower = rawHost.trimmingCharacters(in: .whitespaces).lowercased()
        let host = lower.hasPrefix("www.") ? String(lower.dropFirst(4)) : lower
        func ends(_ suffix: String) -> Bool { host == suffix || host.hasSuffix("." + suffix) }
        switch host {
        case "github.com", "gist.github.com", "raw.githubusercontent.com": return .github
        case "x.com", "twitter.com", "t.co", "mobile.twitter.com": return .x
        case "youtube.com", "youtu.be", "m.youtube.com", "music.youtube.com": return .youtube
        case "open.spotify.com", "spotify.com", "play.spotify.com": return .spotify
        case "music.apple.com", "itunes.apple.com": return .appleMusic
        case "soundcloud.com", "on.soundcloud.com", "m.soundcloud.com": return .soundcloud
        case "vimeo.com", "player.vimeo.com": return .vimeo
        case "loom.com": return .loom
        case "bsky.app", "bsky.social": return .bluesky
        case "reddit.com", "old.reddit.com", "redd.it", "np.reddit.com": return .reddit
        case "docs.google.com": return .googleDocs
        case "sheets.google.com": return .googleSheets
        case "slides.google.com": return .googleSlides
        case "maps.google.com", "maps.app.goo.gl": return .googleMaps
        case "figma.com": return .figma
        case "notion.so", "notion.site", "notion.com": return .notion
        case "linear.app": return .linear
        case "wikipedia.org", "wikimedia.org": return .wikipedia
        case "arbos.life": return .arbos
        default:
            if ends("wikipedia.org") { return .wikipedia }
            if ends("notion.site") { return .notion }
            if ends("arbos.life") { return .arbos }
            if ends("loom.com") { return .loom }
            if ends("figma.com") { return .figma }
            if ends("linear.app") { return .linear }
            return nil
        }
    }

    /// The asset-catalog image (`brand-<slug>`).
    var assetName: String { "brand-" + rawValue }

    /// The brand's display name (the provider line on an embed card).
    var name: String {
        switch self {
        case .github: return "GitHub"
        case .x: return "X"
        case .youtube: return "YouTube"
        case .spotify: return "Spotify"
        case .appleMusic: return "Apple Music"
        case .soundcloud: return "SoundCloud"
        case .vimeo: return "Vimeo"
        case .loom: return "Loom"
        case .bluesky: return "Bluesky"
        case .reddit: return "Reddit"
        case .googleDocs: return "Google Docs"
        case .googleSheets: return "Google Sheets"
        case .googleSlides: return "Google Slides"
        case .googleMaps: return "Google Maps"
        case .figma: return "Figma"
        case .notion: return "Notion"
        case .linear: return "Linear"
        case .wikipedia: return "Wikipedia"
        case .arbos: return "Arbos"
        }
    }

    /// The brand's own colour where it has one (the hex its press page publishes),
    /// mono for the black-and-white brands.
    var ink: Ink {
        switch self {
        case .github, .x, .notion, .wikipedia, .arbos: return .mono
        case .youtube: return .color(0xFF0000)
        case .spotify: return .color(0x1ED760)
        case .appleMusic: return .color(0xFA243C)
        case .soundcloud: return .color(0xFF5500)
        case .vimeo: return .color(0x1AB7EA)
        case .loom: return .color(0x625DF5)
        case .bluesky: return .color(0x1185FE)
        case .reddit: return .color(0xFF4500)
        case .googleDocs, .googleMaps: return .color(0x4285F4)
        case .googleSheets: return .color(0x34A853)
        case .googleSlides: return .color(0xFBBC04)
        case .figma: return .color(0xF24E1E)
        case .linear: return .color(0x5E6AD2)
        }
    }

    /// The colour the mark is painted in on this theme's tile.
    func color(_ theme: Theme) -> Color {
        switch ink {
        case .mono: return theme.text
        case .color(let hex): return Color(red: Double((hex >> 16) & 0xFF) / 255, green: Double((hex >> 8) & 0xFF) / 255, blue: Double(hex & 0xFF) / 255)
        }
    }
}

/// What a fetched icon looks like against a fill, measured once from its pixels.
enum LinkIconTone: Equatable {
    /// Opaque, or a mid-tone mark: fine on the theme's own tile fill.
    case neutral
    /// A transparent picture whose paint is dark: needs a light fill.
    case darkMark
    /// A transparent picture whose paint is light: needs a dark fill.
    case lightMark

    /// A picture is a mark (not a full-bleed icon) when at least this share of it is transparent.
    static let transparentShare: Double = 0.08
    /// Mean luminance (0–1) under which a mark's paint counts as dark.
    static let darkMarkMax: Double = 0.35
    /// Mean luminance over which a mark's paint counts as light.
    static let lightMarkMin: Double = 0.72

    /// The tone of a picture, sampled on a small grid (an icon is 64–180 px; 48 × 48
    /// samples are plenty and keep the decode cheap).
    static func of(_ image: UIImage) -> LinkIconTone {
        guard let cg = image.cgImage else { return .neutral }
        let side = 48
        var pixels = [UInt8](repeating: 0, count: side * side * 4)
        guard let ctx = CGContext(data: &pixels, width: side, height: side, bitsPerComponent: 8, bytesPerRow: side * 4,
                                  space: CGColorSpaceCreateDeviceRGB(),
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue)
        else { return .neutral }
        ctx.interpolationQuality = .medium
        ctx.draw(cg, in: CGRect(x: 0, y: 0, width: side, height: side))
        var clear = 0, painted = 0
        var lum = 0.0
        for i in stride(from: 0, to: pixels.count, by: 4) {
            let a = Double(pixels[i + 3])
            if a < 128 {
                clear += 1
            } else {
                painted += 1
                // Premultiplied: divide the alpha back out.
                let r = Double(pixels[i]) / a, g = Double(pixels[i + 1]) / a, b = Double(pixels[i + 2]) / a
                lum += 0.2126 * r + 0.7152 * g + 0.0722 * b
            }
        }
        let total = Double(side * side)
        guard Double(clear) / total >= transparentShare, painted > 0 else { return .neutral }
        let mean = lum / Double(painted)
        if mean < darkMarkMax { return .darkMark }
        if mean > lightMarkMin { return .lightMark }
        return .neutral
    }
}

/// The tile: a rounded square with the brand mark, the fetched icon or the host's
/// letter on it. Pass the link's URL (for the brand) and the host it is keyed by at
/// the relay; `picture` is what `LinkIcons.picture(for:)` has for that host.
struct LinkTile: View {
    @Environment(\.theme) private var theme
    let url: URL?
    let host: String
    var picture: (image: UIImage, tone: LinkIconTone)? = nil
    var size: CGFloat = PaneKit.avatar

    /// The mark's side as a share of the tile's: 22 % clear space on every side.
    static let markRatio: CGFloat = 0.56
    /// The tile's corner as a share of its side (36 → 10, the pane kit's corner).
    static let radiusRatio: CGFloat = 0.28
    /// The letter on the neutral tile, as a share of the side (36 → 15 pt).
    static let letterRatio: CGFloat = 0.42

    static func radius(_ size: CGFloat) -> CGFloat { max(2, (size * radiusRatio).rounded()) }
    static func markSide(_ size: CGFloat) -> CGFloat { (size * markRatio).rounded() }

    /// The letter on a neutral tile: the host's first letter, upper-cased; `#` for nothing.
    static func initial(_ host: String) -> String {
        let h = host.hasPrefix("www.") ? String(host.dropFirst(4)) : host
        return h.first(where: { $0.isLetter || $0.isNumber }).map { String($0).uppercased() } ?? "#"
    }

    /// What the tile shows, in order of preference: brand, picture, letter.
    enum Kind: Equatable {
        case brand(LinkBrand)
        case picture(LinkIconTone)
        case initial(String)
    }

    static func kind(url: URL?, host: String, picture: (image: UIImage, tone: LinkIconTone)?) -> Kind {
        if let url, let brand = LinkBrand.of(url: url) { return .brand(brand) }
        if let picture { return .picture(picture.tone) }
        return .initial(initial(host))
    }

    var kind: Kind { Self.kind(url: url, host: host, picture: picture) }

    /// The tile's fill for a kind on this theme: the chip fill, flipped when a transparent
    /// mark would vanish on it.
    static func fill(_ kind: Kind, theme: Theme) -> Color {
        switch kind {
        case .picture(.darkMark) where theme.isDark: return Color(hex: "#ECECEE")!
        case .picture(.lightMark) where !theme.isDark: return Color(hex: "#2C2C2E")!
        default: return theme.field
        }
    }

    var body: some View {
        let r = Self.radius(size)
        let shape = RoundedRectangle(cornerRadius: r, style: .continuous)
        ZStack {
            shape.fill(Self.fill(kind, theme: theme))
            switch kind {
            case .brand(let brand):
                Image(brand.assetName)
                    .renderingMode(.template)
                    .resizable()
                    .scaledToFit()
                    .frame(width: Self.markSide(size), height: Self.markSide(size))
                    .foregroundStyle(brand.color(theme))
                    .accessibilityLabel(brand.name)
            case .picture:
                if let picture {
                    Image(uiImage: picture.image)
                        .resizable()
                        .interpolation(.high)
                        .scaledToFill()
                        .frame(width: size, height: size)
                }
            case .initial(let letter):
                Text(letter)
                    .font(theme.font((size * Self.letterRatio).rounded(), weight: .semibold))
                    .foregroundStyle(theme.muted)
            }
        }
        .frame(width: size, height: size)
        .clipShape(shape)
    }
}
