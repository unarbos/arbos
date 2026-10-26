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
    static func == (a: Theme, b: Theme) -> Bool {
        let same = a.isDark == b.isDark && a.surface == b.surface && a.textScale == b.textScale
            && a.bg == b.bg && a.bubbleAssistant == b.bubbleAssistant && a.bubbleUser == b.bubbleUser
            && a.field == b.field && a.fieldBorder == b.fieldBorder && a.text == b.text && a.muted == b.muted
            && a.placeholder == b.placeholder && a.icon == b.icon && a.accent == b.accent && a.unread == b.unread
            && a.online == b.online && a.warn == b.warn && a.danger == b.danger && a.dangerSoft == b.dangerSoft
            && a.card == b.card && a.separator == b.separator && a.codeChip == b.codeChip && a.userCodeChip == b.userCodeChip
        #if DEBUG
        PerfCounters.bump(same ? "themeEq.true" : "themeEq.false")
        #endif
        return same
    }
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
    /// Round icon button (back, search, +, account): one diameter app-wide — `Controls.diameter`.
    static let control: CGFloat = Controls.diameter
    /// Text fields, pills and bars: one height app-wide — `Controls.height`.
    static let fieldHeight: CGFloat = Controls.height
    /// In-call strip chip ([feedback in-call-bar]); recorded exception, see `Controls.exceptions`.
    static let voiceButton: CGFloat = 36

    // MARK: Chat composer — Grok Bot, Jacob's 2026-10-25 target shot
    // ([feedback iphone-composer-grok-parity-2026-10-25]; iPhone Air 420 pt, 3 px/pt,
    // keyboard up). Every number below is that shot's pixel count / 3; the structure
    // tests in `ComposerMetricsTests` pin them. Grok changed its composer between
    // the 2026-10-15 refs (42 pt, no rim, 36 pt white circle) and this shot.

    /// The `+` disc and the field are the same height: 144 px — and, since
    /// [feedback iphone-header-composer-sizing-2026-10-26], the whole control family's.
    static let composerHeight: CGFloat = Controls.height
    static let plusButton: CGFloat = Controls.diameter
    /// `+` glyph: SF `plus`, regular; 54 px arms (22 pt regular draws 53, 22.5 draws 54).
    static let plusGlyph: CGFloat = 22.5
    /// `+` disc → field: 30 px (= `Controls.gap`).
    static let plusFieldGap: CGFloat = Controls.gap
    /// The field's text starts 51 px in from its outer edge (the caret's centre; = `Controls.textInset`).
    static let composerTextInset: CGFloat = Controls.textInset
    /// One 22 pt body line centred in the 48 pt field.
    static let composerTextPadY: CGFloat = (composerHeight - 22) / 2
    /// The white action is a 120 × 96 px capsule (was a 36 pt circle), 24 px from the
    /// field's top and bottom and 26 px from its right edge.
    static let actionPillWidth: CGFloat = 40
    static let actionPillHeight: CGFloat = 32
    static let actionPillTrailing: CGFloat = 26 / 3
    /// Mic glyph centre → action pill: 72 px (the glyph sits in a 44 pt hit frame, so
    /// 22 + `micPillGap`).
    static let micHitWidth: CGFloat = 44
    static let micPillGap: CGFloat = 2
    /// Mic glyph box (Grok's filled mic has no base bar; drawn by `MicGlyph`): 36 × 49 px.
    static let micGlyphWidth: CGFloat = 12
    static let micGlyphHeight: CGFloat = 49 / 3
    /// Field bottom → keyboard with the keyboard up: 54 px (was 24).
    static let composerBottomKeyboard: CGFloat = 18
    /// Keyboard down: the composer sits 2 pt above the home indicator's safe area.
    static let composerBottom: CGFloat = 2
    /// The rim on the `+` disc and the field (dark): brightest at the outer pixel and
    /// fading in over 1 pt — 58 / 47 / 40 luminance over the #181818 fill, measured on the
    /// top, bottom, left and right edges alike. Three nested `strokeBorder`s (outer
    /// 1/3 pt, 2/3 pt, 1 pt) reproduce the three pixels. Light keeps `fieldBorder` at 1 pt.
    static let composerRimLayers: [(width: CGFloat, alpha: Double)] = [
        (1, 0.069), (2 / 3, 0.033), (1 / 3, 0.053),
    ]

    static let avatar: CGFloat = 28
    /// The avatar column (Mac `SENDER_DISC = PLUS_BUTTON`, Jacob [feedback
    /// AAvkkHYz3TOlou0Hu7u71Sc]): every round picture beside a people-chat bubble is
    /// centred on the composer's `+`. The disc stayed 42 pt when the `+` grew to Grok's
    /// 48 ([feedback iphone-composer-grok-parity-2026-10-25]: the ask covered the composer
    /// and the status row, not the bubbles). The `+` moves with the keyboard (31 → 18 pt
    /// inset), so the column follows it via `avatarColumnInset(composerInset:)` (keyboard
    /// up: avatar 21–63, centre 42 = 18 + 48 / 2; down: 34–76, centre 55). The bubble
    /// body follows after `memberAvatarGap`. Shared with
    /// feedback/sender-name-in-bubble-2026-10-23. Agent chats keep #1374's no-avatar rule.
    static let memberAvatar: CGFloat = 42
    static let memberAvatarGap: CGFloat = 13
    /// Transcript edge → the avatar's left edge, for the composer's current side inset.
    static func avatarColumnInset(composerInset: CGFloat) -> CGFloat {
        composerInset + plusButton / 2 - memberAvatar / 2 - 16
    }
    /// The status row over the composer (Grok's "Adding glass buttons" row, same shot):
    /// a 76 px picture whose left edge sits 18 px inside the `+` disc's left edge (72 px
    /// from the screen with the keyboard up), 42 px to the words (17 pt body, muted),
    /// and 68 px between the picture's bottom and the composer's top.
    static let presenceAvatar: CGFloat = 76 / 3
    static let presenceAvatarOffset: CGFloat = 6
    static let presenceGap: CGFloat = 14
    static let presenceBottomGap: CGFloat = 68 / 3
    /// The transcript's tail: the clear space under the last row, before the chat bar's
    /// own top padding, so the last bubble never sits under the bar's fade.
    static let transcriptTail: CGFloat = 28
    /// The chat bar's padding over its first view (the status row or the composer).
    static let chatBarTopPad: CGFloat = 8
    /// Last bubble → the status picture's top: 42 px in the same Grok shot ([feedback
    /// status-row-gap-ios-2026-10-26]). The row used to start under the tail and the
    /// bar's pad (36 pt); it now rises `presenceLift` into the tail, so the bubble →
    /// composer distance with the row showing is Grok's 62⅓ (14 + 25⅓ + 22⅔) and the
    /// bar grows by 26 pt instead of 48 when the row appears.
    static let presenceTopGap: CGFloat = 42 / 3
    static var presenceLift: CGFloat { transcriptTail + chatBarTopPad - presenceTopGap }
    /// Transcript edge → the status row's picture, for the composer's current side inset.
    static func presenceInset(composerInset: CGFloat) -> CGFloat {
        composerInset + presenceAvatarOffset - 16
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
    /// Composer side inset: Grok uses 31 pt with the keyboard down (2026-10-15 refs), 18 pt
    /// with it up (54 px on both sides in Jacob's 2026-10-25 shot; was read as 19).
    static let composerInset: CGFloat = 31
    static let composerInsetKeyboard: CGFloat = 18
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

// MARK: - Chat header glass ([feedback ios-header-glass-2026-10-25], Jacob 2026-10-25)

/// What a chat-header control is made of.
///
/// Jacob's reference is Grok Bot's header (back · name pill · ⋯): iOS 26 Liquid
/// Glass, the transcript visibly blurred through the pills, a light rim. Ours
/// painted `.ultraThinMaterial` under `field` at 85 % — a near-solid disc on
/// every chat kind. On Glass the header now takes the real thing; Matte keeps
/// its solid controls (Matte means no translucency).
enum HeaderMaterial: Equatable, Hashable {
    /// iOS 26+: `.glassEffect` — lensing rim, adaptive tint, blur-through.
    case liquidGlass
    /// Older iOS: `.ultraThinMaterial`, a light tint and a top-lit hairline rim.
    case frosted
    /// Matte: the field colour, as before.
    case solid
}

/// The controls every chat header shows, left to right.
enum HeaderControl: Equatable, CaseIterable {
    case back, titlePill, search
}

/// The chat kinds the header serves — one header, one spec, for all of them.
enum ChatHeaderKind: Equatable, CaseIterable {
    case agent, dm, group, channel
}

/// The chat header's structure and material (read by `ChatView` and by
/// `HeaderGlassStructureTests`): the same three controls and one material for
/// every kind; the Search-in-chat bar under them wears the same material.
enum ChatHeaderGlass {
    static var liquidGlassAvailable: Bool {
        if #available(iOS 26.0, *) { return true } else { return false }
    }

    static func material(surface: Surface, liquidGlassAvailable: Bool = liquidGlassAvailable) -> HeaderMaterial {
        guard surface == .glass else { return .solid }
        return liquidGlassAvailable ? .liquidGlass : .frosted
    }

    /// Back, the title pill, search — for an agent, a DM, a group and a channel alike.
    static func controls(for kind: ChatHeaderKind) -> [HeaderControl] {
        _ = kind
        return [.back, .titlePill, .search]
    }

    /// The material of each control in a kind's header, in order.
    static func materials(kind: ChatHeaderKind, surface: Surface, liquidGlassAvailable: Bool = liquidGlassAvailable) -> [(HeaderControl, HeaderMaterial)] {
        let m = material(surface: surface, liquidGlassAvailable: liquidGlassAvailable)
        return controls(for: kind).map { ($0, m) }
    }

    /// The Search-in-chat bar (Jacob 22:53): the header's material, not the flat field.
    static func searchBarMaterial(surface: Surface, liquidGlassAvailable: Bool = liquidGlassAvailable) -> HeaderMaterial {
        material(surface: surface, liquidGlassAvailable: liquidGlassAvailable)
    }

    /// Fallback frost (iOS < 26): how much of `field` sits over the material.
    static let frostedTint: Double = 0.35
    /// The Search-in-chat bar's Liquid Glass tint: the system blends a tint far
    /// lighter than a fill, and a whole line of transcript runs under the bar.
    static let searchBarTint: Double = 0.55
    /// Tint under the legacy glass controls elsewhere (home header, sheets).
    static let legacyTint: Double = 0.85
}

/// The glass behind a header control: one shape, three materials.
struct HeaderGlassBackground<S: InsettableShape>: View {
    @Environment(\.theme) private var theme
    let shape: S
    let material: HeaderMaterial
    /// Buttons take the interactive glass (press feedback); the pill and the bar do not.
    var interactive: Bool = false
    /// A wide piece (the Search-in-chat bar) shows a whole line of transcript through
    /// clear glass; a light `field` tint keeps its placeholder readable. Discs and the pill
    /// stay untinted, as in the reference.
    var tinted: Bool = false

    var body: some View {
        switch material {
        case .liquidGlass:
            if #available(iOS 26.0, *) {
                // The family's rim (`ComposerRim`) over the system glass: header and
                // composer share one outline weight.
                Color.clear.glassEffect(liquid, in: shape).overlay(ComposerRim(shape: shape))
            } else {
                frosted
            }
        case .frosted:
            frosted
        case .solid:
            ControlSurface(shape: shape)
        }
    }

    @available(iOS 26.0, *)
    private var liquid: Glass {
        let glass: Glass = interactive ? .regular.interactive() : .regular
        return tinted ? glass.tint(theme.field.opacity(ChatHeaderGlass.searchBarTint)) : glass
    }

    /// `.ultraThinMaterial`, a thin tint so the content stays visible through it, and a
    /// rim lit from the top (the specular edge of the reference) — see the Mac's
    /// `Theme::header_glass()` for the same recipe in quads.
    private var frosted: some View {
        ZStack {
            shape.fill(.ultraThinMaterial)
            shape.fill(theme.field.opacity(ChatHeaderGlass.frostedTint))
            ComposerRim(shape: shape)
        }
    }
}

