// The call, collapsed over the chat (Ken's request: keep seeing the chat
// while on a call). The LiveKit/CallKit session is untouched — only the
// full-screen cover goes away. Voice calls become a floating pill at the
// top: call name, timer, who is speaking, mute and hang up; a tap brings the
// full call screen back. Video calls become a draggable picture-in-picture
// tile with the same controls. Grok question-card palette: white in light
// mode, near-black in dark mode.

import SwiftUI
import LiveKit

struct InCallOverlay: View {
    @ObservedObject private var calls = CallManager.shared
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    var expected: [CallPerson] = []
    /// Drawn inside the chat header (no floating offset) instead of over the screen.
    var inline = false

    @State private var pipOffset: CGSize = .zero
    @State private var pipDrag: CGSize = .zero
    @State private var pillDrag: CGFloat = 0

    private var isVideo: Bool { calls.call?.kind == .video }

    var body: some View {
        GeometryReader { geo in
            if isVideo {
                pip(in: geo.size)
            } else {
                pill
            }
        }
        .animation(.spring(duration: 0.3), value: isVideo)
    }

    // MARK: Shared bits

    private var fill: Color { theme.isDark ? theme.bubbleAssistant : .white }
    private var inner: Color { theme.isDark ? Color(hex: "#2C2C2C")! : Color(hex: "#F1F1F3")! }
    private var hairline: Color { theme.isDark ? Color.white.opacity(0.08) : Color.black.opacity(0.06) }

    /// The tile that is talking, else the first remote one, else who we expect.
    private var speaker: CallPerson? {
        calls.tiles.first { $0.isSpeaking }?.person ?? calls.tiles.first?.person ?? expected.first
            ?? calls.call?.participants.first { $0.id != model.user?.id } ?? calls.call?.startedBy
    }
    private var someoneSpeaking: Bool { calls.tiles.contains { $0.isSpeaking } }

    private var callName: String {
        if let id = calls.call?.chatId, let chat = model.peopleChat(id), chat.isGroup {
            return chat.title.isEmpty ? "Group call" : chat.title
        }
        if expected.count >= 2 { return "Group call" }
        return speaker?.displayName ?? (isVideo ? "Video call" : "Voice call")
    }

    private var statusLine: String {
        switch calls.phase {
        case .connecting: return "Calling…"
        case .reconnecting: return "Reconnecting…"
        case .connected:
            if calls.tiles.isEmpty { return "Ringing…" }
            if let s = calls.tiles.first(where: { $0.isSpeaking }) { return "\(s.person.displayName) is speaking" }
            return CallScreen.clock(calls.duration)
        case .ringing, .idle: return ""
        }
    }

    private func roundButton(_ icon: String, tint: Color? = nil, filled: Bool = false, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            ZStack {
                Circle().fill(tint ?? (filled ? theme.text : inner))
                Image(systemName: icon)
                    .font(theme.font(13, weight: .semibold))
                    .foregroundStyle(tint != nil ? .white : (filled ? theme.bg : theme.text))
            }
            .frame(width: 32, height: 32)
            .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }

    private var listenOnly: Bool {
        calls.call?.isHostedSpeak == true && calls.call?.canSpeak == false
    }

    private var muteButton: some View {
        roundButton(
            (calls.muted || listenOnly) ? "mic.slash.fill" : "mic.fill",
            filled: calls.muted || listenOnly,
            label: listenOnly ? "Listen only" : (calls.muted ? "Unmute" : "Mute")
        ) {
            guard !listenOnly else { return }
            Task { await calls.toggleMute() }
        }
    }

    private var hangUpButton: some View {
        roundButton("phone.down.fill", tint: theme.danger, label: "Hang up") { Task { await calls.hangUp() } }
    }

    // MARK: Voice pill

    private var pill: some View {
        VStack {
            TimelineView(.periodic(from: .now, by: 1)) { _ in
                HStack(spacing: 10) {
                    if let p = speaker {
                        CallPersonAvatar(person: p, size: 30)
                            .overlay(Circle().strokeBorder(theme.online, lineWidth: someoneSpeaking ? 2 : 0).frame(width: 36, height: 36))
                    } else {
                        ZStack {
                            Circle().fill(inner).frame(width: 30, height: 30)
                            Image(systemName: "phone.fill").font(.system(size: 12, weight: .semibold)).foregroundStyle(theme.muted)
                        }
                    }
                    VStack(alignment: .leading, spacing: 1) {
                        Text(callName)
                            .font(theme.font(Theme.small, weight: .semibold))
                            .foregroundStyle(theme.text)
                            .lineLimit(1)
                        HStack(spacing: 5) {
                            if someoneSpeaking { PulseDot(color: theme.online) }
                            Text(statusLine)
                                .font(theme.font(Theme.tiny))
                                .foregroundStyle(someoneSpeaking ? theme.online : theme.muted)
                                .lineLimit(1)
                                .monospacedDigit()
                        }
                    }
                    Spacer(minLength: 6)
                    muteButton
                    hangUpButton
                }
                .padding(.leading, 8).padding(.trailing, 8).padding(.vertical, 7)
                .background(Capsule().fill(fill).shadow(color: .black.opacity(theme.isDark ? 0.5 : 0.12), radius: 14, y: 6))
                .overlay(Capsule().strokeBorder(hairline, lineWidth: 1))
                .contentShape(Capsule())
                .onTapGesture { calls.restore() }
                .offset(y: min(0, pillDrag))
                .gesture(
                    DragGesture(minimumDistance: 8)
                        .onChanged { v in pillDrag = v.translation.height }
                        .onEnded { v in
                            withAnimation(.spring(duration: 0.3)) { pillDrag = 0 }
                            if v.translation.height > 40 { calls.restore() }
                        }
                )
                .accessibilityElement(children: .combine)
                .accessibilityLabel("\(callName), \(statusLine). Double-tap to return to the call")
                .accessibilityAddTraits(.isButton)
            }
            .padding(.horizontal, inline ? 16 : 12)
            // Floating: sit just under the screen's header so Back / More stay reachable.
            .padding(.top, inline ? 0 : 56)
            Spacer(minLength: 0)
        }
        .transition(.move(edge: .top).combined(with: .opacity))
    }

