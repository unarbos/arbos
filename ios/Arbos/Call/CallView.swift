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

/// The call: a full-duplex conversation with the project's main agent.
/// Opened from the project chat's handset; the chevron at the top goes
/// back to the chat (the call keeps going until it is hung up).
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
                header
                Spacer(minLength: 12)
                StateOrb(phase: model.phase)
                    .padding(.top, 8)
                Text(model.phase.label)
                    .font(.system(size: 20, weight: .medium))
                    .foregroundStyle(ArbosTheme.text.opacity(0.85))
                    .padding(.top, 28)
                    .contentTransition(.opacity)
                    .animation(.easeInOut(duration: 0.25), value: model.phase)
                if let started = model.startedAt {
                    ElapsedLabel(since: started)
                        .padding(.top, 6)
                }
                if let note = model.note, model.phase.inCall {
                    // Tap the status line to move the call between the
                    // phone's speaker and a connected headset.
                    Button(action: model.toggleSpeaker) {
                        Text(note)
                            .font(ArbosTheme.caption)
                            .foregroundStyle(ArbosTheme.textDim)
                            .padding(.top, 4)
                    }
                    .buttonStyle(.plain)
                }
                if case .failed(let reason) = model.phase {
                    Text(reason)
                        .font(ArbosTheme.callout)
                        .foregroundStyle(ArbosTheme.textFaint)
                        .padding(.top, 6)
                        .padding(.horizontal, 40)
                        .multilineTextAlignment(.center)
                }
                Spacer(minLength: 16)
                TranscriptView(lines: model.lines)
                    .frame(maxHeight: 220)
                    .opacity(model.lines.isEmpty ? 0 : 1)
                Spacer(minLength: 24)
                callButton
                    .padding(.bottom, 26)
                Text(Self.buildLabel)
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(ArbosTheme.text.opacity(0.18))
                    .padding(.bottom, 4)
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

    /// `0.1.0 (57)`: so anyone can say which build they are running.
    static var buildLabel: String {
        let info = Bundle.main.infoDictionary ?? [:]
        let version = info["CFBundleShortVersionString"] as? String ?? "?"
        let build = info["CFBundleVersion"] as? String ?? "?"
        return "\(version) (\(build))"
    }

    /// Back to the chat on the left; the settings on the right while idle.
    private var header: some View {
        HStack {
            Button {
                dismiss()
            } label: {
                Image(systemName: "chevron.down")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(ArbosTheme.textMuted)
                    .frame(width: 44, height: 44)
            }
            Spacer()
            Text(chat.identity?.label ?? chat.title)
                .font(ArbosTheme.calloutMedium)
                .foregroundStyle(ArbosTheme.textFaint)
                .lineLimit(1)
            Spacer()
            Button {
                showSettings = true
            } label: {
                Image(systemName: "ellipsis")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(ArbosTheme.textMuted)
                    .frame(width: 44, height: 44)
            }
            .disabled(model.phase.inCall)
            .opacity(model.phase.inCall ? 0 : 1)
        }
        .padding(.horizontal, 12)
    }

    /// One control. Green to start, red to hang up, a gear when there is
    /// nothing to call yet.
    private var callButton: some View {
        Button(action: tapCall) {
            ZStack {
                Circle()
                    .fill(buttonTint)
                    .frame(width: 76, height: 76)
                    .shadow(color: buttonTint.opacity(0.4), radius: 16, y: 6)
                Image(systemName: buttonSymbol)
                    .font(.system(size: 28, weight: .medium))
                    .foregroundStyle(.white)
            }
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
        case .unconfigured: return Color(white: 0.28)
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

/// mm:ss since the call began.
private struct ElapsedLabel: View {
    let since: Date

    var body: some View {
        TimelineView(.periodic(from: since, by: 1)) { context in
            let seconds = max(0, Int(context.date.timeIntervalSince(since)))
            Text(String(format: "%02d:%02d", seconds / 60, seconds % 60))
                .font(.system(size: 13, design: .monospaced))
                .foregroundStyle(ArbosTheme.textFaint)
        }
    }
}
