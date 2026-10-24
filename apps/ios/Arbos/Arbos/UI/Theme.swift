// Grok Bot visual language, measured from the dark-mode references in
// internal/refs/grokbot-ios (iPhone Air, 1260×2736 @3x): background #141414,
// assistant bubble #202020, user bubble #5C5C5C, 17 pt SF Pro body with the
// default 22 pt line height, 20 pt continuous corners, glass circles for the
// header controls, system lines in #95959B.

import SwiftUI
import UIKit

/// Haptics for the few moments Grok Bot marks with a tap: sending, starting or
/// finishing a dictation, picking a card option. Nothing on scrolling or opening.
enum Haptics {
    private static let light = UIImpactFeedbackGenerator(style: .light)
    private static let medium = UIImpactFeedbackGenerator(style: .medium)
    private static let selection = UISelectionFeedbackGenerator()

    /// Message sent, dictation finished.
    static func send() { light.impactOccurred() }
    /// Hold-to-talk started.
    static func recordStart() { medium.impactOccurred() }
    /// Card option picked, chip toggled.
    static func pick() { selection.selectionChanged() }
}

/// Telegram-style text size steps (Q6). Index 3 = the design size.
enum TextScale {
    static let steps: [CGFloat] = [0.85, 0.9, 0.95, 1.0, 1.1, 1.2, 1.3]
    static let defaultIndex = 3
    static let key = "textScale"

    static func index(for scale: CGFloat) -> Int {
        var best = defaultIndex
        var bestD = CGFloat.greatestFiniteMagnitude
        for (i, s) in steps.enumerated() where abs(s - scale) < bestD { best = i; bestD = abs(s - scale) }
        return best
    }

    static func label(_ scale: CGFloat) -> String {
        switch index(for: scale) {
        case 0: return "Smallest"
        case 1: return "Smaller"
        case 2: return "Small"
        case 3: return "Default"
        case 4: return "Large"
        case 5: return "Larger"
        default: return "Largest"
        }
    }
}

/// Equatable on purpose: the app root re-applies `.environment(\.theme, …)` on every
/// AppModel publish; SwiftUI only propagates an environment value when it differs,
/// and only for Equatable values — without this every `@Environment(\.theme)` reader
/// (every bubble) was re-evaluated on each publish, bypassing the rows' Equatable
/// guards (arbonites-chat-lag, 2026-10-24).
struct Theme: Equatable {
    let isDark: Bool
    var surface: Surface
    /// Q6 text size: one factor over every point size that goes through `font(_:)`
    /// (Settings → Appearance → Text Size; `TextScale.steps`, default 1).
    var textScale: CGFloat = 1

    let bg: Color
    /// Assistant bubble.
    let bubbleAssistant: Color
    /// User bubble.
    let bubbleUser: Color
    /// Composer field / glass circle fill.
    let field: Color
    let fieldBorder: Color
    let text: Color
    /// System lines, timestamps, "is working".
    let muted: Color
    /// Placeholder text.
    let placeholder: Color
    let icon: Color
    /// Grok's blue: links, the caret (Mac `accent`).
    let accent: Color
    /// Home-list unread dot (Grok dark measures #006CEB, a shade deeper than the accent).
    let unread: Color
    let online: Color
    let warn: Color
    let danger: Color
    let dangerSoft: Color
    let card: Color
    let separator: Color
    /// Soft wash behind inline ``code`` in assistant / other bubbles (Mac `code_bg`).
    let codeChip: Color
    /// Soft wash behind inline ``code`` on the dark user bubble.
    let userCodeChip: Color