/// Header / composer control circle: frosted material on Glass, solid field on Matte.
/// `material` (the chat header) picks Liquid Glass / frost / solid explicitly.
struct GlassCircle<Content: View>: View {
    @Environment(\.theme) private var theme
    var size: CGFloat = Theme.control
    var material: HeaderMaterial? = nil
    @ViewBuilder var content: () -> Content

    var body: some View {
        ZStack {
            if let material {
                HeaderGlassBackground(shape: Circle(), material: material, interactive: true)
            } else {
                if theme.isGlass {
                    Circle().fill(.ultraThinMaterial)
                }
                Circle().fill(theme.field.opacity(theme.isGlass ? ChatHeaderGlass.legacyTint : 1))
                ComposerRim(shape: Circle())
            }
            content()
        }
        .frame(width: size, height: size)
        // The touch target stays ≥ 44 for the small exception discs without moving layout.
        .contentShape(Circle().inset(by: -max(0, (44 - size) / 2)))
    }
}

/// The composer's rim (`Theme.composerRimLayers`) on the `+` disc and the field: in the
/// dark theme an edge highlight that fades in over 1 pt, in light the theme's 1 pt
/// `fieldBorder`. `tint` replaces the rim while recording / waiting for an answer.
struct ComposerRim<S: InsettableShape>: View {
    @Environment(\.theme) private var theme
    let shape: S
    var tint: Color? = nil

