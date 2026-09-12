import SwiftUI

struct CallView: View {
    @EnvironmentObject private var settings: AppSettings
    @StateObject private var model: CallViewModel
    @State private var showSettings = false

    init(settings: AppSettings) {
        _model = StateObject(wrappedValue: CallViewModel(settings: settings))
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
                    .padding(.bottom, 36)
            }
        }
        .preferredColorScheme(.dark)
        .sheet(isPresented: $showSettings, onDismiss: model.refreshIdle) {
            SettingsView().environmentObject(settings)
        }
        .onChange(of: settings.openAIKey) { _, _ in model.refreshIdle() }
        .onChange(of: settings.selfHostedURL) { _, _ in model.refreshIdle() }
        .onChange(of: settings.provider) { _, _ in model.refreshIdle() }
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