    static let bubbleRadius: CGFloat = 20
    static let bubbleHPad: CGFloat = 14
    static let bubbleVPad: CGFloat = 11
    /// 17 pt SF Pro renders a 20.3 pt line in SwiftUI; Grok shows UIKit's 22 pt body pitch.
    static let bodyLineSpacing: CGFloat = 1.7
    /// Screen width minus 76 pt: 16 pt own margin (the composer's `+` column
    /// starts 15 pt further in) + 60 pt on the other side. Grok dark, 420 pt
    /// screen, refs 01–03 at 3 px/pt: the assistant block runs x 16 → 360 in
    /// every shot ([feedback agent-bubble-inset-2026-10-22]; was 71).
    static let bubbleInset: CGFloat = 76
    static let body: CGFloat = 17
    /// Inline code sits a point under body so SF Mono matches the prose cap height.
    static let code: CGFloat = 16
    static let small: CGFloat = 15
    static let tiny: CGFloat = 13
    static let control: CGFloat = 42
    static let fieldHeight: CGFloat = 42
    static let voiceButton: CGFloat = 36
    static let plusButton: CGFloat = 42
    static let avatar: CGFloat = 28
    /// The avatar column (Mac `SENDER_DISC = PLUS_BUTTON`, Jacob [feedback
    /// AAvkkHYz3TOlou0Hu7u71Sc]): every round picture in a people chat — the sender's
    /// disc beside a bubble, the presence row over the composer — is the composer's `+`
    /// disc: the same size, centred on it. The `+` moves with the keyboard (31 → 19 pt
    /// inset), so the column follows it via `avatarColumnInset(composerInset:)` (keyboard
    /// up: inset 3 → avatar 19–61, centre 40 = 19 + 42 / 2; down: inset 15 → 31–73,
    /// centre 52). The bubble body follows after `memberAvatarGap` (up: 74 from the
    /// pane). Shared with feedback/sender-name-in-bubble-2026-10-23. Agent chats keep
    /// #1374's no-avatar rule.
    static let memberAvatar: CGFloat = plusButton
    static let memberAvatarGap: CGFloat = 13
    /// Transcript edge → the avatar's left edge, for the composer's current side inset.
    static func avatarColumnInset(composerInset: CGFloat) -> CGFloat {
        composerInset + plusButton / 2 - memberAvatar / 2 - 16
    }
    /// Transcript edge → bubble body in a people-chat row, for the composer's current side inset.
    static func memberLane(composerInset: CGFloat) -> CGFloat {
        avatarColumnInset(composerInset: composerInset) + memberAvatar + memberAvatarGap
    }
    /// Picture → name in the chat header pill: Grok Bot's title measures 5 pt ink to ink
    /// (Mac ref 57); the Mac's `identity::HEADER_GAP` + side bearing.
    static let headerPillGap: CGFloat = 5
    /// The composer's reply preview is always this tall — one row: glyph, "Replying to
    /// @handle", one truncated line of the quoted message, × — whatever the original's
    /// length (Jacob, Oct 23; same on the Mac). 16 pt line + 2 × 8 pt.
    static let replyPreviewHeight: CGFloat = 32
    /// A quoted picture shows as a square thumbnail this big, inside the row.
    static let replyPreviewThumb: CGFloat = 20
    /// The quoted line is pre-cut to this many characters; the row's own ellipsis trims it.
    static let replyPreviewChars = 160
    /// Composer side inset: Grok uses 31 pt with the keyboard down, 19 pt with it up.
    static let composerInset: CGFloat = 31
    static let composerInsetKeyboard: CGFloat = 19
    /// Home list: 48 pt avatar box, 80 pt row pitch, 10 pt unread dot.
    static let rowAvatar: CGFloat = 48
    static let rowVPad: CGFloat = 16
    static let unreadDot: CGFloat = 10

