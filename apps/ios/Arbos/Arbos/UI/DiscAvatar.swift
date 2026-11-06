// Solid-colour agent discs (D92): an agent without a photo shows a plain
// filled circle — ten colours, deterministic from the agent id, with the
// owner's pick on top. Nothing is drawn on the circle. The palette and the
// index rule come from arbos-core through ArbosFFI, so the iPhone draws the
// same colour as the Mac and the relay's `avatar_disc_color_effective`.
// People keep their D79 initials.

import ArbosFFI
import SwiftUI

/// Remote flag `features.voice_mode` from `GET /me` (default **off**; the relay's
/// `ARBOS_FEATURE_VOICE_MODE=1` brings it back without an app release). Off → no voice
/// picker row, no voice catalogue request; the code stays (Jacob, 2026-10-22).
final class VoiceModeFlag: ObservableObject {
    static let shared = VoiceModeFlag()
    @Published private(set) var on: Bool = UserDefaults.standard.object(forKey: "features.voice_mode") as? Bool ?? false

    func set(_ value: Bool) {
        guard value != on else { return }
        on = value
        UserDefaults.standard.set(value, forKey: "features.voice_mode")
    }
}

/// The shared tables, cached per launch.
enum DiscArt {
    static let colorCount = Int(discColorCount())
    static let colorNames: [String] = discColorNames()
    /// The ten fills, index order.
    static let colors: [Color] = (0..<colorCount).map { Color(uiColor: UIColor(hexString: discColorHex(index: UInt8($0))) ?? .gray) }

    /// "No picture" (Jacob, 2026-10-25): an entity created from the Create sheet without an
    /// uploaded or generated picture is stored with `avatar_disc_color` = 255
    /// (`arbos_core::avatar::disc::NEUTRAL`) and draws as **the name's first letter on a
    /// neutral grey disc** — never an automatic colour. Distinct from nil (= the id's automatic
    /// colour), so every existing agent keeps the colour it has.
    static let neutral = Int(discNeutralIndex())
    static func isNeutral(_ color: Int?) -> Bool { color == neutral }

    /// The one letter a neutral disc carries: the first letter or digit of the name, upper-cased
    /// (`?` for an empty name) — the Mac's `disc::initial`.
    static func initial(_ name: String) -> String {
        guard let c = name.unicodeScalars.first(where: { CharacterSet.alphanumerics.contains($0) }) else { return "?" }
        return String(c).uppercased()
    }

    /// Neutral disc fill and letter ink per appearance (the picker's "unselected" grey).
    static func neutralColors(dark: Bool) -> (disc: Color, ink: Color) {
        dark
            ? (Color(red: 0x3A / 255, green: 0x3A / 255, blue: 0x3A / 255), Color(white: 0xE5 / 255))
            : (Color(white: 0xE6 / 255), Color(white: 0x4A / 255))
    }

    /// The colour in effect for a project (the owner's pick, else the id's).
    static func pick(_ project: Project) -> UInt8 {
        discColor(seed: project.id, color: project.avatarDiscColor.map { UInt8(clamping: $0) })
    }

    static func pick(seed: String, color: Int? = nil) -> UInt8 {
        discColor(seed: seed, color: color.map { UInt8(clamping: $0) })
    }

    /// The fill of colour `index`.
    static func color(_ index: UInt8) -> Color {
        colors[Int(index) % max(colorCount, 1)]
    }

    /// The bubble-bar accent of an agent (its disc colour).
    static func accent(_ project: Project, dark: Bool) -> Color {
        let a = discAccent(seed: project.id, color: project.avatarDiscColor.map { UInt8(clamping: $0) })
        return Color(uiColor: UIColor(hexString: dark ? a.dark : a.light) ?? .systemGray)
    }
}

/// An agent's default picture: one plain filled circle, nothing on it and nothing animating.
struct DiscAvatar: View {
    let color: UInt8
    var size: CGFloat = 48

    var body: some View {
        Circle()
            .fill(DiscArt.color(color))
            .frame(width: size, height: size)
            .accessibilityHidden(true)
            .accessibilityIdentifier("agent-disc")
    }
}

/// The neutral avatar (Jacob, 2026-10-25): the name's first letter, semibold, on a plain grey
/// disc — no gradient, no colour. An empty name is an empty disc (the Create sheet before a
/// name is typed).
struct NeutralAvatar: View {
    @Environment(\.theme) private var theme
    let name: String
    var size: CGFloat = Theme.avatar

    var body: some View {
        let c = DiscArt.neutralColors(dark: theme.isDark)
        ZStack {
            Circle().fill(c.disc)
            if !name.trimmingCharacters(in: .whitespaces).isEmpty {
                Text(DiscArt.initial(name))
                    .font(.system(size: size * AvatarPalette.initialsScale, weight: .semibold))
                    .foregroundStyle(c.ink)
                    .lineLimit(1)
                    .offset(y: size * 0.01)
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
        .accessibilityIdentifier("neutral-avatar")
    }
}

/// A server's picture: the colour picked on its Create screen (D92 amendment), else D79
/// initials — or the neutral initial (Jacob, 2026-10-25).
struct ServerAvatar: View {
    @EnvironmentObject private var model: AppModel
    let server: Server
    var size: CGFloat = Theme.avatar

    var body: some View {
        if let key = server.imageKey, let url = model.blobURL(projectId: "servers:\(server.id)", key: key) {
            // D92 amendment: the uploaded / generated picture wins over the disc.
            AsyncImage(url: url) { phase in // avatar-lint: allow (server picture, one copy)
                if let image = phase.image {
                    image.resizable().scaledToFill()
                } else {
                    fallback
                }
            }
            .frame(width: size, height: size)
            .clipShape(Circle())
        } else {
            fallback
        }
    }

    @ViewBuilder
    private var fallback: some View {
        if server.isNeutralAvatar {
            NeutralAvatar(name: server.name, size: size)
        } else if let color = server.avatarDiscColor {
            DiscAvatar(color: UInt8(clamping: color), size: size)
        } else {
            InitialsAvatar(name: server.name, handle: nil, id: server.id, override: server.avatarColorIndex, size: size)
        }
    }
}
