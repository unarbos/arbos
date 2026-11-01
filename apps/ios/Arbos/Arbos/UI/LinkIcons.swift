import SwiftUI
import UIKit

/// The small site icon before every named link in a bubble (Jacob, 2026-10-25: every
/// `[Name](https://…)` in a reply carries its favicon, as in Grok Bot — his example Bubble's New
/// York Post headline list). Icons come from the relay's `GET /link-icon?domain=` (a 64×64 PNG,
/// 204 when the site has none), are asked for once per host and kept for the session; a host still
/// loading or without an icon shows the neutral rounded square. The Mac's `select.rs` LinkIcons.
@MainActor
final class LinkIcons: ObservableObject {
    static let shared = LinkIcons()

    /// Side of the icon in points (Grok's is 16 at its 15 pt body; ours sits in 17 pt text).
    nonisolated static let side: CGFloat = 14
    /// The gap between the icon and the link text.
    nonisolated static let gap: CGFloat = 4

    /// host → the rounded icon, or `nil` when the site has none. A host that is not a key yet is
    /// still loading (or never asked for).
    ///
    /// Not `@Published`: every visible paragraph observes this store, so a fetched host writes
    /// `images` and `pictures` under one `objectWillChange` — one redraw, not two.
    private(set) var images: [String: UIImage?] = [:]
    /// host → the relay's picture as decoded, full size, with its tone — what the link tiles
    /// draw ([feedback link-icons-ios-tile-2026-10-27]); `nil` when the site has none.
    private(set) var pictures: [String: (image: UIImage, tone: LinkIconTone)?] = [:]
    private var inFlight: Set<String> = []
    /// Decoded answers waiting for the next publish (`flush`).
    private var landed: [String: Decoded] = [:]
    private var flushQueued = false
    /// Fetches the PNG for a host: the relay in the app, the bundled icons in the fixture. Set by
    /// `AppModel`; `nil` means never ask (the placeholder stays).
    var fetch: ((String) async -> Data?)?

    /// The picture to draw before a link to `host`: its icon when known, else the placeholder — and
    /// the first ask for a host schedules its fetch.
    func image(for host: String) -> UIImage {
        if let known = images[host] {
            return known ?? Self.placeholder
        }
        request(host)
        return Self.placeholder
    }

    /// The full-size picture for a tile on `host`, with its tone — or `nil` while it loads or
    /// when the site has none (the tile draws the host's letter); the first ask schedules
    /// the fetch. A brand host is never asked: its mark is bundled (`LinkBrand`).
    func picture(for host: String) -> (image: UIImage, tone: LinkIconTone)? {
        if let known = pictures[host] {
            return known
        }
        request(host)
        return nil
    }

    /// Ask once per host; the answer (picture or none) publishes so the bubbles redraw.
    func request(_ host: String) {
        guard images[host] == nil, !inFlight.contains(host), let fetch else { return }
        inFlight.insert(host)
        Task { @MainActor [weak self] in
            let data = await fetch(host)
            let scale = UIScreen.main.scale
            let decoded = await Task.detached(priority: .utility) { () -> Decoded in
                LinkIcons.decode(data, scale: scale)
            }.value
            self?.land(host, decoded)
        }
    }

