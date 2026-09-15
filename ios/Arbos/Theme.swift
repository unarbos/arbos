import SwiftUI

/// The desktop's palette (`desktop/src/view/palette.rs`), radii
/// (`root.rs`, `transcript.rs`) and type scale, so the phone reads as the
/// same material as the Mac app. Dark only for now: the desktop is dark
/// by default and Jacob's phone runs dark.
enum ArbosTheme {
    // Panels.
    static let bg = Color(hex: 0x161514)
    static let surface = Color(hex: 0x1a1a16)
    static let card = Color(hex: 0x212121)
    static let raised = Color(hex: 0x212121)
    static let raisedHover = Color(hex: 0x2a2a2a)
    static let overlay = Color(hex: 0x242424)
    static let inputBg = Color(hex: 0x212121)
    static let border = Color.white.opacity(0.11)
    static let borderStrong = Color.white.opacity(0.18)
    static let elementActive = Color.white.opacity(0.05)

    // Ink.
    static let text = Color(hex: 0xf0f0f0)
    static let textMuted = Color(hex: 0xbbbbbb)
    static let textFaint = Color(hex: 0x999898)
    static let textDim = Color(hex: 0x6b6b6b)
    static let accent = Color(hex: 0x86aee4)
    static let danger = Color(hex: 0xe5533d)
    static let ok = Color(hex: 0x3dbd6e)

    // Radii, as the desktop draws them.
    static let composerRadius: CGFloat = 14
    static let promptRadius: CGFloat = 10
    static let cardRadius: CGFloat = 8
    static let controlRadius: CGFloat = 6

    // Spacing.
    static let gutter: CGFloat = 14
    static let promptPadX: CGFloat = 12
    static let promptPadY: CGFloat = 10
    static let rowGap: CGFloat = 6
    static let itemGap: CGFloat = 8

    /// The desktop sets prose at 14 pt on a Mac. A phone is read at
    /// arm's length, so the whole scale is one point up; every other
    /// size keeps the desktop's ratio to the body.
    static let bodySize: CGFloat = 15
    static let calloutSize: CGFloat = 13.5
    static let captionSize: CGFloat = 11.5
    static let monoSize: CGFloat = 12.5

    static let body = Font.system(size: bodySize)
    static let bodyMedium = Font.system(size: bodySize, weight: .medium)
    static let callout = Font.system(size: calloutSize)
    static let calloutMedium = Font.system(size: calloutSize, weight: .medium)
    static let caption = Font.system(size: captionSize)
    static let mono = Font.system(size: monoSize, design: .monospaced)
}

extension Color {
    init(hex: UInt32, opacity: Double = 1) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xff) / 255,
            green: Double((hex >> 8) & 0xff) / 255,
            blue: Double(hex & 0xff) / 255,
            opacity: opacity
        )
    }
}