    // MARK: Video picture-in-picture

    private var pipSize: CGSize { CGSize(width: 132, height: 188) }

    private func pip(in size: CGSize) -> some View {
        let margin: CGFloat = 12
        let home = CGPoint(x: size.width - pipSize.width / 2 - margin, y: pipSize.height / 2 + 60)
        let remote = calls.tiles.first { $0.cameraOn && $0.videoTrack != nil } ?? calls.tiles.first
        return ZStack(alignment: .bottom) {
            RoundedRectangle(cornerRadius: 18, style: .continuous).fill(fill)
            if let track = remote?.videoTrack, remote?.cameraOn == true {
                SwiftUIVideoView(track, layoutMode: .fill)
                    .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
            } else if calls.cameraOn, let local = calls.localVideoTrack {
                SwiftUIVideoView(local, layoutMode: .fill, mirrorMode: .mirror)
                    .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
            } else if let p = speaker {
                // Camera off: the person sits in the area above the bottom strip.
                VStack(spacing: 5) {
                    CallPersonAvatar(person: p, size: 48)
                        .overlay(Circle().strokeBorder(theme.online, lineWidth: someoneSpeaking ? 2 : 0).frame(width: 56, height: 56))
                    Image(systemName: "video.slash.fill").font(.system(size: 11, weight: .semibold)).foregroundStyle(theme.muted)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .padding(.bottom, 86)
            }
            // Bottom strip: name/timer and the two controls.
            TimelineView(.periodic(from: .now, by: 1)) { _ in
                VStack(alignment: .leading, spacing: 6) {
                    Text(callName).font(theme.font(11, weight: .semibold)).foregroundStyle(theme.text).lineLimit(1)
                    HStack(spacing: 4) {
                        if someoneSpeaking { PulseDot(color: theme.online) }
                        Text(statusLine).font(theme.font(10)).foregroundStyle(someoneSpeaking ? theme.online : theme.muted).lineLimit(1).monospacedDigit()
                    }
                    HStack(spacing: 8) {
                        muteButton
                        hangUpButton
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(8)
                .background(
                    UnevenRoundedRectangle(bottomLeadingRadius: 18, bottomTrailingRadius: 18, style: .continuous)
                        .fill(fill.opacity(0.92))
                )
            }
        }
        .frame(width: pipSize.width, height: pipSize.height)
        // §11: the agent is sampling our camera (shown on the tile whatever it displays).
        .overlay(alignment: .top) { AgentSeesBadge(compact: true).padding(.top, 6) }
        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(hairline, lineWidth: 1))
        .shadow(color: .black.opacity(theme.isDark ? 0.5 : 0.15), radius: 16, y: 8)
        .contentShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
        .onTapGesture { calls.restore() }
        .position(x: home.x + pipOffset.width + pipDrag.width, y: home.y + pipOffset.height + pipDrag.height)
        .gesture(
            DragGesture(minimumDistance: 6)
                .onChanged { v in pipDrag = v.translation }
                .onEnded { v in
                    // Snap to the nearest corner, kept inside the safe area.
                    let x = home.x + pipOffset.width + v.predictedEndTranslation.width
                    let y = home.y + pipOffset.height + v.predictedEndTranslation.height
                    let left = x < size.width / 2
                    let top = y < size.height / 2
                    let targetX = left ? pipSize.width / 2 + margin : size.width - pipSize.width / 2 - margin
                    let targetY = top ? pipSize.height / 2 + 60 : size.height - pipSize.height / 2 - 96
                    withAnimation(.spring(duration: 0.35)) {
                        pipOffset = CGSize(width: targetX - home.x, height: targetY - home.y)
                        pipDrag = .zero
                    }
                }
        )
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Video call, \(callName). Double-tap to return to the call")
        .transition(.scale(scale: 0.9).combined(with: .opacity))
    }
}