    var body: some View {
        if let tint {
            shape.strokeBorder(tint, lineWidth: 1)
        } else if theme.isDark {
            ZStack {
                ForEach(Array(Theme.composerRimLayers.enumerated()), id: \.offset) { _, layer in
                    shape.strokeBorder(Color.white.opacity(layer.alpha), lineWidth: layer.width)
                }
            }
        } else {
            shape.strokeBorder(theme.fieldBorder, lineWidth: 1)
        }
    }
}

/// Grok's composer mic (dark `#96969C`): a filled capsule head, a U cradle and a short
/// stem — no base bar, unlike SF `mic.fill`. Drawn in points so the 3x simulator and
/// Jacob's shot agree pixel for pixel: box 12 × 16⅓ pt (36 × 49 px); head 17 × 31 px at
/// x 9; cradle 36 px wide, 4.5 px stroke, arms from y 16, centre y 23, outer radius 18;
/// stem 4.5 px wide from y 41 to 49, round end.
struct MicGlyph: Shape {
    static let size = CGSize(width: Theme.micGlyphWidth, height: Theme.micGlyphHeight)
    static let headWidth: CGFloat = 17 / 3
    static let headHeight: CGFloat = 31 / 3
    static let cradleStroke: CGFloat = 1.5
    static let cradleArmTop: CGFloat = 16 / 3
    static let cradleCenterY: CGFloat = 23 / 3
    static let stemBottom: CGFloat = 49 / 3

