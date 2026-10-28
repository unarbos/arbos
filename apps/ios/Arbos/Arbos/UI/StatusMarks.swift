// Status marks that are not coloured dots (Jacob, 2026-10-27, [feedback
// neon-dots-ios-2026-10-27]: "replace all neon glimmer icons like this in the
// app with braille spinners always, on a matter of principle never use colored
// dots"). Activity → the braille spinner (`WorkingSpinner`); unread → the count
// pill (`UnreadBadge`); a fault → the red "!" glyph; a resting state (awake,
// asleep, read) → nothing. On a picture the mark sits in a ring cut out of the
// picture (`CornerMark`, `View.statusMark`). `StatusDotLintTests` refuses a new small
// `Circle().fill(<status colour>)` anywhere in the app. Mirrors the Mac's
// `ui/marks.rs`.

import SwiftUI

/// What a state shows instead of its old coloured dot.
enum StatusMark: Equatable {
    /// Something is happening: the braille spinner.
    case working
    /// Something is wrong: the red exclamation glyph.
    case fault
}

/// The machine's mark: the spinner while it wakes, the fault glyph on error,
/// nothing at rest (awake, hibernated, stopped).
func machineMark(_ state: BoxState) -> StatusMark? {
    switch state {
    case .waking: return .working
    case .error: return .fault
    case .awake, .hibernatedLight, .hibernatedDeep, .stoppedByUser: return nil
    }
}

/// The red exclamation glyph at `size` points — the error note's mark, on its own.
struct FaultGlyph: View {
    @Environment(\.theme) private var theme
    var size: CGFloat = 11

    var body: some View {
        Image(systemName: "exclamationmark")
            .font(.system(size: size, weight: .bold))
            .foregroundStyle(theme.danger)
            .frame(width: size, height: size)
            .accessibilityLabel("Problem")
    }
}

/// A mark drawn in a slot of `size` points: the spinner or the fault glyph.
struct StatusMarkView: View {
    let mark: StatusMark
    var size: CGFloat = WorkingSpinnerFrames.cell

    var body: some View {
        switch mark {
        case .working:
            WorkingSpinner(size: size, glyphSize: min(size * 1.3, WorkingSpinnerFrames.glyphSize))
        case .fault:
            FaultGlyph(size: size)
        }
    }
}

/// The mark's spot on a picture (Grok's status badge: past the bottom-right corner,
/// inside a ring **cut out of the picture**, so whatever surface is behind shows through
/// as the ring). Sizes follow the picture: the glyph a quarter of it (6–12), the ring
/// 2 pt wider on each side, the ring box 2 pt past the picture's edge. Mirrors the Mac's
/// `marks::corner_metrics`.
enum CornerMark {
    static let ringWidth: CGFloat = 2
    static let overhang: CGFloat = 2
    static let glyphMin: CGFloat = 6
    static let glyphMax: CGFloat = 12

    struct Metrics: Equatable {
        let glyph: CGFloat
        let ring: CGFloat
        let overhang: CGFloat
    }

    static func metrics(_ size: CGFloat) -> Metrics {
        let glyph = min(max((size / 4).rounded(), glyphMin), glyphMax)
        return Metrics(glyph: glyph, ring: glyph + 2 * ringWidth, overhang: overhang)
    }

    /// Cuts the ring out of `content` (a `size` square) and lays the mark — the spinner
    /// or the fault glyph — over it; `content` untouched (no cut-out) when `mark` is nil.
    struct Modifier: ViewModifier {
        let mark: StatusMark?
        let size: CGFloat

        @ViewBuilder
        func body(content: Content) -> some View {
            if let mark {
                let m = CornerMark.metrics(size)
                content
                    .mask {
                        ZStack(alignment: .bottomTrailing) {
                            // Wider than the picture: a leaf's sway or a shadow must not be clipped.
                            Rectangle().padding(-size)
                            Circle()
                                .frame(width: m.ring, height: m.ring)
                                .offset(x: m.overhang, y: m.overhang)
                                .blendMode(.destinationOut)
                        }
                        .compositingGroup()
                    }
                    .overlay(alignment: .bottomTrailing) {
                        StatusMarkView(mark: mark, size: m.glyph)
                            .frame(width: m.glyph, height: m.glyph)
                            .offset(x: m.overhang - CornerMark.ringWidth, y: m.overhang - CornerMark.ringWidth)
                    }
            } else {
                content
            }
        }
    }
}

extension View {
    /// The machine mark at a picture's badge spot (`CornerMark`): every machine mark on
    /// a picture goes through here. Never a dot.
    func statusMark(_ mark: StatusMark?, size: CGFloat) -> some View {
        modifier(CornerMark.Modifier(mark: mark, size: size))
    }
}

/// The unread mark for a row: the count pill, "1" when only the flag is lit.
struct UnreadMark: View {
    let count: Int

    var body: some View {
        UnreadBadge(count: max(1, count))
    }
}
