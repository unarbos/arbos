import SwiftUI
import UIKit

/// The whole app: one e8 figure on a flat page. Tap to talk.
///
/// Laid out as bittensor.com lays out the page the figure comes from: a flat
/// `--background-default`, a header of upper-case FiraCode across the top, and
/// the figure at 85% of the narrow side, centred in what is left and nudged up by
/// the site's `-4rem`.
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
            page
        }
        .animation(.easeOut(duration: 0.2), value: model.phase.inCall)
        .sheet(isPresented: $showSettings, onDismiss: model.refreshIdle) {
            SettingsView().environmentObject(settings)
        }
        .onAppear(perform: openSettingsIfAsked)
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

    /// The site's column: a sticky header, then `.page_container__d49Io` taking
    /// the rest with `flex: 1` and centring the canvas inside it, pulled up by
    /// `margin-top: -4rem`. Centring against the whole screen instead would sit
    /// the figure too high, because the header is above that space rather than
    /// over it.
    ///
    /// The status line is this app's, not the site's — the site's column holds
    /// only the canvas, its other text being screen-reader-only — so it takes the
    /// room the rise opens up at the bottom.
    private var page: some View {
        VStack(spacing: 0) {
            header
            GeometryReader { geometry in
                let side = min(geometry.size.width, geometry.size.height) * Self.figureFraction
                // Never rise further than the slack above the figure: the site's
                // 4rem is a fixed number taken from a desktop viewport, and on a
                // short phone the whole of it would push the figure up into the
                // header.
                let slack = max(0, (geometry.size.height - side) / 2)
                figure(side: side)
                    .frame(width: geometry.size.width, height: geometry.size.height)
                    .offset(y: -min(Self.figureRise, slack))
            }
            captionBand
            footer
                .frame(minHeight: 56)
                .padding(.horizontal, ArbosTheme.gutter)
                .padding(.bottom, 24)
        }
    }

    /// Square, because the projection is: a stretched viewport would shear the
    /// lattice.
    private func figure(side: CGFloat) -> some View {
        ZStack {
            E8OrbView(level: model.level, phase: model.phase)
            // The figure itself does not take touches — a Metal view and a
            // SwiftUI gesture argue about them. This is the target.
            Circle()
                .fill(.clear)
                .contentShape(Circle())
                .frame(width: side * 0.85, height: side * 0.85)
                .onTapGesture(perform: tapOrb)
                .onLongPressGesture(minimumDuration: 0.55, perform: endIfCalling)
                .accessibilityElement()
                .accessibilityAddTraits(.isButton)
                .accessibilityLabel(model.phase.inCall ? "Mute" : "Call")
                .accessibilityValue(spokenState)
        }
        .frame(width: side, height: side)
    }

    /// What is being said, while it is being said: your words as the recogniser
    /// sharpens them, then the reply in their place, then nothing.
    ///
    /// The band is always this tall, full or empty, because text arriving must not
    /// move the figure. Its height comes from three hidden lines rather than a
    /// number, so a phone with the text size turned up gets a taller band instead
    /// of a clipped one.
    private var captionBand: some View {
        ZStack(alignment: .top) {
            Text(verbatim: "X\nX\nX")
                .captionLine()
                .hidden()
            if let caption = model.caption {
                Text(caption.text)
                    .captionLine()
                    // Your own words are the echo and the reply is the answer, so
                    // the reply is the one at full strength. Which of the two is
                    // speaking is not worth a label on a screen this bare.
                    .foregroundStyle(caption.speaker == .user ? ArbosTheme.textMuted : ArbosTheme.text)
                    // A line still being written grows at its end, so a long one
                    // loses its beginning rather than the words just spoken.
                    .truncationMode(.head)
                    .id(caption.id)
                    .transition(.opacity)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.horizontal, ArbosTheme.gutter)
        .animation(.easeInOut(duration: 0.25), value: model.caption?.id)
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
                    // Not `warning`: the site's amber is #eeac3c, which against a
                    // white page is under 2:1 and cannot be read. A muted
                    // microphone is the one thing on this screen that has to be
                    // noticed.
                    Text("Muted")
                        .bittensorLabel()
                        .foregroundStyle(ArbosTheme.danger)
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

    /// What the figure is saying, for somebody who cannot see it.
    private var spokenState: String {
        if model.muted { return "muted" }
        switch model.phase {
        case .idle: return "not on a call"
        case .connecting: return "connecting"
        case .listening: return "listening"
        case .thinking: return "working"
        case .speaking: return "speaking"
        case .unconfigured(let why): return why
        case .failed(let message): return message
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

    /// `-previewSettings 1` opens the sheet on launch, so CI can photograph it
    /// without having to drive a tap.
    private func openSettingsIfAsked() {
        #if DEBUG
        if UserDefaults.standard.bool(forKey: "previewSettings") { showSettings = true }
        #endif
    }
}

private extension Text {
    /// The caption's one measure of type, shared by the real line and by the
    /// hidden one that reserves its height — they have to agree or the band is
    /// the wrong size.
    func captionLine() -> some View {
        font(ArbosTheme.body)
            .tracking(ArbosTheme.tracking(ArbosTheme.bodySize))
            .lineSpacing(ArbosTheme.lineSpacing)
            .multilineTextAlignment(.center)
            .lineLimit(3)
    }
}
