import SwiftUI

/// Builds the call from the environment so a project chat can present it
/// full-screen without holding the model itself.
struct CallScreen: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var chat: ChatStore
    @EnvironmentObject private var link: VoiceLink

    var body: some View {
        CallView(settings: settings, chat: chat, link: link)
            .environmentObject(settings)
            .environmentObject(chat)
            .environmentObject(link)
    }
}

/// The call: one ring that moves with the live level, the state in a
/// word, the last thing said, and one way to hang up. Designed to be
/// glanced at with AirPods in: few elements, generous space, legible at
/// arm's length. Same type, margins and palette as the two pages.
struct CallView: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var chat: ChatStore
    @Environment(\.dismiss) private var dismiss
    @StateObject private var model: CallViewModel
    @State private var showSettings = false

    init(settings: AppSettings, chat: ChatStore, link: VoiceLink) {
        _model = StateObject(wrappedValue: CallViewModel(settings: settings, chat: chat, link: link))
    }

    var body: some View {
        ZStack {
            ArbosTheme.bg.ignoresSafeArea()
            VStack(spacing: 0) {
                topBar
                Spacer(minLength: 0)
                VoiceRing(level: model.level, phase: model.phase)
                    .frame(width: 196, height: 196)
                Text(stateWord)
                    .font(ArbosTheme.bodyMedium)
                    .foregroundStyle(stateInk)
                    .padding(.top, 28)
                    .contentTransition(.opacity)
                    .animation(.easeInOut(duration: 0.2), value: model.phase)
                words
                    .padding(.top, 22)
                    .padding(.horizontal, ArbosTheme.gutter + 8)
                Spacer(minLength: 0)
                callButton
                    .padding(.bottom, 36)
            }
        }
        .preferredColorScheme(.dark)
        .sheet(isPresented: $showSettings, onDismiss: model.refreshIdle) {
            SettingsView().environmentObject(settings)
        }
        .onChange(of: settings.openAIKey) { _, _ in model.refreshIdle() }
        .onChange(of: settings.selfHostedURL) { _, _ in model.refreshIdle() }
        .onChange(of: settings.voiceToken) { _, _ in model.refreshIdle() }
        .onChange(of: settings.provider) { _, _ in model.refreshIdle() }
        .task { await chat.connect() }
        #if DEBUG
        .task { await previewIfAsked() }
        #endif
    }

    // MARK: - Pieces

    /// Back to the chat at the left; the route and the clock, small, in
    /// the middle; the menu only while there is no call to disturb.
    private var topBar: some View {
        ZStack {
            VStack(spacing: 3) {
                Text(chat.identity?.label ?? chat.title)
                    .font(ArbosTheme.bodySemibold)
                    .foregroundStyle(ArbosTheme.text)
                    .lineLimit(1)
                if model.phase.inCall, let started = model.startedAt {
                    HStack(spacing: 6) {
                        ElapsedLabel(since: started)
                        if !model.outputRoute.isEmpty {
                            Text("·").foregroundStyle(ArbosTheme.textDim)
                            Button(action: model.toggleSpeaker) {
                                Text(model.outputRoute)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .font(ArbosTheme.caption)
                    .foregroundStyle(ArbosTheme.textFaint)
                }
            }
            .frame(maxWidth: 220)
            HStack {
                RoundButton(symbol: "chevron.down") { dismiss() }
                Spacer()
                if !model.phase.inCall {
                    RoundButton(symbol: "ellipsis") { showSettings = true }
                }
            }
        }
        .padding(.horizontal, ArbosTheme.gutter)
        .padding(.top, 4)
    }

    private var stateWord: String {
        switch model.phase {
        case .idle: return "Ready to call"
        case .unconfigured(let why): return why
        case .connecting: return "Connecting"
        case .listening: return "Listening"
        case .thinking: return "Thinking"
        case .speaking: return "Speaking"
        case .failed: return "Call failed"
        }
    }

    private var stateInk: Color {
        switch model.phase {
        case .listening, .speaking: return ArbosTheme.text
        case .thinking, .connecting: return ArbosTheme.textMuted
        case .idle, .unconfigured: return ArbosTheme.textFaint
        case .failed: return ArbosTheme.danger
        }
    }

    /// What was just said: Jacob's last line, faint, then Arbos's reply
    /// as it streams. Short by rule — he asks for more if he wants it.
    @ViewBuilder
    private var words: some View {
        let user = model.lines.last { $0.speaker == .user }
        let arbos = model.lines.last { $0.speaker == .arbos }
        VStack(spacing: 12) {
            if case .failed(let reason) = model.phase {
                Text(reason)
                    .font(ArbosTheme.callout)
                    .foregroundStyle(ArbosTheme.textFaint)
                    .multilineTextAlignment(.center)
            }
            if let user, !user.text.isEmpty {
                Text(user.text)
                    .font(ArbosTheme.body)
                    .lineSpacing(ArbosTheme.lineSpacing)
                    .foregroundStyle(ArbosTheme.textFaint)
                    .multilineTextAlignment(.center)
                    .lineLimit(2)
                    .truncationMode(.head)
            }
            if let arbos, !arbos.text.isEmpty, isLatest(arbos) {
                Text(arbos.text)
                    .font(ArbosTheme.body)
                    .lineSpacing(ArbosTheme.lineSpacing)
                    .foregroundStyle(ArbosTheme.text)
                    .multilineTextAlignment(.center)
                    .lineLimit(4)
                    .truncationMode(.head)
            }
        }
        .frame(maxWidth: .infinity)
        .frame(minHeight: 132, alignment: .top)
    }

    /// The reply is shown only while it is the newest thing said; once
    /// Jacob speaks again it makes room.
    private func isLatest(_ line: TranscriptLine) -> Bool {
        guard let last = model.lines.last(where: { $0.speaker != .system }) else { return false }
        return last.speaker == .arbos || model.phase == .speaking || model.phase == .thinking
    }

    /// One control: start, or hang up. A gear when nothing is set up.
    private var callButton: some View {
        Button(action: tapCall) {
            Image(systemName: buttonSymbol)
                .font(.system(size: 26, weight: .medium))
                .foregroundStyle(.white)
                .frame(width: 68, height: 68)
                .background(Circle().fill(buttonTint))
        }
        .buttonStyle(.plain)
        .animation(.easeInOut(duration: 0.25), value: model.phase)
    }

    private func tapCall() {
        switch model.phase {
        case .unconfigured:
            showSettings = true
        case .idle, .failed:
            model.startCall()
        case .connecting, .listening, .thinking, .speaking:
            model.endCall()
        }
    }

    private var buttonTint: Color {
        switch model.phase {
        case .unconfigured: return ArbosTheme.raisedHover
        case .idle, .failed: return ArbosTheme.ok
        case .connecting, .listening, .thinking, .speaking: return ArbosTheme.danger
        }
    }

    private var buttonSymbol: String {
        switch model.phase {
        case .unconfigured: return "gearshape.fill"
        case .idle, .failed: return "phone.fill"
        case .connecting, .listening, .thinking, .speaking: return "phone.down.fill"
        }
    }

    #if DEBUG
    /// Launch arguments for review and scripted tests:
    /// `-previewChat 1` sends `-chatText` (or a default) to the chat;
    /// `-previewCall 1` starts a call at once (pair with `-injectWav`).
    private func previewIfAsked() async {
        let defaults = UserDefaults.standard
        DebugScreenshots.startIfAsked()
        if defaults.bool(forKey: "previewCall") {
            try? await Task.sleep(for: .milliseconds(300))
            model.startCall()
        }
        guard defaults.bool(forKey: "previewChat") else { return }
        try? await Task.sleep(for: .milliseconds(1500))
        chat.send(defaults.string(forKey: "chatText") ?? "What's left before I can merge?")
    }
    #endif
}

/// The one moving thing: a thin ring, and inside it a disc that breathes
/// with the live level — the microphone while Jacob talks (white ink),
/// the reply while Arbos speaks (the accent). Thinking is a slow pulse
/// with no sound behind it; idle is a still, faint ring.
struct VoiceRing: View {
    let level: Float
    let phase: CallViewModel.Phase
    /// The level the disc is drawn at: eased toward `level` every frame,
    /// so 25 Hz meter updates read as one motion, not steps.
    @State private var shown: Float = 0

    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 60)) { context in
            let t = context.date.timeIntervalSinceReferenceDate
            let pulse = sin(t * 2 * .pi / 2.4) * 0.5 + 0.5
            let scale = discScale(pulse: pulse)
            ZStack {
                Circle()
                    .strokeBorder(ringInk, lineWidth: 1.5)
                Circle()
                    .fill(discInk)
                    .scaleEffect(scale)
            }
            .onChange(of: context.date) { _, _ in
                shown += (level - shown) * 0.35
            }
        }
        .animation(.easeInOut(duration: 0.3), value: phase)
    }

    /// 0.30 of the ring at rest, up to 0.92 at full level.
    private func discScale(pulse: Double) -> CGFloat {
        switch phase {
        case .listening, .speaking: return 0.30 + 0.62 * CGFloat(min(1, max(0, shown)))
        case .thinking, .connecting: return 0.30 + 0.10 * CGFloat(pulse)
        case .idle, .unconfigured, .failed: return 0.30
        }
    }

    private var ringInk: Color {
        switch phase {
        case .listening, .thinking, .connecting: return ArbosTheme.borderStrong
        case .speaking: return ArbosTheme.accent.opacity(0.5)
        case .idle, .unconfigured: return ArbosTheme.border
        case .failed: return ArbosTheme.danger.opacity(0.5)
        }
    }

    private var discInk: Color {
        switch phase {
        case .listening: return ArbosTheme.text.opacity(0.9)
        case .speaking: return ArbosTheme.accent
        case .thinking, .connecting: return ArbosTheme.textMuted.opacity(0.7)
        case .idle, .unconfigured: return ArbosTheme.textDim.opacity(0.6)
        case .failed: return ArbosTheme.danger.opacity(0.7)
        }
    }
}

/// mm:ss since the call began.
private struct ElapsedLabel: View {
    let since: Date

    var body: some View {
        TimelineView(.periodic(from: since, by: 1)) { context in
            let seconds = max(0, Int(context.date.timeIntervalSince(since)))
            Text(String(format: "%d:%02d", seconds / 60, seconds % 60))
                .monospacedDigit()
        }
    }
}