    func path(in rect: CGRect) -> Path {
        let s = rect.width / Self.size.width
        let cx = rect.minX + rect.width / 2
        var p = Path()
        // Head.
        let head = CGRect(x: cx - Self.headWidth * s / 2, y: rect.minY, width: Self.headWidth * s, height: Self.headHeight * s)
        p.addPath(Capsule().path(in: head))
        // Cradle: a U drawn as a stroked path.
        let outerR = Self.size.width * s / 2
        let stroke = Self.cradleStroke * s
        var u = Path()
        u.move(to: CGPoint(x: cx - outerR + stroke / 2, y: rect.minY + Self.cradleArmTop * s))
        u.addLine(to: CGPoint(x: cx - outerR + stroke / 2, y: rect.minY + Self.cradleCenterY * s))
        u.addArc(center: CGPoint(x: cx, y: rect.minY + Self.cradleCenterY * s), radius: outerR - stroke / 2,
                 startAngle: .degrees(180), endAngle: .degrees(0), clockwise: true)
        u.addLine(to: CGPoint(x: cx + outerR - stroke / 2, y: rect.minY + Self.cradleArmTop * s))
        p.addPath(u.strokedPath(StrokeStyle(lineWidth: stroke, lineCap: .butt)))
        // Stem: from the cradle's outer bottom to the box's bottom, round end.
        let stemTop = rect.minY + (Self.cradleCenterY * s + outerR) - stroke
        let stem = CGRect(x: cx - stroke / 2, y: stemTop, width: stroke, height: rect.minY + Self.stemBottom * s - stemTop)
        p.addPath(RoundedRectangle(cornerRadius: stroke / 2, style: .circular).path(in: stem))
        return p
    }
}

