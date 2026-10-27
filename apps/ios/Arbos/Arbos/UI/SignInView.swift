// First launch, Grok Bot welcome style: blobs, name, tagline and one
// button — "Sign in with GitHub" — against the compiled-in relay. "Advanced"
// reveals the relay address for self-hosters; a relay in paircode mode shows
// the one-time-code fields instead of the GitHub button.

import SwiftUI

struct SignInView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @State private var advanced = false
    @State private var relayUrl = ""

    /// Grok's welcome screen scatters five bots around the title (positions read
    /// off the App Store shot, as fractions of the screen; the last one sits half
    /// off the bottom edge). Coloured trees (D74) stand in for the blobs.
    /// Five initials circles (D79) spelling the name: green, orange, red, blue, pink.
    private static let scatter: [(x: CGFloat, y: CGFloat, color: Int, letter: String)] = [
        (0.10, 0.15, 3, "A"), (0.86, 0.22, 1, "R"), (0.11, 0.74, 0, "B"),
        (0.90, 0.80, 5, "O"), (0.52, 0.985, 6, "S"),
    ]

    private func scatteredTrees(in size: CGSize) -> some View {
        ZStack {
            ForEach(Array(Self.scatter.enumerated()), id: \.offset) { _, spot in
                InitialsAvatar(initials: spot.letter, colorIndex: spot.color, size: 64)
                    .position(x: size.width * spot.x, y: size.height * spot.y)
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    var body: some View {
        ZStack {
            theme.bg.ignoresSafeArea()
            GeometryReader { geo in
                scatteredTrees(in: geo.size)
            }
            .ignoresSafeArea()
            ScrollView {
                VStack(spacing: 16) {
                    // Grok: the title sits at ~38 % of the screen height.
                    Spacer(minLength: 250)
                    Text("Arbos")
                        .font(theme.font(30, weight: .semibold))
                        .foregroundStyle(theme.text)
                    Text("Your team of always-on agents\nthat finish the work.")
                        .font(theme.font(Theme.body))
                        .foregroundStyle(theme.muted)
                        .multilineTextAlignment(.center)
                        .padding(.bottom, 26)

                    if model.authMode == .paircode {
                        PairingFields()
                    } else {
                        Button(action: model.signInWithGitHub) {
                            HStack(spacing: 10) {
                                if model.signingIn {
                                    ProgressView().tint(theme.onPrimary)
                                } else {
                                    GitHubMark().fill(theme.onPrimary).frame(width: 20, height: 20)
                                }
                                Text(model.signingIn ? "Signing in…" : "Sign in with GitHub")
                                    .font(theme.font(Theme.body, weight: .semibold))
                            }
                            .foregroundStyle(theme.onPrimary)
                            .frame(maxWidth: .infinity)
                            .frame(height: 50)
                            .background(Capsule().fill(theme.primary))
                        }
                        .buttonStyle(.plain)
                        .disabled(model.signingIn)
                        .padding(.horizontal, 24)

                        // The sign-in note: under the button, before the hint (the shared element).
                        if let err = model.signInError {
                            ErrorNote(id: "sign-in-error", error: err, detailsOpen: $model.signInErrorDetailsOpen,
                                      onTryAgain: { model.signInWithGitHub() },
                                      onSignIn: { model.signInWithGitHub() })
                                .padding(.horizontal, 24)
                        }
                        Text("Any GitHub account works. Your agents run on your own cloud machine.")
                            .font(theme.font(Theme.tiny))
                            .foregroundStyle(theme.muted)
                            .multilineTextAlignment(.center)
                            .padding(.horizontal, 36)
                    }

                    Button {
                        withAnimation(.easeOut(duration: 0.2)) { advanced.toggle() }
                    } label: {
                        HStack(spacing: 4) {
                            Text("Advanced")
                            Image(systemName: advanced ? "chevron.up" : "chevron.down").font(.system(size: 11, weight: .semibold))
                        }
                        .font(theme.font(Theme.tiny))
                        .foregroundStyle(theme.muted)
                    }
                    .buttonStyle(.plain)
                    .padding(.top, 18)

                    if advanced {
                        VStack(spacing: 8) {
                            HStack(spacing: 10) {
                                Image(systemName: "network").font(.system(size: 15)).foregroundStyle(theme.icon)
                                TextField("", text: $relayUrl, prompt: Text("Relay address").foregroundStyle(theme.placeholder))
                                    .keyboardType(.URL)
                                    .textInputAutocapitalization(.never)
                                    .autocorrectionDisabled()
                                    .font(theme.font(Theme.body))
                                    .foregroundStyle(theme.text)
                                    .onSubmit(applyRelay)
                                if RelayEndpoint(relayUrl).httpUrl != model.relayUrl {
                                    Button("Use", action: applyRelay)
                                        .font(theme.font(Theme.small, weight: .semibold))
                                        .foregroundStyle(theme.text)
                                }
                            }
                            .controlField()
                            Text("Self-hosting? Point the app at your own relay. Relays in pairing-code mode show a code field here.")
                                .font(theme.font(Theme.tiny))
                                .foregroundStyle(theme.muted)
                                .multilineTextAlignment(.center)
                                .padding(.horizontal, 12)
                        }
                        .padding(.horizontal, 24)
                        .transition(.opacity)
                    }
                    Spacer(minLength: 40)
                }
            }
            .scrollDismissesKeyboard(.interactively)
        }
        .onAppear {
            relayUrl = model.relayUrl
            if DebugHooks.show == "signin" {
                DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) { model.signInWithGitHub() }
            }
        }
    }

    private func applyRelay() {
        let trimmed = relayUrl.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return }
        model.setRelayUrl(trimmed)
    }
}