    static let dark = Theme(
        isDark: true, surface: .matte,
        bg: Color(hex: "#141414")!,
        bubbleAssistant: Color(hex: "#202020")!,
        bubbleUser: Color(hex: "#5C5C5C")!,
        field: Color(hex: "#181818")!,
        // Grok dark draws no rim on the field or the glass circles; keep a whisper.
        fieldBorder: Color(hex: "#1A1A1A")!,
        text: .white,
        muted: Color(hex: "#95959B")!,
        placeholder: Color(hex: "#58585B")!,
        icon: Color(hex: "#96969C")!,
        accent: Color(hex: "#1084FE")!,
        unread: Color(hex: "#006CEB")!,
        online: Color(hex: "#34C759")!,
        warn: Color(hex: "#FFB340")!,
        danger: Color(hex: "#FF453A")!,
        dangerSoft: Color(hex: "#FF453A")!.opacity(0.14),
        card: Color(hex: "#1C1C1C")!,
        separator: Color.white.opacity(0.08),
        codeChip: Color.black.opacity(0.28),
        userCodeChip: Color.white.opacity(0.14)
    )

    // Light, measured from Grok's App Store shots: page #FCFCFC, assistant
    // bubble #F2F2F2, user bubble near-black, white field with a hairline.
    // Greys meet WCAG AA on the surfaces they sit on (Jacob, 2026-10-21):
    // muted 5.3:1 on white / 4.7:1 on a bubble (#8A8A8E was 3.4:1), placeholder
    // 4.6:1, link blue 4.8:1 (#1084FE was 3.7:1); the unread dot keeps Grok's blue.
    static let light = Theme(
        isDark: false, surface: .matte,
        bg: Color(hex: "#FCFCFC")!,
        bubbleAssistant: Color(hex: "#F2F2F2")!,
        bubbleUser: Color(hex: "#111111")!,
        field: .white,
        fieldBorder: Color(hex: "#DEDEDE")!,
        text: Color(hex: "#111111")!,
        muted: Color(hex: "#6B6B70")!,
        placeholder: Color(hex: "#757579")!,
        icon: Color(hex: "#6E6E73")!,
        accent: Color(hex: "#0A6FE0")!,
        unread: Color(hex: "#1084FE")!,
        online: Color(hex: "#34C759")!,
        warn: Color(hex: "#E59A00")!,
        danger: Color(hex: "#D7332A")!,
        dangerSoft: Color(hex: "#D7332A")!.opacity(0.10),
        card: .white,
        separator: Color.black.opacity(0.10),
        codeChip: Color.black.opacity(0.06),
        userCodeChip: Color.white.opacity(0.14)
    )

    static func resolve(appearance: Appearance, surface: Surface, systemDark: Bool, textScale: CGFloat = 1) -> Theme {
        let dark: Bool
        switch appearance {
        case .system: dark = systemDark
        case .dark: dark = true
        case .light: dark = false
        }
        var t = dark ? Theme.dark : Theme.light
        t.surface = surface
        t.textScale = textScale
        return t
    }

    /// The app's text font: a fixed design size scaled by the user's Text Size.
    /// Every label and chat bubble goes through here (not Dynamic Type: Grok's
    /// design sizes are the baseline, the slider moves them together).
    func font(_ size: CGFloat, weight: Font.Weight = .regular, design: Font.Design = .default) -> Font {
        .system(size: pt(size), weight: weight, design: design)
    }

    /// A point size scaled like the text, for frames that must grow with it.
    func pt(_ size: CGFloat) -> CGFloat { (size * textScale * 2).rounded() / 2 }

    /// Text colour on a user bubble (white on dark grey, white on black).
    var userText: Color { .white }
    /// Primary filled control (send/voice button, main CTA): white on dark, black on light.
    var primary: Color { isDark ? .white : Color(hex: "#111111")! }
    var onPrimary: Color { isDark ? .black : .white }
    /// Frosted chrome (header blur, glass circles/pills) when Surface is Glass.
    var isGlass: Bool { surface == .glass }

    /// Grok's bot palette; a project's hex colour snaps to the nearest one.
    static let palette: [Color] = [
        Color(hex: "#1084FE")!, Color(hex: "#10B981")!, Color(hex: "#FF7A00")!, Color(hex: "#FF3B5C")!,
        Color(hex: "#FF2D9B")!, Color(hex: "#8B5CF6")!, Color(hex: "#8E5A3C")!, Color(hex: "#8E8E93")!,
        Color(hex: "#FFB000")!, Color(hex: "#14B8A6")!,
    ]