/// Grok's voice-mode glyph in the composer's white pill: five round-capped bars, 2 pt
/// wide, 3.25 pt apart, 6.5 / 14.5 / 8.5 / 14.5 / 6.5 pt tall, centred in the pill.
/// Arbos keeps its call semantics behind it (a tap starts a voice call).
struct VoiceWaveGlyph: Shape {
    static let barWidth: CGFloat = 2
    static let pitch: CGFloat = 3.25
    static let heights: [CGFloat] = [6.5, 14.5, 8.5, 14.5, 6.5]

    func path(in rect: CGRect) -> Path {
        var p = Path()
        let cx = rect.midX, cy = rect.midY
        let n = Self.heights.count
        for (i, h) in Self.heights.enumerated() {
            let x = cx + (CGFloat(i) - CGFloat(n - 1) / 2) * Self.pitch
            let bar = CGRect(x: x - Self.barWidth / 2, y: cy - h / 2, width: Self.barWidth, height: h)
            p.addPath(Capsule().path(in: bar))
        }
        return p
    }
}

/// Header title pill: frosted material on Glass, solid field on Matte.
/// `material` (the chat header) picks Liquid Glass / frost / solid explicitly.
struct GlassPill<Content: View>: View {
    @Environment(\.theme) private var theme
    var material: HeaderMaterial? = nil
    @ViewBuilder var content: () -> Content

    var body: some View {
        content()
            .background {
                if let material {
                    HeaderGlassBackground(shape: Capsule(), material: material)
                } else {
                    ZStack {
                        if theme.isGlass {
                            Capsule().fill(.ultraThinMaterial)
                        }
                        Capsule().fill(theme.field.opacity(theme.isGlass ? ChatHeaderGlass.legacyTint : 1))
                        ComposerRim(shape: Capsule())
                    }
                }
            }
    }
}

// MARK: - The iPhone control family ([feedback iphone-header-composer-sizing-2026-10-26])

/// One set of sizes for every floating or round control on the iPhone: round icon
/// buttons, name/title pills, text fields, search bars (Jacob 2026-10-26 08:49–08:52 CR;
/// `docs/mac-design-rules.md` § "iPhone control family"). The numbers are the composer's
/// (#1841, Grok Bot's 2026-10-25 shot): the `+` disc and the field set the family, the
/// header, home, search and sheet controls follow. `ControlMetricsTests` pins the relations
/// and `ControlTokenLintTests` fails the gate on a hand-sized control outside this file.
///
/// Mac sizes are untouched (Jacob 08:52): the Mac keeps its own `CONTROL` family.
enum Controls {
    /// Round icon button diameter = the composer `+`.
    static let diameter: CGFloat = 48
    /// Pill, text field and bar height = the composer field.
    static let height: CGFloat = 48
    /// Capsule corners; a field that grows keeps this radius (the composer's rule).
    static let radius: CGFloat = height / 2
    /// SF symbol inside a disc (chevron, magnifier, ellipsis, xmark): 17 in the old 42 → 19.
    static let glyph: CGFloat = 19
    /// A top bar's side inset: bar → screen edge (home used 20, chat 16 — one number now).
    static let inset: CGFloat = 16
    /// A top bar's gap to the safe-area top.
    static let top: CGFloat = 6
    /// Two adjacent controls (the composer's `+` → field).
    static let gap: CGFloat = 10
    /// Text start inside a pill or field (the composer's caret inset).
    static let textInset: CGFloat = 17
    /// Picture inside a title pill, `avatarInset` from the capsule's end on every side.
    static let avatar: CGFloat = 32
    static var avatarInset: CGFloat { (height - avatar) / 2 }
    /// Outline: `ComposerRim` (#1841's thin light rim) on every member of the family.

