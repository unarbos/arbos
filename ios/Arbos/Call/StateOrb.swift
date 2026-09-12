import SwiftUI

/// The one big signal on screen. Colour says who is talking; breathing
/// speed says how alive the call is.
struct StateOrb: View {
    let phase: CallViewModel.Phase

    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 30)) { context in
            let t = context.date.timeIntervalSinceReferenceDate
            let breath = sin(t * 2 * .pi / period) * 0.5 + 0.5
            ZStack {
                Circle()
                    .fill(tint.opacity(0.18 + 0.18 * breath * glowStrength))
                    .frame(width: 260, height: 260)
                    .blur(radius: 40)
                    .scaleEffect(1 + 0.12 * breath * glowStrength)
                Circle()
                    .fill(
                        RadialGradient(
                            colors: [tint.opacity(0.95), tint.opacity(0.55)],
                            center: .init(x: 0.38, y: 0.32),
                            startRadius: 4,
                            endRadius: 110
                        )
                    )
                    .frame(width: 180, height: 180)
                    .scaleEffect(1 + 0.04 * breath * glowStrength)
                    .shadow(color: tint.opacity(0.35), radius: 24, y: 8)
            }
        }
        .animation(.easeInOut(duration: 0.6), value: phase)
    }

    private var tint: Color {
        switch phase {
        case .idle: return Color(white: 0.32)
        case .unconfigured: return Color(white: 0.22)
        case .connecting: return Color(white: 0.45)
        case .listening: return Color(red: 0.36, green: 0.80, blue: 0.64)
        case .thinking: return Color(red: 0.92, green: 0.72, blue: 0.36)
        case .speaking: return Color(red: 0.48, green: 0.58, blue: 0.98)
        case .failed: return Color(red: 0.86, green: 0.36, blue: 0.36)
        }
    }

    /// Seconds per breath.
    private var period: Double {
        switch phase {
        case .idle, .unconfigured, .failed: return 6
        case .connecting: return 1.2
        case .listening: return 3
        case .thinking: return 1
        case .speaking: return 0.7
        }
    }

    private var glowStrength: Double {
        switch phase {
        case .idle, .unconfigured, .failed: return 0.4
        case .connecting, .listening, .thinking, .speaking: return 1
        }
    }
}