    func projectColor(_ hex: String) -> Color {
        Color(hex: hex) ?? Theme.palette[0]
    }

    /// The relay hands every project the same default colour (#8b7cf6); Grok
    /// gives each bot its own, so a default-coloured project picks a palette
    /// colour from a stable hash of its id.
    func projectColor(_ project: Project) -> Color {
        let hex = project.color.lowercased()
        if hex == "#8b7cf6" || hex.isEmpty {
            var h: UInt32 = 2166136261
            for b in project.id.utf8 { h = (h ^ UInt32(b)) &* 16777619 }
            return Theme.palette[Int(h % UInt32(Theme.palette.count))]
        }
        return Color(hex: project.color) ?? Theme.palette[0]
    }
}

private func hsla(_ h: Double, _ s: Double, _ l: Double, _ a: Double = 1) -> Color {
    let v = l + s * min(l, 1 - l)
    let sv = v == 0 ? 0 : 2 * (1 - l / v)
    return Color(hue: h, saturation: sv, brightness: v, opacity: a)
}

extension Color {
    init?(hex: String) {
        var s = hex.trimmingCharacters(in: .whitespacesAndNewlines)
        if s.hasPrefix("#") { s.removeFirst() }
        guard s.count == 6, let v = UInt32(s, radix: 16) else { return nil }
        self.init(
            red: Double((v >> 16) & 0xff) / 255,
            green: Double((v >> 8) & 0xff) / 255,
            blue: Double(v & 0xff) / 255
        )
    }
}

/// The composer's current side inset (`Theme.composerInset` / `composerInsetKeyboard`),
/// published by ChatView so the transcript rows and the presence row can keep their
/// avatar column centred on the `+` as it moves with the keyboard.
private struct ComposerInsetKey: EnvironmentKey {
    static let defaultValue: CGFloat = Theme.composerInset
}

extension EnvironmentValues {
    var composerInset: CGFloat {
        get { self[ComposerInsetKey.self] }
        set { self[ComposerInsetKey.self] = newValue }
    }
}

private struct ThemeKey: EnvironmentKey {
    /// Views shown without an injected theme fall back to the app default, Dark (D90).
    static let defaultValue = Theme.dark
}

extension EnvironmentValues {
    var theme: Theme {
        get { self[ThemeKey.self] }
        set { self[ThemeKey.self] = newValue }
    }
}

/// Header / composer control circle: frosted material on Glass, solid field on Matte.
struct GlassCircle<Content: View>: View {
    @Environment(\.theme) private var theme
    var size: CGFloat = Theme.control
    @ViewBuilder var content: () -> Content

    var body: some View {
        ZStack {
            if theme.isGlass {
                Circle().fill(.ultraThinMaterial)
            }
            Circle().fill(theme.field.opacity(theme.isGlass ? 0.85 : 1))
            Circle().strokeBorder(theme.fieldBorder, lineWidth: 1)
            content()
        }
        .frame(width: size, height: size)
        // Grok's circles are 42 pt; the touch target stays ≥ 44 without moving layout.
        .contentShape(Circle().inset(by: -max(0, (44 - size) / 2)))
    }
}

/// Header title pill: frosted material on Glass, solid field on Matte.
struct GlassPill<Content: View>: View {
    @Environment(\.theme) private var theme
    @ViewBuilder var content: () -> Content

    var body: some View {
        content()
            .background {
                ZStack {
                    if theme.isGlass {
                        Capsule().fill(.ultraThinMaterial)
                    }
                    Capsule().fill(theme.field.opacity(theme.isGlass ? 0.85 : 1))
                    Capsule().strokeBorder(theme.fieldBorder, lineWidth: 1)
                }
            }
    }
}