/// GitHub's mark as a path (so no asset is needed), drawn in a 20×20 box.
struct GitHubMark: Shape {
    func path(in rect: CGRect) -> Path {
        let s = min(rect.width, rect.height) / 16
        var p = Path()
        // Simplified octocat silhouette: head circle + body.
        p.addEllipse(in: CGRect(x: rect.minX + 2 * s, y: rect.minY + 1 * s, width: 12 * s, height: 12 * s))
        var bottom = Path()
        bottom.move(to: CGPoint(x: rect.minX + 5 * s, y: rect.minY + 10 * s))
        bottom.addLine(to: CGPoint(x: rect.minX + 5 * s, y: rect.minY + 16 * s))
        bottom.addLine(to: CGPoint(x: rect.minX + 11 * s, y: rect.minY + 16 * s))
        bottom.addLine(to: CGPoint(x: rect.minX + 11 * s, y: rect.minY + 10 * s))
        bottom.closeSubpath()
        p.addPath(bottom)
        return p
    }
}

/// Pairing-code fallback for relays with `auth_mode = paircode`.
struct PairingFields: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @State private var code = ""

    var body: some View {
        VStack(spacing: 12) {
            HStack(spacing: 10) {
                Image(systemName: "key").font(.system(size: 15)).foregroundStyle(theme.icon)
                TextField("", text: $code, prompt: Text("One-time code").foregroundStyle(theme.placeholder))
                    .keyboardType(.numberPad)
                    .textContentType(.oneTimeCode)
                    .font(theme.font(Theme.body, design: .monospaced))
                    .foregroundStyle(theme.text)
                    .onChange(of: code) { _, new in
                        let digits = new.filter(\.isNumber)
                        if digits != new || digits.count > 6 { code = String(digits.prefix(6)) }
                    }
            }
            .controlField()
            Button {
                model.pair(code: code)
            } label: {
                Text(model.pairingBusy ? "Pairing…" : "Pair this iPhone")
                    .font(theme.font(Theme.body, weight: .semibold))
                    .foregroundStyle(theme.onPrimary)
                    .frame(maxWidth: .infinity)
                    .frame(height: 50)
                    .background(Capsule().fill(theme.primary))
            }
            .buttonStyle(.plain)
            .disabled(code.count != 6 || model.pairingBusy)
            .opacity(code.count == 6 && !model.pairingBusy ? 1 : 0.45)
            if let err = model.pairingError {
                ErrorNote(id: "pairing-error", error: err, detailsOpen: $model.pairingErrorDetailsOpen,
                          onTryAgain: { model.pair(code: code) },
                          onSignIn: { model.pair(code: code) })
            }
            Text("This relay uses one-time codes: run `arbos-relay pair-code` on it.")
                .font(theme.font(Theme.tiny)).foregroundStyle(theme.muted).multilineTextAlignment(.center)
        }
        .padding(.horizontal, 24)
    }
}
