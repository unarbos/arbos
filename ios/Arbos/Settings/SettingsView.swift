import SwiftUI

/// Server URL and OpenAI API key — nothing else.
struct SettingsView: View {
    @EnvironmentObject private var settings: AppSettings
    @Environment(\.dismiss) private var dismiss
    @State private var keyDraft = ""

    private static var buildLine: String {
        let info = Bundle.main.infoDictionary ?? [:]
        let version = info["CFBundleShortVersionString"] as? String ?? "?"
        let build = info["CFBundleVersion"] as? String ?? "?"
        return "\(version) (\(build))"
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("wss://host/ws", text: $settings.selfHostedURL)
                        .keyboardType(.URL)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                    SecureField(keyPlaceholder, text: $keyDraft)
                        .textContentType(.oneTimeCode)
                        .autocorrectionDisabled()
                } header: {
                    Text("Call")
                } footer: {
                    Text("The voice server on your machine runs GPT-Live and Codex. Your OpenAI key is stored in the Keychain and sent only for the call.")
                }
                .listRowBackground(ArbosTheme.card)

                Section {
                    LabeledContent("Arbos", value: Self.buildLine)
                        .foregroundStyle(ArbosTheme.textMuted)
                } footer: {
                    Text("This TestFlight build.")
                }
                .listRowBackground(ArbosTheme.card)
            }
            .scrollContentBackground(.hidden)
            .background(ArbosTheme.bg)
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") {
                        if !keyDraft.isEmpty { settings.saveOpenAIKey(keyDraft) }
                        dismiss()
                    }
                }
            }
        }
    }

    private var keyPlaceholder: String {
        settings.openAIKey.isEmpty ? "OpenAI API key" : "API key saved · paste to replace"
    }
}
