import SwiftUI

/// The call. Voice first; the main chat is one swipe up.
struct CallView: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var chat: ChatStore
    @StateObject private var model: CallViewModel
    @State private var showSettings = false
    @State private var showChat = false
    /// True when the chat was opened by swiping: bring the keyboard up.
    @State private var chatWantsKeyboard = false

    init(settings: AppSettings, chat: ChatStore, link: VoiceLink) {
        _model = StateObject(wrappedValue: CallViewModel(settings: settings, chat: chat, link: link))
    }

    var body: some View {
        ZStack {
            Color(red: 0.04, green: 0.04, blue: 0.05).ignoresSafeArea()
            VStack(spacing: 0) {
                header
                Spacer(minLength: 12)
                StateOrb(phase: model.phase)
                    .padding(.top, 8)
                Text(model.phase.label)
                    .font(.system(.title3, design: .rounded, weight: .medium))
                    .foregroundStyle(.white.opacity(0.85))
                    .padding(.top, 28)
                    .contentTransition(.opacity)
                    .animation(.easeInOut(duration: 0.25), value: model.phase)
                if let started = model.startedAt {
                    ElapsedLabel(since: started)
                        .padding(.top, 6)
                }
                if let note = model.note, model.phase.inCall {
                    Text(note)
                        .font(.footnote)
                        .foregroundStyle(.white.opacity(0.35))
                        .padding(.top, 4)
                }
                if case .failed(let reason) = model.phase {
                    Text(reason)
                        .font(.footnote)
                        .foregroundStyle(.white.opacity(0.5))
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
                    .padding(.bottom, 18)
                chatHandle
                    .padding(.bottom, 6)
            }
        }
        .contentShape(Rectangle())
        .gesture(swipeUp)
        .preferredColorScheme(.dark)
        .sheet(isPresented: $showSettings, onDismiss: model.refreshIdle) {
            SettingsView().environmentObject(settings)
        }
        .sheet(isPresented: $showChat) {
            MainChatView(focusComposer: chatWantsKeyboard)
                .environmentObject(chat)
                .presentationDetents([.large])
                .presentationDragIndicator(.visible)
                .presentationBackground(Color(red: 0.06, green: 0.06, blue: 0.07))
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

    private var header: some View {
        HStack {
            Spacer()
            Button {
                showSettings = true
            } label: {
                Image(systemName: "ellipsis")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(.white.opacity(0.35))
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

    /// The only hint that text exists: a chevron. Tap or swipe up.
    private var chatHandle: some View {
        Button {
            openChat(keyboard: false)
        } label: {
            VStack(spacing: 3) {
                Image(systemName: "chevron.compact.up")
                    .font(.system(size: 22, weight: .regular))
                Text("chat")
                    .font(.caption2)
            }
            .foregroundStyle(.white.opacity(0.3))
            .frame(maxWidth: .infinity)
            .frame(height: 44)
        }
        .buttonStyle(.plain)
    }

    private var swipeUp: some Gesture {
        DragGesture(minimumDistance: 30)
            .onEnded { value in
                if value.translation.height < -60, abs(value.translation.width) < 80 {
                    openChat(keyboard: true)
                }
            }
    }

    private func openChat(keyboard: Bool) {
        chatWantsKeyboard = keyboard
        showChat = true
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
        case .idle, .failed: return Color(red: 0.20, green: 0.72, blue: 0.42)
        case .connecting, .listening, .thinking, .speaking: return Color(red: 0.90, green: 0.30, blue: 0.30)
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
    /// `-previewChat 1` opens the chat and sends `-chatText` (or a default);
    /// `-previewCall 1` starts a call at once (pair with `-injectWav`).
    private func previewIfAsked() async {
        let defaults = UserDefaults.standard
        DebugScreenshots.startIfAsked()
        if defaults.bool(forKey: "previewCall") {
            try? await Task.sleep(for: .milliseconds(300))
            model.startCall()
        }
        guard defaults.bool(forKey: "previewChat") else { return }
        try? await Task.sleep(for: .milliseconds(400))
        openChat(keyboard: false)
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
                .font(.system(.footnote, design: .monospaced))
                .foregroundStyle(.white.opacity(0.4))
        }
    }
}
