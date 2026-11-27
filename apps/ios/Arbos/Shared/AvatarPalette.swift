// Telegram-style initials avatars (D79): a coloured gradient circle with the
// person's or agent's initials in white. The colour is one of seven, picked
// from the stable id so every device and the relay agree:
//   index = (sum of the UTF-8 bytes of the id) mod 7
// (project id for agents, user id for people), unless the owner chose one
// (`avatar_color_index`; the relay sends `avatar_color_index_effective`). A
// GitHub or uploaded picture always replaces the circle. Shared with the
// Notification Service Extension, which draws the same thing into a PNG.

import UIKit

enum AvatarPalette {
    /// Gradient stops (top → bottom), index 0–6, light and dark (the relay's
    /// GET /avatar-colors table, arbos-relay avatar.rs; dark = lightness −7 %).
    static let light: [(top: String, bottom: String)] = [
        ("#FF845E", "#D45246"), // 0 red
        ("#FEBB5B", "#F68136"), // 1 orange
        ("#B694F9", "#6C61DF"), // 2 violet
        ("#9AD164", "#46BA43"), // 3 green
        ("#53EDD6", "#28C9B7"), // 4 cyan
        ("#5CAFFA", "#408ACF"), // 5 blue
        ("#FF8AA5", "#E05F8E"), // 6 pink
    ]
    static let dark: [(top: String, bottom: String)] = [
        ("#FF693A", "#C83B2E"),
        ("#FEAC38", "#F46C14"),
        ("#9F72F7", "#5043D9"),
        ("#88C948", "#3CA03A"),
        ("#33EACE", "#22AB9C"),
        ("#399EF9", "#2F78BC"),
        ("#FF668A", "#DA4179"),
    ]
    static let count = 7
    static let names = ["Red", "Orange", "Violet", "Green", "Cyan", "Blue", "Pink"]
    /// Initials font size as a share of the diameter. Measured on Telegram's chat
    /// list (60 pt circle, 18.7 pt cap height ≈ 26.7 pt SF Semibold): 0.445 for one
    /// and two letters alike; `minimumScaleFactor` catches wide pairs.
    static let initialsScale: CGFloat = 0.445

    /// The deterministic colour for a stable id (project id for agents, user id for people).
    static func index(for id: String) -> Int {
        let sum = id.utf8.reduce(0) { $0 + Int($1) }
        return sum % count
    }

    /// Clamp a relay/user value into the table.
    static func clamp(_ i: Int?) -> Int? {
        guard let i else { return nil }
        return (0..<count).contains(i) ? i : nil
    }

    /// Gradient colours for an index in light or dark mode (the relay's pairs, so both clients match).
    static func stops(_ index: Int, dark isDark: Bool) -> (top: UIColor, bottom: UIColor) {
        let p = (isDark ? dark : light)[((index % count) + count) % count]
        return (UIColor(hexString: p.top) ?? .systemGray, UIColor(hexString: p.bottom) ?? .systemGray)
    }

    /// Initials: first letter of the first two words of the display name
    /// ("Growth Lab" → GL, "Arbos" → A), else the handle's first letter;
    /// uppercase; the first grapheme for non-Latin scripts.
    static func initials(name: String?, handle: String?) -> String {
        let n = (name ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        // The Mac's rule (`avatars::initials`): words split on space, - _ · /, and a word gives
        // its first letter or digit, so an emoji, quote or bracket in front is stepped over.
        let letters = n.split(whereSeparator: { $0.isWhitespace || "-_·/".contains($0) })
            .compactMap { $0.first(where: { $0.isLetter || $0.isNumber }) }.prefix(2).map { String($0) }
        if !letters.isEmpty { return letters.joined().uppercased() }
        let h = (handle ?? "").trimmingCharacters(in: .whitespacesAndNewlines).trimmingCharacters(in: CharacterSet(charactersIn: "@"))
        if let f = h.first { return String(f).uppercased() }
        return "?"
    }
}

extension AvatarPalette {
    /// The same circle as a PNG, for notification attachments (INPerson image) and other UIKit spots.
    static func initialsPNG(name: String?, handle: String?, id: String, override: Int? = nil, pixels: CGFloat, dark: Bool = false, scale: CGFloat = 3) -> Data? {
        let idx = clamp(override) ?? index(for: id)
        let text = initials(name: name, handle: handle)
        let side = pixels / scale
        let rect = CGRect(x: 0, y: 0, width: side, height: side)
        let format = UIGraphicsImageRendererFormat()
        format.scale = scale
        format.opaque = false
        let s = stops(idx, dark: dark)
        let image = UIGraphicsImageRenderer(size: rect.size, format: format).image { ctx in
            let cg = ctx.cgContext
            cg.addEllipse(in: rect)
            cg.clip()
            let colors = [s.top.cgColor, s.bottom.cgColor] as CFArray
            if let grad = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: colors, locations: [0, 1]) {
                cg.drawLinearGradient(grad, start: CGPoint(x: side / 2, y: 0), end: CGPoint(x: side / 2, y: side), options: [])
            }
            let font = UIFont.systemFont(ofSize: side * AvatarPalette.initialsScale, weight: .semibold)
            let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: UIColor.white]
            let bounds = (text as NSString).size(withAttributes: attrs)
            (text as NSString).draw(at: CGPoint(x: (side - bounds.width) / 2, y: (side - bounds.height) / 2 + side * 0.01), withAttributes: attrs)
        }
        return image.pngData()
    }
}

extension UIColor {
    convenience init?(hexString: String) {
        var s = hexString.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("#") { s.removeFirst() }
        guard s.count == 6, let v = UInt32(s, radix: 16) else { return nil }
        self.init(red: CGFloat((v >> 16) & 0xff) / 255, green: CGFloat((v >> 8) & 0xff) / 255, blue: CGFloat(v & 0xff) / 255, alpha: 1)
    }
}
