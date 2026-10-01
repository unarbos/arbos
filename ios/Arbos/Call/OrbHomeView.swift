import SwiftUI
import UIKit

/// The whole app: one e8 figure on a flat page. Tap to talk.
///
/// Laid out as bittensor.com lays out the page the figure comes from: a flat
/// `--background-default`, a header of upper-case FiraCode across the top, the
/// figure about 85% of the narrow side and nudged up by the site's `-4rem`, and
/// everything else centred underneath it.
struct OrbHomeView: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var chat: ChatStore
    @EnvironmentObject private var link: VoiceLink
    @StateObject private var model: CallViewModel
    @State private var showSettings = false
    @Environment(\.scenePhase) private var scenePhase

    /// `.page_container__d49Io { max-width: 600px; width: 85vw }`, which is what
    /// bounds the canvas on a phone.
    private static let figureFraction: CGFloat = 0.85
    /// `margin-top: -4rem`.
    private static let figureRise: CGFloat = 64

    init(settings: AppSettings, chat: ChatStore, link: VoiceLink) {
        _model = StateObject(wrappedValue: CallViewModel(settings: settings, chat: chat, link: link))
    }

    var body: some View {
        ZStack {
            ArbosTheme.bg.ignoresSafeArea()
            orb
            chrome
        }
        .animation(.easeOut(duration: 0.2), value: model.phase.inCall)
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

    /// Square, because the projection is: a stretched viewport would shear the
    /// lattice.
    private var orb: some View {
        GeometryReader { geometry in
            let side = min(geometry.size.width, geometry.size.height) * Self.figureFraction
            ZStack {
                E8OrbView(level: model.level, phase: model.phase)
                    .frame(width: side, height: side)
                // The figure itself does not take touches — a Metal view and a
                // SwiftUI gesture argue about them. This is the target.
                Circle()
                    .fill(.clear)
                    .contentShape(Circle())
                    .frame(width: side * 0.85, height: side * 0.85)
                    .onTapGesture(perform: tapOrb)
                    .onLongPressGesture(minimumDuration: 0.55, perform: endIfCalling)
            }
            .frame(width: geometry.size.width, height: geometry.size.height)
            .offset(y: -Self.figureRise)
        }
    }

    private var chrome: some View {
        VStack(spacing: 0) {
            header
            Spacer()
            footer
                .frame(minHeight: 64)
                .padding(.horizontal, ArbosTheme.gutter)
                .padding(.bottom, 24)
        }
    }

    /// `.Header_container__3ik1i`: the wordmark at one end, upper-case links at
    /// the other.
    private var header: some View {
        HStack {
            Text("Arbos")
                .bittensorLabel()
                .foregroundStyle(ArbosTheme.text)
            Spacer()
            BittensorLink(title: "Settings") { showSettings = true }
        }
        .frame(height: 44)
        .padding(.horizontal, ArbosTheme.gutter)
    }

    @ViewBuilder
    private var footer: some View {
        switch model.phase {
        case .connecting, .listening, .thinking, .speaking:
            VStack(spacing: 14) {
                // Tapping the figure mutes, so the screen has to say when it did.
                if model.muted {
                    Text("Muted")
                        .bittensorLabel()
                        .foregroundStyle(ArbosTheme.warning)
                } else if let note = model.note {
                    Text(note)
                        .bittensorLabel()
                        .foregroundStyle(ArbosTheme.textMuted)
                        .multilineTextAlignment(.center)
                        .fixedSize(horizontal: false, vertical: true)
                }
                BittensorLink(title: "End call") { model.endCall() }
            }
            .transition(.opacity)
        case .unconfigured(let why):
            BittensorLink(title: why, tint: ArbosTheme.textMuted, shout: false) {
                showSettings = true
            }
        case .failed(let message):
            VStack(spacing: 12) {
                Text(message)
                    .bittensorNote()
                    .foregroundStyle(ArbosTheme.danger)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                // A refused microphone cannot be asked for again from inside
                // the app, so the only useful thing to offer is the way out.
                if message == CallViewModel.microphoneDenied {
                    BittensorLink(title: "Open iOS Settings", action: openSystemSettings)
                } else {
                    Text("Tap to try again")
                        .bittensorLabel()
                        .foregroundStyle(ArbosTheme.textMuted)
                }
            }
        case .idle:
            Text("Tap to call")
                .bittensorLabel()
                .foregroundStyle(ArbosTheme.textMuted)
        }
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
