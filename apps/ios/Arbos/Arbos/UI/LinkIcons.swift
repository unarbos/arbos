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
    static let side: CGFloat = 14
    /// The gap between the icon and the link text.
    static let gap: CGFloat = 4

    /// host → the rounded icon, or `nil` when the site has none. A host that is not a key yet is
    /// still loading (or never asked for).
    @Published private(set) var images: [String: UIImage?] = [:]
    private var inFlight: Set<String> = []
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

    /// Ask once per host; the answer (picture or none) publishes so the bubbles redraw.
    func request(_ host: String) {
        guard images[host] == nil, !inFlight.contains(host), let fetch else { return }
        inFlight.insert(host)
        Task { @MainActor [weak self] in
            let data = await fetch(host)
            guard let self else { return }
            self.inFlight.remove(host)
            self.images[host] = data.flatMap(Self.rounded)
        }
    }

    /// The fixture's fetcher: the real icons of the fixture replies' sites ship in the asset
    /// catalog as `link-icon-<host with dots as dashes>`; any other host has none.
    static func fixtureIcon(host: String) -> Data? {
        UIImage(named: "link-icon-" + host.replacingOccurrences(of: ".", with: "-"))?.pngData()
    }

    /// Forget everything (tests).
    func reset() {
        images = [:]
        inFlight = []
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
                case .code, .math, .chart: break
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
    static func rounded(_ data: Data) -> UIImage? {
        guard let src = UIImage(data: data) else { return nil }
        let r = UIGraphicsImageRenderer(size: CGSize(width: side + gap, height: side))
        return r.image { _ in
            let rect = CGRect(x: 0, y: 0, width: side, height: side)
            UIBezierPath(roundedRect: rect, cornerRadius: side / 4).addClip()
            src.draw(in: rect)
        }.withRenderingMode(.alwaysOriginal)
    }
}