    /// Surfaces that keep their own size on purpose, each with its reason. A lint
    /// exception marker in code (`// control-size: exception <case>`) must name one.
    enum Exception: String, CaseIterable {
        /// The composer's side inset follows the keyboard (31 down / 18 up, Grok's).
        case composerSideInset
        /// The composer `+` glyph is Grok's 22.5 pt plus; the family glyph is 19.
        case plusGlyph
        /// Call screen: 64 pt action discs, 108 pt avatar disc, 150 pt ring (Grok call UI).
        case callControls
        /// In-call strip chip and voice button: 36 pt (the strip is 48 tall).
        case inCallStrip
        /// Grok's 30 pt filter chips (All · Agents · People · Servers) and pane tab pills.
        case filterChips
        /// Row-leading 36 pt icon tiles (home banners, sheet rows, the camera badge on a hero).
        case rowIconTiles
        /// 72 / 92 / 112 pt illustration discs (primer, feedback done, link, hero pictures).
        case illustrationDiscs
        /// Grok's 36 pt scroll-to-bottom circle and the "New messages" chip in the transcript.
        case transcriptJumpChip
        /// Small inline chips and segmented controls (32 pt or less) inside cards.
        case inlineChips
        /// Full-width primary CTAs (sign-in, primer, create): 50 pt filled capsules, not fields.
        case primaryCTA
        /// Free-form text areas and cards (feedback note, description): 18 pt corners, grow.
        case textAreas
    }
    static let exceptions: [Exception] = Exception.allCases

    /// What a member of the family is, and its one size.
    enum Member: Equatable, CaseIterable {
        case disc, pill, field, bar
        var size: CGFloat {
            switch self {
            case .disc: return Controls.diameter
            case .pill, .field, .bar: return Controls.height
            }
        }
    }

    /// The main iPhone screens and the family members each one shows, left to right /
    /// top to bottom — the structure the snapshot test (`ControlMetricsTests`) walks.
    enum Screen: String, CaseIterable {
        case home, hiddenChats, chatHeader, chatSearchBar, composer, bottomSearchBar, serverHeader
        case settingsProfileFields, personPaneMessagePill, newMessageSheetSearch, signIn, secretsFields, callHeader
        var members: [Member] {
            switch self {
            case .home: return [.disc, .disc]
            case .hiddenChats: return [.disc, .pill]
            case .chatHeader: return [.disc, .pill, .disc]
            case .chatSearchBar: return [.bar]
            case .composer: return [.disc, .field]
            case .bottomSearchBar: return [.bar]
            case .serverHeader: return [.disc, .pill, .disc]
            case .settingsProfileFields: return [.field, .field]
            case .personPaneMessagePill: return [.pill]
            case .newMessageSheetSearch: return [.field]
            case .signIn: return [.field, .field]
            case .secretsFields: return [.field]
            case .callHeader: return [.disc]
            }
        }
        /// Side inset of the screen's bar; the composer follows the keyboard (`Exception.composerSideInset`).
        var sideInset: CGFloat? {
            switch self {
            case .composer: return nil
            default: return Controls.inset
            }
        }
    }
}

/// A field or pill of the family: the `field` fill on the family capsule and the
/// composer's rim. Use `.controlField()` / `.controlPill()`; the lint flags hand-built ones.
struct ControlSurface<S: InsettableShape>: View {
    @Environment(\.theme) private var theme
    let shape: S
    var pressed: Bool = false

    var body: some View {
        ZStack {
            shape.fill(theme.field)
            if pressed { shape.fill(theme.text.opacity(0.12)) }
            ComposerRim(shape: shape)
        }
    }
}

extension View {
    /// A one-line text field or search bar of the family: `Controls.height` tall,
    /// `Controls.textInset` text start, capsule, `field` fill, composer rim.
    func controlField() -> some View {
        self
            .padding(.horizontal, Controls.textInset)
            .frame(height: Controls.height)
            .background(ControlSurface(shape: Capsule()))
            .contentShape(Capsule())
    }

    /// A text area of the family that grows from one line: the composer's rule, the
    /// radius stays `Controls.radius` as it grows.
    func controlFieldGrowing() -> some View {
        self
            .padding(.horizontal, Controls.textInset)
            .padding(.vertical, (Controls.height - 22) / 2)
            .frame(minHeight: Controls.height, alignment: .topLeading)
            .background(ControlSurface(shape: RoundedRectangle(cornerRadius: Controls.radius, style: .circular)))
    }

    /// A text pill button of the family (Message, Add, Done): `Controls.height` tall.
    func controlPill(pressed: Bool = false) -> some View {
        self
            .padding(.horizontal, Controls.textInset)
            .frame(height: Controls.height)
            .background(ControlSurface(shape: Capsule(), pressed: pressed))
            .contentShape(Capsule())
    }

    /// A top bar of the family: the same side inset and top gap on every screen.
    func controlBar() -> some View {
        self.padding(.horizontal, Controls.inset).padding(.top, Controls.top)
    }
}
