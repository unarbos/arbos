// One-line explainer before the system notification prompt (asked once).

import SwiftUI

struct NotificationsPrimerView: View {
    @Environment(\.theme) private var theme
    @Environment(\.dismiss) private var dismiss
    @State private var asking = false

    var body: some View {
        VStack(spacing: 18) {
            Capsule().fill(theme.separator).frame(width: 36, height: 5).padding(.top, 10)
            Spacer(minLength: 8)
            GlassCircle(size: 72) { // control-size: exception illustrationDiscs
                Image(systemName: "bell.badge.fill")
                    .font(theme.font(30, weight: .semibold))
                    .foregroundStyle(theme.text)
            }
            Text("Know when an agent is done")
                .font(theme.font(22, weight: .semibold))
                .foregroundStyle(theme.text)
                .multilineTextAlignment(.center)
            Text("Arbos sends a message when an agent replies or a person writes to you — nothing else.")
                .font(theme.font(Theme.small))
                .foregroundStyle(theme.muted)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 36)
            Spacer(minLength: 8)
            Button {
                guard !asking else { return }
                asking = true
                Task {
                    await PushManager.shared.requestPermission()
                    dismiss()
                }
            } label: {
                Text(asking ? "…" : "Turn on notifications")
                    .font(theme.font(Theme.body, weight: .semibold))
                    .foregroundStyle(theme.onPrimary)
                    .frame(maxWidth: .infinity).frame(height: 50)
                    .background(Capsule().fill(theme.primary))
            }
            .buttonStyle(.plain)
            .padding(.horizontal, 24)
            Button {
                PushManager.shared.primerShown = true
                dismiss()
            } label: {
                Text("Not now").font(theme.font(Theme.small, weight: .medium)).foregroundStyle(theme.muted)
            }
            .buttonStyle(.plain)
            .padding(.bottom, 18)
        }
        .frame(maxWidth: .infinity)
        .background(theme.bg)
        .presentationDetents([.height(380)])
        .presentationDragIndicator(.hidden)
        .presentationBackground(theme.bg)
        .presentationCornerRadius(28)
        .interactiveDismissDisabled(false)
        .onDisappear { PushManager.shared.primerShown = true }
    }
}
