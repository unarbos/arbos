import SwiftUI

/// A project's face — the same three fields the desktop's tab sheet sets
/// and files at `<project>/.arbos/project.toml` (`desktop/src/model/
/// identity.rs`): a name, a glyph by name, a colour by name or `#rrggbb`.
/// The phone reads that file over the kernel's `read` frame, so a project
/// looks the same here as on its tab.
struct ProjectIdentity: Equatable, Codable {
    var name: String?
    var icon: String
    var color: String

    init(name: String?, icon: String, color: String) {
        self.name = name
        self.icon = icon
        self.color = color
    }

    /// The hub's shape (#233): `icon` always, `name` and `color` only when
    /// the file says so.
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decodeIfPresent(String.self, forKey: .name)
        icon = try c.decodeIfPresent(String.self, forKey: .icon) ?? Self.glyphs[0].name
        color = try c.decodeIfPresent(String.self, forKey: .color) ?? ""
    }

    private enum CodingKeys: String, CodingKey { case name, icon, color }

    /// The desktop's glyph names → SF Symbols with the same meaning.
    static let glyphs: [(name: String, symbol: String)] = [
        ("folder", "folder"),
        ("home", "house"),
        ("star", "star"),
        ("book", "book"),
        ("tag", "tag"),
        ("globe", "globe"),
        ("cpu", "cpu"),
        ("terminal", "terminal"),
        ("chat", "bubble.left"),
        ("checklist", "checklist"),
        ("branch", "arrow.triangle.branch"),
        ("grid", "square.grid.2x2"),
    ]

    /// The desktop's palette, same names, same values.
    static let colors: [(name: String, hex: UInt32)] = [
        ("blue", 0x4C8DFF),
        ("orange", 0xF08A3C),
        ("purple", 0x9B7BFF),
        ("red", 0xE5533D),
        ("green", 0x3DBD6E),
        ("teal", 0x2FB7B0),
        ("pink", 0xE0609E),
        ("yellow", 0xE0B23C),
    ]

    /// What a project wears before anyone chose: a glyph and a colour
    /// both picked from its name, so every row has a face of its own and
    /// the same project gets the same face on every device (FNV-1a, as
    /// the desktop hashes its place for the colour).
    static func defaults(key: String, remote: Bool) -> ProjectIdentity {
        let hash = fnv1a(key)
        let color = colors[Int(hash % UInt64(colors.count))].name
        // The home glyph is the home tab's alone.
        let choices = glyphs.filter { $0.name != "home" }
        let icon = choices[Int((hash >> 8) % UInt64(choices.count))].name
        return ProjectIdentity(name: nil, icon: icon, color: color)
    }

    /// A roster face filled out: a missing colour takes the name-derived one.
    func filled(key: String) -> ProjectIdentity {
        var face = self
        if face.color.isEmpty { face.color = Self.defaults(key: key, remote: true).color }
        return face
    }

    var symbol: String {
        Self.glyphs.first { $0.name == icon }?.symbol ?? Self.glyphs[0].symbol
    }

    var tint: Color {
        if let hex = Self.colors.first(where: { $0.name == color })?.hex {
            return Color(hex: hex)
        }
        let trimmed = color.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: "#", with: "")
        if let hex = UInt32(trimmed, radix: 16), trimmed.count == 6 {
            return Color(hex: hex)
        }
        return Color(hex: Self.colors[0].hex)
    }

    var label: String? {
        let trimmed = name?.trimmingCharacters(in: .whitespaces) ?? ""
        return trimmed.isEmpty ? nil : trimmed
    }

    /// `project.toml` as the desktop writes it: three top-level keys,
    /// `name` optional. Other tables (`[root]`, `[spend]`) are skipped.
    static func parse(toml: String) -> ProjectIdentity? {
        var name: String?
        var icon: String?
        var color: String?
        for rawLine in toml.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("[") { break }
            guard let eq = line.firstIndex(of: "=") else { continue }
            let key = line[..<eq].trimmingCharacters(in: .whitespaces)
            var value = line[line.index(after: eq)...].trimmingCharacters(in: .whitespaces)
            if let hash = value.firstIndex(of: "#"), !value.hasPrefix("\"") {
                value = value[..<hash].trimmingCharacters(in: .whitespaces)
            }
            value = value.trimmingCharacters(in: CharacterSet(charactersIn: "\""))
            switch key {
            case "name": name = value
            case "icon": icon = value
            case "color": color = value
            default: break
            }
        }
        guard icon != nil || color != nil || name != nil else { return nil }
        return ProjectIdentity(name: name, icon: icon ?? glyphs[0].name, color: color ?? colors[0].name)
    }

    private static func fnv1a(_ s: String) -> UInt64 {
        var h: UInt64 = 14695981039346656037
        for b in s.utf8 {
            h ^= UInt64(b)
            h = h &* 1099511628211
        }
        return h
    }
}

/// The glyph tile a project wears in the list and at the top of its
/// chat: the desktop's 30 pt cell, the glyph in the project's colour.
struct ProjectGlyph: View {
    let identity: ProjectIdentity
    var size: CGFloat = 30
    var working = false

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: ArbosTheme.controlRadius, style: .continuous)
                .fill(ArbosTheme.elementActive)
            if working {
                BrailleSpinner(tint: identity.tint)
                    .font(.system(size: size * 0.5, design: .monospaced))
            } else {
                Image(systemName: identity.symbol)
                    .font(.system(size: size * 0.5, weight: .medium))
                    .foregroundStyle(identity.tint)
            }
        }
        .frame(width: size, height: size)
        // Decorative: the project's name is always beside it. Left visible
        // to the tree it announced its SF Symbol's name — the chat header
        // read "Move" next to the word "pod".
        .accessibilityHidden(true)
    }
}

/// The desktop's braille spinner: eight frames at 12.5 fps, in the
/// project's colour, standing in for the glyph while an agent works.
struct BrailleSpinner: View {
    let tint: Color
    private static let frames: [String] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]

    var body: some View {
        TimelineView(.periodic(from: .now, by: 0.08)) { context in
            let index = Int(context.date.timeIntervalSinceReferenceDate / 0.08) % Self.frames.count
            Text(Self.frames[index]).foregroundStyle(tint)
        }
    }
}
