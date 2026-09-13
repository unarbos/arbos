import SwiftUI

struct SettingsView: View {
    @EnvironmentObject private var settings: AppSettings
    @Environment(\.dismiss) private var dismiss
    @State private var keyDraft = ""

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    if VoiceProvider.available.count > 1 {
                        Picker("Provider", selection: $settings.provider) {
                            ForEach(VoiceProvider.available) { provider in
                                Text(provider.label).tag(provider)
                            }
                        }
                    }
                    switch settings.provider {
                    case .selfHosted:
                        TextField("wss://host:port/voice", text: $settings.selfHostedURL)
                            .keyboardType(.URL)
                            .autocorrectionDisabled()
                            .textInputAutocapitalization(.never)
                    case .openAIRealtime:
                        SecureField(keyPlaceholder, text: $keyDraft)
                            .textContentType(.password)
                            .autocorrectionDisabled()
                        TextField("Model", text: $settings.openAIModel)
                            .autocorrectionDisabled()
                            .textInputAutocapitalization(.never)
                    }
                } header: {
                    Text("Voice")
                } footer: {
                    Text(voiceFooter)
                }
                Section {
                    TextField("Host", text: $settings.kernelHost)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                    TextField("Port", value: $settings.kernelPort, format: .number)
                        .keyboardType(.numberPad)
                } header: {
                    Text("Arbos kernel")
                } footer: {
                    Text("The attach port from .arbos/kernel.json, reachable from this phone. Replies come from here.")
                }
            }
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

    private var voiceFooter: String {
        switch settings.provider {
        case .selfHosted: return "Our speech server: hears you, reads replies aloud."
        case .openAIRealtime: return "Stored in the Keychain. Replies come from the model, not the kernel."
        }
    }

    private var keyPlaceholder: String {
        settings.openAIKey.isEmpty ? "OpenAI API key" : "API key saved · paste to replace"
    }
}