    /// Every visible paragraph observes this store, so answers that land within one frame share
    /// one publish: one redraw of the transcript however many hosts arrived. A host stays in
    /// `inFlight` until its answer is published, so it is never asked for twice.
    private func land(_ host: String, _ decoded: Decoded) {
        landed[host] = decoded
        guard !flushQueued else { return }
        flushQueued = true
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(16))
            self?.flush()
        }
    }

    private func flush() {
        flushQueued = false
        guard !landed.isEmpty else { return }
        objectWillChange.send()
        for (host, decoded) in landed {
            pictures[host] = decoded.picture
            images[host] = decoded.icon
            inFlight.remove(host)
        }
        landed = [:]
    }

    /// A fetched host's tile picture with its tone, and its inline icon. A struct, not a tuple:
    /// `Task.detached` cannot resolve its overload for a closure returning a labeled tuple.
    struct Decoded: Sendable {
        let picture: (image: UIImage, tone: LinkIconTone)?
        let icon: UIImage?
    }

    /// The tile picture with its tone, and the inline icon, from the relay's bytes.
    nonisolated static func decode(_ data: Data?, scale: CGFloat) -> Decoded {
        guard let image = data.flatMap(UIImage.init(data:)) else { return Decoded(picture: nil, icon: nil) }
        return Decoded(picture: (image: image, tone: LinkIconTone.of(image)), icon: roundedIcon(image, scale: scale))
    }

    /// The fixture's fetcher: the real icons of the fixture replies' sites ship in the asset
    /// catalog as `link-icon-<host with dots as dashes>`; any other host has none.
    static func fixtureIcon(host: String) -> Data? {
        UIImage(named: "link-icon-" + host.replacingOccurrences(of: ".", with: "-"))?.pngData()
    }

    /// Forget everything (tests).
    func reset() {
        objectWillChange.send()
        images = [:]
        pictures = [:]
        inFlight = []
        landed = [:]
    }

    /// The host an icon is fetched for, or `nil` when the link is not an http(s) URL to a named host
    /// (mailto:, an IP literal, localhost).
    static func host(of url: URL) -> String? {
        guard let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https",
              let raw = url.host?.lowercased(), raw.contains(".") else { return nil }
        if raw.split(separator: ".").allSatisfy({ $0.allSatisfy(\.isNumber) }) { return nil }
        return raw
    }

    /// A link whose text is itself an address keeps no icon: the address is the label.
    static func labelIsURL(_ label: String) -> Bool {
        let l = label.trimmingCharacters(in: .whitespaces).lowercased()
        return l.hasPrefix("http://") || l.hasPrefix("https://") || l.hasPrefix("www.")
    }

    /// The named-link runs of `attr` in order: each run's range, its host and its label.
    static func namedLinks(in attr: AttributedString) -> [(range: Range<AttributedString.Index>, host: String)] {
        var out: [(Range<AttributedString.Index>, String)] = []
        for run in attr.runs {
            guard let url = run.link, let host = host(of: url) else { continue }
            let label = String(attr[run.range].characters)
            if labelIsURL(label) { continue }
            out.append((run.range, host))
        }
        return out
    }

    /// Every host a reply's named links point at, once each, in reading order — what a chat asks the
    /// relay for when its messages arrive (prose, lists and table cells; never code).
    static func hosts(in text: String) -> [String] {
        var out: [String] = []
        func scan(_ piece: String) {
            for (_, host) in namedLinks(in: Markdown.inline(piece, color: .primary, link: .primary, codeChip: .clear)) where !out.contains(host) {
                out.append(host)
            }
        }
        func walk(_ blocks: [Block]) {
            for block in blocks {
                switch block {
                case .paragraph(let t), .heading(_, let t): scan(t)
                case .quote(let inner): walk(inner)
                case .bullets(let items), .numbered(let items): items.forEach(scan)
                case .table(let header, let rows): header.forEach(scan); rows.forEach { $0.forEach(scan) }
                case .code, .math, .chart, .linkPreview: break
                }
            }
        }
        walk(Markdown.blocks(text))
        return out
    }

    /// `attr` as a `Text`, with each named link's site icon before its text (the Mac's icon cell).
    /// Plain `Text(attr)` when there is no named link.
    func text(_ attr: AttributedString, lineHeight: CGFloat) -> Text {
        let links = Self.namedLinks(in: attr)
        guard !links.isEmpty else { return Text(attr) }
        var out = Text("")
        var cursor = attr.startIndex
        for (range, host) in links {
            out = out + Text(AttributedString(attr[cursor..<range.lowerBound]))
            let icon = image(for: host)
            // The picture rides the baseline a little low so its centre sits on the x-height.
            out = out + Text(Image(uiImage: icon)).baselineOffset(-(Self.side - lineHeight * 0.62) / 2)
            out = out + Text(AttributedString(attr[range]))
            cursor = range.upperBound
        }
        out = out + Text(AttributedString(attr[cursor..<attr.endIndex]))
        return out
    }

    /// The neutral rounded square shown while an icon loads or when the site has none.
    static let placeholder: UIImage = {
        let r = UIGraphicsImageRenderer(size: CGSize(width: side + gap, height: side))
        return r.image { ctx in
            let rect = CGRect(x: 0, y: 0, width: side, height: side)
            UIColor.secondaryLabel.withAlphaComponent(0.28).setFill()
            UIBezierPath(roundedRect: rect, cornerRadius: side / 4).fill()
        }.withRenderingMode(.alwaysOriginal)
    }()

    /// The relay's PNG as the icon: scaled to `side` points (sharp at 3×), corners rounded at a
    /// quarter, with the gap on its right so the text never touches it. `nil` when it does not decode.
    nonisolated static func rounded(_ data: Data) -> UIImage? {
        UIImage(data: data).flatMap { roundedIcon($0) }
    }

    /// The decoded picture as the inline icon (see `rounded(_ data:)`).
    /// `scale` is the screen's, passed in when this runs off the main thread (where the
    /// renderer's default format cannot be trusted to match the screen).
    nonisolated static func roundedIcon(_ src: UIImage, scale: CGFloat? = nil) -> UIImage? {
        let format = UIGraphicsImageRendererFormat.default()
        if let scale { format.scale = scale }
        let r = UIGraphicsImageRenderer(size: CGSize(width: side + gap, height: side), format: format)
        return r.image { _ in
            let rect = CGRect(x: 0, y: 0, width: side, height: side)
            UIBezierPath(roundedRect: rect, cornerRadius: side / 4).addClip()
            src.draw(in: rect)
        }.withRenderingMode(.alwaysOriginal)
    }
}
