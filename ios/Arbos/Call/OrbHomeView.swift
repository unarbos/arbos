import SwiftUI
import UIKit

/// The whole app: one full-bleed orb. Tap to talk. Settings top-right.
struct OrbHomeView: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var chat: ChatStore
    @EnvironmentObject private var link: VoiceLink
    @StateObject private var model: CallViewModel
    @State private var showSettings = false
    @Environment(\.scenePhase) private var scenePhase

    init(settings: AppSettings, chat: ChatStore, link: VoiceLink) {
        _model = StateObject(wrappedValue: CallViewModel(settings: settings, chat: chat, link: link))
    }

    var body: some View {
        ZStack {
            background.ignoresSafeArea()
            orb
            controls
        }
        .animation(.easeOut(duration: 0.2), value: model.phase.inCall)
        .preferredColorScheme(.dark)
        .sheet(isPresented: $showSettings, onDismiss: model.refreshIdle) {
            SettingsView().environmentObject(settings)
        }
        .onChange(of: settings.openAIKey) { _, _ in model.refreshIdle() }
        .onChange(of: settings.selfHostedURL) { _, _ in model.refreshIdle() }
        .onChange(of: scenePhase) { _, phase in
            guard phase == .active else { return }
            // A process that launched before the first unlock cannot read the
            // Keychain, and the key then looks unset until the app is killed.
            settings.reloadSecrets()
            model.refreshIdle()
        }
    }

    /// Full-bleed, as on the page it comes from. Square, because the projection
    /// is: a stretched viewport would shear the lattice.
    private var orb: some View {
        GeometryReader { geometry in
            let side = min(geometry.size.width, geometry.size.height) * 0.98
            ZStack {
                E8OrbView(level: model.level, phase: model.phase)
                    .frame(width: side, height: side)
                // The orb itself does not take touches — a Metal view and a
                // SwiftUI gesture argue about them. This is the target.
                Circle()
                    .fill(.clear)
                    .contentShape(Circle())
                    .frame(width: side * 0.72, height: side * 0.72)
                    .onTapGesture(perform: tapOrb)
                    .onLongPressGesture(minimumDuration: 0.55, perform: endIfCalling)
            }
            .frame(width: geometry.size.width, height: geometry.size.height)
        }
        .ignoresSafeArea()
    }

    private var controls: some View {
        VStack(spacing: 0) {
            HStack {
                Spacer()
                Button {
                    showSettings = true
                } label: {
                    Image(systemName: "gearshape")
                        .font(.system(size: 17, weight: .medium))
                        .foregroundStyle(ArbosTheme.textMuted)
                        .frame(width: 44, height: 44)
                        .contentShape(Rectangle())
                }
                .accessibilityLabel("Settings")
            }
            .padding(.horizontal, ArbosTheme.gutter)
            .padding(.top, 4)

            Spacer()

            footer
                .padding(.bottom, 28)
                .frame(minHeight: 48)
        }
    }

    @ViewBuilder
    private var footer: some View {
        switch model.phase {
        case .connecting, .listening, .thinking, .speaking:
            VStack(spacing: 10) {
                // Tapping the orb mutes, so the screen has to say when it did.
                if model.muted {
                    Label("Muted", systemImage: "mic.slash.fill")
                        .font(ArbosTheme.caption)
                        .foregroundStyle(ArbosTheme.textMuted)
                } else if let note = model.note {
                    Text(note)
                        .font(ArbosTheme.caption)
                        .foregroundStyle(ArbosTheme.textDim)
                }
                Button {
                    model.endCall()
                } label: {
                    Text("End")
                        .font(ArbosTheme.calloutMedium)
                        .foregroundStyle(ArbosTheme.textMuted)
                        .padding(.horizontal, 20)
                        .padding(.vertical, 10)
                        .contentShape(Rectangle())
                }
            }
            .transition(.opacity)
        case .unconfigured(let why):
            Button {
                showSettings = true
            } label: {
                Text(why)
                    .font(ArbosTheme.caption)
                    .foregroundStyle(ArbosTheme.textDim)
                    .padding(.horizontal, 20)
                    .padding(.vertical, 8)
                    .contentShape(Rectangle())
            }
        case .failed(let message):
            VStack(spacing: 10) {
                Text(message)
                    .font(ArbosTheme.caption)
                    .foregroundStyle(ArbosTheme.danger.opacity(0.9))
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 40)
                // A refused microphone cannot be asked for again from inside
                // the app, so the only useful thing to offer is the way out.
                if message == CallViewModel.microphoneDenied {
                    Button("Open iOS Settings", action: openSystemSettings)
                        .font(ArbosTheme.calloutMedium)
                        .foregroundStyle(ArbosTheme.textMuted)
                } else {
                    Text("Tap the orb to try again.")
                        .font(ArbosTheme.caption)
                        .foregroundStyle(ArbosTheme.textDim)
                }
            }
        case .idle:
            Color.clear.frame(height: 20)
        }
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

    private func openSystemSettings() {
        guard let url = URL(string: UIApplication.openSettingsURLString) else { return }
        UIApplication.shared.open(url)
    }
}
