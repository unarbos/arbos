import SwiftUI

/// The whole app: one full-bleed orb. Tap to talk. Settings top-right.
struct OrbHomeView: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var chat: ChatStore
    @EnvironmentObject private var link: VoiceLink
    @StateObject private var model: CallViewModel
    @State private var showSettings = false

    init(settings: AppSettings, chat: ChatStore, link: VoiceLink) {
        _model = StateObject(wrappedValue: CallViewModel(settings: settings, chat: chat, link: link))
    }

    var body: some View {
        ZStack {
            background.ignoresSafeArea()

            E8OrbView(level: model.level, phase: model.phase)
                .frame(width: 260, height: 260)
                .offset(y: 10)
                .onTapGesture(perform: tapOrb)
                .onLongPressGesture(minimumDuration: 0.55, perform: endIfCalling)

            VStack {
                HStack {
                    Spacer()
                    Button {
                        showSettings = true
                    } label: {
                        Image(systemName: "gearshape")
                            .font(.system(size: 17, weight: .medium))
                            .foregroundStyle(ArbosTheme.textMuted)
                            .frame(width: 40, height: 40)
                            .contentShape(Rectangle())
                    }
                    .accessibilityLabel("Settings")
                }
                .padding(.horizontal, ArbosTheme.gutter)
                .padding(.top, 4)

                Spacer()

                if model.phase.inCall {
                    Button {
                        model.endCall()
                    } label: {
                        Text("End")
                            .font(ArbosTheme.calloutMedium)
                            .foregroundStyle(ArbosTheme.textMuted)
                            .padding(.horizontal, 20)
                            .padding(.vertical, 10)
                    }
                    .padding(.bottom, 28)
                    .transition(.opacity)
                } else if case .unconfigured(let why) = model.phase {
                    Text(why)
                        .font(ArbosTheme.caption)
                        .foregroundStyle(ArbosTheme.textDim)
                        .padding(.bottom, 36)
                } else if case .failed(let message) = model.phase {
                    Text(message)
                        .font(ArbosTheme.caption)
                        .foregroundStyle(ArbosTheme.danger.opacity(0.9))
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, 40)
                        .padding(.bottom, 36)
                } else {
                    Color.clear.frame(height: 48)
                        .padding(.bottom, 28)
                }
            }
        }
        .animation(.easeOut(duration: 0.2), value: model.phase.inCall)
        .preferredColorScheme(.dark)
        .sheet(isPresented: $showSettings, onDismiss: model.refreshIdle) {
            SettingsView().environmentObject(settings)
        }
        .onChange(of: settings.openAIKey) { _, _ in model.refreshIdle() }
        .onChange(of: settings.selfHostedURL) { _, _ in model.refreshIdle() }
    }

    private var background: some View {
        LinearGradient(
            colors: [
                Color(hex: 0x0a0c0e),
                Color(hex: 0x12161a),
                Color(hex: 0x0c1014),
            ],
            startPoint: .topLeading,
            endPoint: .bottomTrailing
        )
    }

    private func tapOrb() {
        switch model.phase {
        case .idle, .failed:
            if settings.isConfigured {
                model.startCall()
            } else {
                showSettings = true
            }
        case .unconfigured:
            showSettings = true
        case .connecting, .listening, .thinking, .speaking:
            model.muted.toggle()
        }
    }

    private func endIfCalling() {
        guard model.phase.inCall else { return }
        model.endCall()
    }
}
