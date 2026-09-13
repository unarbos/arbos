import SwiftUI

/// What was said, most recent at the bottom. Jacob's words are bright;
/// Arbos's replies are dim. Older lines fade so the eye lands on the last.
struct TranscriptView: View {
    let lines: [TranscriptLine]

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView(showsIndicators: false) {
                VStack(alignment: .leading, spacing: 14) {
                    ForEach(Array(lines.enumerated()), id: \.element.id) { index, line in
                        Text(line.text)
                            .font(line.speaker == .user ? .title3 : .callout)
                            .fontWeight(line.speaker == .user ? .medium : .regular)
                            .foregroundStyle(color(for: line, at: index))
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .id(line.id)
                            .transition(.opacity)
                    }
                }
                .padding(.horizontal, 28)
                .animation(.easeOut(duration: 0.2), value: lines)
            }
            .mask(
                LinearGradient(
                    stops: [
                        .init(color: .clear, location: 0),
                        .init(color: .black, location: 0.25),
                        .init(color: .black, location: 1),
                    ],
                    startPoint: .top,
                    endPoint: .bottom
                )
            )
            .onChange(of: lines) { _, new in
                guard let last = new.last else { return }
                withAnimation(.easeOut(duration: 0.2)) {
                    proxy.scrollTo(last.id, anchor: .bottom)
                }
            }
        }
    }

    private func color(for line: TranscriptLine, at index: Int) -> Color {
        let age = Double(lines.count - 1 - index)
        let fade = max(0.35, 1 - age * 0.2)
        switch line.speaker {
        case .user: return Color.white.opacity(fade)
        case .arbos: return Color.white.opacity(0.55 * fade)
        }
    }
}
