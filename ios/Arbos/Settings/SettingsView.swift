import SwiftUI

/// Server URL and OpenAI API key — nothing else.
///
/// Both fields save as they are edited. Nothing here waits for a Done button:
/// a sheet can be dismissed by swiping it down, and a key typed into a draft
/// that only commits on Done is lost every time somebody does that, which
/// reads as "the app forgot my key".
///
/// Laid out as bittensor.com lays out a form: upper-case FiraCode labels, a
/// hairline box around each field, square corners, no fills.
struct SettingsView: View {
    @EnvironmentObject private var settings: AppSettings
    @Environment(\.dismiss) private var dismiss

    @State private var keyDraft = ""
    @State private var urlDraft = ""
    @State private var revealKey = false
    @State private var keychainRefused = false
    @FocusState private var focus: Field?

    private enum Field: Hashable {
        case url
        case key
    }

    private static var buildLine: String {
        let info = Bundle.main.infoDictionary ?? [:]
        let version = info["CFBundleShortVersionString"] as? String ?? "?"
        let build = info["CFBundleVersion"] as? String ?? "?"
        return "\(version) (\(build))"
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            ScrollView {
                VStack(alignment: .leading, spacing: 26) {
                    field("Voice server") {
                        TextField("wss://host/ws", text: $urlDraft)
                            .keyboardType(.URL)
                            .autocorrectionDisabled()
                            .textInputAutocapitalization(.never)
                            .submitLabel(.done)
                            .focused($focus, equals: .url)
                            .onSubmit(commitURL)
                    }

                    field("OpenAI API key") {
                        keyField
                        Button {
                            revealKey.toggle()
                        } label: {
                            Text(revealKey ? "Hide" : "Show")
                                .bittensorLabel()
                                .foregroundStyle(ArbosTheme.textMuted)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel(revealKey ? "Hide key" : "Show key")
                    }

                    Text(footer)
                        .bittensorNote()
                        .foregroundStyle(footerTint)
                        .fixedSize(horizontal: false, vertical: true)

                    Rectangle()
                        .fill(ArbosTheme.border)
                        .frame(height: 1)

                    HStack {
                        Text("Build")
                            .bittensorLabel()
                            .foregroundStyle(ArbosTheme.textMuted)
                        Spacer()
                        Text(Self.buildLine)
                            .bittensorLabel()
                            .foregroundStyle(ArbosTheme.text)
                    }
                }
                .padding(.horizontal, ArbosTheme.gutter)
                .padding(.vertical, 24)
            }
            .scrollDismissesKeyboard(.interactively)
        }
        .background(ArbosTheme.bg.ignoresSafeArea())
        .presentationDragIndicator(.visible)
        .onAppear {
            urlDraft = settings.selfHostedURL
            keyDraft = settings.openAIKey
        }
        // Swiped away, backgrounded, or Done: the same commit runs.
        .onDisappear(perform: commitAll)
        .onChange(of: keyDraft) { _, _ in saveKey() }
        .onChange(of: focus) { previous, _ in
            if previous == .url { commitURL() }
        }
    }

    private var header: some View {
        HStack {
            Text("Settings")
                .bittensorLabel()
                .foregroundStyle(ArbosTheme.text)
            Spacer()
            BittensorLink(title: "Done") {
                commitAll()
                dismiss()
            }
        }
        .frame(height: 44)
        .padding(.horizontal, ArbosTheme.gutter)
    }

    /// An upper-case label over a hairline box, which is how the site draws an
    /// input.
    private func field<Content: View>(
        _ label: String,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(label)
                .bittensorLabel()
                .foregroundStyle(ArbosTheme.textMuted)
            HStack(spacing: 12) {
                content()
            }
            .font(ArbosTheme.body)
            .foregroundStyle(ArbosTheme.text)
            .padding(.horizontal, ArbosTheme.promptPadX)
            .padding(.vertical, ArbosTheme.promptPadY + 2)
            .overlay {
                RoundedRectangle(cornerRadius: ArbosTheme.controlRadius)
                    .stroke(ArbosTheme.border, lineWidth: 1)
            }
        }
    }

    @ViewBuilder
    private var keyField: some View {
        // `SecureField` and `TextField` cannot be the same view with a toggled
        // flag: SwiftUI keeps the old text editing state and the field goes
        // blank on the first reveal.
        if revealKey {
            TextField(keyPlaceholder, text: $keyDraft)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .focused($focus, equals: .key)
        } else {
            SecureField(keyPlaceholder, text: $keyDraft)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .focused($focus, equals: .key)
        }
    }

    private var keyPlaceholder: String { "sk-…" }

    /// One line that says what is wrong, or what is ready. The address is
    /// checked first: a key is no use without somewhere to call.
    private var footer: String {
        if keychainRefused {
            return "The Keychain would not store the key. Unlock the phone and try again."
        }
        if let problem = AppSettings.problem(withServerURL: urlDraft) {
            return problem
        }
        let key = keyDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        if key.isEmpty {
            return "Paste your OpenAI API key. It is kept in the Keychain and sent only to your own server for the call."
        }
        if !key.hasPrefix("sk-") {
            return "Saved, though OpenAI keys normally begin with “sk-”."
        }
        return "Ready. Tap the figure to call. The voice server on your machine runs GPT-Live and Codex."
    }

    private var footerTint: Color {
        if keychainRefused { return ArbosTheme.danger }
        if AppSettings.problem(withServerURL: urlDraft) != nil { return ArbosTheme.danger }
        return ArbosTheme.textMuted
    }

    private func commitAll() {
        commitURL()
        saveKey()
    }

    private func commitURL() {
        let normalised = AppSettings.normaliseServerURL(urlDraft)
        if normalised != urlDraft { urlDraft = normalised }
        if normalised != settings.selfHostedURL { settings.selfHostedURL = normalised }
    }

    private func saveKey() {
        let key = keyDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard key != settings.openAIKey else {
            keychainRefused = false
            return
        }
        keychainRefused = !settings.saveOpenAIKey(key) && !key.isEmpty
    }
}
